// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{
    admin::{self, Escape},
    auth::Principal,
    config::{Defaults, MediaTypeConfig, PrinterConfig, ProfileSettings},
    domain::{DriverOptions, JobTemplate, Resolution},
    profile::{PpdInventory, PrinterProfile},
    recipe_admin::{self, field},
    server::App,
    setup::{self, NativeQueue, PrinterIdentity, SetupChoices},
    upstream::{Upstream, UpstreamStatus},
};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Extension, Form, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use std::{
    collections::{BTreeMap, HashMap},
    fmt::Write,
    path::PathBuf,
    sync::Arc,
    time::{Duration, Instant},
};

const FORM_LIMIT: usize = 1024 * 1024;
const MAX_DRAFTS: usize = 16;
const LIFETIME: Duration = Duration::from_secs(15 * 60);
const FORMATS: &[&str] = &[
    "application/pdf",
    "image/jpeg",
    "image/png",
    "image/pwg-raster",
    "image/urf",
];

#[derive(Clone, Copy, PartialEq, Eq)]
enum Step {
    Identity,
    Mappings,
    Defaults,
    Recipes,
    Review,
}

/// Private, bounded, memory-only review state. Neither native endpoints nor PPD
/// paths come from forms. Exact acquired bytes survive until commit or expiry.
#[derive(Clone)]
pub struct Draft {
    username: String,
    account_revision: u64,
    expires: Instant,
    revision: u64,
    in_flight: bool,
    source_id: String,
    queue: NativeQueue,
    ppd: Arc<Vec<u8>>,
    inventory: Arc<PpdInventory>,
    status: UpstreamStatus,
    identity: PrinterIdentity,
    settings: ProfileSettings,
    native_confirmed: bool,
    profile: Option<Arc<PrinterProfile>>,
    step: Step,
    catalog_revision: u64,
}
impl Draft {
    fn belongs_to(&self, principal: &Principal) -> bool {
        self.username == principal.username && self.account_revision == principal.revision
    }
    fn choices(&self) -> SetupChoices {
        SetupChoices {
            confirmed_native_mappings: self.native_confirmed,
            settings: self.settings.clone(),
            previews: vec![JobTemplate::default()],
        }
    }
}

pub fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/admin/onboarding", get(index))
        .route(
            "/admin/onboarding/start",
            post(start).layer(DefaultBodyLimit::max(FORM_LIMIT)),
        )
        .route(
            "/admin/onboarding/{token}",
            get(show)
                .post(advance)
                .layer(DefaultBodyLimit::max(FORM_LIMIT)),
        )
}
fn prune(drafts: &mut HashMap<String, Draft>) {
    let now = Instant::now();
    drafts.retain(|_, draft| draft.expires > now);
}
fn current_admin(app: &App, principal: &Principal) -> bool {
    principal.is_admin() && app.auth.current(principal)
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct InventoryQuery {
    source: Option<String>,
}
async fn index(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Query(query): Query<InventoryQuery>,
) -> Response {
    if !current_admin(&app, &principal) {
        return admin::error(StatusCode::FORBIDDEN, "Current administrator required");
    }
    let mut body = admin::page("Add an existing native CUPS queue");
    body.push_str("<p>This wizard only reads a protected trusted server's inventory, metadata and PPD. It never creates or changes a native queue, driver, device or default, and never submits a print. Native CUPS remains the renderer.</p>");
    if app.config.cups_sources.is_empty() {
        body.push_str("<p>No trusted CUPS sources are configured. An operator must configure a protected cups_sources entry before browser onboarding. Server URIs, credentials, CA files and PPD paths cannot be entered here.</p>");
        return admin::finish(body).into_response();
    }
    body.push_str("<form method=\"get\"><label>Protected CUPS source <select name=\"source\">");
    for source in &app.config.cups_sources {
        let _ = write!(
            body,
            "<option value=\"{}\"{}>{}</option>",
            Escape(&source.id),
            if query.source.as_deref() == Some(source.id.as_str()) {
                " selected"
            } else {
                ""
            },
            Escape(&source.id)
        );
    }
    body.push_str("</select></label> <button>Refresh read-only queue inventory</button></form>");
    if let Some(source) = query.source {
        if setup::source(&app.config, &source).is_err() {
            return admin::error(
                StatusCode::BAD_REQUEST,
                "Choose an enumerated protected CUPS source",
            );
        }
        let queues = match setup::inventory(&app.config, &source).await {
            Ok(queues) => queues,
            Err(e) => {
                return admin::error(
                    StatusCode::BAD_GATEWAY,
                    format!(
                        "Trusted CUPS inventory unavailable or invalid; no printer was installed: {e:#}"
                    ),
                );
            }
        };
        let Some(csrf) = app.auth.csrf(&principal) else {
            return admin::error(StatusCode::FORBIDDEN, "Account changed; authenticate again");
        };
        body.push_str("<table><thead><tr><th>Existing queue</th><th>Model provenance</th><th>Native state</th><th>Accepting</th><th>Read-only acquisition</th></tr></thead><tbody>");
        for queue in queues {
            let _ = write!(
                body,
                "<tr><td>{} ({})</td><td>{}</td><td>{}</td><td>{}</td><td><form method=\"post\" action=\"/admin/onboarding/start\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"source\" value=\"{}\"><input type=\"hidden\" name=\"queue\" value=\"{}\"><button>Acquire current PPD and review</button></form></td></tr>",
                Escape(&queue.display_name),
                Escape(&queue.name),
                Escape(&queue.model),
                queue.state,
                queue.accepting,
                Escape(csrf),
                Escape(&source),
                Escape(&queue.name)
            );
        }
        body.push_str("</tbody></table>");
    }
    admin::finish(body).into_response()
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct StartForm {
    csrf: String,
    source: String,
    queue: String,
}

/// Build a read-only queue client exclusively from protected source data and a
/// freshly enumerated name. No request supplies transport or filesystem data.
fn queue_config(
    app: &App,
    source_id: &str,
    queue: &str,
    identity: &PrinterIdentity,
) -> Result<PrinterConfig> {
    let source = setup::source(&app.config, source_id)?;
    Ok(PrinterConfig {
        name: identity.name.clone(),
        display_name: identity.display_name.clone(),
        service_name: identity.service_name.clone(),
        location: identity.location.clone(),
        ppd: PathBuf::new(),
        upstream_uri: source.queue_uri(queue)?,
        upstream_username: source.username.clone(),
        upstream_auth: source.auth.clone(),
        upstream_ca_certificate: source.ca_certificate.clone(),
        telemetry: None,
    })
}
async fn start(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    Form(form): Form<StartForm>,
) -> Response {
    if !principal.is_admin() || !admin::csrf_valid(&app, &principal, &headers, &form.csrf) {
        return admin::error(
            StatusCode::FORBIDDEN,
            "Current administrator and valid same-origin CSRF token required",
        );
    }
    if setup::source(&app.config, &form.source).is_err() {
        return admin::error(StatusCode::BAD_REQUEST, "Unknown protected CUPS source");
    }
    {
        let mut drafts = app.onboarding.lock();
        prune(&mut drafts);
        if drafts.len() >= MAX_DRAFTS {
            return admin::error(
                StatusCode::TOO_MANY_REQUESTS,
                "At most 16 private onboarding drafts are retained for 15 minutes; finish or discard a draft first",
            );
        }
    }
    let acquired = async {
        let (queue, ppd) = setup::acquire(&app.config, &form.source, &form.queue).await?;
        let inventory = PpdInventory::from_bytes(&ppd)?;
        let identity = PrinterIdentity {
            name: queue.name.clone(),
            display_name: queue.display_name.clone(),
            service_name: queue.display_name.chars().take(50).collect(),
            location: String::new(),
        };
        let native = queue_config(&app, &form.source, &queue.name, &identity)?;
        let status = Upstream::new(
            &native,
            Duration::from_secs(app.config.backend_timeout_secs),
        )?
        .inspect()
        .await?;
        ensure!(status.accepting, "native queue is not accepting jobs");
        let document_formats: Vec<_> = FORMATS
            .iter()
            .filter(|format| {
                status
                    .document_formats
                    .iter()
                    .any(|native| native == **format)
            })
            .map(|format| (*format).to_owned())
            .collect();
        ensure!(
            !document_formats.is_empty(),
            "native queue has no implemented PDF/JPEG/PNG/PWG/URF decoder intersection"
        );
        let mut resolutions = Vec::new();
        if document_formats.iter().any(|format| format == "image/urf") {
            for value in status
                .urf_supported
                .iter()
                .filter_map(|value| value.strip_prefix("RS"))
                .flat_map(|value| value.split('-'))
                .filter_map(|value| value.parse::<i32>().ok())
                .filter(|value| *value > 0)
            {
                let resolution = Resolution { x: value, y: value };
                if !resolutions.contains(&resolution) {
                    resolutions.push(resolution);
                }
            }
            ensure!(
                !resolutions.is_empty() && resolutions.len() <= 32,
                "native URF decoder did not report bounded usable input resolutions"
            );
        } else {
            resolutions.push(Resolution { x: 300, y: 300 });
        }
        let settings = ProfileSettings {
            schema_version: 1,
            sources: BTreeMap::new(),
            media_types: BTreeMap::new(),
            color_modes: BTreeMap::new(),
            sides: BTreeMap::new(),
            defaults: Defaults {
                media: String::new(),
                borderless: false,
                source: String::new(),
                media_type: String::new(),
                quality: 4,
                color_mode: String::new(),
                sides: String::new(),
            },
            recipes: BTreeMap::new(),
            declared_ready: Vec::new(),
            document_formats,
            resolutions,
        };
        Ok::<_, anyhow::Error>((queue, ppd, inventory, identity, status, settings))
    }
    .await;
    let (queue, ppd, inventory, identity, status, settings) = match acquired {
        Ok(value) => value,
        Err(e) => {
            return admin::error(
                StatusCode::BAD_GATEWAY,
                format!(
                    "Could not acquire this enumerated queue's native PPD/decoder metadata; nothing installed: {e:#}"
                ),
            );
        }
    };
    let catalog_revision = app.catalog.lock().await.revision;
    let token = uuid::Uuid::new_v4().to_string();
    let draft = Draft {
        username: principal.username.clone(),
        account_revision: principal.revision,
        expires: Instant::now() + LIFETIME,
        revision: 0,
        in_flight: false,
        source_id: form.source,
        queue,
        ppd: Arc::new(ppd),
        inventory: Arc::new(inventory),
        status,
        identity,
        settings,
        native_confirmed: false,
        profile: None,
        step: Step::Identity,
        catalog_revision,
    };
    let inserted = app.auth.with_access(&principal, None, true, || {
        let mut drafts = app.onboarding.lock();
        prune(&mut drafts);
        ensure!(
            drafts.len() < MAX_DRAFTS,
            "Onboarding draft capacity reached; no printer installed"
        );
        drafts.insert(token.clone(), draft);
        Ok(())
    });
    if let Err(e) = inserted {
        return admin::error(StatusCode::CONFLICT, e);
    }
    Redirect::to(&format!("/admin/onboarding/{token}")).into_response()
}

fn snapshot(app: &App, principal: &Principal, token: &str) -> Result<Draft> {
    ensure!(
        current_admin(app, principal),
        "Current administrator required"
    );
    let mut drafts = app.onboarding.lock();
    prune(&mut drafts);
    let draft = drafts
        .get(token)
        .filter(|draft| draft.belongs_to(principal))
        .context("Draft not found, expired, or account revision changed; begin a fresh review")?;
    ensure!(
        !draft.in_flight,
        "This draft is already being prepared; do not resubmit"
    );
    Ok(draft.clone())
}
async fn show(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Path(token): Path<String>,
) -> Response {
    match snapshot(&app, &principal, &token) {
        Ok(draft) => render(&app, &principal, &token, &draft, None, StatusCode::OK),
        Err(e) => admin::error(StatusCode::NOT_FOUND, e),
    }
}

fn begin_form(body: &mut String, token: &str, draft: &Draft, csrf: &str, intent: &str) {
    let _ = write!(
        body,
        "<form method=\"post\" action=\"/admin/onboarding/{}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"revision\" value=\"{}\"><input type=\"hidden\" name=\"intent\" value=\"{}\">",
        Escape(token),
        Escape(csrf),
        draft.revision,
        Escape(intent)
    );
}
fn confirmation(body: &mut String, message: &str) {
    let _ = write!(
        body,
        "<p><label><input type=\"checkbox\" name=\"confirmed\" value=\"true\" required> {}</label></p>",
        Escape(message)
    );
}
fn native_controls(body: &mut String, inventory: &PpdInventory, prefix: &str) {
    for (index, (option, choices)) in inventory
        .choices
        .iter()
        .filter(|(option, _)| !matches!(option.as_str(), "PageSize" | "PageRegion" | "InputSlot"))
        .enumerate()
    {
        let _ = write!(
            body,
            "<p><label>Native {} <select name=\"{}_{index}\"><option value=\"\">Native default ({})</option>",
            Escape(option),
            Escape(prefix),
            Escape(
                inventory
                    .defaults
                    .get(option)
                    .map_or("unspecified", String::as_str)
            )
        );
        for choice in choices {
            let _ = write!(
                body,
                "<option value=\"{}\">{}</option>",
                Escape(choice),
                Escape(choice)
            );
        }
        body.push_str("</select></label></p>");
    }
}
fn json_preview(body: &mut String, value: &impl serde::Serialize) {
    let _ = write!(
        body,
        "<pre>{}</pre>",
        Escape(&serde_json::to_string_pretty(value).expect("review data serializes"))
    );
}
fn render(
    app: &App,
    principal: &Principal,
    token: &str,
    draft: &Draft,
    failure: Option<&str>,
    status: StatusCode,
) -> Response {
    let Some(csrf) = app.auth.csrf(principal) else {
        return admin::error(
            StatusCode::FORBIDDEN,
            "Account changed; begin a fresh review",
        );
    };
    let mut body = admin::page("Existing CUPS queue onboarding");
    if let Some(failure) = failure {
        let _ = write!(
            body,
            "<p class=\"warning\">Not applied: {}. Your private draft is retained for correction.</p>",
            Escape(failure)
        );
    }
    let step = match draft.step {
        Step::Identity => "1 · Ferriprint identity",
        Step::Mappings => "2 · Explicit native mappings",
        Step::Defaults => "3 · Ordinary defaults",
        Step::Recipes => "4 · Complete named recipes",
        Step::Review => "5 · Full nonprinting review and commit",
    };
    let _ = write!(
        body,
        "<h2>{step}</h2><p>Protected source <strong>{}</strong>; fresh native queue <strong>{}</strong>; reported model <strong>{}</strong>; native state {}; accepting {}. Review revision {}. Draft expires in at most 15 minutes and is bound to your current account revision.</p>",
        Escape(&draft.source_id),
        Escape(&draft.queue.name),
        Escape(&draft.queue.model),
        draft.status.state,
        draft.status.accepting,
        draft.revision
    );
    body.push_str("<p>No native print or queue/default/driver mutation occurs. Native labels such as HIGH and input DPI do not establish material or engine-quality semantics. Confirm only meanings you know are correct for this installed driver.</p><details><summary>Actual native numeric pages, margins, choices, defaults and constraints</summary>");
    json_preview(&mut body, draft.inventory.as_ref());
    body.push_str("</details><details><summary>Actual native decoder formats and input capabilities</summary>");
    json_preview(
        &mut body,
        &serde_json::json!({"document_formats": draft.status.document_formats, "urf_supported": draft.status.urf_supported, "pwg_types": draft.status.pwg_types}),
    );
    body.push_str("</details>");
    match draft.step {
        Step::Identity => {
            begin_form(&mut body, token, draft, csrf, "identity");
            recipe_admin::input(
                &mut body,
                "Unique Ferriprint name",
                "name",
                &draft.identity.name,
                63,
            );
            recipe_admin::input(
                &mut body,
                "Display name",
                "display_name",
                &draft.identity.display_name,
                200,
            );
            recipe_admin::input(
                &mut body,
                "Unique discovery service label",
                "service_name",
                &draft.identity.service_name,
                63,
            );
            let _ = write!(
                body,
                "<p><label>Location <input name=\"location\" value=\"{}\" maxlength=\"200\"></label></p><button>Continue to explicit mappings</button></form>",
                Escape(&draft.identity.location)
            );
        }
        Step::Mappings => {
            body.push_str("<h3>Current explicitly selected mappings</h3>");
            json_preview(
                &mut body,
                &serde_json::json!({"sources":draft.settings.sources,"materials":draft.settings.media_types,"color_modes":draft.settings.color_modes,"sides":draft.settings.sides}),
            );
            body.push_str("<p>Add or replace one mapping at a time. Every native selector is taken from this exact PPD; empty means native default, not an inferred semantic. The shared profile resolver validates all choices, defaults, recipes and cross-option constraints before review.</p>");
            begin_form(&mut body, token, draft, csrf, "source");
            body.push_str("<fieldset><legend>Tray/source mapping</legend>");
            recipe_admin::input(
                &mut body,
                "Standard source keyword (for example main or tray-1; auto if no InputSlot)",
                "key",
                if draft.inventory.choices.contains_key("InputSlot") {
                    "main"
                } else {
                    "auto"
                },
                127,
            );
            let _ = write!(
                body,
                "<p><label>Native InputSlot <select name=\"source_native\"><option value=\"\">Native default / no InputSlot</option>"
            );
            if let Some(choices) = draft.inventory.choices.get("InputSlot") {
                for choice in choices {
                    let _ = write!(
                        body,
                        "<option value=\"{}\">{}</option>",
                        Escape(choice),
                        Escape(choice)
                    );
                }
            }
            body.push_str("</select></label></p>");
            confirmation(&mut body, "I explicitly confirm this tray/source mapping");
            body.push_str("<button>Add or replace source</button></fieldset></form>");
            begin_form(&mut body, token, draft, csrf, "material");
            body.push_str("<fieldset><legend>Material with explicit per-quality native choices</legend><p><label>Standard material keyword <input name=\"key\" list=\"material-keywords\" maxlength=\"127\" required></label></p><datalist id=\"material-keywords\"><option value=\"stationery\"><option value=\"photographic-glossy\"><option value=\"photographic-matte\"></datalist><h4>Normal (required)</h4>");
            native_controls(&mut body, &draft.inventory, "normal");
            body.push_str("<details><summary>Optional draft mapping</summary><label><input type=\"checkbox\" name=\"draft_enabled\" value=\"true\"> Enable explicitly supported draft</label>");
            native_controls(&mut body, &draft.inventory, "draft");
            body.push_str("</details><details><summary>Optional high mapping</summary><label><input type=\"checkbox\" name=\"high_enabled\" value=\"true\"> Enable explicitly supported high</label>");
            native_controls(&mut body, &draft.inventory, "high");
            body.push_str("</details>");
            confirmation(
                &mut body,
                "I explicitly confirm this material and each enabled quality mapping; native labels/DPI are not proof",
            );
            body.push_str("<button>Add or replace material</button></fieldset></form>");
            for (intent, legend, values) in [
                ("color", "Color mode", &["color", "monochrome"][..]),
                (
                    "sides",
                    "Sides",
                    &["one-sided", "two-sided-long-edge", "two-sided-short-edge"][..],
                ),
            ] {
                begin_form(&mut body, token, draft, csrf, intent);
                let _ = write!(body, "<fieldset><legend>{legend} mapping</legend>");
                admin::selector(&mut body, legend, "key", values.iter().copied(), values[0]);
                native_controls(&mut body, &draft.inventory, "native");
                confirmation(
                    &mut body,
                    "I explicitly confirm this mode's native choices (including any native defaults)",
                );
                body.push_str("<button>Add or replace mode</button></fieldset></form>");
            }
            body.push_str("<details><summary>Remove an incorrect draft mapping</summary>");
            for (intent, keys) in [
                (
                    "remove_source",
                    draft.settings.sources.keys().collect::<Vec<_>>(),
                ),
                (
                    "remove_material",
                    draft.settings.media_types.keys().collect(),
                ),
                ("remove_color", draft.settings.color_modes.keys().collect()),
                ("remove_sides", draft.settings.sides.keys().collect()),
            ] {
                if keys.is_empty() {
                    continue;
                }
                begin_form(&mut body, token, draft, csrf, intent);
                admin::selector(
                    &mut body,
                    intent,
                    "key",
                    keys.iter().map(|key| key.as_str()),
                    "",
                );
                body.push_str("<button>Remove from draft only</button></form>");
            }
            for key in draft.settings.recipes.keys() {
                begin_form(&mut body, token, draft, csrf, "remove_recipe");
                let _ = write!(
                    body,
                    "<input type=\"hidden\" name=\"key\" value=\"{}\"><button>Remove recipe {} before changing its mappings</button></form>",
                    Escape(key),
                    Escape(key)
                );
            }
            body.push_str("</details><details><summary>Input raster resolutions (not native engine quality)</summary>");
            json_preview(&mut body, &draft.settings.resolutions);
            begin_form(&mut body, token, draft, csrf, "resolution");
            body.push_str("<label>Horizontal input DPI <input type=\"number\" min=\"1\" name=\"x\" value=\"300\" required></label> <label>Vertical input DPI <input type=\"number\" min=\"1\" name=\"y\" value=\"300\" required></label><button>Add explicit input resolution</button></form></details>");
            begin_form(&mut body, token, draft, csrf, "mappings_done");
            confirmation(
                &mut body,
                "I have reviewed all tray, material/quality, color and sides meanings. Unsupported qualities/modes have been omitted.",
            );
            body.push_str("<button>Continue to defaults</button></form>");
        }
        Step::Defaults => {
            let d = &draft.settings.defaults;
            begin_form(&mut body, token, draft, csrf, "defaults");
            recipe_admin::media_selector(
                &mut body,
                draft.inventory.pages.iter().map(|page| &page.media),
                "media_variant",
                &d.media,
                d.borderless,
            );
            admin::selector(
                &mut body,
                "Source",
                "source",
                draft.settings.sources.keys().map(String::as_str),
                &d.source,
            );
            admin::selector(
                &mut body,
                "Material",
                "media_type",
                draft.settings.media_types.keys().map(String::as_str),
                &d.media_type,
            );
            let qualities = qualities(&draft.settings);
            admin::selector(
                &mut body,
                "Quality (3 draft, 4 normal, 5 high)",
                "quality",
                qualities.iter().map(String::as_str),
                &d.quality.to_string(),
            );
            admin::selector(
                &mut body,
                "Color",
                "color_mode",
                draft.settings.color_modes.keys().map(String::as_str),
                &d.color_mode,
            );
            admin::selector(
                &mut body,
                "Sides",
                "sides",
                draft.settings.sides.keys().map(String::as_str),
                &d.sides,
            );
            body.push_str(
                "<fieldset><legend>Frontend formats restricted to actual native decoders</legend>",
            );
            for (index, format) in FORMATS.iter().enumerate().filter(|(_, format)| {
                draft
                    .status
                    .document_formats
                    .iter()
                    .any(|native| native == **format)
            }) {
                let _ = write!(
                    body,
                    "<p><label><input type=\"checkbox\" name=\"format_{index}\" value=\"true\"{}> {}</label></p>",
                    if draft
                        .settings
                        .document_formats
                        .iter()
                        .any(|current| current.as_str() == *format)
                    {
                        " checked"
                    } else {
                        ""
                    },
                    Escape(format)
                );
            }
            body.push_str("</fieldset><p>Declared-ready paper is left empty: no tray contents are sensed or inferred.</p><button>Validate defaults and continue to complete recipes</button></form>");
        }
        Step::Recipes => {
            let profile = draft
                .profile
                .as_ref()
                .expect("validated defaults precede recipes");
            body.push_str("<h3>Fully resolved defaults</h3>");
            json_preview(&mut body, &profile.capabilities().defaults);
            body.push_str("<h3>Complete named recipes in this draft</h3>");
            if profile.recipes().is_empty() {
                body.push_str("<p>No recipes added. You may add multiple complete tickets, or explicitly continue without recipes.</p>");
            }
            for (key, recipe) in profile.recipes() {
                let _ = write!(
                    body,
                    "<h4>{} — {}</h4>",
                    Escape(key),
                    Escape(&recipe.display_name)
                );
                json_preview(&mut body, &recipe.resolved);
                begin_form(&mut body, token, draft, csrf, "remove_recipe");
                let _ = write!(
                    body,
                    "<input type=\"hidden\" name=\"key\" value=\"{}\"><button>Remove this draft recipe</button></form>",
                    Escape(key)
                );
            }
            body.push_str("<h3>Add or replace one complete recipe</h3>");
            begin_form(&mut body, token, draft, csrf, "recipe");
            recipe_admin::recipe_fields(
                &mut body,
                profile.capabilities(),
                "",
                &recipe_admin::default_recipe(profile.capabilities()),
            );
            body.push_str("<button>Validate and add this complete recipe to draft</button></form>");
            begin_form(&mut body, token, draft, csrf, "review");
            body.push_str("<p>Review freshly re-enumerates the native queue, compares the exact PPD bytes, and validates the live decoder intersection. No printing occurs.</p><button>Prepare full nonprinting review</button></form>");
        }
        Step::Review => {
            let profile = draft
                .profile
                .as_ref()
                .expect("prepared review has a profile");
            let _ = write!(
                body,
                "<h3>Trusted binding and frontend identity</h3><p>Source {} → native queue {}. Ferriprint name {}; display {}; service {}; location {}. Reviewed catalog revision {}. Usable endpoint after commit: <code>{}</code>.</p><p>Nonprinting validation passed through the shared PPD parser, native constraints, live decoder intersection, portable discovery preflight and complete recipe resolver. The actual PPD is compared again on final commit.</p>",
                Escape(&draft.source_id),
                Escape(&draft.queue.name),
                Escape(&draft.identity.name),
                Escape(&draft.identity.display_name),
                Escape(&draft.identity.service_name),
                Escape(&draft.identity.location),
                draft.catalog_revision,
                Escape(&app.config.printer_uri(&draft.identity.name))
            );
            body.push_str("<h3>Actual selected native mappings</h3>");
            json_preview(&mut body, &draft.settings);
            body.push_str(
                "<h3>Complete resolved defaults, including numeric margins and native options</h3>",
            );
            json_preview(&mut body, &profile.capabilities().defaults);
            body.push_str("<h3>Complete resolved named recipes</h3>");
            json_preview(&mut body, profile.recipes());
            body.push_str("<p>ACL defaults: administrators have explicit global administration rights. No regular user or anonymous identity is automatically granted this new printer. Grant controls are available after HTTP activation. Discovery is separate: saving cannot claim successful advertisement.</p>");
            begin_form(&mut body, token, draft, csrf, "commit");
            confirmation(
                &mut body,
                "I confirm every reviewed native semantic mapping, default and complete recipe, this exact source/queue/PPD binding, the new endpoint and deny-by-default ordinary/anonymous access",
            );
            body.push_str(
                "<button>Commit durable printer and activate HTTP once (no print)</button></form>",
            );
        }
    }
    if draft.step != Step::Identity {
        begin_form(&mut body, token, draft, csrf, "back");
        body.push_str("<button>Back to previous step (invalidates final review)</button></form>");
    }
    begin_form(&mut body, token, draft, csrf, "discard");
    body.push_str("<button>Discard private draft</button></form><p><a href=\"/admin/onboarding\">Start a fresh trusted-source/PPD review</a></p>");
    (status, admin::finish(body)).into_response()
}
fn qualities(settings: &ProfileSettings) -> Vec<String> {
    let mut result = Vec::new();
    if settings
        .media_types
        .values()
        .any(|material| material.draft.is_some())
    {
        result.push("3".into());
    }
    result.push("4".into());
    if settings
        .media_types
        .values()
        .any(|material| material.high.is_some())
    {
        result.push("5".into());
    }
    result
}
fn confirmed(fields: &HashMap<String, String>) -> Result<()> {
    ensure!(
        field(fields, "confirmed")? == "true",
        "Explicit semantic confirmation is required"
    );
    Ok(())
}
fn native_mapping(draft: &Draft, fields: &HashMap<String, String>, prefix: &str) -> DriverOptions {
    draft
        .inventory
        .choices
        .iter()
        .filter(|(option, _)| !matches!(option.as_str(), "PageSize" | "PageRegion" | "InputSlot"))
        .enumerate()
        .filter_map(|(index, (option, _))| {
            fields
                .get(&format!("{prefix}_{index}"))
                .filter(|choice| !choice.is_empty())
                .map(|choice| (option.clone(), choice.clone()))
        })
        .collect()
}
fn allowed_fields(
    draft: &Draft,
    fields: &HashMap<String, String>,
    names: &[&str],
    prefixes: &[&str],
) -> Result<()> {
    let count = draft
        .inventory
        .choices
        .keys()
        .filter(|option| !matches!(option.as_str(), "PageSize" | "PageRegion" | "InputSlot"))
        .count();
    ensure!(
        fields
            .keys()
            .all(|key| ["csrf", "revision", "intent"].contains(&key.as_str())
                || names.contains(&key.as_str())
                || prefixes.iter().any(|prefix| key
                    .strip_prefix(prefix)
                    .and_then(|rest| rest.strip_prefix('_'))
                    .and_then(|index| index.parse::<usize>().ok())
                    .is_some_and(|index| index < count))),
        "Unexpected form field; native server, URI, path, backend and transport are not editable"
    );
    Ok(())
}
fn validate_profile(draft: &mut Draft) -> Result<()> {
    let mut profile = PrinterProfile::from_ppd_bytes(&draft.ppd, &draft.settings)?;
    profile.restrict_formats(&draft.status.document_formats)?;
    profile.urf_supported(&draft.status.urf_supported)?;
    draft.profile = Some(Arc::new(profile));
    Ok(())
}
fn mutate(
    app: &App,
    draft: &mut Draft,
    fields: &HashMap<String, String>,
    intent: &str,
) -> Result<()> {
    match (draft.step, intent) {
        (_, "back") => {
            allowed_fields(draft, fields, &[], &[])?;
            draft.step = match draft.step {
                Step::Identity => bail!("Already at first step"),
                Step::Mappings => Step::Identity,
                Step::Defaults => Step::Mappings,
                Step::Recipes => Step::Defaults,
                Step::Review => Step::Recipes,
            };
        }
        (Step::Identity, "identity") => {
            allowed_fields(
                draft,
                fields,
                &["name", "display_name", "service_name", "location"],
                &[],
            )?;
            draft.identity = PrinterIdentity {
                name: field(fields, "name")?.into(),
                display_name: field(fields, "display_name")?.into(),
                service_name: field(fields, "service_name")?.into(),
                location: field(fields, "location")?.into(),
            };
            let mut validation = app.config.clone();
            validation.printers.push(queue_config(
                app,
                &draft.source_id,
                &draft.queue.name,
                &draft.identity,
            )?);
            validation.validate()?;
            draft.step = Step::Mappings;
        }
        (Step::Mappings, "source") => {
            allowed_fields(draft, fields, &["key", "source_native", "confirmed"], &[])?;
            confirmed(fields)?;
            draft.settings.sources.insert(
                field(fields, "key")?.into(),
                field(fields, "source_native")?.into(),
            );
            draft.native_confirmed = false;
        }
        (Step::Mappings, "material") => {
            allowed_fields(
                draft,
                fields,
                &["key", "draft_enabled", "high_enabled", "confirmed"],
                &["normal", "draft", "high"],
            )?;
            confirmed(fields)?;
            ensure!(
                fields.get("draft_enabled").is_none_or(|v| v == "true")
                    && fields.get("high_enabled").is_none_or(|v| v == "true"),
                "Invalid optional quality selection"
            );
            let material = MediaTypeConfig {
                normal: native_mapping(draft, fields, "normal"),
                draft: fields
                    .contains_key("draft_enabled")
                    .then(|| native_mapping(draft, fields, "draft")),
                high: fields
                    .contains_key("high_enabled")
                    .then(|| native_mapping(draft, fields, "high")),
            };
            draft
                .settings
                .media_types
                .insert(field(fields, "key")?.into(), material);
            draft.native_confirmed = false;
        }
        (Step::Mappings, "color" | "sides") => {
            allowed_fields(draft, fields, &["key", "confirmed"], &["native"])?;
            confirmed(fields)?;
            let mapping = native_mapping(draft, fields, "native");
            let target = if intent == "color" {
                &mut draft.settings.color_modes
            } else {
                &mut draft.settings.sides
            };
            target.insert(field(fields, "key")?.into(), mapping);
            draft.native_confirmed = false;
        }
        (Step::Mappings, "remove_source" | "remove_material" | "remove_color" | "remove_sides") => {
            allowed_fields(draft, fields, &["key"], &[])?;
            let key = field(fields, "key")?;
            match intent {
                "remove_source" => {
                    draft.settings.sources.remove(key);
                }
                "remove_material" => {
                    draft.settings.media_types.remove(key);
                }
                "remove_color" => {
                    draft.settings.color_modes.remove(key);
                }
                _ => {
                    draft.settings.sides.remove(key);
                }
            }
            draft.native_confirmed = false;
        }
        (Step::Mappings, "resolution") => {
            allowed_fields(draft, fields, &["x", "y"], &[])?;
            let resolution = Resolution {
                x: field(fields, "x")?.parse()?,
                y: field(fields, "y")?.parse()?,
            };
            ensure!(
                resolution.x > 0 && resolution.y > 0 && draft.settings.resolutions.len() < 32,
                "Input resolution must be positive; at most 32 entries"
            );
            if !draft.settings.resolutions.contains(&resolution) {
                draft.settings.resolutions.push(resolution);
            }
        }
        (Step::Mappings, "mappings_done") => {
            allowed_fields(draft, fields, &["confirmed"], &[])?;
            confirmed(fields)?;
            draft.settings.validate()?;
            draft.native_confirmed = true;
            draft.step = Step::Defaults;
        }
        (Step::Defaults, "defaults") => {
            let mut names = vec![
                "media_variant",
                "source",
                "media_type",
                "quality",
                "color_mode",
                "sides",
            ];
            let format_fields: Vec<_> = (0..FORMATS.len())
                .map(|index| format!("format_{index}"))
                .collect();
            names.extend(format_fields.iter().map(String::as_str));
            allowed_fields(draft, fields, &names, &[])?;
            let (media, borderless) = recipe_admin::parse_media(field(fields, "media_variant")?)?;
            draft.settings.defaults = Defaults {
                media,
                borderless,
                source: field(fields, "source")?.into(),
                media_type: field(fields, "media_type")?.into(),
                quality: field(fields, "quality")?.parse()?,
                color_mode: field(fields, "color_mode")?.into(),
                sides: field(fields, "sides")?.into(),
            };
            draft.settings.document_formats.clear();
            for (index, format) in FORMATS.iter().enumerate() {
                if let Some(value) = fields.get(&format!("format_{index}")) {
                    ensure!(
                        value == "true"
                            && draft
                                .status
                                .document_formats
                                .iter()
                                .any(|native| native.as_str() == *format),
                        "Choose only actual native decoder formats"
                    );
                    draft.settings.document_formats.push((*format).into());
                }
            }
            validate_profile(draft)?;
            draft.step = Step::Recipes;
        }
        (Step::Recipes, "recipe") => {
            allowed_fields(draft, fields, recipe_admin::RECIPE_FIELDS, &[])?;
            let (key, recipe) = recipe_admin::read_recipe(fields)?;
            draft.settings.recipes.insert(key, recipe);
            validate_profile(draft)?;
        }
        (Step::Mappings | Step::Recipes, "remove_recipe") => {
            allowed_fields(draft, fields, &["key"], &[])?;
            ensure!(
                draft
                    .settings
                    .recipes
                    .remove(field(fields, "key")?)
                    .is_some(),
                "Unknown draft recipe"
            );
            if draft.step == Step::Recipes {
                validate_profile(draft)?;
            }
        }
        _ => bail!("This action does not match the current wizard step; reload the draft"),
    }
    ensure!(
        draft.settings.sources.len() <= 128
            && draft.settings.media_types.len() <= 128
            && draft.settings.color_modes.len() <= 2
            && draft.settings.sides.len() <= 3
            && draft.settings.recipes.len() <= 128,
        "Too many draft mapping entries"
    );
    ensure!(
        toml::to_string(&draft.settings)?.len() <= 256 * 1024,
        "Profile exceeds the managed 256 KiB limit"
    );
    Ok(())
}

/// Compare-and-swap the reviewed form revision. Concurrent tabs cannot silently
/// overwrite another step or commit an obsolete review.
fn store(
    app: &App,
    principal: &Principal,
    token: &str,
    original: &Draft,
    mut next: Draft,
) -> Result<Draft> {
    app.auth.with_access(principal, None, true, || {
        let mut drafts = app.onboarding.lock();
        prune(&mut drafts);
        let current = drafts
            .get(token)
            .filter(|draft| draft.belongs_to(principal))
            .context("Draft expired or account changed; start a fresh review")?;
        ensure!(
            current.revision == original.revision && current.in_flight == original.in_flight,
            "Draft changed; reload before continuing"
        );
        next.revision = original
            .revision
            .checked_add(1)
            .context("Draft revision exhausted")?;
        next.in_flight = false;
        drafts.insert(token.into(), next.clone());
        Ok(next)
    })
}
fn set_in_flight(
    app: &App,
    principal: &Principal,
    token: &str,
    draft: &Draft,
    value: bool,
) -> Result<()> {
    app.auth.with_access(principal, None, true, || {
        let mut drafts = app.onboarding.lock();
        prune(&mut drafts);
        let current = drafts
            .get_mut(token)
            .filter(|draft| draft.belongs_to(principal))
            .context("Draft expired or account changed")?;
        ensure!(
            current.revision == draft.revision && current.in_flight != value,
            "Draft changed or already being prepared"
        );
        current.in_flight = value;
        Ok(())
    })
}
async fn advance(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Path(token): Path<String>,
    headers: HeaderMap,
    Form(fields): Form<HashMap<String, String>>,
) -> Response {
    if !principal.is_admin()
        || !admin::csrf_valid(
            &app,
            &principal,
            &headers,
            fields.get("csrf").map_or("", String::as_str),
        )
    {
        return admin::error(
            StatusCode::FORBIDDEN,
            "Current administrator and valid same-origin CSRF token required",
        );
    }
    let mut draft = match snapshot(&app, &principal, &token) {
        Ok(draft) => draft,
        Err(e) => return admin::error(StatusCode::CONFLICT, e),
    };
    if fields
        .get("revision")
        .and_then(|value| value.parse::<u64>().ok())
        != Some(draft.revision)
    {
        return render(
            &app,
            &principal,
            &token,
            &draft,
            Some("Stale review revision; use this current draft form"),
            StatusCode::CONFLICT,
        );
    }
    let intent = match field(&fields, "intent") {
        Ok(intent) => intent,
        Err(e) => {
            return render(
                &app,
                &principal,
                &token,
                &draft,
                Some(&e.to_string()),
                StatusCode::BAD_REQUEST,
            );
        }
    };
    if intent == "discard" {
        if let Err(e) = allowed_fields(&draft, &fields, &[], &[]) {
            return render(
                &app,
                &principal,
                &token,
                &draft,
                Some(&e.to_string()),
                StatusCode::BAD_REQUEST,
            );
        }
        let result = app.auth.with_access(&principal, None, true, || {
            let mut drafts = app.onboarding.lock();
            ensure!(
                drafts
                    .get(&token)
                    .is_some_and(|current| current.belongs_to(&principal)
                        && current.revision == draft.revision
                        && !current.in_flight),
                "Draft changed; reload"
            );
            drafts.remove(&token);
            Ok(())
        });
        return match result {
            Ok(()) => Redirect::to("/admin/onboarding").into_response(),
            Err(e) => admin::error(StatusCode::CONFLICT, e),
        };
    }
    if matches!(intent, "review" | "commit") {
        let request = (|| -> Result<()> {
            ensure!(
                (intent == "review" && draft.step == Step::Recipes)
                    || (intent == "commit" && draft.step == Step::Review),
                "Final preparation requires the current completed wizard step"
            );
            allowed_fields(
                &draft,
                &fields,
                if intent == "commit" {
                    &["confirmed"]
                } else {
                    &[]
                },
                &[],
            )?;
            if intent == "commit" {
                confirmed(&fields)?;
            }
            Ok(())
        })();
        if let Err(e) = request {
            return render(
                &app,
                &principal,
                &token,
                &draft,
                Some(&e.to_string()),
                StatusCode::BAD_REQUEST,
            );
        }
        if intent == "review" {
            draft.catalog_revision = app.catalog.lock().await.revision;
        } else if app.catalog.lock().await.revision != draft.catalog_revision {
            return render(
                &app,
                &principal,
                &token,
                &draft,
                Some("Catalog changed since review; go back and prepare a fresh review"),
                StatusCode::CONFLICT,
            );
        }
        if let Err(e) = set_in_flight(&app, &principal, &token, &draft, true) {
            return admin::error(StatusCode::CONFLICT, e);
        }
        draft.in_flight = true;
        let prepared = setup::prepare(
            &app.config,
            &draft.source_id,
            &draft.queue.name,
            &draft.identity,
            &draft.choices(),
            Some(&draft.ppd),
        )
        .await;
        let prepared = match prepared {
            Ok(prepared) => prepared,
            Err(e) => {
                let _ = set_in_flight(&app, &principal, &token, &draft, false);
                draft.in_flight = false;
                return render(
                    &app,
                    &principal,
                    &token,
                    &draft,
                    Some(&format!("{e:#}")),
                    StatusCode::BAD_REQUEST,
                );
            }
        };
        if !admin::csrf_valid(
            &app,
            &principal,
            &headers,
            fields.get("csrf").map_or("", String::as_str),
        ) || draft.expires <= Instant::now()
        {
            let _ = set_in_flight(&app, &principal, &token, &draft, false);
            return admin::error(
                StatusCode::FORBIDDEN,
                "Account/CSRF revision changed or draft expired during preparation; nothing installed",
            );
        }
        if app.catalog.lock().await.revision != draft.catalog_revision {
            let _ = set_in_flight(&app, &principal, &token, &draft, false);
            draft.in_flight = false;
            return render(
                &app,
                &principal,
                &token,
                &draft,
                Some("Catalog changed during preparation; go back and prepare a fresh review"),
                StatusCode::CONFLICT,
            );
        }
        if intent == "review" {
            let mut next = draft.clone();
            next.profile = Some(prepared.profile);
            next.step = Step::Review;
            return match store(&app, &principal, &token, &draft, next) {
                Ok(next) => render(&app, &principal, &token, &next, None, StatusCode::OK),
                Err(e) => {
                    let _ = set_in_flight(&app, &principal, &token, &draft, false);
                    admin::error(StatusCode::CONFLICT, e)
                }
            };
        }
        let installed_ppd = prepared.config.ppd.clone();
        let result = app
            .install_printer(&principal, prepared, draft.catalog_revision)
            .await;
        // A post-rename fsync failure is APPLIED, not a failed installation. The
        // request's unique private PPD binding proves activation even if another
        // administrator has already edited the newly active profile.
        let applied = result.is_ok()
            || if let Some(entry) = app.printer(&draft.identity.name) {
                entry.read().await.config.ppd == installed_ppd
            } else {
                false
            };
        if applied {
            app.onboarding.lock().remove(&token);
            return installed(&app, &draft, result.err().map(|error| format!("{error:#}"))).await;
        }
        let _ = set_in_flight(&app, &principal, &token, &draft, false);
        draft.in_flight = false;
        return render(
            &app,
            &principal,
            &token,
            &draft,
            Some(&result.err().map_or_else(
                || "Printer was not activated".into(),
                |error| format!("{error:#}"),
            )),
            StatusCode::CONFLICT,
        );
    }
    let mut next = draft.clone();
    if let Err(e) = mutate(&app, &mut next, &fields, intent) {
        return render(
            &app,
            &principal,
            &token,
            &draft,
            Some(&format!("{e:#}")),
            StatusCode::BAD_REQUEST,
        );
    }
    match store(&app, &principal, &token, &draft, next) {
        Ok(next) => render(&app, &principal, &token, &next, None, StatusCode::OK),
        Err(e) => admin::error(StatusCode::CONFLICT, e),
    }
}
async fn installed(app: &App, draft: &Draft, warning: Option<String>) -> Response {
    let mut body = admin::page("Printer saved and HTTP-active");
    if let Some(warning) = warning {
        let _ = write!(body, "<p class=\"warning\">{}</p>", Escape(&warning));
    }
    let catalog_revision = app.catalog.lock().await.revision;
    let publication = app.discovery_status.read().clone();
    let _ = write!(
        body,
        "<p>Catalog authority cutover completed; the real backend and HTTP route are active. Ferriprint IPP endpoint: <code>{}</code>. Catalog revision {catalog_revision}; initial printer profile revision 0.</p><p>Discovery state: <strong>{}</strong>, publication revision {}. This is separate from HTTP activation; pending/failed/disabled is not successful advertisement.</p>",
        Escape(&app.config.printer_uri(&draft.identity.name)),
        Escape(&publication.state),
        publication.revision
    );
    if let Some(failure) = publication.failure {
        let _ = write!(
            body,
            "<p class=\"warning\">Discovery failure: {}</p>",
            Escape(&failure)
        );
    }
    body.push_str("<p>No native queue, driver or default was changed, and no print was submitted. No regular user or anonymous identity was automatically granted access.</p>");
    let _ = write!(
        body,
        "<p><a href=\"/admin/printers/{}\">Active profile</a> · <a href=\"/admin/printers/{}/recipes\">Complete recipes</a> · <a href=\"/admin/users\">Grant/revoke individual account rights</a> · <a href=\"/admin/users#anonymous-grants\">Explicit anonymous grant controls</a></p>",
        Escape(&draft.identity.name),
        Escape(&draft.identity.name)
    );
    admin::finish(body).into_response()
}
