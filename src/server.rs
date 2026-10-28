// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::telemetry::Telemetry;
use crate::{
    auth::{AuthError, Authenticator, Principal},
    config::{Config, ProfileSettings},
    jobs::{JobEngine, JobRecord},
    profile::PrinterProfile,
    protocol::{self, Rejection},
    upstream::{Upstream, UpstreamStatus},
    wire::*,
};
use anyhow::{Context, Result};
use axum::{
    Router,
    extract::{ConnectInfo, Path, Request, State},
    http::{HeaderValue, StatusCode as HttpStatus},
    middleware::{self, Next},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use futures_util::TryStreamExt;
use ipp::prelude::*;
use std::{collections::HashMap, net::SocketAddr, sync::Arc, time::Duration};
use tokio::io::{AsyncRead, AsyncReadExt};
use tokio::sync::RwLock;
use tokio_util::io::StreamReader;

pub struct Printer {
    pub config: crate::config::PrinterConfig,
    pub profile: Arc<PrinterProfile>,
    pub upstream: Upstream,
    pub urf_supported: Vec<String>,
    pub pwg_types: Vec<String>,
    pub telemetry: Arc<Telemetry>,
    pub revision: u64,
    pub config_change_time: i32,
}
pub struct App {
    pub config: Config,
    pub engine: Arc<JobEngine>,
    pub printers: parking_lot::RwLock<HashMap<String, Arc<RwLock<Printer>>>>,
    pub auth: Arc<Authenticator>,
    pub catalog: tokio::sync::Mutex<crate::managed::PrinterCatalog>,
    pub onboarding: parking_lot::Mutex<HashMap<String, crate::onboarding::Draft>>,
    pub notifications: Arc<crate::notifications::Notifications>,
    pub discovery_status: parking_lot::RwLock<crate::discovery::PublicationStatus>,
    pub profile_changes: tokio::sync::watch::Sender<u64>,
}
impl App {
    /// Startup requires the authoritative profile and actual native queue formats.
    pub async fn open(config: Config) -> Result<Arc<Self>> {
        config.validate()?;
        let catalog = crate::managed::load_catalog(&config)?;
        let engine = JobEngine::open(&config).await?;
        let auth = engine.auth.clone();
        let notifications = crate::notifications::Notifications::open(
            config.notifications.as_ref(),
            engine.clone(),
        )?;
        let mut printers = HashMap::new();
        for printer in &config.printers {
            let profile_path = config.profile_path(&printer.name);
            let settings = ProfileSettings::load(&profile_path)?;
            let config_change_time = crate::profile::configuration_change_time(
                std::fs::metadata(&profile_path)?.modified()?,
                &printer.ppd,
                engine.time_origin(),
            )?;
            let mut profile = PrinterProfile::load(printer, &settings)?;
            let upstream =
                Upstream::new(printer, Duration::from_secs(config.backend_timeout_secs))?;
            let status = upstream
                .inspect()
                .await
                .with_context(|| format!("inspect upstream for {}", printer.name))?;
            profile.restrict_formats(&status.document_formats)?;
            let urf_supported = profile.urf_supported(&status.urf_supported)?;
            let telemetry = Telemetry::open(
                printer.telemetry.as_ref(),
                Duration::from_secs(config.backend_timeout_secs),
            )?;
            printers.insert(
                printer.name.clone(),
                Arc::new(RwLock::new(Printer {
                    config: printer.clone(),
                    profile: Arc::new(profile),
                    upstream,
                    urf_supported,
                    pwg_types: status.pwg_types,
                    telemetry,
                    revision: 0,
                    config_change_time,
                })),
            );
        }
        let (profile_changes, _) = tokio::sync::watch::channel(0);
        let publication = crate::discovery::PublicationStatus::initial(config.discovery.is_some());
        Ok(Arc::new(Self {
            config,
            engine,
            printers: parking_lot::RwLock::new(printers),
            auth,
            catalog: tokio::sync::Mutex::new(catalog),
            onboarding: parking_lot::Mutex::new(HashMap::new()),
            notifications,
            discovery_status: parking_lot::RwLock::new(publication),
            profile_changes,
        }))
    }
    pub fn printer(&self, name: &str) -> Option<Arc<RwLock<Printer>>> {
        self.printers.read().get(name).cloned()
    }
    pub fn printer_snapshot(&self) -> Vec<(String, Arc<RwLock<Printer>>)> {
        self.printers
            .read()
            .iter()
            .map(|(name, printer)| (name.clone(), printer.clone()))
            .collect()
    }
    pub fn configuration_changed(&self) {
        let mut publication = self.discovery_status.write();
        let revision = (*self.profile_changes.borrow()).wrapping_add(1);
        publication.revision = revision;
        if self.config.discovery.is_some() {
            publication.state = "pending".into();
            publication.failure = None;
        }
        self.profile_changes.send_replace(revision);
    }
    pub async fn install_printer(
        &self,
        principal: &Principal,
        prepared: crate::setup::PreparedPrinter,
        expected_catalog_revision: u64,
    ) -> Result<()> {
        let mut catalog = self.catalog.lock().await;
        anyhow::ensure!(
            catalog.revision == expected_catalog_revision,
            "catalog changed; inspect and review the wizard again"
        );
        let urf_supported = prepared
            .profile
            .urf_supported(&prepared.status.urf_supported)?;
        let telemetry = Telemetry::open(
            prepared.config.telemetry.as_ref(),
            Duration::from_secs(self.config.backend_timeout_secs),
        )?;
        let name = prepared.config.name.clone();
        let runtime = Arc::new(RwLock::new(Printer {
            config: prepared.config.clone(),
            profile: prepared.profile.clone(),
            upstream: prepared.upstream.clone(),
            urf_supported,
            pwg_types: prepared.status.pwg_types.clone(),
            telemetry,
            revision: 0,
            config_change_time: self.engine.uptime(),
        }));
        let mut next = self.printers.read().clone();
        next.insert(name, runtime);
        let committed = self.auth.with_access(principal, None, true, || {
            let committed = crate::setup::commit(&self.config, &catalog, &prepared)?;
            self.engine
                .register_backend(&prepared.config, prepared.upstream.clone());
            *self.printers.write() = next;
            Ok(committed)
        })?;
        *catalog = committed.catalog;
        self.configuration_changed();
        committed.durability.context("Printer APPLIED and HTTP-active, but directory synchronization failed; crash durability unconfirmed. Discovery is separately supervised.")
    }
    pub async fn run_notifications(
        self: Arc<Self>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> Result<()> {
        let mut interval = tokio::time::interval(Duration::from_secs(5));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                _=interval.tick()=>{
                    tokio::select! {
                        result=self.notification_tick()=>result?,
                        changed=shutdown.changed()=>{if changed.is_err()||*shutdown.borrow(){return Ok(());}}
                    }
                },
                changed=shutdown.changed()=>{if changed.is_err()||*shutdown.borrow(){return Ok(());}}
            }
        }
    }
    async fn notification_tick(&self) -> Result<()> {
        if !self.notifications.configured() {
            return Ok(());
        }
        for (name, entry) in self.printer_snapshot() {
            if !self.notifications.is_enabled(&name)? {
                continue;
            }
            let (upstream, telemetry) = {
                let printer = entry.read().await;
                (printer.upstream.clone(), printer.telemetry.clone())
            };
            match upstream.inspect().await {
                Ok(status) => {
                    self.notifications.observe(
                        &name,
                        "native-queue-unreachable",
                        Some(false),
                        "native-queue",
                    )?;
                    let paper = (!status.reasons.is_empty())
                        .then(|| reported_paper_shortage(&status.reasons));
                    self.notifications
                        .observe(&name, "paper-shortage", paper, "native-queue")?;
                }
                Err(_) => {
                    self.notifications.observe(
                        &name,
                        "native-queue-unreachable",
                        Some(true),
                        "native-queue",
                    )?;
                }
            }
            let snapshot = telemetry.snapshot();
            if snapshot.source.is_some() {
                let unreachable = snapshot
                    .last_error
                    .as_ref()
                    .map(|_| true)
                    .or_else(|| snapshot.observed_at.map(|_| false));
                self.notifications.observe(
                    &name,
                    "telemetry-source-unreachable",
                    unreachable,
                    "telemetry",
                )?;
                let paper = (snapshot.freshness == crate::telemetry::Freshness::Fresh
                    && !snapshot.measurements.printer_state_reasons.is_empty())
                .then(|| reported_paper_shortage(&snapshot.measurements.printer_state_reasons));
                self.notifications
                    .observe(&name, "paper-shortage", paper, "telemetry")?;
            }
        }
        self.notifications.tick().await
    }
    pub fn router(self: &Arc<Self>) -> Router {
        Router::new()
            .merge(crate::accounts::router())
            .merge(crate::onboarding::router())
            .merge(crate::notification_admin::router())
            .merge(crate::recipe_admin::router())
            .route("/", get(crate::admin::status_page))
            .route("/admin", get(crate::admin::admin_page))
            .route(
                "/admin/printers/{printer}",
                get(crate::admin::profile_page)
                    .post(crate::admin::save_profile)
                    .layer(axum::extract::DefaultBodyLimit::max(800 * 1024)),
            )
            .route("/api/jobs", get(crate::admin::jobs_api))
            .route("/api/jobs/{id}", get(crate::admin::job_api))
            .route(
                "/api/jobs/{id}/cancel",
                post(crate::admin::cancel_job).layer(axum::extract::DefaultBodyLimit::max(2048)),
            )
            .route("/api/printers", get(crate::admin::printers_api))
            .route("/source", get(source))
            .route("/health", get(health))
            .route("/ipp/print/{printer}", post(ipp_handler))
            .route("/ipp/print/{printer}/jobs/{id}", post(ipp_handler))
            .layer(axum::extract::DefaultBodyLimit::disable())
            .layer(middleware::from_fn_with_state(self.clone(), access))
            .with_state(self.clone())
    }
}
fn reported_paper_shortage(reasons: &[String]) -> bool {
    reasons.iter().any(|reason| {
        matches!(
            reason.as_str(),
            "media-empty"
                | "media-empty-warning"
                | "media-empty-error"
                | "media-needed"
                | "media-needed-warning"
                | "media-needed-error"
        )
    })
}
#[derive(Clone, Copy)]
struct SourceAuthentication {
    required: bool,
}
async fn access(State(app): State<Arc<App>>, mut request: Request, next: Next) -> Response {
    let Some(peer) = request
        .extensions()
        .get::<ConnectInfo<SocketAddr>>()
        .map(|p| p.0.ip())
    else {
        return HttpStatus::FORBIDDEN.into_response();
    };
    let allowed = if app.config.allowed_clients.is_empty() {
        peer.is_loopback()
    } else {
        app.config.allowed_clients.iter().any(|n| n.contains(&peer))
    };
    if !allowed {
        return (HttpStatus::FORBIDDEN, "Client network is not allowed").into_response();
    }
    let anonymous_allowed = app.config.access.anonymous_trusted_lan
        && app
            .config
            .access
            .anonymous_clients
            .iter()
            .any(|network| network.contains(&peer));
    let principal = if let Some(header) = request.headers().get("authorization") {
        match header.to_str() {
            Ok(value) => match app.auth.authenticate(value).await {
                Ok(principal) => Some(principal),
                Err(AuthError::Busy) => {
                    return (
                        HttpStatus::SERVICE_UNAVAILABLE,
                        "Authentication capacity reached",
                    )
                        .into_response();
                }
                Err(AuthError::Invalid) => return authentication_required(),
            },
            Err(_) => return authentication_required(),
        }
    } else if anonymous_allowed {
        app.auth.anonymous()
    } else {
        None
    };
    if principal.is_none() {
        let capabilities = request.method() == axum::http::Method::POST
            && request
                .extensions()
                .get::<axum::extract::MatchedPath>()
                .is_some_and(|path| path.as_str() == "/ipp/print/{printer}");
        let source = matches!(
            *request.method(),
            axum::http::Method::GET | axum::http::Method::HEAD
        ) && request.uri().path() == "/source";
        if !capabilities && !source {
            return authentication_required();
        }
    }
    if request.uri().path().starts_with("/admin") {
        match principal.as_ref() {
            Some(principal) if principal.is_admin() => {}
            Some(principal) if principal.authenticated => {
                return (HttpStatus::FORBIDDEN, "Administrator role required").into_response();
            }
            _ => return authentication_required(),
        }
    }
    if let Some(principal) = principal {
        request.extensions_mut().insert(principal);
    }
    request.extensions_mut().insert(SourceAuthentication {
        required: !anonymous_allowed,
    });
    let mut response = next.run(request).await;
    for (name, value) in [
        (
            "content-security-policy",
            "default-src 'none'; style-src 'unsafe-inline'; form-action 'self'; frame-ancestors 'none'; base-uri 'none'",
        ),
        ("x-content-type-options", "nosniff"),
        ("referrer-policy", "same-origin"),
        ("cache-control", "no-store"),
        (
            "link",
            "</source>; rel=\"related\"; type=\"application/gzip\"; title=\"Corresponding source (AGPL-3.0-only)\"",
        ),
    ] {
        response.headers_mut().insert(
            axum::http::HeaderName::from_static(name),
            HeaderValue::from_static(value),
        );
    }
    response
}
fn authentication_required() -> Response {
    let mut response = (HttpStatus::UNAUTHORIZED, "Authentication required").into_response();
    response.headers_mut().insert(
        "www-authenticate",
        HeaderValue::from_static("Basic realm=\"Ferriprint\", charset=\"UTF-8\""),
    );
    response
}
async fn health() -> impl IntoResponse {
    (HttpStatus::OK, "Ferriprint HTTP service is running\n")
}
async fn source() -> impl IntoResponse {
    (
        [
            ("content-type", "application/gzip"),
            (
                "content-disposition",
                "attachment; filename=\"ferriprint-source.tar.gz\"",
            ),
            ("cache-control", "public, max-age=0, must-revalidate"),
        ],
        include_bytes!(concat!(env!("OUT_DIR"), "/ferriprint-source.tar.gz")).as_slice(),
    )
}
async fn ipp_handler(
    State(app): State<Arc<App>>,
    Path(path): Path<HashMap<String, String>>,
    mut request: Request,
) -> Response {
    if request
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.split(';').next())
        != Some("application/ipp")
    {
        return (
            HttpStatus::UNSUPPORTED_MEDIA_TYPE,
            "application/ipp required",
        )
            .into_response();
    }
    let principal = request.extensions_mut().remove::<Principal>();
    let authentication = request
        .extensions_mut()
        .remove::<SourceAuthentication>()
        .expect("access middleware");
    let Some(printer_name) = path.get("printer") else {
        return HttpStatus::NOT_FOUND.into_response();
    };
    if app.printer(printer_name).is_none() {
        return HttpStatus::NOT_FOUND.into_response();
    }
    let path_id = match path.get("id") {
        Some(s) => match s.parse::<i32>() {
            Ok(id) if id > 0 => Some(id),
            _ => return HttpStatus::NOT_FOUND.into_response(),
        },
        None => None,
    };
    let mut reader = StreamReader::new(
        request
            .into_body()
            .into_data_stream()
            .map_err(std::io::Error::other),
    );
    let parsed = tokio::time::timeout(
        Duration::from_secs(60),
        read_metadata(&mut reader, app.config.max_attribute_bytes),
    )
    .await;
    let message = match parsed {
        Ok(Ok(message)) => Metadata::from(message),
        Ok(Err(error)) => {
            return (
                HttpStatus::BAD_REQUEST,
                format!("Invalid IPP metadata: {error}"),
            )
                .into_response();
        }
        Err(_) => {
            return (HttpStatus::REQUEST_TIMEOUT, "IPP metadata read timeout").into_response();
        }
    };
    // A capability-only peer has no job owner. Challenge every other operation
    // after bounded metadata parsing, before touching any job state.
    if principal.is_none() && message.header().operation_or_status != 11 {
        return authentication_required();
    }
    let result = tokio::time::timeout(
        Duration::from_secs(300),
        handle(
            &app,
            printer_name,
            path_id,
            principal.as_ref(),
            authentication.required,
            &message,
            &mut reader,
        ),
    )
    .await;
    let result = match result {
        Ok(result) => result,
        Err(_) => protocol::reject(
            &message,
            Rejection::status(
                ipp::model::StatusCode::ClientErrorTimeout,
                "request timed out; incomplete documents are not submitted",
            ),
        ),
    };
    match result {
        Ok(reply) => ([("content-type", "application/ipp")], reply.to_bytes()).into_response(),
        Err(error) => {
            tracing::error!(%error,"IPP request failed");
            match protocol::reject(
                &message,
                Rejection::status(
                    ipp::model::StatusCode::ServerErrorInternalError,
                    "Internal storage or transport failure",
                ),
            ) {
                Ok(reply) => {
                    ([("content-type", "application/ipp")], reply.to_bytes()).into_response()
                }
                Err(_) => HttpStatus::INTERNAL_SERVER_ERROR.into_response(),
            }
        }
    }
}
async fn no_payload<R: AsyncRead + Unpin>(reader: &mut R) -> std::result::Result<(), Rejection> {
    let mut byte = [0; 1];
    match reader.read(&mut byte).await {
        Ok(0) => Ok(()),
        _ => Err(Rejection::bad(
            "document data is not allowed for this operation",
        )),
    }
}
fn engine_error(error: &anyhow::Error) -> Rejection {
    let message = error.to_string();
    let status = if message.contains("capacity") {
        ipp::model::StatusCode::ServerErrorBusy
    } else if message.contains("not authorized") {
        ipp::model::StatusCode::ClientErrorNotAuthorized
    } else if message.contains("not found") {
        ipp::model::StatusCode::ClientErrorNotFound
    } else if message.contains("limit") || message.contains("too large") {
        ipp::model::StatusCode::ClientErrorRequestEntityTooLong
    } else {
        ipp::model::StatusCode::ClientErrorNotPossible
    };
    Rejection::status(status, message)
}
async fn selected_job(
    app: &App,
    id: i32,
    printer: &str,
) -> std::result::Result<JobRecord, Rejection> {
    app.engine
        .get_job(id)
        .await
        .map_err(|e| engine_error(&e))?
        .filter(|j| j.printer == printer)
        .ok_or_else(|| {
            Rejection::status(
                ipp::model::StatusCode::ClientErrorNotFound,
                "job is not on this local printer",
            )
        })
}
async fn handle<R: AsyncRead + Unpin + Send>(
    app: &App,
    printer_name: &str,
    path_id: Option<i32>,
    principal: Option<&Principal>,
    authentication_required: bool,
    request: &Metadata,
    reader: &mut R,
) -> Result<IppRequestResponse> {
    let id = match protocol::validate_envelope(
        request,
        &app.config.printer_uri(printer_name),
        path_id,
    ) {
        Ok(id) => id,
        Err(e) => return protocol::reject(request, e),
    };
    let op = request.header().operation_or_status;
    if !matches!(op, 2 | 6)
        && let Err(error) = no_payload(reader).await
    {
        return protocol::reject(request, error);
    }
    // This read lock spans resolution and durable job creation. Profile writers
    // hold the exclusive lock across the active-job gate and durable apply.
    let entry = app.printer(printer_name).context("printer disappeared")?;
    let printer = entry.read().await;
    let g = DelimiterTag::OperationAttributes;
    let mut reply = protocol::response(request, ipp::model::StatusCode::SuccessfulOk)?;
    if let Some(principal) = principal
        && !app.auth.can_print(principal, printer_name)
    {
        return protocol::reject(
            request,
            Rejection::status(
                ipp::model::StatusCode::ClientErrorNotAuthorized,
                "Current printer permission required",
            ),
        );
    }
    if op == 11 {
        let mut status = match printer.upstream.inspect().await {
            Ok(mut status) => {
                status.urf_supported = printer.urf_supported.clone();
                status.pwg_types = printer.pwg_types.clone();
                status
            }
            Err(error) => UpstreamStatus {
                state: 5,
                accepting: false,
                reasons: vec!["service-off-line".into()],
                document_formats: {
                    tracing::warn!(%error,"upstream status unavailable");
                    Vec::new()
                },
                urf_supported: printer.urf_supported.clone(),
                pwg_types: printer.pwg_types.clone(),
            },
        };
        status.accepting &= app.engine.has_capacity().await?;
        let queued = app.engine.active_count(printer_name).await?;
        protocol::printer_attributes(
            &mut reply,
            &app.config,
            &printer.config,
            printer.profile.capabilities(),
            &status,
            protocol::PrinterRuntime {
                uptime: app.engine.uptime(),
                config_changed: printer.config_change_time,
                queued,
                authentication_required,
            },
        )?;
        crate::telemetry::add_ipp(&printer.telemetry.snapshot(), &mut reply)?;
        protocol::filter_requested(request, &mut reply);
        return Ok(reply);
    }
    let principal = principal.context("non-capability IPP operation requires a principal")?;
    let owner = principal.username.as_str();
    match op {
        2 | 4 | 5 => {
            let template = match protocol::template(request) {
                Ok(t) => t,
                Err(e) => return protocol::reject(request, e),
            };
            let format = attr(request, g, "document-format")
                .and_then(str_value)
                .unwrap_or(&printer.profile.capabilities().defaults.document_format);
            let resolved = match printer.profile.resolve(&template, format) {
                Ok(resolved) => resolved,
                Err(e) => {
                    return protocol::reject(request, protocol::profile_rejection(request, e));
                }
            };
            if op == 4 {
                return Ok(reply);
            }
            match printer.upstream.inspect().await {
                Ok(s) if s.accepting => {}
                _ => {
                    return protocol::reject(
                        request,
                        Rejection::status(
                            ipp::model::StatusCode::ServerErrorNotAcceptingJobs,
                            "native queue is unavailable or not accepting jobs",
                        ),
                    );
                }
            }
            let title = attr(request, g, "job-name")
                .and_then(str_value)
                .unwrap_or("Untitled");
            let job = match app
                .engine
                .create_job(printer_name, principal, title, format, resolved)
                .await
            {
                Ok(job) => job,
                Err(e) => return protocol::reject(request, engine_error(&e)),
            };
            let job = if op == 2 {
                match app.engine.ingest_document(job.id, principal, reader).await {
                    Ok(job) => job,
                    Err(e) => return protocol::reject(request, engine_error(&e)),
                }
            } else {
                job
            };
            protocol::job_attributes(
                &mut reply,
                &app.config,
                &job,
                app.engine.time_origin(),
                app.engine.uptime(),
            )?;
        }
        6 => {
            let job = match selected_job(app, id.context("missing job id")?, printer_name).await {
                Ok(job) => job,
                Err(e) => return protocol::reject(request, e),
            };
            if job.owner != owner {
                return protocol::reject(
                    request,
                    Rejection::status(
                        ipp::model::StatusCode::ClientErrorNotAuthorized,
                        "not authorized to attach to this job",
                    ),
                );
            }
            let format = attr(request, g, "document-format")
                .and_then(str_value)
                .unwrap_or(&job.document_format);
            if !printer
                .profile
                .capabilities()
                .document_formats
                .iter()
                .any(|f| f == format)
            {
                return protocol::reject(
                    request,
                    Rejection::status(
                        ipp::model::StatusCode::ClientErrorDocumentFormatNotSupported,
                        "unsupported document format",
                    ),
                );
            }
            if let Err(e) = app
                .engine
                .set_document_format(job.id, principal, format)
                .await
            {
                return protocol::reject(request, engine_error(&e));
            }
            let job = match app.engine.ingest_document(job.id, principal, reader).await {
                Ok(job) => job,
                Err(e) => return protocol::reject(request, engine_error(&e)),
            };
            protocol::job_attributes(
                &mut reply,
                &app.config,
                &job,
                app.engine.time_origin(),
                app.engine.uptime(),
            )?;
        }
        8 | 9 => {
            let job = match selected_job(app, id.context("missing job id")?, printer_name).await {
                Ok(job) => job,
                Err(e) => return protocol::reject(request, e),
            };
            if !app.auth.can_access(principal, &job.printer, &job.owner) {
                return protocol::reject(
                    request,
                    Rejection::status(
                        ipp::model::StatusCode::ClientErrorNotAuthorized,
                        "not authorized to access this job",
                    ),
                );
            }
            if op == 8 {
                if let Err(e) = app.engine.cancel_job(job.id, principal).await {
                    return protocol::reject(request, engine_error(&e));
                }
            } else {
                protocol::job_attributes(
                    &mut reply,
                    &app.config,
                    &job,
                    app.engine.time_origin(),
                    app.engine.uptime(),
                )?;
            }
        }
        10 => {
            let which = attr(request, g, "which-jobs")
                .and_then(str_value)
                .unwrap_or("not-completed");
            let mine = matches!(attr(request, g, "my-jobs"), Some(IppValue::Boolean(true)));
            let limit = attr(request, g, "limit").and_then(integer).unwrap_or(1000) as usize;
            for job in app
                .engine
                .query_jobs(
                    printer_name,
                    (mine || !principal.is_admin()).then_some(owner),
                    which,
                    limit,
                )
                .await?
            {
                protocol::job_attributes(
                    &mut reply,
                    &app.config,
                    &job,
                    app.engine.time_origin(),
                    app.engine.uptime(),
                )?;
            }
        }
        _ => unreachable!("validated operation"),
    }
    protocol::filter_requested(request, &mut reply);
    Ok(reply)
}
