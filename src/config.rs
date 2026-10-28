// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::domain::{DriverOptions, Resolution};
use anyhow::{Context, Result, ensure};
use ipnet::IpNet;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    net::SocketAddr,
    path::{Path, PathBuf},
};
use url::Url;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    pub public_url: Url,
    pub state_dir: PathBuf,
    #[serde(default = "default_max_document_bytes")]
    pub max_document_bytes: u64,
    #[serde(default = "default_max_attribute_bytes")]
    pub max_attribute_bytes: usize,
    #[serde(default = "default_max_queued_jobs")]
    pub max_queued_jobs: usize,
    #[serde(default = "default_poll_interval_ms")]
    pub poll_interval_ms: u64,
    #[serde(default = "default_backend_timeout_secs")]
    pub backend_timeout_secs: u64,
    pub discovery: Option<crate::discovery::DiscoveryConfig>,
    #[serde(default)]
    pub allowed_clients: Vec<IpNet>,
    pub tls: Option<TlsConfig>,
    pub access: AccessConfig,
    #[serde(default)]
    pub cups_sources: Vec<crate::setup::CupsSource>,
    pub notifications: Option<crate::notifications::NotificationConfig>,
    /// Startup snapshot loaded only from the authoritative managed catalog.
    #[serde(skip)]
    pub printers: Vec<PrinterConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TlsConfig {
    pub certificate: PathBuf,
    pub private_key: PathBuf,
}

/// Backend authentication is separate from frontend principals.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AuthConfig {
    pub username: String,
    pub password_env: String,
}

#[derive(Clone, Copy, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    User,
    Admin,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessConfig {
    #[serde(default)]
    pub anonymous_trusted_lan: bool,
    pub anonymous_owner: Option<String>,
    #[serde(default)]
    pub anonymous_clients: Vec<IpNet>,
}

/// Protected bootstrap identity. No web-editable paths, credentials or queue URIs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrinterConfig {
    pub name: String,
    pub display_name: String,
    pub service_name: String,
    #[serde(default)]
    pub location: String,
    pub ppd: PathBuf,
    pub upstream_uri: Url,
    #[serde(default = "default_backend_username")]
    pub upstream_username: String,
    pub upstream_auth: Option<AuthConfig>,
    pub upstream_ca_certificate: Option<PathBuf>,
    pub telemetry: Option<crate::telemetry::TelemetryConfig>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct BackendIdentity {
    pub uri: String,
    pub username: String,
    pub authentication_username: Option<String>,
}
impl PrinterConfig {
    pub fn backend_identity(&self) -> BackendIdentity {
        BackendIdentity {
            uri: self.upstream_uri.to_string(),
            username: self.upstream_username.clone(),
            authentication_username: self.upstream_auth.as_ref().map(|a| a.username.clone()),
        }
    }
    pub fn validate_transport(&self) -> Result<()> {
        ensure!(
            matches!(self.upstream_uri.scheme(), "ipp" | "ipps")
                && self.upstream_uri.host_str().is_some()
                && self.upstream_uri.path() != "/"
                && !self.upstream_uri.path().is_empty(),
            "upstream_uri must identify an IPP CUPS queue"
        );
        ensure!(
            self.upstream_uri.username().is_empty() && self.upstream_uri.password().is_none(),
            "use upstream_auth rather than credentials in an upstream URI"
        );
        ensure!(
            self.upstream_uri.query().is_none() && self.upstream_uri.fragment().is_none(),
            "upstream_uri cannot contain a query or fragment"
        );
        ensure!(
            !self.upstream_username.is_empty()
                && self.upstream_username.len() <= 255
                && !self.upstream_username.chars().any(char::is_control),
            "invalid upstream_username"
        );
        if self.upstream_auth.is_some() && self.upstream_uri.scheme() == "ipp" {
            ensure!(
                is_loopback_url(&self.upstream_uri),
                "upstream_auth over plaintext IPP is allowed only to a literal loopback IP; use ipps for remote authentication"
            );
        }
        if let Some(auth) = &self.upstream_auth {
            validate_auth(auth)?;
        }
        Ok(())
    }
}

/// The ONLY authoritative editable settings, stored in state_dir/profiles/NAME.toml.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileSettings {
    pub schema_version: u32,
    pub sources: BTreeMap<String, String>,
    pub media_types: BTreeMap<String, MediaTypeConfig>,
    pub color_modes: BTreeMap<String, DriverOptions>,
    pub sides: BTreeMap<String, DriverOptions>,
    pub defaults: Defaults,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub recipes: BTreeMap<String, crate::recipes::Recipe>,
    #[serde(default)]
    pub declared_ready: Vec<ReadyMediaConfig>,
    #[serde(default = "default_document_formats")]
    pub document_formats: Vec<String>,
    #[serde(default = "default_resolutions")]
    pub resolutions: Vec<Resolution>,
}
impl ProfileSettings {
    pub fn load(path: &Path) -> Result<Self> {
        let mut text = String::new();
        std::fs::File::open(path)
            .with_context(|| format!("open managed profile {}", path.display()))?
            .take(256 * 1024 + 1)
            .read_to_string(&mut text)?;
        ensure!(text.len() <= 256 * 1024, "managed profile exceeds 256 KiB");
        let settings: Self = toml::from_str(&text).context("parse managed profile")?;
        settings.validate()?;
        Ok(settings)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported profile schema_version; expected 1"
        );
        ensure!(
            !self.sources.is_empty()
                && !self.media_types.is_empty()
                && !self.color_modes.is_empty()
                && !self.sides.is_empty(),
            "sources, media_types, color_modes and sides mappings are required"
        );
        ensure!(
            !self.document_formats.is_empty(),
            "document_formats cannot be empty"
        );
        ensure!(
            !self.resolutions.is_empty() && self.resolutions.iter().all(|r| r.x > 0 && r.y > 0),
            "invalid resolutions"
        );
        ensure!(
            self.sources.len() <= 128
                && self.media_types.len() <= 128
                && self.resolutions.len() <= 32
                && self.declared_ready.len() <= 128,
            "too many profile entries"
        );
        Ok(())
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct MediaTypeConfig {
    #[serde(default)]
    pub draft: Option<DriverOptions>,
    pub normal: DriverOptions,
    #[serde(default)]
    pub high: Option<DriverOptions>,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Defaults {
    pub media: String,
    #[serde(default)]
    pub borderless: bool,
    pub source: String,
    pub media_type: String,
    #[serde(default = "default_quality")]
    pub quality: i32,
    pub color_mode: String,
    pub sides: String,
}
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ReadyMediaConfig {
    pub media: String,
    pub source: String,
    pub media_type: String,
    #[serde(default)]
    pub borderless: bool,
}

impl Config {
    pub fn load(path: &Path) -> Result<Self> {
        let mut config = Self::load_bootstrap(path)?;
        config.printers = crate::managed::load_catalog(&config)?.printers;
        config.validate()?;
        Ok(config)
    }
    /// Explicit initialization only: parse protected settings without inventing a catalog.
    pub fn load_bootstrap(path: &Path) -> Result<Self> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("read configuration {}", path.display()))?;
        let table: toml::Table = toml::from_str(&text).context("parse configuration")?;
        ensure!(
            !table.contains_key("printers")
                && !table
                    .get("access")
                    .and_then(toml::Value::as_table)
                    .is_some_and(|access| access.contains_key("users")),
            "legacy bootstrap users/printers require the explicit migrate-0.2 command"
        );
        let mut config: Self = table.try_into().context("parse configuration")?;
        let parent = path.parent().unwrap_or_else(|| Path::new("."));
        let directory = if parent.is_absolute() {
            parent.to_owned()
        } else {
            std::env::current_dir()?.join(parent)
        };
        let base = directory.as_path();
        config.state_dir = absolute_from(base, &config.state_dir);
        if let Some(tls) = &mut config.tls {
            tls.certificate = absolute_from(base, &tls.certificate);
            tls.private_key = absolute_from(base, &tls.private_key);
        }
        for source in &mut config.cups_sources {
            source.resolve_paths(base);
        }
        if let Some(notifications) = &mut config.notifications {
            notifications.resolve_paths(base);
        }
        config.validate()?;
        Ok(config)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.public_url.scheme(), "ipp" | "ipps")
                && self.public_url.host_str().is_some(),
            "public_url must use ipp:// or ipps:// with a host"
        );
        ensure!(
            self.public_url.username().is_empty() && self.public_url.password().is_none(),
            "do not embed credentials in public_url"
        );
        ensure!(
            self.public_url.path() == "/"
                && self.public_url.query().is_none()
                && self.public_url.fragment().is_none(),
            "public_url is a base URI without a path, query or fragment"
        );
        ensure!(
            (self.public_url.scheme() == "ipps") == self.tls.is_some(),
            "ipps requires TLS certificate/key; ipp requires no TLS configuration"
        );
        ensure!(
            self.max_document_bytes > 0 && self.max_document_bytes <= i64::MAX as u64,
            "invalid max_document_bytes"
        );
        ensure!(
            (1024..=1024 * 1024).contains(&self.max_attribute_bytes),
            "max_attribute_bytes must be between 1 KiB and 1 MiB"
        );
        ensure!(
            (1..=10000).contains(&self.max_queued_jobs),
            "max_queued_jobs must be between 1 and 10000"
        );
        ensure!(
            self.poll_interval_ms >= 100
                && self.backend_timeout_secs > 0
                && self.backend_timeout_secs <= 300,
            "invalid polling interval or backend timeout"
        );
        if !self.listen.ip().is_loopback() {
            ensure!(
                !self.allowed_clients.is_empty(),
                "non-loopback listeners require explicit allowed_clients networks"
            );
            ensure!(self.tls.is_some(), "non-loopback listeners require TLS");
        }
        if let Some(discovery) = &self.discovery {
            discovery.validate()?;
        }
        let access = &self.access;
        if access.anonymous_trusted_lan {
            let owner = access
                .anonymous_owner
                .as_deref()
                .context("anonymous_trusted_lan requires explicit anonymous_owner")?;
            validate_principal(owner)?;
            ensure!(
                !access.anonymous_clients.is_empty(),
                "anonymous_trusted_lan requires explicit anonymous_clients networks"
            );
            for network in &access.anonymous_clients {
                ensure!(
                    self.allowed_clients
                        .iter()
                        .any(|allowed| allowed.contains(&network.network())
                            && allowed.prefix_len() <= network.prefix_len())
                        || (self.allowed_clients.is_empty() && network.network().is_loopback()),
                    "anonymous_clients must be within allowed_clients (or literal loopback networks for loopback-only configuration)"
                );
            }
        } else {
            ensure!(
                access.anonymous_clients.is_empty() && access.anonymous_owner.is_none(),
                "anonymous settings require anonymous_trusted_lan=true"
            );
        }
        let mut names = std::collections::HashSet::new();
        for printer in &self.printers {
            validate_printer_name(&printer.name)?;
            ensure!(
                names.insert(&printer.name),
                "duplicate printer name {}",
                printer.name
            );
            ensure!(
                !printer.display_name.is_empty()
                    && printer.display_name.len() <= 200
                    && !printer.display_name.chars().any(char::is_control),
                "invalid printer display_name"
            );
            ensure!(
                !printer.service_name.is_empty()
                    && printer.service_name.len() <= 63
                    && !printer.service_name.chars().any(char::is_control),
                "service_name must contain 1..63 UTF-8 bytes without control characters"
            );
            printer.validate_transport()?;
            if let Some(telemetry) = &printer.telemetry {
                telemetry.validate()?;
            }
        }
        let mut source_ids = std::collections::HashSet::new();
        for source in &self.cups_sources {
            source.validate()?;
            ensure!(
                source_ids.insert(&source.id),
                "duplicate trusted CUPS source"
            );
        }
        if let Some(notifications) = &self.notifications {
            notifications.validate()?;
        }
        Ok(())
    }
    pub fn printer_uri(&self, name: &str) -> String {
        format!("{}ipp/print/{name}", self.public_url)
    }
    pub fn profile_path(&self, name: &str) -> PathBuf {
        self.state_dir.join("profiles").join(format!("{name}.toml"))
    }
}

pub fn validate_printer_name(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 63
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_')),
        "printer name must contain 1..63 ASCII letters, digits, underscores or hyphens"
    );
    Ok(())
}
pub fn validate_principal(value: &str) -> Result<()> {
    ensure!(
        !value.is_empty()
            && value.len() <= 64
            && value
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'@')),
        "principal must contain 1..64 ASCII letters, digits, -, _, . or @"
    );
    Ok(())
}
fn validate_auth(auth: &AuthConfig) -> Result<()> {
    ensure!(
        !auth.username.is_empty()
            && !auth.username.contains(':')
            && auth.username.len() <= 255
            && !auth.username.chars().any(char::is_control),
        "invalid authentication username"
    );
    ensure!(
        !auth.password_env.is_empty()
            && auth
                .password_env
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'_'),
        "password_env must name an environment variable"
    );
    Ok(())
}
fn absolute_from(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.to_owned()
    } else {
        base.join(path)
    }
}
fn default_listen() -> SocketAddr {
    "127.0.0.1:8631".parse().expect("constant address")
}
fn default_max_document_bytes() -> u64 {
    128 * 1024 * 1024
}
fn default_max_attribute_bytes() -> usize {
    256 * 1024
}
fn default_max_queued_jobs() -> usize {
    128
}
fn default_poll_interval_ms() -> u64 {
    1000
}
fn default_backend_timeout_secs() -> u64 {
    30
}
fn default_backend_username() -> String {
    "ferriprint".to_owned()
}
fn default_quality() -> i32 {
    4
}
fn default_document_formats() -> Vec<String> {
    [
        "application/pdf",
        "image/jpeg",
        "image/png",
        "image/pwg-raster",
        "image/urf",
    ]
    .into_iter()
    .map(str::to_owned)
    .collect()
}
fn default_resolutions() -> Vec<Resolution> {
    vec![Resolution { x: 300, y: 300 }]
}
pub fn is_loopback_url(url: &Url) -> bool {
    match url.host() {
        Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
        Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
        Some(url::Host::Domain(host)) => host
            .trim_start_matches('[')
            .trim_end_matches(']')
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback()),
        _ => false,
    }
}
