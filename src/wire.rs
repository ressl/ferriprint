// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use anyhow::{Result, bail, ensure};
use bytes::Bytes;
use ipp::{prelude::*, value::BoundedString};
use std::collections::BTreeMap;
use tokio::io::{AsyncRead, AsyncReadExt};

fn one_or_array(mut values: Vec<IppValue>) -> IppValue {
    if values.len() == 1 {
        values.remove(0)
    } else {
        IppValue::Array(values)
    }
}
#[derive(Default)]
struct CollectionFrame {
    members: BTreeMap<ipp::value::IppName, IppValue>,
    current: Option<ipp::value::IppName>,
    values: Vec<IppValue>,
}
impl CollectionFrame {
    fn finish_member(&mut self) -> Result<()> {
        if let Some(name) = self.current.take() {
            ensure!(!self.values.is_empty(), "collection member has no value");
            ensure!(
                self.members
                    .insert(name, one_or_array(std::mem::take(&mut self.values)))
                    .is_none(),
                "duplicate collection member"
            );
        }
        Ok(())
    }
}
type PendingAttribute = Option<(String, Vec<IppValue>)>;
fn finish_attribute(attrs: &mut IppAttributes, current: &mut PendingAttribute) -> Result<()> {
    if let Some((name, values)) = current.take() {
        ensure!(!values.is_empty(), "attribute has no value");
        attrs
            .groups_mut()
            .last_mut()
            .ok_or_else(|| anyhow::anyhow!("attribute outside group"))?
            .attributes_mut()
            .push(IppAttribute::with_name(name, one_or_array(values))?);
    }
    Ok(())
}
fn append_value(
    frames: &mut [CollectionFrame],
    current: &mut PendingAttribute,
    value: IppValue,
) -> Result<()> {
    if let Some(frame) = frames.last_mut() {
        ensure!(frame.current.is_some(), "value without a collection member");
        frame.values.push(value);
    } else {
        current
            .as_mut()
            .ok_or_else(|| anyhow::anyhow!("value without an attribute name"))?
            .1
            .push(value);
    }
    Ok(())
}
/// Bounded framing plus the ipp crate's primitive codec. Collection grammar is
/// assembled here because ipp 8's generic parser drops orphan/array members.
/// Documents are never read ahead or copied into this metadata buffer.
pub async fn read_metadata<R: AsyncRead + Unpin>(
    reader: &mut R,
    limit: usize,
) -> Result<IppRequestResponse> {
    ensure!(limit >= 8, "IPP metadata limit is too small");
    let mut raw = [0; 8];
    reader.read_exact(&mut raw).await?;
    let header = ipp::IppHeader::new(
        IppVersion(u16::from_be_bytes([raw[0], raw[1]])),
        i16::from_be_bytes([raw[2], raw[3]]),
        i32::from_be_bytes([raw[4], raw[5], raw[6], raw[7]]),
    );
    let mut used = 8;
    let mut attributes = IppAttributes::new();
    let mut current: PendingAttribute = None;
    let mut frames: Vec<CollectionFrame> = Vec::new();
    loop {
        ensure!(used < limit, "IPP metadata exceeds configured limit");
        let tag = reader.read_u8().await?;
        used += 1;
        if tag < 0x10 {
            ensure!(frames.is_empty(), "unclosed IPP collection");
            finish_attribute(&mut attributes, &mut current)?;
            if tag == 3 {
                break;
            }
            let tag = DelimiterTag::from_u8(tag)
                .ok_or_else(|| anyhow::anyhow!("invalid IPP delimiter"))?;
            attributes.groups_mut().push(IppAttributeGroup::new(tag));
            continue;
        }
        ensure!(
            !attributes.groups().is_empty(),
            "attribute before operation group"
        );
        let name_len = reader.read_u16().await? as usize;
        used += 2;
        ensure!(
            name_len <= 255 && used + name_len + 2 <= limit,
            "IPP attribute name or metadata too large"
        );
        let mut name = vec![0; name_len];
        reader.read_exact(&mut name).await?;
        used += name_len;
        let name = String::from_utf8(name)?;
        let value_len = reader.read_u16().await? as usize;
        used += 2;
        ensure!(
            used + value_len <= limit,
            "IPP metadata exceeds configured limit"
        );
        let mut value = vec![0; value_len];
        reader.read_exact(&mut value).await?;
        used += value_len;
        let unnamed = name.is_empty();
        if frames.is_empty() && !matches!(tag, 0x37 | 0x4a) && !unnamed {
            finish_attribute(&mut attributes, &mut current)?;
            current = Some((name, Vec::new()));
        }
        match tag {
            0x4a => {
                ensure!(
                    unnamed,
                    "collection member must have an empty attribute name"
                );
                let frame = frames
                    .last_mut()
                    .ok_or_else(|| anyhow::anyhow!("member name outside collection"))?;
                frame.finish_member()?;
                ensure!(
                    !value.is_empty() && value.len() <= 255,
                    "invalid collection member name"
                );
                frame.current = Some(std::str::from_utf8(&value)?.try_into()?);
            }
            0x34 => {
                ensure!(
                    value.is_empty() && frames.len() < 8,
                    "invalid or overly nested collection"
                );
                if let Some(frame) = frames.last() {
                    ensure!(
                        unnamed && frame.current.is_some(),
                        "collection without a member"
                    );
                } else {
                    ensure!(current.is_some(), "collection without an attribute");
                }
                frames.push(CollectionFrame::default());
            }
            0x37 => {
                ensure!(unnamed && value.is_empty(), "invalid collection end");
                let mut frame = frames
                    .pop()
                    .ok_or_else(|| anyhow::anyhow!("collection end outside collection"))?;
                frame.finish_member()?;
                append_value(
                    &mut frames,
                    &mut current,
                    IppValue::Collection(frame.members),
                )?;
            }
            _ => {
                if !frames.is_empty() {
                    ensure!(
                        unnamed,
                        "collection value must have an empty attribute name"
                    );
                }
                match tag {
                    0x21 | 0x23 => ensure!(value_len == 4, "invalid integer/enum length"),
                    0x22 => ensure!(value_len == 1 && value[0] <= 1, "invalid boolean"),
                    0x31 => ensure!(value_len == 11, "invalid date length"),
                    0x32 => ensure!(value_len == 9, "invalid resolution length"),
                    0x33 => ensure!(value_len == 8, "invalid range length"),
                    0x35 | 0x36 => {
                        let mut offset = 0;
                        for _ in 0..2 {
                            ensure!(offset + 2 <= value.len(), "truncated language string");
                            let n = u16::from_be_bytes([value[offset], value[offset + 1]]) as usize;
                            offset += 2;
                            ensure!(offset + n <= value.len(), "truncated language string");
                            offset += n;
                        }
                        ensure!(offset == value.len(), "extra language string bytes");
                    }
                    0x10 | 0x12 | 0x13 => ensure!(value_len == 0, "invalid out-of-band value"),
                    0x30 | 0x41 | 0x42 | 0x44..=0x49 => {}
                    _ => bail!("invalid IPP value tag"),
                }
                append_value(
                    &mut frames,
                    &mut current,
                    IppValue::parse(tag, Bytes::from(value))?,
                )?;
            }
        }
    }
    let mut message = IppRequestResponse::new_response(
        header.version,
        StatusCode::SuccessfulOk,
        header.request_id,
    )?;
    *message.header_mut() = header;
    *message.attributes_mut() = attributes;
    Ok(message)
}

pub fn add(
    msg: &mut IppRequestResponse,
    group: DelimiterTag,
    name: &str,
    value: IppValue,
) -> Result<()> {
    msg.attributes_mut()
        .add(group, IppAttribute::with_name(name, value)?);
    Ok(())
}
pub fn keyword(s: &str) -> Result<IppValue> {
    Ok(IppValue::Keyword(s.try_into()?))
}
pub fn name(s: &str) -> Result<IppValue> {
    Ok(IppValue::NameWithoutLanguage(s.try_into()?))
}
pub fn text(s: &str) -> Result<IppValue> {
    Ok(IppValue::TextWithoutLanguage(s.try_into()?))
}
pub fn uri(s: &str) -> Result<IppValue> {
    Ok(IppValue::Uri(s.try_into()?))
}
pub fn mime(s: &str) -> Result<IppValue> {
    Ok(IppValue::MimeMediaType(s.try_into()?))
}
pub fn strings(items: &[String], make: fn(&str) -> Result<IppValue>) -> Result<IppValue> {
    Ok(IppValue::Array(
        items.iter().map(|s| make(s)).collect::<Result<_>>()?,
    ))
}
pub fn collection(items: Vec<(&str, IppValue)>) -> Result<IppValue> {
    let mut map = BTreeMap::<BoundedString<255>, IppValue>::new();
    for (k, v) in items {
        map.insert(k.try_into()?, v);
    }
    Ok(IppValue::Collection(map))
}
/// Only metadata crosses async handler boundaries. The codec's payload trait
/// object is Send but not Sync, although it is empty after bounded parsing.
pub struct Metadata {
    header: ipp::IppHeader,
    attributes: IppAttributes,
}
impl From<IppRequestResponse> for Metadata {
    fn from(mut message: IppRequestResponse) -> Self {
        Self {
            header: *message.header(),
            attributes: std::mem::take(message.attributes_mut()),
        }
    }
}
impl Metadata {
    pub fn header(&self) -> &ipp::IppHeader {
        &self.header
    }
    pub fn attributes(&self) -> &IppAttributes {
        &self.attributes
    }
}
pub trait HasAttributes {
    fn ipp_attributes(&self) -> &IppAttributes;
}
impl HasAttributes for Metadata {
    fn ipp_attributes(&self) -> &IppAttributes {
        &self.attributes
    }
}
impl HasAttributes for IppRequestResponse {
    fn ipp_attributes(&self) -> &IppAttributes {
        self.attributes()
    }
}
pub fn attr<'a>(
    msg: &'a impl HasAttributes,
    group: DelimiterTag,
    key: &str,
) -> Option<&'a IppValue> {
    msg.ipp_attributes()
        .first_of(group)?
        .get(key)
        .map(IppAttribute::value)
}
pub fn str_value(value: &IppValue) -> Option<&str> {
    match value {
        IppValue::Keyword(s) | IppValue::NameWithoutLanguage(s) => Some(s),
        IppValue::Uri(s) | IppValue::UriScheme(s) => Some(s),
        IppValue::MimeMediaType(s) => Some(s),
        IppValue::Charset(s) | IppValue::NaturalLanguage(s) => Some(s),
        IppValue::TextWithoutLanguage(s) => Some(s),
        IppValue::NameWithLanguage { name, .. } => Some(name),
        _ => None,
    }
}
pub fn integer(value: &IppValue) -> Option<i32> {
    match value {
        IppValue::Integer(n) | IppValue::Enum(n) => Some(*n),
        _ => None,
    }
}
pub fn values(value: &IppValue) -> &[IppValue] {
    match value {
        IppValue::Array(a) => a,
        v => std::slice::from_ref(v),
    }
}
pub fn string_list(value: Option<&IppValue>) -> Vec<String> {
    value
        .into_iter()
        .flat_map(values)
        .filter_map(str_value)
        .map(str::to_owned)
        .collect()
}
/// UTF-8-safe truncation; unlike BoundedString::new_truncated this never cuts a codepoint.
pub fn bounded(s: &str, max: usize) -> &str {
    let mut end = s.len().min(max);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

/// Compare local IPP resources without rewriting their path or authority.
/// DNS hostname case and a single absolute-root dot do not change identity.
pub(crate) fn same_ipp_resource(target: &str, canonical: &str) -> bool {
    if target == canonical {
        return true;
    }
    let Some((target_scheme, target)) = target.split_once("://") else {
        return false;
    };
    let Some((canonical_scheme, canonical)) = canonical.split_once("://") else {
        return false;
    };
    if target_scheme != canonical_scheme || !matches!(canonical_scheme, "ipp" | "ipps") {
        return false;
    }
    let Some((target_authority, target_resource)) = target.split_once('/') else {
        return false;
    };
    let Some((canonical_authority, canonical_resource)) = canonical.split_once('/') else {
        return false;
    };
    // Keep the raw suffix: URI parsers can discard fragments or normalize paths.
    if target_resource != canonical_resource {
        return false;
    }
    let (Ok(target), Ok(canonical)) = (
        target_authority.parse::<axum::http::uri::Authority>(),
        canonical_authority.parse::<axum::http::uri::Authority>(),
    ) else {
        return false;
    };
    let (target_userinfo, target_host, target_port) = authority_parts(&target);
    let (canonical_userinfo, canonical_host, canonical_port) = authority_parts(&canonical);
    if target_userinfo != canonical_userinfo || target_port != canonical_port {
        return false;
    }
    if target_host == canonical_host {
        return true;
    }
    let target_host = target_host.strip_suffix('.').unwrap_or(target_host);
    let canonical_host = canonical_host.strip_suffix('.').unwrap_or(canonical_host);
    // Do not reinterpret IP literals or other reg-name syntax as DNS aliases.
    if [target_host, canonical_host].into_iter().any(|host| {
        host.is_empty()
            || host.parse::<std::net::Ipv4Addr>().is_ok()
            || !host
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'.' | b'-'))
    }) {
        return false;
    }
    target_host.eq_ignore_ascii_case(canonical_host)
}

fn authority_parts(authority: &axum::http::uri::Authority) -> (Option<&str>, &str, &str) {
    let (userinfo, host_port) = match authority.as_str().rsplit_once('@') {
        Some((userinfo, host_port)) => (Some(userinfo), host_port),
        None => (None, authority.as_str()),
    };
    let host = authority.host();
    (userinfo, host, &host_port[host.len()..])
}

/// Url::set_scheme cannot cross the special/non-special scheme boundary.
/// Reparse the authority/path and explicitly preserve IPP's default port.
pub fn http_url(ipp: &url::Url) -> Result<url::Url> {
    let scheme = match ipp.scheme() {
        "ipp" => "http",
        "ipps" => "https",
        _ => bail!("expected an ipp or ipps URL"),
    };
    let (_, rest) = ipp
        .as_str()
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("invalid IPP URL"))?;
    let mut http = url::Url::parse(&format!("{scheme}:{rest}"))?;
    http.set_port(Some(ipp.port().unwrap_or(631)))
        .map_err(|_| anyhow::anyhow!("invalid transport port"))?;
    Ok(http)
}
