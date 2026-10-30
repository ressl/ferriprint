// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{
    config::{Config, PrinterConfig},
    domain::*,
    jobs::JobRecord,
    upstream::UpstreamStatus,
    wire::*,
};
use anyhow::Result;
use ipp::prelude::*;
use std::collections::{BTreeMap, HashSet};

pub const OPERATIONS: [i32; 8] = [2, 4, 5, 6, 8, 9, 10, 11];
const JOB_KEYS: &[&str] = &[
    "media",
    "media-col",
    "copies",
    "print-quality",
    "print-color-mode",
    "sides",
    "orientation-requested",
    "printer-resolution",
    "print-scaling",
    "number-up",
    "job-hold-until",
    "multiple-document-handling",
];
#[derive(Debug)]
pub struct Rejection {
    pub status: StatusCode,
    pub message: String,
    pub unsupported: Vec<IppAttribute>,
}
impl Rejection {
    pub fn bad(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::ClientErrorBadRequest,
            message: message.into(),
            unsupported: Vec::new(),
        }
    }
    pub fn status(status: StatusCode, message: impl Into<String>) -> Self {
        Self {
            status,
            message: message.into(),
            unsupported: Vec::new(),
        }
    }
    fn unsupported(attr: &IppAttribute, message: &str) -> Self {
        Self {
            status: if attr.name().as_str() == "document-format" {
                StatusCode::ClientErrorDocumentFormatNotSupported
            } else {
                StatusCode::ClientErrorAttributesOrValuesNotSupported
            },
            message: message.into(),
            unsupported: vec![attr.clone()],
        }
    }
}
pub fn response(request: &Metadata, status: StatusCode) -> Result<IppRequestResponse> {
    Ok(IppRequestResponse::new_response(
        if matches!(request.header().version.0, 0x0101 | 0x0200) {
            request.header().version
        } else {
            IppVersion::v2_0()
        },
        status,
        request.header().request_id,
    )?)
}
pub fn reject(request: &Metadata, rejection: Rejection) -> Result<IppRequestResponse> {
    let mut reply = response(request, rejection.status)?;
    add(
        &mut reply,
        DelimiterTag::OperationAttributes,
        "status-message",
        text(bounded(&rejection.message, 255))?,
    )?;
    for attribute in rejection.unsupported {
        reply
            .attributes_mut()
            .add(DelimiterTag::UnsupportedAttributes, attribute);
    }
    Ok(reply)
}
pub fn validate_envelope(
    request: &Metadata,
    printer_uri: &str,
    path_id: Option<i32>,
) -> std::result::Result<Option<i32>, Rejection> {
    if !matches!(request.header().version.0, 0x0101 | 0x0200) {
        return Err(Rejection::status(
            StatusCode::ServerErrorVersionNotSupported,
            "supported IPP versions are 1.1 and 2.0",
        ));
    }
    if request.header().request_id <= 0 {
        return Err(Rejection::bad("request-id must be positive"));
    }
    let groups = request.attributes().groups();
    if groups.is_empty() || groups[0].tag() != DelimiterTag::OperationAttributes {
        return Err(Rejection::bad("operation attributes must be first"));
    }
    let mut tags = HashSet::new();
    let mut keys = HashSet::new();
    for group in groups {
        if !matches!(
            group.tag(),
            DelimiterTag::OperationAttributes | DelimiterTag::JobAttributes
        ) || !tags.insert(group.tag())
        {
            return Err(Rejection::bad("invalid or repeated attribute group"));
        }
        for attribute in group.attributes() {
            if !keys.insert(attribute.name().as_str()) {
                return Err(Rejection::bad("duplicate attribute"));
            }
        }
    }
    let operation = groups[0].attributes();
    if operation.len() < 3
        || operation[0].name().as_str() != "attributes-charset"
        || operation[1].name().as_str() != "attributes-natural-language"
    {
        return Err(Rejection::bad("charset and natural language must be first"));
    }
    if !matches!(operation[0].value(),IppValue::Charset(s) if s.as_str().eq_ignore_ascii_case("utf-8"))
    {
        return Err(Rejection::status(
            StatusCode::ClientErrorCharsetNotSupported,
            "only UTF-8 is supported",
        ));
    }
    if !matches!(operation[1].value(),IppValue::NaturalLanguage(s) if !s.is_empty()) {
        return Err(Rejection::bad("invalid natural language"));
    }
    let target = &operation[2];
    let mut id = None;
    match (target.name().as_str(), target.value()) {
        ("printer-uri", IppValue::Uri(uri)) if same_ipp_resource(uri.as_str(), printer_uri) => {
            if let Some(attribute) = groups[0].get("job-id") {
                if operation.get(3).map(|a| a.name().as_str()) != Some("job-id") {
                    return Err(Rejection::bad("job-id must follow printer-uri"));
                }
                id = match attribute.value() {
                    IppValue::Integer(n) if *n > 0 => Some(*n),
                    _ => return Err(Rejection::bad("invalid job-id")),
                };
            }
        }
        ("job-uri", IppValue::Uri(uri)) => {
            let (digits, number) = uri
                .rsplit_once("/jobs/")
                .filter(|(resource, _)| same_ipp_resource(resource, printer_uri))
                .and_then(|(_, digits)| {
                    digits
                        .parse::<i32>()
                        .ok()
                        .filter(|n| *n > 0)
                        .map(|number| (digits, number))
                })
                .ok_or_else(|| {
                    Rejection::status(
                        StatusCode::ClientErrorNotFound,
                        "job-uri is not this local resource",
                    )
                })?;
            if digits.starts_with(['+', '0']) {
                return Err(Rejection::bad("noncanonical job-uri"));
            }
            id = Some(number);
        }
        _ => {
            return Err(Rejection::status(
                StatusCode::ClientErrorNotFound,
                "target URI does not match this local printer",
            ));
        }
    }
    if path_id.is_some() && path_id != id {
        return Err(Rejection::bad("job URI does not match HTTP resource"));
    }
    let op = request.header().operation_or_status as i32;
    if !OPERATIONS.contains(&op) {
        return Err(Rejection::status(
            StatusCode::ServerErrorOperationNotSupported,
            "operation is not implemented",
        ));
    }
    if matches!(op, 6 | 8 | 9) != id.is_some() {
        return Err(Rejection::bad("incorrect target for operation"));
    }
    if !matches!(op, 2 | 4 | 5) && tags.contains(&DelimiterTag::JobAttributes) {
        return Err(Rejection::bad(
            "job template is not allowed for this operation",
        ));
    }
    if (groups[0].get("printer-uri").is_some() && groups[0].get("job-uri").is_some())
        || (target.name().as_str() == "job-uri" && groups[0].get("job-id").is_some())
    {
        return Err(Rejection::bad("multiple or conflicting target identifiers"));
    }
    for attribute in groups[0].attributes() {
        let key = attribute.name().as_str();
        let allowed = matches!(
            key,
            "attributes-charset"
                | "attributes-natural-language"
                | "printer-uri"
                | "job-uri"
                | "job-id"
                | "requesting-user-name"
        ) || match op {
            2 | 4 | 5 => matches!(
                key,
                "job-name"
                    | "ipp-attribute-fidelity"
                    | "document-format"
                    | "compression"
                    | "document-name"
                    | "document-natural-language"
            ),
            6 => matches!(
                key,
                "document-format"
                    | "compression"
                    | "document-name"
                    | "document-natural-language"
                    | "last-document"
            ),
            9 | 11 => key == "requested-attributes",
            10 => matches!(
                key,
                "requested-attributes" | "which-jobs" | "my-jobs" | "limit"
            ),
            _ => false,
        };
        if !allowed {
            return Err(Rejection::unsupported(
                attribute,
                "unsupported operation attribute",
            ));
        }
        let valid = match key {
            "requesting-user-name" | "job-name" | "document-name" => matches!(
                attribute.value(),
                IppValue::NameWithoutLanguage(_) | IppValue::NameWithLanguage { .. }
            ),
            "ipp-attribute-fidelity" | "my-jobs" | "last-document" => {
                matches!(attribute.value(), IppValue::Boolean(_))
            }
            "document-format" => matches!(attribute.value(), IppValue::MimeMediaType(_)),
            "compression" => matches!(attribute.value(),IppValue::Keyword(s) if s.as_str()=="none"),
            "document-natural-language" => {
                matches!(attribute.value(), IppValue::NaturalLanguage(_))
            }
            "requested-attributes" => {
                values(attribute.value())
                    .iter()
                    .all(|v| matches!(v, IppValue::Keyword(_)))
                    && !values(attribute.value()).is_empty()
            }
            "which-jobs" => {
                matches!(attribute.value(),IppValue::Keyword(s) if matches!(s.as_str(),"all"|"completed"|"not-completed"))
            }
            "limit" => matches!(attribute.value(),IppValue::Integer(n) if *n>0 && *n<=1000),
            _ => true,
        };
        if !valid {
            return Err(Rejection::unsupported(
                attribute,
                "invalid or unsupported operation value",
            ));
        }
    }
    if op == 6
        && !matches!(
            attr(request, DelimiterTag::OperationAttributes, "last-document"),
            Some(IppValue::Boolean(true))
        )
    {
        return Err(Rejection::status(
            StatusCode::ServerErrorMultipleDocumentJobsNotSupported,
            "exactly one document is supported; last-document must be true",
        ));
    }
    Ok(id)
}
/// A parsed Job Template plus the attributes/values that were ignored because
/// the Client asked for best-effort handling (`ipp-attribute-fidelity=false`).
pub struct ParsedTemplate {
    pub job: JobTemplate,
    /// Client-supplied attributes that were ignored or substituted. Reported in
    /// the Unsupported Attributes group so the Client can tell what changed.
    pub ignored: Vec<IppAttribute>,
}
/// RFC 8011 Appendix C.1: with "ipp-attribute-fidelity" 'false' (also the
/// default when omitted) "the Printer MUST always accept the request by
/// ignoring unsupported Job Template attributes and by substituting unsupported
/// values of supported Job Template attributes with supported values"; with
/// 'true' it "MUST reject the request" instead (status semantics: B.1.4.12).
///
/// Two families are never softened, at either fidelity, because silently
/// proceeding would print something the Client did not ask for:
/// `media`/`media-col` (a substituted paper size means wrong paper in the tray)
/// and `copies` (a substituted count means the wrong number of copies). Their
/// value validation lives in [`crate::profile::PrinterProfile::resolve`], which
/// rejects unsupported media and out-of-range copies unconditionally.
pub fn template(request: &Metadata) -> std::result::Result<ParsedTemplate, Rejection> {
    let fidelity = matches!(
        attr(
            request,
            DelimiterTag::OperationAttributes,
            "ipp-attribute-fidelity"
        ),
        Some(IppValue::Boolean(true))
    );
    let mut job = JobTemplate::default();
    let mut ignored = Vec::new();
    if let Some(group) = request.attributes().first_of(DelimiterTag::JobAttributes) {
        for a in group.attributes() {
            let fail = || Rejection::unsupported(a, "unsupported job template value");
            // Substitutable attributes whose unsupported form has a defined
            // best-effort meaning: print on one-up, at the Printer's priority,
            // with no finishing, no output-bin selection, all pages and no
            // banner. Ignoring them changes presentation, never paper identity.
            let substitutable = matches!(
                a.name().as_str(),
                "number-up"
                    | "job-priority"
                    | "finishings"
                    | "output-bin"
                    | "page-ranges"
                    | "job-sheets"
            );
            match (a.name().as_str(), a.value()) {
                ("media", IppValue::Keyword(s)) => job.media = Some(s.to_string()),
                ("media-col", IppValue::Collection(map)) => {
                    job.media_col = Some(parse_media(map).map_err(|_| fail())?)
                }
                ("copies", IppValue::Integer(n)) => job.copies = Some(*n),
                ("print-quality", IppValue::Enum(n)) => job.quality = Some(*n),
                ("print-color-mode", IppValue::Keyword(s)) => job.color_mode = Some(s.to_string()),
                ("sides", IppValue::Keyword(s)) => job.sides = Some(s.to_string()),
                ("orientation-requested", IppValue::Enum(n)) => job.orientation = Some(*n),
                (
                    "printer-resolution",
                    IppValue::Resolution {
                        cross_feed,
                        feed,
                        units: 3,
                    },
                ) => {
                    job.resolution = Some(Resolution {
                        x: *cross_feed,
                        y: *feed,
                    })
                }
                ("print-scaling", IppValue::Keyword(s)) => job.print_scaling = Some(s.to_string()),
                ("number-up", IppValue::Integer(1)) => {}
                ("job-hold-until", IppValue::Keyword(s)) if s.as_str() == "no-hold" => {}
                ("multiple-document-handling", IppValue::Keyword(s))
                    if s.as_str() == "separate-documents-collated-copies" => {}
                // Never substituted at any fidelity: paper identity and copy
                // count, including a malformed collection or wrong syntax.
                ("media" | "media-col" | "copies", _) => return Err(fail()),
                // `job-preset-name` is not a standard request selector, and a
                // preset carries media and quality: ignoring it would print
                // other paper than selected, so it stays a hard rejection.
                ("job-preset-name", _) => return Err(fail()),
                _ if substitutable && !fidelity => ignored.push(a.clone()),
                // An unknown attribute is ignorable only for best-effort Jobs.
                _ if !fidelity => ignored.push(a.clone()),
                _ => return Err(fail()),
            }
        }
    }
    Ok(ParsedTemplate { job, ignored })
}
/// RFC 8011 Section 4.1.7 requires the Unsupported Attributes group exactly
/// when the status-code is one of the four "ignored/substituted" or
/// "not-supported/conflicting" values.
pub fn mark_ignored(reply: &mut IppRequestResponse, ignored: &[IppAttribute]) -> Result<()> {
    if ignored.is_empty() {
        return Ok(());
    }
    reply.header_mut().operation_or_status =
        StatusCode::SuccessfulOkIgnoredOrSubstitutedAttributes as i16;
    for attribute in ignored {
        reply
            .attributes_mut()
            .add(DelimiterTag::UnsupportedAttributes, attribute.clone());
    }
    Ok(())
}
fn parse_media(
    map: &BTreeMap<ipp::value::IppName, IppValue>,
) -> std::result::Result<MediaRequest, ()> {
    let mut media = MediaRequest::default();
    for (key, value) in map {
        match (key.as_str(), value) {
            ("media-size", IppValue::Collection(size)) if size.len() == 2 => {
                media.width = match size.get("x-dimension") {
                    Some(IppValue::Integer(n)) => Some(*n),
                    _ => return Err(()),
                };
                media.height = match size.get("y-dimension") {
                    Some(IppValue::Integer(n)) => Some(*n),
                    _ => return Err(()),
                };
            }
            ("media-size-name", IppValue::Keyword(s)) => media.name = Some(s.to_string()),
            ("media-source", IppValue::Keyword(s)) => media.source = Some(s.to_string()),
            ("media-type", IppValue::Keyword(s)) => media.media_type = Some(s.to_string()),
            ("media-left-margin", IppValue::Integer(n)) if *n >= 0 => media.left = Some(*n),
            ("media-bottom-margin", IppValue::Integer(n)) if *n >= 0 => media.bottom = Some(*n),
            ("media-right-margin", IppValue::Integer(n)) if *n >= 0 => media.right = Some(*n),
            ("media-top-margin", IppValue::Integer(n)) if *n >= 0 => media.top = Some(*n),
            _ => return Err(()),
        }
    }
    Ok(media)
}
pub fn profile_rejection(request: &Metadata, error: AttributeError) -> Rejection {
    let group = if error.attribute == "document-format" {
        DelimiterTag::OperationAttributes
    } else {
        DelimiterTag::JobAttributes
    };
    Rejection {
        status: if error.attribute == "document-format" {
            StatusCode::ClientErrorDocumentFormatNotSupported
        } else {
            StatusCode::ClientErrorAttributesOrValuesNotSupported
        },
        message: error.to_string(),
        unsupported: request
            .attributes()
            .first_of(group)
            .and_then(|g| g.get(error.attribute))
            .cloned()
            .into_iter()
            .collect(),
    }
}
pub fn media_collection(
    media: &Media,
    source: Option<&str>,
    media_type: Option<&str>,
) -> Result<IppValue> {
    let mut items = vec![
        (
            "media-size",
            collection(vec![
                ("x-dimension", IppValue::Integer(media.width)),
                ("y-dimension", IppValue::Integer(media.height)),
            ])?,
        ),
        ("media-size-name", keyword(&media.name)?),
        ("media-left-margin", IppValue::Integer(media.margins.left)),
        (
            "media-bottom-margin",
            IppValue::Integer(media.margins.bottom),
        ),
        ("media-right-margin", IppValue::Integer(media.margins.right)),
        ("media-top-margin", IppValue::Integer(media.margins.top)),
    ];
    if let Some(source) = source {
        items.push(("media-source", keyword(source)?));
    }
    if let Some(kind) = media_type {
        items.push(("media-type", keyword(kind)?));
    }
    collection(items)
}
fn resolution(r: &Resolution) -> IppValue {
    IppValue::Resolution {
        cross_feed: r.x,
        feed: r.y,
        units: 3,
    }
}
// PWG 5100.13-2023 §§6.5.8 and 12.1.2: clients copy the standard
// Job Template members. There is no job-preset-name request selector.
fn preset_collection(key: &str, recipe: &ResolvedRecipe) -> Result<IppValue> {
    let job = &recipe.resolved;
    collection(vec![
        ("preset-name", name(key)?),
        ("preset-category", keyword("site")?),
        (
            "media-col",
            media_collection(&job.media, Some(&job.source), Some(&job.media_type))?,
        ),
        ("copies", IppValue::Integer(job.copies)),
        ("print-quality", IppValue::Enum(job.quality)),
        ("print-color-mode", keyword(&job.color_mode)?),
        ("sides", keyword(&job.sides)?),
        ("orientation-requested", IppValue::Enum(job.orientation)),
        ("printer-resolution", resolution(&job.resolution)),
        ("print-scaling", keyword(&job.print_scaling)?),
    ])
}
pub struct PrinterRuntime {
    pub uptime: i32,
    pub config_changed: i32,
    pub queued: i32,
    pub authentication_required: bool,
}
pub fn printer_attributes(
    reply: &mut IppRequestResponse,
    config: &Config,
    printer: &PrinterConfig,
    cap: &Capabilities,
    status: &UpstreamStatus,
    runtime: PrinterRuntime,
) -> Result<()> {
    let PrinterRuntime {
        uptime,
        config_changed,
        queued,
        authentication_required,
    } = runtime;
    let g = DelimiterTag::PrinterAttributes;
    let d = &cap.defaults;
    // Deliberately absent: `printer-device-id` (the real value carries a serial
    // number, so any rewrite would be a false identity), `ipp-features-supported`
    // (upstream values describe CUPS/device feature sets this frontend does not
    // implement), and `finishings-/output-bin-supported` (Ferriprint has no
    // finishing or output-bin semantics). `media-col-database` therefore carries
    // size and margins only: a media x source x type cross-product would claim
    // every tray accepts every stock, which neither observed source asserts.
    let mut attrs = vec![
        (
            "printer-uri-supported",
            uri(&config.printer_uri(&printer.name))?,
        ),
        (
            "uri-authentication-supported",
            keyword(if authentication_required {
                "basic"
            } else {
                "none"
            })?,
        ),
        (
            "auth-info-required",
            if authentication_required {
                IppValue::Array(vec![keyword("username")?, keyword("password")?])
            } else {
                keyword("none")?
            },
        ),
        (
            "uri-security-supported",
            keyword(if config.tls.is_some() { "tls" } else { "none" })?,
        ),
        ("printer-name", name(&printer.name)?),
        ("printer-info", text(&printer.display_name)?),
        ("printer-location", text(&printer.location)?),
        (
            "printer-make-and-model",
            text(&format!("Ferriprint {}", bounded(&cap.make_and_model, 240)))?,
        ),
        (
            "printer-uuid",
            uri(&format!(
                "urn:uuid:{}",
                uuid::Uuid::new_v5(
                    &uuid::Uuid::NAMESPACE_URL,
                    config.printer_uri(&printer.name).as_bytes()
                )
            ))?,
        ),
        ("printer-state", IppValue::Enum(status.state)),
        (
            "printer-state-reasons",
            strings(
                &if status.reasons.is_empty() {
                    vec!["none".into()]
                } else {
                    status.reasons.clone()
                },
                keyword,
            )?,
        ),
        (
            "printer-state-message",
            text("Native CUPS queue; driver options explicitly mapped by Ferriprint")?,
        ),
        (
            "printer-is-accepting-jobs",
            IppValue::Boolean(status.accepting),
        ),
        ("printer-up-time", IppValue::Integer(uptime)),
        ("queued-job-count", IppValue::Integer(queued)),
        (
            "printer-config-change-time",
            IppValue::Integer(config_changed),
        ),
        ("printer-state-change-time", IppValue::Integer(0)),
        (
            "ipp-versions-supported",
            strings(&["1.1".into(), "2.0".into()], keyword)?,
        ),
        (
            "operations-supported",
            IppValue::Array(OPERATIONS.iter().map(|n| IppValue::Enum(*n)).collect()),
        ),
        ("charset-configured", IppValue::Charset("utf-8".try_into()?)),
        ("charset-supported", IppValue::Charset("utf-8".try_into()?)),
        (
            "natural-language-configured",
            IppValue::NaturalLanguage("en".try_into()?),
        ),
        (
            "generated-natural-language-supported",
            IppValue::NaturalLanguage("en".try_into()?),
        ),
        (
            "document-format-supported",
            strings(&cap.document_formats, mime)?,
        ),
        ("document-format-default", mime(&d.document_format)?),
        ("compression-supported", keyword("none")?),
        ("multiple-document-jobs-supported", IppValue::Boolean(false)),
        (
            "multiple-document-handling-supported",
            keyword("separate-documents-collated-copies")?,
        ),
        (
            "multiple-document-handling-default",
            keyword("separate-documents-collated-copies")?,
        ),
        (
            "job-creation-attributes-supported",
            strings(
                &JOB_KEYS.iter().map(|s| s.to_string()).collect::<Vec<_>>(),
                keyword,
            )?,
        ),
        ("job-ids-supported", IppValue::Boolean(true)),
        ("job-hold-until-supported", keyword("no-hold")?),
        ("job-hold-until-default", keyword("no-hold")?),
        ("copies-default", IppValue::Integer(1)),
        (
            "copies-supported",
            IppValue::RangeOfInteger { min: 1, max: 999 },
        ),
        ("number-up-supported", IppValue::Integer(1)),
        ("number-up-default", IppValue::Integer(1)),
        ("page-ranges-supported", IppValue::Boolean(false)),
        (
            "orientation-requested-supported",
            IppValue::Array((3..=6).map(IppValue::Enum).collect()),
        ),
        ("orientation-requested-default", IppValue::Enum(3)),
        (
            "print-quality-supported",
            IppValue::Array(cap.qualities.iter().map(|n| IppValue::Enum(*n)).collect()),
        ),
        ("print-quality-default", IppValue::Enum(d.quality)),
        (
            "print-color-mode-supported",
            strings(&cap.color_modes, keyword)?,
        ),
        ("print-color-mode-default", keyword(&d.color_mode)?),
        (
            "color-supported",
            IppValue::Boolean(cap.color_modes.iter().any(|m| m == "color")),
        ),
        ("sides-supported", strings(&cap.sides, keyword)?),
        ("sides-default", keyword(&d.sides)?),
        (
            "printer-resolution-supported",
            IppValue::Array(cap.resolutions.iter().map(resolution).collect()),
        ),
        ("printer-resolution-default", resolution(&d.resolution)),
        (
            "print-scaling-supported",
            strings(
                &[
                    "none".into(),
                    "fit".into(),
                    "fill".into(),
                    "auto".into(),
                    "auto-fit".into(),
                ],
                keyword,
            )?,
        ),
        ("print-scaling-default", keyword("none")?),
        ("media-default", keyword(&d.media.name)?),
        ("media-source-supported", strings(&cap.sources, keyword)?),
        ("media-type-supported", strings(&cap.media_types, keyword)?),
        (
            "media-col-supported",
            strings(
                &[
                    "media-size".into(),
                    "media-size-name".into(),
                    "media-left-margin".into(),
                    "media-bottom-margin".into(),
                    "media-right-margin".into(),
                    "media-top-margin".into(),
                    "media-source".into(),
                    "media-type".into(),
                ],
                keyword,
            )?,
        ),
        (
            "media-col-default",
            media_collection(&d.media, Some(&d.source), Some(&d.media_type))?,
        ),
        (
            "media-col-database",
            IppValue::Array(
                cap.media
                    .iter()
                    .map(|m| media_collection(m, None, None))
                    .collect::<Result<_>>()?,
            ),
        ),
        (
            "media-size-supported",
            IppValue::Array(
                cap.media
                    .iter()
                    .map(|m| {
                        collection(vec![
                            ("x-dimension", IppValue::Integer(m.width)),
                            ("y-dimension", IppValue::Integer(m.height)),
                        ])
                    })
                    .collect::<Result<_>>()?,
            ),
        ),
        ("pdl-override-supported", keyword("not-attempted")?),
    ];
    if !cap.recipes.is_empty() {
        attrs.push((
            "job-presets-supported",
            IppValue::Array(
                cap.recipes
                    .iter()
                    .map(|(key, recipe)| preset_collection(key, recipe))
                    .collect::<Result<_>>()?,
            ),
        ));
    }
    let mut media: Vec<_> = cap.media.iter().map(|m| m.name.clone()).collect();
    media.sort();
    media.dedup();
    attrs.push(("media-supported", strings(&media, keyword)?));
    for (key, get) in [
        ("media-left-margin-supported", 0),
        ("media-bottom-margin-supported", 1),
        ("media-right-margin-supported", 2),
        ("media-top-margin-supported", 3),
    ] {
        let mut margins: Vec<_> = cap
            .media
            .iter()
            .map(|m| match get {
                0 => m.margins.left,
                1 => m.margins.bottom,
                2 => m.margins.right,
                _ => m.margins.top,
            })
            .collect();
        margins.sort();
        margins.dedup();
        attrs.push((
            key,
            IppValue::Array(margins.into_iter().map(IppValue::Integer).collect()),
        ));
    }
    if cap.document_formats.iter().any(|f| f == "image/urf") {
        attrs.push(("urf-supported", strings(&status.urf_supported, keyword)?));
    }
    if cap.document_formats.iter().any(|f| f == "image/pwg-raster") {
        attrs.push((
            "pwg-raster-document-type-supported",
            strings(&status.pwg_types, keyword)?,
        ));
        attrs.push((
            "pwg-raster-document-resolution-supported",
            IppValue::Array(cap.resolutions.iter().map(resolution).collect()),
        ));
    }
    let web = http_url(&config.public_url)?;
    attrs.push(("printer-more-info", uri(web.as_str())?));
    for (key, value) in attrs {
        add(reply, g, key, value)?;
    }
    Ok(())
}
/// IPP time-at-* and printer-up-time share one durable initialization origin.
/// Recovery keeps the origin, so previously created jobs never acquire negative ages.
pub fn job_attributes(
    reply: &mut IppRequestResponse,
    config: &Config,
    job: &JobRecord,
    origin: i64,
    uptime: i32,
) -> Result<()> {
    let mut group = IppAttributeGroup::new(DelimiterTag::JobAttributes);
    let mut attributes = vec![
        ("job-id", IppValue::Integer(job.id)),
        (
            "job-uri",
            uri(&format!(
                "{}/jobs/{}",
                config.printer_uri(&job.printer),
                job.id
            ))?,
        ),
        ("job-uuid", uri(&job.uuid)?),
        ("job-printer-uri", uri(&config.printer_uri(&job.printer))?),
        ("job-name", name(&job.title)?),
        ("job-originating-user-name", name(&job.owner)?),
        ("job-state", IppValue::Enum(job.state)),
        ("job-state-reasons", strings(&job.reasons, keyword)?),
        ("job-state-message", text(bounded(&job.message, 1023))?),
        (
            "time-at-creation",
            IppValue::Integer((job.created_at - origin).clamp(0, i32::MAX as i64) as i32),
        ),
        ("job-printer-up-time", IppValue::Integer(uptime)),
        (
            "job-k-octets",
            IppValue::Integer(job.document_bytes.div_ceil(1024).min(i32::MAX as u64) as i32),
        ),
        (
            "number-of-documents",
            IppValue::Integer(i32::from(job.document_bytes > 0)),
        ),
        (
            "job-impressions-completed",
            IppValue::Integer(job.impressions_completed),
        ),
        (
            "job-media-sheets-completed",
            IppValue::Integer(job.sheets_completed),
        ),
        ("copies", IppValue::Integer(job.resolved.copies)),
        ("document-format", mime(&job.document_format)?),
        ("media", keyword(&job.resolved.media.name)?),
        (
            "media-col",
            media_collection(
                &job.resolved.media,
                Some(&job.resolved.source),
                Some(&job.resolved.media_type),
            )?,
        ),
        ("print-quality", IppValue::Enum(job.resolved.quality)),
        ("sides", keyword(&job.resolved.sides)?),
        ("print-color-mode", keyword(&job.resolved.color_mode)?),
        ("printer-resolution", resolution(&job.resolved.resolution)),
    ];
    for (key, time) in [
        ("time-at-processing", job.processing_at),
        ("time-at-completed", job.completed_at),
    ] {
        attributes.push((
            key,
            time.map(|n| IppValue::Integer((n - origin).clamp(0, i32::MAX as i64) as i32))
                .unwrap_or(IppValue::NoValue),
        ));
    }
    for (key, value) in attributes {
        group
            .attributes_mut()
            .push(IppAttribute::with_name(key, value)?);
    }
    reply.attributes_mut().groups_mut().push(group);
    Ok(())
}
pub fn filter_requested(request: &Metadata, reply: &mut IppRequestResponse) {
    let wanted = string_list(attr(
        request,
        DelimiterTag::OperationAttributes,
        "requested-attributes",
    ));
    if wanted.is_empty()
        || wanted.iter().any(|s| {
            matches!(
                s.as_str(),
                "all" | "printer-description" | "job-description" | "job-template"
            )
        })
    {
        return;
    }
    for group in reply.attributes_mut().groups_mut() {
        if matches!(
            group.tag(),
            DelimiterTag::JobAttributes | DelimiterTag::PrinterAttributes
        ) {
            group
                .attributes_mut()
                .retain(|a| wanted.iter().any(|s| s == a.name().as_str()));
        }
    }
}
