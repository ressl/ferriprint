// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl
// Every listener is isolated loopback metadata-only IPP; no CUPS or printer access.

use axum::{
    Router,
    body::{Body, to_bytes},
    extract::{Request, State},
    http::{StatusCode as HttpStatus, header},
    response::{IntoResponse, Response},
    routing::post,
};
use bytes::Bytes;
use ferriprint::{
    telemetry::{
        Availability, Freshness, MeasurementUnit, ReportedValue, Telemetry, TelemetryConfig,
        TelemetrySourceKind, add_ipp,
    },
    wire::*,
};
use ipp::prelude::*;
use parking_lot::RwLock;
use std::{
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU8, AtomicU64, AtomicUsize, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

const NORMAL: u8 = 0;
const HTTP_ERROR: u8 = 1;
const WRONG_ID: u8 = 2;
const OVERSIZED: u8 = 3;
const CHUNKED_OVERSIZED: u8 = 4;
const WRONG_MIME: u8 = 5;
const MALFORMED: u8 = 6;
const REDIRECT: u8 = 7;
const IPP_ERROR: u8 = 8;
const TRAILING_DATA: u8 = 9;

struct Metadata {
    attributes: RwLock<Vec<(&'static str, IppValue)>>,
    requested: RwLock<Vec<String>>,
    mode: AtomicU8,
    delay_ms: AtomicU64,
    requests: AtomicUsize,
    active: AtomicUsize,
    max_active: AtomicUsize,
}
struct MetadataSource {
    address: SocketAddr,
    secure: bool,
    metadata: Arc<Metadata>,
    task: tokio::task::JoinHandle<()>,
}
impl MetadataSource {
    async fn start(attributes: Vec<(&'static str, IppValue)>) -> Self {
        Self::bind(attributes, None).await
    }
    async fn tls(attributes: Vec<(&'static str, IppValue)>, name: &str) -> (Self, String) {
        // A new disposable key is generated in memory; no repository fixture key
        // and no trust-store modification is needed for these consumer tests.
        let rcgen::CertifiedKey { cert, key_pair } =
            rcgen::generate_simple_self_signed(vec![name.to_owned()]).unwrap();
        let pem = cert.pem();
        let tls = axum_server::tls_rustls::RustlsConfig::from_pem(
            pem.as_bytes().to_vec(),
            key_pair.serialize_pem().into_bytes(),
        )
        .await
        .unwrap();
        (Self::bind(attributes, Some(tls)).await, pem)
    }
    async fn bind(
        attributes: Vec<(&'static str, IppValue)>,
        tls: Option<axum_server::tls_rustls::RustlsConfig>,
    ) -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        assert!(address.ip().is_loopback());
        let metadata = Arc::new(Metadata {
            attributes: RwLock::new(attributes),
            requested: RwLock::new(Vec::new()),
            mode: AtomicU8::new(NORMAL),
            delay_ms: AtomicU64::new(0),
            requests: AtomicUsize::new(0),
            active: AtomicUsize::new(0),
            max_active: AtomicUsize::new(0),
        });
        let router = Router::new()
            .route("/metadata", post(observe))
            .route("/redirect-target", post(observe))
            .with_state(metadata.clone());
        let secure = tls.is_some();
        let task = tokio::spawn(async move {
            if let Some(tls) = tls {
                axum_server::from_tcp_rustls(listener.into_std().unwrap(), tls)
                    .serve(router.into_make_service())
                    .await
                    .unwrap();
            } else {
                axum::serve(listener, router).await.unwrap();
            }
        });
        Self {
            address,
            secure,
            metadata,
            task,
        }
    }
    fn uri(&self) -> String {
        format!(
            "{}://{}/metadata",
            if self.secure { "ipps" } else { "ipp" },
            self.address
        )
    }
    fn config(&self) -> TelemetryConfig {
        toml::from_str(&format!("uri = {:?}", self.uri())).unwrap()
    }
    fn mode(&self, mode: u8) {
        self.metadata.mode.store(mode, Ordering::SeqCst);
    }
    fn delay(&self, milliseconds: u64) {
        self.metadata.delay_ms.store(milliseconds, Ordering::SeqCst);
    }
    fn attributes(&self, attributes: Vec<(&'static str, IppValue)>) {
        *self.metadata.attributes.write() = attributes;
    }
}
impl Drop for MetadataSource {
    fn drop(&mut self) {
        self.task.abort();
    }
}
struct ActiveRequest(Arc<Metadata>);
impl Drop for ActiveRequest {
    fn drop(&mut self) {
        self.0.active.fetch_sub(1, Ordering::SeqCst);
    }
}
async fn observe(State(metadata): State<Arc<Metadata>>, request: Request) -> Response {
    let bytes = to_bytes(request.into_body(), 256 * 1024).await.unwrap();
    let mut input = bytes.as_ref();
    let request = read_metadata(&mut input, 256 * 1024).await.unwrap();
    assert!(
        input.is_empty(),
        "metadata request must not contain a document"
    );
    assert_eq!(
        request.header().operation_or_status,
        Operation::GetPrinterAttributes as i16,
        "the telemetry source must never receive job or scan operations"
    );
    let requested = string_list(attr(
        &request,
        DelimiterTag::OperationAttributes,
        "requested-attributes",
    ));
    *metadata.requested.write() = requested;
    metadata.requests.fetch_add(1, Ordering::SeqCst);
    let active = metadata.active.fetch_add(1, Ordering::SeqCst) + 1;
    metadata.max_active.fetch_max(active, Ordering::SeqCst);
    let _active = ActiveRequest(metadata.clone());
    let mode = metadata.mode.load(Ordering::SeqCst);
    let delay = metadata.delay_ms.load(Ordering::SeqCst);
    if delay != 0 {
        tokio::time::sleep(Duration::from_millis(delay)).await;
    }
    match mode {
        HTTP_ERROR => {
            return (
                HttpStatus::SERVICE_UNAVAILABLE,
                "https://secret-user:secret-password@example.test/secret?token=secret-token",
            )
                .into_response();
        }
        OVERSIZED => {
            return (
                [(header::CONTENT_TYPE, "application/ipp")],
                vec![0_u8; 256 * 1024 + 1],
            )
                .into_response();
        }
        CHUNKED_OVERSIZED => {
            let chunks =
                (0..5).map(|_| Ok::<_, std::io::Error>(Bytes::from(vec![0_u8; 64 * 1024])));
            return Response::builder()
                .header(header::CONTENT_TYPE, "application/ipp")
                .body(Body::from_stream(futures_util::stream::iter(chunks)))
                .unwrap();
        }
        MALFORMED => {
            return ([(header::CONTENT_TYPE, "application/ipp")], "not IPP").into_response();
        }
        REDIRECT => {
            return Response::builder()
                .status(HttpStatus::TEMPORARY_REDIRECT)
                .header(header::LOCATION, "/redirect-target")
                .body(Body::empty())
                .unwrap();
        }
        _ => {}
    }
    let mut response = IppRequestResponse::new_response(
        IppVersion::v2_0(),
        if mode == IPP_ERROR {
            StatusCode::ServerErrorServiceUnavailable
        } else {
            StatusCode::SuccessfulOk
        },
        request.header().request_id + i32::from(mode == WRONG_ID),
    )
    .unwrap();
    if mode == IPP_ERROR {
        add(
            &mut response,
            DelimiterTag::OperationAttributes,
            "status-message",
            text("https://secret-user:secret-password@example.test/?token=secret-token").unwrap(),
        )
        .unwrap();
    }
    for (key, value) in metadata.attributes.read().iter() {
        add(
            &mut response,
            DelimiterTag::PrinterAttributes,
            key,
            value.clone(),
        )
        .unwrap();
    }
    let mut bytes = response.to_bytes().to_vec();
    if mode == TRAILING_DATA {
        bytes.extend_from_slice(b"not a metadata document");
    }
    (
        [(
            header::CONTENT_TYPE,
            if mode == WRONG_MIME {
                "text/plain"
            } else {
                "application/ipp; charset=utf-8"
            },
        )],
        bytes,
    )
        .into_response()
}
fn string_set(values: &[&str], make: fn(&str) -> anyhow::Result<IppValue>) -> IppValue {
    IppValue::Array(values.iter().map(|value| make(value).unwrap()).collect())
}
fn integers(values: &[i32]) -> IppValue {
    IppValue::Array(values.iter().copied().map(IppValue::Integer).collect())
}
fn fixture() -> Vec<(&'static str, IppValue)> {
    vec![
        (
            "marker-names",
            string_set(&["Black ink", "Cyan ink", "Magenta ink", "Yellow ink"], name),
        ),
        ("marker-types", string_set(&["ink", "ink", "ink", "ink"], keyword)),
        (
            "marker-colors",
            string_set(&["#000000", "#00FFFF", "#FF00FF", "#FFFF00"], name),
        ),
        ("marker-levels", integers(&[26, 56, 55, 56])),
        ("marker-low-levels", integers(&[15, 15, 15, 15])),
        ("marker-high-levels", integers(&[100, 100, 100, 100])),
        ("marker-change-time", IppValue::Integer(1_793_100_671)),
        ("printer-state", IppValue::Enum(3)),
        ("printer-state-reasons", keyword("media-empty-report").unwrap()),
        ("printer-up-time", IppValue::Integer(128_000)),
        (
            "printer-supply",
            IppValue::OctetString(Bytes::from_static(
                b"type=inkCartridge;maxcapacity=100;level=26;unit=percent;colorantname=black;class=supplyThatIsConsumed;",
            )),
        ),
        (
            "printer-supply-description",
            IppValue::TextWithLanguage {
                language: "en".try_into().unwrap(),
                text: "Black cartridge".try_into().unwrap(),
            },
        ),
        (
            "printer-input-tray",
            IppValue::OctetString(Bytes::from_static(
                b"type=sheetFeedAutoRemovableTray;mediafeed=110000;mediaxfeed=85000;maxcapacity=500;level=100;status=8;name=Tray1;dimunit=tenThousandthsOfInches;unit=sheets;",
            )),
        ),
        (
            "printer-output-tray",
            IppValue::OctetString(Bytes::from_static(
                b"type=removableBin;maxcapacity=500;remaining=320;status=0;name=Output;unit=sheets;",
            )),
        ),
        ("media-ready", string_set(&["na_letter_8.5x11in", "na_legal_8.5x14in"], keyword)),
        (
            "media-col-ready",
            collection(vec![
                ("media-source", keyword("main").unwrap()),
                ("media-type", keyword("stationery").unwrap()),
                ("media-size-name", keyword("na_letter_8.5x11in").unwrap()),
                ("media-bottom-margin", IppValue::Integer(296)),
                (
                    "media-size",
                    collection(vec![
                        ("x-dimension", IppValue::Integer(21590)),
                        ("y-dimension", IppValue::Integer(27940)),
                    ])
                    .unwrap(),
                ),
            ])
            .unwrap(),
        ),
    ]
}
fn reply() -> IppRequestResponse {
    IppRequestResponse::new_response(IppVersion::v2_0(), StatusCode::SuccessfulOk, 1).unwrap()
}

#[tokio::test]
async fn decodes_actual_metadata_into_typed_source_reports() {
    let source = MetadataSource::start(fixture()).await;
    let telemetry = Telemetry::open(Some(&source.config()), Duration::from_secs(3)).unwrap();
    telemetry.refresh().await.unwrap();
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.freshness, Freshness::Fresh);
    assert_eq!(snapshot.freshness_scope, "metadata-fetch");
    assert_eq!(snapshot.physical_freshness, Freshness::Unknown);
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_secs();
    assert!(
        snapshot
            .observed_at
            .is_some_and(|time| time <= now && now - time < 5)
    );
    assert_eq!(
        snapshot.source.as_ref().unwrap().kind,
        TelemetrySourceKind::NativeCupsIpp
    );
    assert_eq!(snapshot.source.as_ref().unwrap().uri, source.uri());
    let measurements = &snapshot.measurements;
    assert_eq!(measurements.markers.len(), 4);
    for (marker, (expected_name, level)) in measurements.markers.iter().zip([
        ("Black ink", 26),
        ("Cyan ink", 56),
        ("Magenta ink", 55),
        ("Yellow ink", 56),
    ]) {
        assert_eq!(marker.name.as_deref(), Some(expected_name));
        assert_eq!(marker.marker_type.as_deref(), Some("ink"));
        assert_eq!(marker.level.as_ref().unwrap().value, Some(level));
        assert!(matches!(
            marker.level.as_ref().unwrap().unit,
            MeasurementUnit::Percent
        ));
        assert_eq!(marker.low_level.as_ref().unwrap().value, Some(15));
        assert_eq!(marker.high_level.as_ref().unwrap().value, Some(100));
    }
    assert_eq!(measurements.markers[1].color.as_deref(), Some("#00FFFF"));
    assert_eq!(measurements.marker_change_time, Some(1_793_100_671));
    assert_eq!(
        measurements.marker_change_time_clock,
        "source-defined-seconds"
    );
    let supply = &measurements.supplies[0];
    assert_eq!(supply.description.as_deref(), Some("Black cartridge"));
    assert_eq!(supply.level.as_ref().unwrap().value, Some(26));
    assert_eq!(supply.colorant_name.as_deref(), Some("black"));
    assert_eq!(supply.report.fields["class"], "supplyThatIsConsumed");
    let tray = &measurements.input_trays[0];
    assert_eq!(tray.level.as_ref().unwrap().value, Some(100));
    assert!(matches!(
        tray.level.as_ref().unwrap().unit,
        MeasurementUnit::Sheets
    ));
    assert!(matches!(
        &tray.media_feed.as_ref().unwrap().unit,
        MeasurementUnit::SourceReported(unit) if unit == "tenThousandthsOfInches"
    ));
    assert_eq!(
        measurements.output_trays[0]
            .remaining_capacity
            .as_ref()
            .unwrap()
            .value,
        Some(320)
    );
    assert!(measurements.output_trays[0].level.is_none());
    assert_eq!(measurements.media_ready.len(), 2);
    let ready = &measurements.media_col_ready[0];
    assert_eq!(ready.media_source.as_deref(), Some("main"));
    assert_eq!(ready.x_dimension.as_ref().unwrap().value, Some(21590));
    assert!(matches!(
        ready.y_dimension.as_ref().unwrap().unit,
        MeasurementUnit::HundredthsMillimeter
    ));
    assert!(matches!(
        ready.members.get("media-bottom-margin"),
        Some(ReportedValue::Integer(296))
    ));
    assert_eq!(measurements.printer_state_reasons, ["media-empty-report"]);
    let json = serde_json::to_value(&snapshot).unwrap();
    assert_eq!(json["measurements"]["markers"][0]["level"]["value"], 26);
    assert_eq!(json["source"]["kind"], "native-cups-ipp");
    assert_eq!(json["physical_freshness"], "unknown");
    let requested = source.metadata.requested.read();
    for key in [
        "marker-names",
        "marker-types",
        "marker-colors",
        "marker-levels",
        "marker-low-levels",
        "marker-high-levels",
        "marker-change-time",
        "printer-supply",
        "printer-supply-description",
        "printer-input-tray",
        "printer-output-tray",
        "media-ready",
        "media-col-ready",
    ] {
        assert!(
            requested.iter().any(|requested| requested == key),
            "missing {key}"
        );
    }
    drop(requested);
    let mut response = reply();
    add_ipp(&snapshot, &mut response).unwrap();
    assert!(attr(&response, DelimiterTag::PrinterAttributes, "marker-levels").is_none());
    assert!(attr(&response, DelimiterTag::PrinterAttributes, "media-ready").is_none());
    assert_eq!(
        attr(
            &response,
            DelimiterTag::PrinterAttributes,
            "ferriprint-telemetry-marker-levels"
        )
        .map(values)
        .unwrap()
        .iter()
        .filter_map(integer)
        .collect::<Vec<_>>(),
        [26, 56, 55, 56]
    );
    assert_eq!(
        attr(
            &response,
            DelimiterTag::PrinterAttributes,
            "ferriprint-telemetry-source-kind"
        )
        .and_then(str_value),
        Some("native-cups-ipp")
    );
}

#[tokio::test]
async fn errors_preserve_last_observation_as_stale_without_echoing_secrets() {
    let source = MetadataSource::start(fixture()).await;
    let telemetry = Telemetry::open(Some(&source.config()), Duration::from_secs(3)).unwrap();
    telemetry.refresh().await.unwrap();
    let observed_at = telemetry.snapshot().observed_at;
    for mode in [
        HTTP_ERROR,
        WRONG_ID,
        OVERSIZED,
        CHUNKED_OVERSIZED,
        WRONG_MIME,
        MALFORMED,
        REDIRECT,
        IPP_ERROR,
        TRAILING_DATA,
    ] {
        source.mode(mode);
        let error = telemetry.refresh().await.unwrap_err().to_string();
        assert!(!error.contains("secret-user"));
        assert!(!error.contains("secret-password"));
        assert!(!error.contains("secret-token"));
        let snapshot = telemetry.snapshot();
        assert_eq!(snapshot.freshness, Freshness::Stale);
        assert_eq!(snapshot.observed_at, observed_at);
        assert_eq!(
            snapshot.measurements.markers[0]
                .level
                .as_ref()
                .unwrap()
                .value,
            Some(26)
        );
        assert_eq!(snapshot.last_error.as_deref(), Some(error.as_str()));
        let json = serde_json::to_string(&snapshot).unwrap();
        assert!(!json.contains("secret-password"));
        let mut response = reply();
        add_ipp(&snapshot, &mut response).unwrap();
        assert!(attr(&response, DelimiterTag::PrinterAttributes, "marker-levels").is_none());
        assert_eq!(
            attr(
                &response,
                DelimiterTag::PrinterAttributes,
                "ferriprint-telemetry-freshness"
            )
            .and_then(str_value),
            Some("stale")
        );
    }
    source.mode(NORMAL);
    source.attributes(Vec::new());
    telemetry.refresh().await.unwrap();
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.freshness, Freshness::Fresh);
    assert!(snapshot.last_error.is_none());
    assert!(
        snapshot.measurements.markers.is_empty(),
        "new omission must not retain old readings"
    );
    assert_eq!(
        snapshot.measurements.availability["marker-levels"],
        Availability::Unknown
    );
}

#[tokio::test]
async fn unavailable_and_negative_values_are_not_zero_or_invented_percentages() {
    let source = MetadataSource::start(vec![
        ("marker-names", string_set(&["Black", "Cyan"], name)),
        ("marker-levels", integers(&[-1, -2, -3, 0])),
        (
            "printer-supply",
            IppValue::OctetString(Bytes::from_static(
                b"type=toner;level=14;colorantname=black;",
            )),
        ),
        (
            "printer-input-tray",
            IppValue::OctetString(Bytes::from_static(
                b"name=Main;unit=sheets;maxcapacity=-2;level=-3;status=8;",
            )),
        ),
        (
            "printer-output-tray",
            IppValue::OctetString(Bytes::from_static(b"name=Output;remaining=-3;unit=sheets;")),
        ),
        (
            "media-col-ready",
            IppValue::Other {
                tag: 0x10,
                data: Bytes::new(),
            },
        ),
        (
            "printer-state-reasons",
            keyword("media-empty-report").unwrap(),
        ),
    ])
    .await;
    let telemetry = Telemetry::open(Some(&source.config()), Duration::from_secs(3)).unwrap();
    telemetry.refresh().await.unwrap();
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.measurements.markers.len(), 4);
    for (index, sentinel) in [-1, -2, -3].into_iter().enumerate() {
        let level = snapshot.measurements.markers[index].level.as_ref().unwrap();
        assert_eq!(level.value, None);
        assert_eq!(level.raw, sentinel);
    }
    assert_eq!(
        snapshot.measurements.markers[3]
            .level
            .as_ref()
            .unwrap()
            .value,
        Some(0)
    );
    assert!(snapshot.measurements.markers[2].name.is_none());
    assert!(snapshot.measurements.markers[0].low_level.is_none());
    assert!(matches!(
        snapshot.measurements.supplies[0]
            .level
            .as_ref()
            .unwrap()
            .unit,
        MeasurementUnit::Unknown
    ));
    assert!(snapshot.measurements.supplies[0].max_capacity.is_none());
    assert_eq!(
        snapshot.measurements.input_trays[0]
            .level
            .as_ref()
            .unwrap()
            .raw,
        -3
    );
    assert!(
        snapshot.measurements.input_trays[0]
            .level
            .as_ref()
            .unwrap()
            .value
            .is_none()
    );
    assert!(
        snapshot.measurements.output_trays[0]
            .remaining_capacity
            .as_ref()
            .unwrap()
            .value
            .is_none()
    );
    assert!(
        snapshot.measurements.media_ready.is_empty(),
        "queue warning is not a tray sensor"
    );
    assert_eq!(
        snapshot.measurements.availability["media-col-ready"],
        Availability::Unsupported
    );
    assert_eq!(
        snapshot.measurements.availability["marker-high-levels"],
        Availability::Unknown
    );
    assert_eq!(
        snapshot.measurements.availability["marker-levels"],
        Availability::Reported
    );
    let json = serde_json::to_value(snapshot).unwrap();
    assert!(json["measurements"]["markers"][0]["level"]["value"].is_null());
    assert_eq!(json["measurements"]["markers"][0]["level"]["raw"], -1);
}

#[tokio::test]
async fn preserves_unknown_and_malformed_reports_without_fabricating_levels() {
    let source = MetadataSource::start(vec![
        ("marker-levels", IppValue::NoValue),
        (
            "marker-high-levels",
            IppValue::Other {
                tag: 0x12,
                data: Bytes::new(),
            },
        ),
        (
            "printer-supply",
            IppValue::Array(vec![
                IppValue::OctetString(Bytes::from_static(b"level=99;level=0;unit=percent;")),
                IppValue::OctetString(Bytes::from_static(&[0xff, 0xfe])),
            ]),
        ),
        (
            "printer-supply-description",
            string_set(
                &["Conflicting source", "Non-UTF-8 source", "Description only"],
                text,
            ),
        ),
    ])
    .await;
    let telemetry = Telemetry::open(Some(&source.config()), Duration::from_secs(3)).unwrap();
    telemetry.refresh().await.unwrap();
    let snapshot = telemetry.snapshot();
    assert!(snapshot.measurements.markers[0].level.is_none());
    assert_eq!(
        snapshot.measurements.availability["marker-levels"],
        Availability::Unknown
    );
    assert_eq!(
        snapshot.measurements.availability["marker-high-levels"],
        Availability::Unknown
    );
    let supplies = &snapshot.measurements.supplies;
    assert_eq!(supplies.len(), 3);
    assert!(supplies.iter().all(|supply| supply.level.is_none()));
    assert!(supplies[0].report.decoding_error.is_some());
    assert_eq!(
        supplies[0].report.raw.as_deref(),
        Some("level=99;level=0;unit=percent;")
    );
    assert_eq!(
        supplies[1].report.raw_octets.as_deref(),
        Some([0xff, 0xfe].as_slice())
    );
    assert_eq!(supplies[2].description.as_deref(), Some("Description only"));
}

#[tokio::test]
async fn direct_current_readings_are_standard_but_stale_readings_are_namespaced() {
    let source = MetadataSource::start(fixture()).await;
    let mut config = source.config();
    config.source = TelemetrySourceKind::DeviceIpp;
    let telemetry = Telemetry::open(Some(&config), Duration::from_secs(3)).unwrap();
    telemetry.refresh().await.unwrap();
    let mut response = reply();
    add_ipp(&telemetry.snapshot(), &mut response).unwrap();
    assert!(attr(&response, DelimiterTag::PrinterAttributes, "marker-levels").is_some());
    assert!(
        attr(
            &response,
            DelimiterTag::PrinterAttributes,
            "printer-input-tray"
        )
        .is_some()
    );
    assert!(
        attr(
            &response,
            DelimiterTag::PrinterAttributes,
            "media-col-ready"
        )
        .is_some()
    );
    assert!(attr(&response, DelimiterTag::PrinterAttributes, "printer-state").is_none());
    source.mode(HTTP_ERROR);
    telemetry.refresh().await.unwrap_err();
    let mut response = reply();
    add_ipp(&telemetry.snapshot(), &mut response).unwrap();
    for key in [
        "marker-levels",
        "printer-input-tray",
        "media-ready",
        "media-col-ready",
    ] {
        assert!(attr(&response, DelimiterTag::PrinterAttributes, key).is_none());
        assert!(
            attr(
                &response,
                DelimiterTag::PrinterAttributes,
                &format!("ferriprint-telemetry-{key}")
            )
            .is_some()
        );
    }
}

#[tokio::test]
async fn concurrent_refreshes_share_one_observation_and_later_readings_change() {
    let source = MetadataSource::start(fixture()).await;
    source.delay(100);
    let telemetry = Telemetry::open(Some(&source.config()), Duration::from_secs(3)).unwrap();
    let (a, b, c, d) = tokio::join!(
        telemetry.refresh(),
        telemetry.refresh(),
        telemetry.refresh(),
        telemetry.refresh()
    );
    a.unwrap();
    b.unwrap();
    c.unwrap();
    d.unwrap();
    assert_eq!(source.metadata.requests.load(Ordering::SeqCst), 1);
    assert_eq!(source.metadata.max_active.load(Ordering::SeqCst), 1);
    source.attributes(vec![("marker-levels", integers(&[25]))]);
    telemetry.refresh().await.unwrap();
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.freshness, Freshness::Fresh);
    assert_eq!(
        snapshot.measurements.markers[0]
            .level
            .as_ref()
            .unwrap()
            .value,
        Some(25)
    );
}

#[tokio::test]
async fn an_operation_timeout_keeps_the_last_observation_stale_and_reports_failure() {
    let source = MetadataSource::start(fixture()).await;
    let telemetry = Telemetry::open(Some(&source.config()), Duration::from_secs(1)).unwrap();
    telemetry.refresh().await.unwrap();
    source.delay(2500);
    let started = std::time::Instant::now();
    assert!(telemetry.refresh().await.is_err());
    assert!(started.elapsed() < Duration::from_secs(2));
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.freshness, Freshness::Stale);
    assert!(snapshot.last_error.is_some());
    assert_eq!(
        snapshot.measurements.markers[0]
            .level
            .as_ref()
            .unwrap()
            .value,
        Some(26)
    );
}

#[test]
fn disabled_telemetry_is_explicitly_unknown_without_a_runtime() {
    let telemetry = Telemetry::open(None, Duration::ZERO).unwrap();
    let snapshot = telemetry.snapshot();
    assert!(snapshot.source.is_none());
    assert!(snapshot.observed_at.is_none());
    assert_eq!(snapshot.freshness, Freshness::Unknown);
    assert!(snapshot.measurements.markers.is_empty());
    assert!(snapshot.last_error.is_none());
    let mut response = reply();
    add_ipp(&snapshot, &mut response).unwrap();
    assert_eq!(
        attr(
            &response,
            DelimiterTag::PrinterAttributes,
            "ferriprint-telemetry-freshness"
        )
        .and_then(str_value),
        Some("unknown")
    );
    assert!(
        attr(
            &response,
            DelimiterTag::PrinterAttributes,
            "ferriprint-telemetry-observed-at"
        )
        .is_none()
    );
}

#[test]
fn validates_transport_security_and_polling_clock_constraints() {
    let config: TelemetryConfig =
        toml::from_str("uri = 'ipps://printer.example.test./ipp/print'").unwrap();
    config.validate().unwrap();
    for uri in [
        "ipp://192.0.2.1/ipp/print",
        "ipp://localhost/ipp/print",
        "https://printer.example.test/ipp/print",
        "ipps://secret-user:secret-password@printer.example.test/ipp/print",
        "ipps://printer.example.test/ipp/print?token=secret-token",
        "ipps://printer.example.test/ipp/print#secret-token",
        "ipps://printer.example.test:0/ipp/print",
    ] {
        let invalid: TelemetryConfig = toml::from_str(&format!("uri = {uri:?}")).unwrap();
        let error = invalid.validate().unwrap_err().to_string();
        assert!(!error.contains("secret-password"));
        assert!(!error.contains("secret-token"));
    }
    for uri in ["ipp://127.0.0.1:631/metadata", "ipp://[::1]:631/metadata"] {
        let valid: TelemetryConfig = toml::from_str(&format!("uri = {uri:?}")).unwrap();
        valid.validate().unwrap();
    }
    let mut invalid = config;
    invalid.poll_interval_secs = 4;
    assert!(invalid.validate().is_err());
    invalid.poll_interval_secs = 30;
    invalid.stale_after_secs = 29;
    assert!(invalid.validate().is_err());
    assert!(
        toml::from_str::<TelemetryConfig>(
            "uri = 'ipps://printer.example.test/ipp/print'\ninsecure = true"
        )
        .is_err()
    );
    let mut plaintext: TelemetryConfig =
        toml::from_str("uri = 'ipp://127.0.0.1/metadata'").unwrap();
    plaintext.ca_certificate = Some("ca.pem".into());
    assert!(plaintext.validate().is_err());
}

#[tokio::test]
async fn configured_ca_is_additive_and_tls_verification_cannot_be_disabled() {
    let (source, certificate) = MetadataSource::tls(fixture(), "127.0.0.1").await;
    let config = source.config();
    let untrusted = Telemetry::open(Some(&config), Duration::from_secs(3)).unwrap();
    untrusted.refresh().await.unwrap_err();
    assert_eq!(untrusted.snapshot().freshness, Freshness::Unknown);
    assert!(untrusted.snapshot().observed_at.is_none());
    assert_eq!(source.metadata.requests.load(Ordering::SeqCst), 0);
    drop(untrusted);
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ephemeral-ca.pem");
    tokio::fs::write(&path, certificate).await.unwrap();
    let mut trusted = config;
    trusted.ca_certificate = Some(path);
    let telemetry = Telemetry::open(Some(&trusted), Duration::from_secs(3)).unwrap();
    telemetry.refresh().await.unwrap();
    assert_eq!(telemetry.snapshot().freshness, Freshness::Fresh);
    assert_eq!(
        telemetry.snapshot().measurements.markers[0]
            .level
            .as_ref()
            .unwrap()
            .value,
        Some(26)
    );
}

#[tokio::test]
async fn an_explicit_ca_does_not_bypass_hostname_verification() {
    let (source, certificate) = MetadataSource::tls(fixture(), "wrong.example.test").await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("ephemeral-ca.pem");
    tokio::fs::write(&path, certificate).await.unwrap();
    let mut config = source.config();
    config.ca_certificate = Some(path);
    let telemetry = Telemetry::open(Some(&config), Duration::from_secs(3)).unwrap();
    let error = telemetry.refresh().await.unwrap_err().to_string();
    assert!(error.contains("TLS verification"));
    assert!(telemetry.snapshot().measurements.markers.is_empty());
    assert_eq!(source.metadata.requests.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn invalid_ca_files_fail_without_disclosing_contents_or_contacting_the_source() {
    let source = MetadataSource::start(fixture()).await;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("secret-file-name.pem");
    tokio::fs::write(&path, "secret-password not a certificate")
        .await
        .unwrap();
    let mut config = source.config();
    config.uri = config
        .uri
        .as_str()
        .replacen("ipp:", "ipps:", 1)
        .parse()
        .unwrap();
    config.ca_certificate = Some(path);
    let telemetry = Telemetry::open(Some(&config), Duration::from_secs(3)).unwrap();
    let error = telemetry.refresh().await.unwrap_err().to_string();
    assert!(error.contains("CA certificate"));
    assert!(!error.contains("secret-file-name"));
    assert!(!error.contains("secret-password"));
    assert_eq!(source.metadata.requests.load(Ordering::SeqCst), 0);
    assert_eq!(telemetry.snapshot().freshness, Freshness::Unknown);
    assert!(telemetry.snapshot().last_error.is_some());
}

#[tokio::test]
async fn cache_age_expires_even_before_a_pending_refresh_reports_an_error() {
    let source = MetadataSource::start(fixture()).await;
    let mut config = source.config();
    config.poll_interval_secs = 5;
    config.stale_after_secs = 5;
    let telemetry = Telemetry::open(Some(&config), Duration::from_secs(30)).unwrap();
    telemetry.refresh().await.unwrap();
    let observed_at = telemetry.snapshot().observed_at;
    // The next background fetch stays pending, so no successful refresh can
    // conceal expiration and no failed refresh is needed to mark the cache stale.
    source.delay(60_000);
    tokio::time::sleep(Duration::from_millis(5150)).await;
    let snapshot = telemetry.snapshot();
    assert_eq!(snapshot.observed_at, observed_at);
    assert!(snapshot.last_error.is_none());
    assert_eq!(snapshot.freshness, Freshness::Stale);
    assert_eq!(
        snapshot.measurements.markers[0]
            .level
            .as_ref()
            .unwrap()
            .value,
        Some(26)
    );
    let mut response = reply();
    add_ipp(&snapshot, &mut response).unwrap();
    assert!(attr(&response, DelimiterTag::PrinterAttributes, "marker-levels").is_none());
}
