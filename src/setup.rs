// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{
    config::{AuthConfig, Config, PrinterConfig, ProfileSettings},
    domain::JobTemplate,
    managed::PrinterCatalog,
    profile::{PpdInventory, PrinterProfile},
    upstream::{Upstream, UpstreamStatus},
    wire::*,
};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use futures_util::StreamExt;
use ipp::prelude::*;
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use url::Url;
use zeroize::Zeroizing;

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct CupsSource {
    pub id: String,
    pub server_uri: Url,
    #[serde(default = "default_username")]
    pub username: String,
    pub auth: Option<AuthConfig>,
    pub ca_certificate: Option<PathBuf>,
}
fn default_username() -> String {
    "ferriprint".into()
}
impl CupsSource {
    pub fn validate(&self) -> Result<()> {
        crate::config::validate_printer_name(&self.id)?;
        ensure!(
            matches!(self.server_uri.scheme(), "ipp" | "ipps")
                && self.server_uri.host().is_some()
                && self.server_uri.path() == "/"
                && self.server_uri.query().is_none()
                && self.server_uri.fragment().is_none()
                && self.server_uri.username().is_empty()
                && self.server_uri.password().is_none()
                && self.server_uri.port() != Some(0),
            "trusted CUPS source requires a fixed ipp(s) server root without credentials/query/fragment"
        );
        ensure!(
            self.server_uri.scheme() == "ipps" || crate::config::is_loopback_url(&self.server_uri),
            "remote trusted CUPS sources require IPPS"
        );
        self.printer("validation", "Validation", "Validation", "")?
            .validate_transport()
    }
    pub fn resolve_paths(&mut self, base: &Path) {
        if let Some(path) = &mut self.ca_certificate
            && path.is_relative()
        {
            *path = base.join(&*path);
        }
    }
    pub fn queue_uri(&self, queue: &str) -> Result<Url> {
        validate_queue_name(queue)?;
        let mut url = self.server_uri.clone();
        url.path_segments_mut()
            .map_err(|_| anyhow::anyhow!("invalid trusted CUPS source"))?
            .clear()
            .push("printers")
            .push(queue);
        Ok(url)
    }
    fn printer(
        &self,
        queue: &str,
        display: &str,
        service: &str,
        location: &str,
    ) -> Result<PrinterConfig> {
        Ok(PrinterConfig {
            name: queue.into(),
            display_name: display.into(),
            service_name: service.into(),
            location: location.into(),
            ppd: PathBuf::new(),
            upstream_uri: self.queue_uri(queue)?,
            upstream_username: self.username.clone(),
            upstream_auth: self.auth.clone(),
            upstream_ca_certificate: self.ca_certificate.clone(),
            telemetry: None,
        })
    }
}
pub fn validate_queue_name(name: &str) -> Result<()> {
    ensure!(
        !name.is_empty()
            && name.len() <= 127
            && !matches!(name, "." | "..")
            && name
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.')),
        "invalid enumerated native CUPS queue name"
    );
    Ok(())
}
struct CupsClient {
    source: CupsSource,
    client: reqwest::Client,
    password: Option<Zeroizing<String>>,
}
impl CupsClient {
    fn new(source: &CupsSource, timeout: Duration) -> Result<Self> {
        source.validate()?;
        let mut builder = reqwest::Client::builder()
            .timeout(timeout)
            .redirect(reqwest::redirect::Policy::none());
        if let Some(path) = &source.ca_certificate {
            let mut bytes = Vec::new();
            std::fs::File::open(path)?
                .take(1024 * 1024 + 1)
                .read_to_end(&mut bytes)?;
            ensure!(bytes.len() <= 1024 * 1024, "CUPS CA exceeds size limit");
            builder = builder.add_root_certificate(reqwest::Certificate::from_pem(&bytes)?);
        }
        let password = source
            .auth
            .as_ref()
            .map(|a| {
                std::env::var(&a.password_env)
                    .map(Zeroizing::new)
                    .context("trusted CUPS credential environment unavailable")
            })
            .transpose()?;
        Ok(Self {
            source: source.clone(),
            client: builder.build()?,
            password,
        })
    }
    fn authenticate(&self, request: reqwest::RequestBuilder) -> reqwest::RequestBuilder {
        if let Some(auth) = &self.source.auth {
            request.basic_auth(&auth.username, self.password.as_deref())
        } else {
            request
        }
    }
    async fn inventory(&self) -> Result<Vec<NativeQueue>> {
        let mut request = IppRequestResponse::new(
            IppVersion::v2_0(),
            Operation::CupsGetPrinters,
            Some(self.source.server_uri.as_str().parse()?),
        )?;
        add(
            &mut request,
            DelimiterTag::OperationAttributes,
            "requesting-user-name",
            name(&self.source.username)?,
        )?;
        add(
            &mut request,
            DelimiterTag::OperationAttributes,
            "requested-attributes",
            strings(
                &[
                    "printer-name".into(),
                    "printer-uri-supported".into(),
                    "printer-info".into(),
                    "printer-make-and-model".into(),
                    "printer-state".into(),
                    "printer-is-accepting-jobs".into(),
                ],
                keyword,
            )?,
        )?;
        let id = request.header().request_id;
        let response = self
            .authenticate(
                self.client
                    .post(http_url(&self.source.server_uri)?)
                    .header("content-type", "application/ipp")
                    .body(request.to_bytes()),
            )
            .send()
            .await?
            .error_for_status()?;
        ensure!(
            response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok())
                .is_some_and(|v| v.split(';').next() == Some("application/ipp")),
            "CUPS inventory response is not IPP"
        );
        let mut reader = tokio_util::io::StreamReader::new(
            response
                .bytes_stream()
                .map(|r| r.map_err(std::io::Error::other)),
        );
        let response = read_metadata(&mut reader, 1024 * 1024).await?;
        ensure!(
            response.header().request_id == id && response.header().status_code().is_success(),
            "CUPS inventory request rejected"
        );
        let mut queues = Vec::new();
        for group in response
            .attributes()
            .groups()
            .iter()
            .filter(|g| g.tag() == DelimiterTag::PrinterAttributes)
        {
            let get = |key: &str| group.get(key).and_then(|a| str_value(a.value()));
            let queue = get("printer-name").context("CUPS inventory omitted queue name")?;
            validate_queue_name(queue)?;
            let expected = self.source.queue_uri(queue)?;
            let uris = group
                .get("printer-uri-supported")
                .map(|a| string_list(Some(a.value())))
                .unwrap_or_default();
            ensure!(
                uris.iter()
                    .any(|uri| same_ipp_resource(uri, expected.as_str())),
                "CUPS advertised a queue outside its trusted origin/resource"
            );
            ensure!(
                !queues.iter().any(|q: &NativeQueue| q.name == queue),
                "duplicate CUPS inventory queue"
            );
            queues.push(NativeQueue {
                name: queue.into(),
                display_name: get("printer-info")
                    .unwrap_or(queue)
                    .chars()
                    .take(200)
                    .collect(),
                model: get("printer-make-and-model")
                    .unwrap_or("unknown")
                    .chars()
                    .take(255)
                    .collect(),
                state: group
                    .get("printer-state")
                    .and_then(|a| integer(a.value()))
                    .unwrap_or(5),
                accepting: matches!(
                    group.get("printer-is-accepting-jobs").map(|a| a.value()),
                    Some(IppValue::Boolean(true))
                ),
            });
            ensure!(queues.len() <= 256, "too many native CUPS queues");
        }
        queues.sort_unstable_by(|a, b| a.name.cmp(&b.name));
        Ok(queues)
    }
    async fn ppd(&self, queue: &str) -> Result<Vec<u8>> {
        let mut uri = http_url(&self.source.queue_uri(queue)?)?;
        uri.set_path(&format!("/printers/{queue}.ppd"));
        let response = self
            .authenticate(self.client.get(uri))
            .send()
            .await?
            .error_for_status()?;
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::new();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            ensure!(
                bytes.len().saturating_add(chunk.len()) <= 16 * 1024 * 1024,
                "native PPD exceeds 16 MiB"
            );
            bytes.extend_from_slice(&chunk);
        }
        PpdInventory::from_bytes(&bytes)?;
        Ok(bytes)
    }
}
#[derive(Clone, Debug, Serialize)]
pub struct NativeQueue {
    pub name: String,
    pub display_name: String,
    pub model: String,
    pub state: i32,
    pub accepting: bool,
}
pub async fn inventory(config: &Config, source_id: &str) -> Result<Vec<NativeQueue>> {
    CupsClient::new(
        source(config, source_id)?,
        Duration::from_secs(config.backend_timeout_secs),
    )?
    .inventory()
    .await
}
pub fn source<'a>(config: &'a Config, id: &str) -> Result<&'a CupsSource> {
    config
        .cups_sources
        .iter()
        .find(|s| s.id == id)
        .context("unknown protected CUPS source")
}
pub async fn acquire(
    config: &Config,
    source_id: &str,
    queue: &str,
) -> Result<(NativeQueue, Vec<u8>)> {
    let client = CupsClient::new(
        source(config, source_id)?,
        Duration::from_secs(config.backend_timeout_secs),
    )?;
    let queue = client
        .inventory()
        .await?
        .into_iter()
        .find(|q| q.name == queue)
        .context("queue is not in fresh trusted CUPS inventory")?;
    let ppd = client.ppd(&queue.name).await?;
    Ok((queue, ppd))
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrinterIdentity {
    pub name: String,
    pub display_name: String,
    pub service_name: String,
    #[serde(default)]
    pub location: String,
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupChoices {
    pub confirmed_native_mappings: bool,
    pub settings: ProfileSettings,
    #[serde(default)]
    pub previews: Vec<JobTemplate>,
}
pub struct PreparedPrinter {
    pub config: PrinterConfig,
    pub profile: Arc<PrinterProfile>,
    pub upstream: Upstream,
    pub status: UpstreamStatus,
    pub ppd: Vec<u8>,
}
pub async fn prepare(
    config: &Config,
    source_id: &str,
    queue: &str,
    identity: &PrinterIdentity,
    choices: &SetupChoices,
    reviewed_ppd: Option<&[u8]>,
) -> Result<PreparedPrinter> {
    ensure!(
        choices.confirmed_native_mappings,
        "explicit confirmation of native material/quality/tray/color/sides mappings is required"
    );
    ensure!(
        identity.location.len() <= 200 && !identity.location.chars().any(char::is_control),
        "new printer location must be at most 200 bytes without control characters"
    );
    let (_, ppd) = acquire(config, source_id, queue).await?;
    if let Some(reviewed) = reviewed_ppd {
        ensure!(
            reviewed == ppd,
            "native PPD changed; inspect and review again"
        );
    }
    let source = source(config, source_id)?;
    let mut printer = source.printer(
        queue,
        &identity.display_name,
        &identity.service_name,
        &identity.location,
    )?;
    printer.name = identity.name.clone();
    printer.ppd = config
        .state_dir
        .join("ppds")
        .join(format!("{}.ppd", uuid::Uuid::new_v4()));
    let mut validation = config.clone();
    validation.printers.push(printer.clone());
    validation.validate()?;
    let mut profile = PrinterProfile::from_ppd_bytes(&ppd, &choices.settings)?;
    let upstream = Upstream::new(&printer, Duration::from_secs(config.backend_timeout_secs))?;
    let status = upstream
        .inspect()
        .await
        .context("native queue metadata unavailable; nothing installed")?;
    ensure!(
        status.accepting,
        "native queue is not accepting jobs; nothing installed"
    );
    profile.restrict_formats(&status.document_formats)?;
    profile.urf_supported(&status.urf_supported)?;
    for preview in &choices.previews {
        profile.resolve(preview, &profile.capabilities().defaults.document_format)?;
    }
    crate::discovery::preflight(
        config,
        &printer,
        profile.capabilities(),
        &profile.urf_supported(&status.urf_supported)?,
        &status.pwg_types,
    )?;
    Ok(PreparedPrinter {
        config: printer,
        profile: Arc::new(profile),
        upstream,
        status,
        ppd,
    })
}
fn private_directory(path: &Path) -> Result<()> {
    if !path.try_exists()? {
        std::fs::create_dir_all(path)?;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700))?;
    }
    ensure!(
        std::fs::symlink_metadata(path)?.is_dir()
            && std::fs::metadata(path)?.permissions().mode() & 0o077 == 0,
        "managed directory must be private and not a symlink"
    );
    Ok(())
}
fn write_new(path: &Path, bytes: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let result = (|| {
        ensure!(
            crate::auth::private_file_descriptor(&file)?,
            "private PPD/profile file has an unsafe inherited ACL"
        );
        file.write_all(bytes)?;
        file.sync_all()?;
        crate::storage::sync_parent(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}
pub struct CatalogCommit {
    pub catalog: PrinterCatalog,
    pub durability: std::io::Result<()>,
}
/// Caller holds either the offline service lock or the live catalog write lock.
pub fn commit(
    config: &Config,
    catalog: &PrinterCatalog,
    prepared: &PreparedPrinter,
) -> Result<CatalogCommit> {
    ensure!(
        !catalog
            .printers
            .iter()
            .any(|p| p.name == prepared.config.name
                || p.service_name == prepared.config.service_name),
        "printer/service name already exists; reload inventory"
    );
    ensure!(
        catalog.printers.len() < 1024,
        "managed printer capacity reached"
    );
    let mut next = catalog.clone();
    next.revision = next
        .revision
        .checked_add(1)
        .context("catalog revision exhausted")?;
    next.printers.push(prepared.config.clone());
    let catalog_contents = toml::to_string_pretty(&next)?;
    let profile_contents = toml::to_string_pretty(prepared.profile.settings())?;
    ensure!(
        catalog_contents.len() <= 1024 * 1024 && profile_contents.len() <= 256 * 1024,
        "managed catalog/profile exceeds storage limit"
    );
    private_directory(&config.state_dir.join("ppds"))?;
    private_directory(&config.state_dir.join("profiles"))?;
    let profile = config.profile_path(&prepared.config.name);
    ensure!(
        !profile.try_exists()?,
        "profile destination already exists; refusing overwrite"
    );
    write_new(&prepared.config.ppd, &prepared.ppd)?;
    if let Err(error) = write_new(&profile, profile_contents.as_bytes()) {
        let _ = std::fs::remove_file(&prepared.config.ppd);
        return Err(error);
    }
    let installation = (|| {
        crate::storage::StagedFile::new(
            &config.state_dir.join("printers.toml"),
            catalog_contents.as_bytes(),
        )?
        .install()
    })();
    match installation {
        Ok(durability) => Ok(CatalogCommit {
            catalog: next,
            durability,
        }),
        Err(error) => {
            let _ = std::fs::remove_file(&profile);
            let _ = std::fs::remove_file(&prepared.config.ppd);
            Err(error)
        }
    }
}
#[derive(Subcommand)]
pub enum SetupCommand {
    /// Read only: enumerate queues from one protected trusted CUPS source.
    Inventory {
        #[arg(long)]
        source: String,
    },
    /// Read only: inspect an existing enumerated queue and its privately fetched PPD.
    Inspect {
        #[arg(long)]
        source: String,
        #[arg(long)]
        queue: String,
    },
    /// Validate explicit choices and install the same managed catalog used by the browser wizard.
    Install {
        #[arg(long)]
        source: String,
        #[arg(long)]
        queue: String,
        #[arg(long)]
        printer: String,
        #[arg(long)]
        display_name: String,
        #[arg(long)]
        service_name: String,
        #[arg(long, default_value = "")]
        location: String,
        #[arg(long)]
        choices: PathBuf,
        #[arg(long)]
        confirm: bool,
    },
}
pub async fn run(command: SetupCommand, config_path: &Path) -> Result<()> {
    let config = Config::load(config_path)?;
    match command {
        SetupCommand::Inventory { source } => println!(
            "{}",
            serde_json::to_string_pretty(&inventory(&config, &source).await?)?
        ),
        SetupCommand::Inspect { source, queue } => {
            let (queue, ppd) = acquire(&config, &source, &queue).await?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"queue":queue,"native_ppd":PpdInventory::from_bytes(&ppd)?,"notice":"Read-only; no jobs submitted and no native queue/default changed. Native labels and input DPI do not establish material/engine-quality semantics."})
                )?
            );
        }
        SetupCommand::Install {
            source,
            queue,
            printer,
            display_name,
            service_name,
            location,
            choices,
            confirm,
        } => {
            ensure!(
                confirm,
                "reviewed installation requires --confirm; no settings are guessed"
            );
            let _lock = crate::managed::lock_state(&config)?;
            let mut text = String::new();
            std::fs::File::open(choices)?
                .take(256 * 1024 + 1)
                .read_to_string(&mut text)?;
            ensure!(text.len() <= 256 * 1024, "setup choices exceed 256 KiB");
            let choices: SetupChoices = toml::from_str(&text)?;
            let identity = PrinterIdentity {
                name: printer,
                display_name,
                service_name,
                location,
            };
            let prepared = prepare(&config, &source, &queue, &identity, &choices, None).await?;
            println!(
                "Resolved defaults: {}",
                serde_json::to_string_pretty(&prepared.profile.capabilities().defaults)?
            );
            let committed = commit(&config, &crate::managed::load_catalog(&config)?, &prepared)?;
            committed.durability.context("Printer APPLIED but directory synchronization failed; crash durability unconfirmed")?;
            println!(
                "Installed {} in the authoritative managed catalog. HTTP/discovery activation happens on serve; no advertisement or physical print claimed.",
                config.printer_uri(&prepared.config.name)
            );
        }
    }
    Ok(())
}
