// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl
// Deterministic wire-level test device. It has no CUPS or physical printer connection.
#![allow(dead_code)]
use axum::{
    Router,
    body::Body,
    extract::{Request, State},
    response::IntoResponse,
    routing::post,
};
use ferriprint::{config::Config, wire::*};
use futures_util::TryStreamExt;
use ipp::prelude::*;
use parking_lot::Mutex;
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicUsize, Ordering},
    },
    time::Duration,
};
use tokio::{io::AsyncReadExt, sync::Notify};
use tokio_util::io::StreamReader;

#[derive(Clone, Debug)]
pub struct Received {
    pub id: i32,
    pub state: i32,
    pub options: BTreeMap<String, String>,
    pub format: String,
    pub title: String,
    pub bytes: u64,
    pub owner: String,
    pub resolution: Option<(i32, i32)>,
}
pub struct Device {
    pub jobs: Mutex<Vec<Received>>,
    pub cancellations: Mutex<Vec<i32>>,
    pub attempts: AtomicUsize,
    /// 0 normal; 1 reject; 2 delayed response; 3 gated response; 4 PDF-only capability.
    pub mode: AtomicU8,
    pub accepted: Notify,
    pub gate: Notify,
    pub formats: Vec<String>,
}
pub struct Receiver {
    pub address: SocketAddr,
    pub device: Arc<Device>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Receiver {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Receiver {
    pub async fn start() -> Self {
        Self::bind("127.0.0.1:0".parse().unwrap()).await
    }
    pub async fn bind(address: SocketAddr) -> Self {
        assert!(
            address.ip().is_loopback(),
            "test receiver must bind loopback"
        );
        let listener = tokio::net::TcpListener::bind(address).await.unwrap();
        let address = listener.local_addr().unwrap();
        let device = Arc::new(Device {
            jobs: Mutex::new(Vec::new()),
            cancellations: Mutex::new(Vec::new()),
            attempts: AtomicUsize::new(0),
            mode: AtomicU8::new(0),
            accepted: Notify::new(),
            gate: Notify::new(),
            formats: vec![
                "application/pdf".into(),
                "image/jpeg".into(),
                "image/png".into(),
                "image/pwg-raster".into(),
                "image/urf".into(),
            ],
        });
        let router = Router::new()
            .route("/printers/receiver", post(receive))
            .with_state(device.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            address,
            device,
            task,
        }
    }
    pub fn uri(&self) -> String {
        format!("ipp://{}/printers/receiver", self.address)
    }
    pub fn complete(&self, id: i32) {
        self.device
            .jobs
            .lock()
            .iter_mut()
            .find(|j| j.id == id)
            .unwrap()
            .state = 9;
    }
}
async fn receive(State(device): State<Arc<Device>>, request: Request) -> impl IntoResponse {
    let mut reader = StreamReader::new(
        request
            .into_body()
            .into_data_stream()
            .map_err(std::io::Error::other),
    );
    let request = read_metadata(&mut reader, 256 * 1024).await.unwrap();
    let mut response = IppRequestResponse::new_response(
        IppVersion::v2_0(),
        StatusCode::SuccessfulOk,
        request.header().request_id,
    )
    .unwrap();
    let op = DelimiterTag::OperationAttributes;
    let job_group = DelimiterTag::JobAttributes;
    match request.header().operation_or_status {
        11 => {
            for (k, v) in [
                ("printer-state", IppValue::Enum(3)),
                ("printer-is-accepting-jobs", IppValue::Boolean(true)),
                ("printer-state-reasons", keyword("none").unwrap()),
                (
                    "document-format-supported",
                    strings(
                        &if device.mode.load(Ordering::SeqCst) == 4 {
                            vec!["application/pdf".into()]
                        } else {
                            device.formats.clone()
                        },
                        mime,
                    )
                    .unwrap(),
                ),
                (
                    "urf-supported",
                    strings(
                        &[
                            "V1.4".into(),
                            "W8".into(),
                            "SRGB24".into(),
                            "RS300".into(),
                            "PQ4".into(),
                        ],
                        keyword,
                    )
                    .unwrap(),
                ),
                (
                    "pwg-raster-document-type-supported",
                    strings(&["sgray_8".into(), "srgb_8".into()], keyword).unwrap(),
                ),
            ] {
                add(&mut response, DelimiterTag::PrinterAttributes, k, v).unwrap();
            }
        }
        2 => {
            device.attempts.fetch_add(1, Ordering::SeqCst);
            let mut bytes = 0u64;
            let mut buffer = [0; 8192];
            loop {
                let n = reader.read(&mut buffer).await.unwrap();
                if n == 0 {
                    break;
                }
                bytes += n as u64;
            }
            if device.mode.load(Ordering::SeqCst) == 1 {
                response.header_mut().operation_or_status =
                    StatusCode::ClientErrorDocumentFormatNotSupported as i16;
                add(
                    &mut response,
                    op,
                    "status-message",
                    text("deterministic decoder rejects this document").unwrap(),
                )
                .unwrap();
            } else {
                let mut options = BTreeMap::new();
                if let Some(group) = request.attributes().first_of(job_group) {
                    for a in group.attributes() {
                        if let Some(value) = str_value(a.value()) {
                            options.insert(a.name().to_string(), value.into());
                        }
                    }
                }
                // The receiver enforces native media, just as a CUPS driver would.
                assert!(matches!(
                    options.get("PageSize").map(String::as_str),
                    Some("A4" | "A3" | "TA3" | "Letter")
                ));
                assert!(options.contains_key("Quality") && options.contains_key("InputSlot"));
                let id = {
                    let mut jobs = device.jobs.lock();
                    let id = 700 + jobs.len() as i32;
                    jobs.push(Received {
                        id,
                        state: 5,
                        options,
                        format: attr(&request, op, "document-format")
                            .and_then(str_value)
                            .unwrap()
                            .into(),
                        title: attr(&request, op, "job-name")
                            .and_then(str_value)
                            .unwrap()
                            .into(),
                        bytes,
                        owner: attr(&request, op, "requesting-user-name")
                            .and_then(str_value)
                            .unwrap()
                            .into(),
                        resolution: match attr(&request, job_group, "printer-resolution") {
                            Some(IppValue::Resolution {
                                cross_feed,
                                feed,
                                units: 3,
                            }) => Some((*cross_feed, *feed)),
                            _ => None,
                        },
                    });
                    id
                };
                device.accepted.notify_one();
                match device.mode.load(Ordering::SeqCst) {
                    2 => tokio::time::sleep(Duration::from_secs(2)).await,
                    3 => device.gate.notified().await,
                    _ => {}
                }
                remote_attributes(&mut response, id, 5);
            }
        }
        9 => {
            let id = attr(&request, op, "job-id").and_then(integer).unwrap();
            let state = device
                .jobs
                .lock()
                .iter()
                .find(|j| j.id == id)
                .unwrap()
                .state;
            remote_attributes(&mut response, id, state);
        }
        8 => {
            let id = attr(&request, op, "job-id").and_then(integer).unwrap();
            device.cancellations.lock().push(id);
            device
                .jobs
                .lock()
                .iter_mut()
                .find(|j| j.id == id)
                .unwrap()
                .state = 7;
        }
        _ => panic!("unimplemented receiver operation"),
    }
    (
        [("content-type", "application/ipp")],
        Body::from(response.to_bytes()),
    )
}
fn remote_attributes(response: &mut IppRequestResponse, id: i32, state: i32) {
    for (k, v) in [
        ("job-id", IppValue::Integer(id)),
        (
            "job-uri",
            uri(&format!("ipp://127.0.0.1/jobs/{id}")).unwrap(),
        ),
        ("job-state", IppValue::Enum(state)),
        (
            "job-state-reasons",
            keyword(if state == 9 {
                "job-completed-successfully"
            } else if state == 7 {
                "job-canceled-by-user"
            } else {
                "job-printing"
            })
            .unwrap(),
        ),
        (
            "job-impressions-completed",
            IppValue::Integer(if state == 9 { 2 } else { 0 }),
        ),
        (
            "job-media-sheets-completed",
            IppValue::Integer(if state == 9 { 1 } else { 0 }),
        ),
    ] {
        add(response, DelimiterTag::JobAttributes, k, v).unwrap();
    }
}
pub fn config(directory: &std::path::Path, upstream: &str, port: u16) -> Config {
    let text = include_str!("../../examples/ferriprint.toml");
    let mut config: Config = toml::from_str(text).unwrap();
    config.state_dir = directory.to_owned();
    config.public_url = format!("ipp://127.0.0.1:{port}/").parse().unwrap();
    config.listen = format!("127.0.0.1:{port}").parse().unwrap();
    config.backend_timeout_secs = 1;
    config.poll_interval_ms = 100;
    config.printers[0].ppd =
        std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/native.ppd");
    config.printers[0].upstream_uri = upstream.parse().unwrap();
    std::fs::create_dir_all(config.state_dir.join("profiles")).unwrap();
    if !config.profile_path("demo").exists() {
        std::fs::write(
            config.profile_path("demo"),
            include_str!("../../examples/profile.toml"),
        )
        .unwrap();
    }
    config
}
pub fn settings(config: &Config) -> ferriprint::config::ProfileSettings {
    ferriprint::config::ProfileSettings::load(&config.profile_path("demo")).unwrap()
}
pub fn save_settings(config: &Config, settings: &ferriprint::config::ProfileSettings) {
    std::fs::write(
        config.profile_path("demo"),
        toml::to_string_pretty(settings).unwrap(),
    )
    .unwrap();
}
pub fn add_user(
    config: &mut Config,
    username: &str,
    role: ferriprint::config::Role,
    password: &str,
) {
    use std::os::unix::fs::PermissionsExt;
    let path = config.state_dir.join(format!("{username}.argon2"));
    std::fs::write(
        &path,
        ferriprint::auth::password_hash(password.as_bytes()).unwrap(),
    )
    .unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
    config.access.users.push(ferriprint::config::UserConfig {
        username: username.into(),
        role,
        password_hash_file: path,
    });
}
pub struct Frontend {
    pub app: Arc<ferriprint::server::App>,
    pub address: SocketAddr,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Frontend {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Frontend {
    pub async fn start(config: Config) -> Self {
        let app = ferriprint::server::App::open(config).await.unwrap();
        Self::from_app(app).await
    }
    pub async fn from_app(app: Arc<ferriprint::server::App>) -> Self {
        let listener = tokio::net::TcpListener::bind(app.config.listen)
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let router = app
            .router()
            .into_make_service_with_connect_info::<SocketAddr>();
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self { app, address, task }
    }
    pub fn printer_uri(&self) -> String {
        self.app.config.printer_uri("demo")
    }
    pub fn request(&self, operation: Operation, id: Option<i32>) -> IppRequestResponse {
        let mut r = IppRequestResponse::new(
            IppVersion::v2_0(),
            operation,
            Some(self.printer_uri().parse().unwrap()),
        )
        .unwrap();
        if let Some(id) = id {
            add(
                &mut r,
                DelimiterTag::OperationAttributes,
                "job-id",
                IppValue::Integer(id),
            )
            .unwrap();
        }
        r
    }
    pub async fn send(&self, request: IppRequestResponse, document: &[u8]) -> IppRequestResponse {
        let prefix = request.to_bytes();
        let document = bytes::Bytes::copy_from_slice(document);
        let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(prefix), Ok(document)]);
        let response = reqwest::Client::new()
            .post(format!("http://{}/ipp/print/demo", self.address))
            .header("content-type", "application/ipp")
            .body(reqwest::Body::wrap_stream(stream))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let mut reader = StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
        read_metadata(&mut reader, 1024 * 1024).await.unwrap()
    }
}
