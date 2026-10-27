// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

//! Read-only IPP observations. Cache freshness describes the metadata fetch, not
//! the age of a device's sensor sample or the contents of a CUPS queue's cache.

use crate::wire::*;
use anyhow::{Result, anyhow, ensure};
use futures_util::StreamExt;
use ipp::prelude::*;
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::Read,
    path::{Path, PathBuf},
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{Mutex, OnceCell, watch};
use url::Url;

const RESPONSE_LIMIT: usize = 256 * 1024;
const CA_LIMIT: u64 = 1024 * 1024;
const REQUESTED_ATTRIBUTES: &[&str] = &[
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
    "printer-state",
    "printer-state-reasons",
    "printer-up-time",
    "printer-current-time",
];

#[derive(Clone, Copy, Debug, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum TelemetrySourceKind {
    /// Native queue metadata can include cached device readings and declarations.
    #[default]
    NativeCupsIpp,
    /// Only select this for an explicitly configured, trusted physical endpoint.
    DeviceIpp,
}
impl TelemetrySourceKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::NativeCupsIpp => "native-cups-ipp",
            Self::DeviceIpp => "device-ipp",
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct TelemetryConfig {
    pub uri: Url,
    #[serde(default)]
    pub ca_certificate: Option<PathBuf>,
    #[serde(default)]
    pub source: TelemetrySourceKind,
    #[serde(default = "default_poll_interval")]
    pub poll_interval_secs: u64,
    #[serde(default = "default_stale_after")]
    pub stale_after_secs: u64,
}
fn default_poll_interval() -> u64 {
    30
}
fn default_stale_after() -> u64 {
    120
}
impl TelemetryConfig {
    pub fn validate(&self) -> Result<()> {
        ensure!(
            matches!(self.uri.scheme(), "ipp" | "ipps") && self.uri.host().is_some(),
            "telemetry requires an ipp or ipps URL with a host"
        );
        ensure!(
            self.uri.username().is_empty()
                && self.uri.password().is_none()
                && self.uri.query().is_none()
                && self.uri.fragment().is_none(),
            "telemetry URL must not contain credentials, a query, or a fragment"
        );
        ensure!(
            self.uri.scheme() == "ipps" || crate::config::is_loopback_url(&self.uri),
            "plaintext telemetry is restricted to a literal loopback address"
        );
        ensure!(
            self.uri.port() != Some(0),
            "telemetry URL must not use port zero"
        );
        ensure!(
            self.ca_certificate.is_none() || self.uri.scheme() == "ipps",
            "a telemetry CA certificate requires IPPS"
        );
        ensure!(
            self.poll_interval_secs >= 5,
            "telemetry poll_interval_secs must be at least 5"
        );
        ensure!(
            self.stale_after_secs >= self.poll_interval_secs,
            "telemetry stale_after_secs must be at least poll_interval_secs"
        );
        Ok(())
    }

    pub fn resolve_paths(&mut self, base: &Path) {
        if let Some(path) = &mut self.ca_certificate
            && path.is_relative()
        {
            *path = base.join(&*path);
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Freshness {
    Unknown,
    Fresh,
    Stale,
}
impl Freshness {
    fn as_str(self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Fresh => "fresh",
            Self::Stale => "stale",
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TelemetrySource {
    pub kind: TelemetrySourceKind,
    pub uri: String,
}

#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "kebab-case", tag = "kind", content = "name")]
pub enum MeasurementUnit {
    Percent,
    Sheets,
    HundredthsMillimeter,
    SourceReported(String),
    Unknown,
}
impl MeasurementUnit {
    fn reported(unit: Option<&String>) -> Self {
        match unit.map(String::as_str) {
            Some("percent") => Self::Percent,
            Some("sheets") => Self::Sheets,
            Some("hundredthsOfMillimeters") => Self::HundredthsMillimeter,
            Some(unit) => Self::SourceReported(unit.to_owned()),
            None => Self::Unknown,
        }
    }
}

/// `value` is absent for negative sentinels (and invalid percentages); `raw`
/// always preserves the source integer. Missing attributes are `None`, not zero.
#[derive(Clone, Debug, Serialize)]
pub struct Quantity {
    pub value: Option<i32>,
    pub raw: i32,
    pub unit: MeasurementUnit,
}
impl Quantity {
    fn new(raw: i32, unit: MeasurementUnit) -> Self {
        let known = raw >= 0 && (!matches!(unit, MeasurementUnit::Percent) || raw <= 100);
        Self {
            value: known.then_some(raw),
            raw,
            unit,
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct Marker {
    /// Position in the corresponding IPP 1setOf values, starting at one.
    pub index: usize,
    pub name: Option<String>,
    pub marker_type: Option<String>,
    pub color: Option<String>,
    pub level: Option<Quantity>,
    pub low_level: Option<Quantity>,
    pub high_level: Option<Quantity>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Supply {
    pub index: usize,
    pub description: Option<String>,
    pub supply_type: Option<String>,
    pub colorant_name: Option<String>,
    pub level: Option<Quantity>,
    pub max_capacity: Option<Quantity>,
    pub report: KeyValueReport,
}

#[derive(Clone, Debug, Serialize)]
pub struct Tray {
    pub index: usize,
    pub name: Option<String>,
    pub tray_type: Option<String>,
    pub level: Option<Quantity>,
    pub max_capacity: Option<Quantity>,
    /// Output remaining capacity is not the number of sheets currently present.
    pub remaining_capacity: Option<Quantity>,
    pub media_feed: Option<Quantity>,
    pub media_cross_feed: Option<Quantity>,
    pub status: Option<i32>,
    pub report: KeyValueReport,
}

/// PWG 5100.13 semicolon-delimited octet strings. Preserve undecodable reports
/// without interpreting them as empty supplies or empty trays.
#[derive(Clone, Debug, Default, Serialize)]
pub struct KeyValueReport {
    pub raw: Option<String>,
    pub raw_octets: Option<Vec<u8>>,
    pub fields: BTreeMap<String, String>,
    pub decoding_error: Option<String>,
}

/// The typed IPP syntax and all media collection members remain available even
/// when a vendor uses members unknown to Ferriprint.
#[derive(Clone, Debug, Serialize)]
#[serde(tag = "syntax", content = "value", rename_all = "kebab-case")]
pub enum ReportedValue {
    Integer(i32),
    Enum(i32),
    Boolean(bool),
    Keyword(String),
    Name(String),
    Text(String),
    Uri(String),
    OctetString(Vec<u8>),
    Collection(BTreeMap<String, ReportedValue>),
    Array(Vec<ReportedValue>),
    Range { min: i32, max: i32 },
    Other { tag: u8, octets: Vec<u8> },
    Unknown,
    Unsupported,
    NoValue,
}

#[derive(Clone, Debug, Serialize)]
pub struct ReadyMedia {
    pub media_size_name: Option<String>,
    pub media_source: Option<String>,
    pub media_type: Option<String>,
    pub x_dimension: Option<Quantity>,
    pub y_dimension: Option<Quantity>,
    pub members: BTreeMap<String, ReportedValue>,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Availability {
    Reported,
    Unknown,
    Unsupported,
}

#[derive(Clone, Debug, Serialize)]
pub struct Measurements {
    pub markers: Vec<Marker>,
    /// Source-defined seconds; deliberately NOT interpreted as a UNIX timestamp.
    pub marker_change_time: Option<i32>,
    pub marker_change_time_clock: &'static str,
    pub supplies: Vec<Supply>,
    pub input_trays: Vec<Tray>,
    pub output_trays: Vec<Tray>,
    /// Reports from the configured source, not proof of physically loaded paper.
    pub media_ready: Vec<String>,
    pub media_col_ready: Vec<ReadyMedia>,
    /// Queue/device advisories, never converted into synthetic sensor readings.
    pub printer_state: Option<i32>,
    pub printer_state_reasons: Vec<String>,
    pub printer_up_time: Option<i32>,
    pub printer_current_time: Option<String>,
    /// Omission means unknown; only an explicit unsupported value means unsupported.
    pub availability: BTreeMap<String, Availability>,
}
impl Default for Measurements {
    fn default() -> Self {
        Self {
            markers: Vec::new(),
            marker_change_time: None,
            marker_change_time_clock: "source-defined-seconds",
            supplies: Vec::new(),
            input_trays: Vec::new(),
            output_trays: Vec::new(),
            media_ready: Vec::new(),
            media_col_ready: Vec::new(),
            printer_state: None,
            printer_state_reasons: Vec::new(),
            printer_up_time: None,
            printer_current_time: None,
            availability: REQUESTED_ATTRIBUTES
                .iter()
                .map(|key| ((*key).to_owned(), Availability::Unknown))
                .collect(),
        }
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct TelemetrySnapshot {
    pub source: Option<TelemetrySource>,
    /// Successful metadata retrieval time, in UNIX seconds, not sensor sample time.
    pub observed_at: Option<u64>,
    pub freshness: Freshness,
    pub freshness_scope: &'static str,
    /// Neither a successful CUPS fetch nor direct IPP proves sensor sample age.
    pub physical_freshness: Freshness,
    pub measurements: Arc<Measurements>,
    pub last_error: Option<String>,
    #[serde(skip)]
    attributes: Arc<Vec<IppAttribute>>,
}

struct Cache {
    snapshot: TelemetrySnapshot,
    successful_at: Option<Instant>,
}

struct Source {
    config: TelemetryConfig,
    http_uri: Url,
    timeout: Duration,
    client: OnceCell<reqwest::Client>,
}

pub struct Telemetry {
    source: Option<Arc<Source>>,
    cache: RwLock<Cache>,
    refresh_lock: Mutex<()>,
    completed_refreshes: AtomicU64,
    stop: watch::Sender<bool>,
}
impl Telemetry {
    pub fn open(config: Option<&TelemetryConfig>, timeout: Duration) -> Result<Arc<Self>> {
        if let Some(config) = config {
            config.validate()?;
            ensure!(!timeout.is_zero(), "telemetry timeout must be positive");
        }
        let source = config
            .map(|config| {
                Ok::<_, anyhow::Error>(Arc::new(Source {
                    config: config.clone(),
                    http_uri: http_url(&config.uri)
                        .map_err(|_| anyhow!("invalid telemetry transport URL"))?,
                    timeout,
                    client: OnceCell::new(),
                }))
            })
            .transpose()?;
        let runtime = source
            .as_ref()
            .map(|_| {
                tokio::runtime::Handle::try_current()
                    .map_err(|_| anyhow!("configured telemetry requires a Tokio runtime"))
            })
            .transpose()?;
        let (stop, mut stopped) = watch::channel(false);
        let telemetry = Arc::new(Self {
            cache: RwLock::new(Cache {
                snapshot: TelemetrySnapshot {
                    source: config.map(|config| TelemetrySource {
                        kind: config.source,
                        uri: config.uri.to_string(),
                    }),
                    observed_at: None,
                    freshness: Freshness::Unknown,
                    freshness_scope: "metadata-fetch",
                    physical_freshness: Freshness::Unknown,
                    measurements: Arc::new(Measurements::default()),
                    last_error: None,
                    attributes: Arc::new(Vec::new()),
                },
                successful_at: None,
            }),
            source,
            refresh_lock: Mutex::new(()),
            completed_refreshes: AtomicU64::new(0),
            stop,
        });
        if let (Some(config), Some(runtime)) = (config, runtime) {
            let weak = Arc::downgrade(&telemetry);
            let interval = Duration::from_secs(config.poll_interval_secs);
            runtime.spawn(async move {
                loop {
                    let Some(telemetry) = weak.upgrade() else {
                        return;
                    };
                    // The cache owns the sanitized outcome; a failure is not a
                    // reason to manufacture readings or terminate polling.
                    let _ = telemetry.refresh().await;
                    drop(telemetry);
                    tokio::select! {
                        _ = tokio::time::sleep(interval) => {},
                        _ = stopped.changed() => return,
                    }
                }
            });
        }
        Ok(telemetry)
    }

    /// Concurrent callers share the in-flight refresh and its outcome. A later
    /// explicit call still performs a new observation, independently of polling.
    pub async fn refresh(&self) -> Result<()> {
        let Some(source) = &self.source else {
            return Ok(());
        };
        let generation = self.completed_refreshes.load(Ordering::Acquire);
        let _guard = self.refresh_lock.lock().await;
        if self.completed_refreshes.load(Ordering::Acquire) != generation {
            return self
                .cache
                .read()
                .snapshot
                .last_error
                .as_ref()
                .map_or(Ok(()), |error| Err(anyhow!(error.clone())));
        }
        let result = tokio::time::timeout(source.timeout, source.fetch())
            .await
            .map_err(|_| anyhow!("telemetry refresh timed out"))
            .and_then(|result| result);
        let outcome = match result {
            Ok((measurements, attributes)) => {
                let mut cache = self.cache.write();
                cache.snapshot.measurements = Arc::new(measurements);
                cache.snapshot.attributes = Arc::new(attributes);
                cache.snapshot.observed_at = SystemTime::now()
                    .duration_since(UNIX_EPOCH)
                    .ok()
                    .map(|time| time.as_secs());
                cache.snapshot.last_error = None;
                cache.successful_at = Some(Instant::now());
                Ok(())
            }
            Err(error) => {
                self.cache.write().snapshot.last_error = Some(error.to_string());
                Err(error)
            }
        };
        self.completed_refreshes.fetch_add(1, Ordering::Release);
        outcome
    }

    pub fn snapshot(&self) -> TelemetrySnapshot {
        let cache = self.cache.read();
        let mut snapshot = cache.snapshot.clone();
        snapshot.freshness = match (cache.successful_at, &self.source) {
            (Some(observed), Some(source))
                if snapshot.last_error.is_none()
                    && observed.elapsed() < Duration::from_secs(source.config.stale_after_secs) =>
            {
                Freshness::Fresh
            }
            (Some(_), _) => Freshness::Stale,
            _ => Freshness::Unknown,
        };
        snapshot
    }
}
impl Drop for Telemetry {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
    }
}

impl Source {
    async fn client(&self) -> Result<&reqwest::Client> {
        self.client
            .get_or_try_init(|| async {
                let config = self.config.clone();
                let timeout = self.timeout;
                // Loading native roots and an operator CA involves filesystem
                // work, which must never block an async executor thread.
                tokio::task::spawn_blocking(move || build_client(&config, timeout))
                    .await
                    .map_err(|_| anyhow!("telemetry TLS client initialization failed"))?
            })
            .await
    }

    async fn fetch(&self) -> Result<(Measurements, Vec<IppAttribute>)> {
        let client = self.client().await?;
        let mut request = IppRequestResponse::new(
            IppVersion::v2_0(),
            Operation::GetPrinterAttributes,
            Some(
                self.config
                    .uri
                    .as_str()
                    .parse()
                    .map_err(|_| anyhow!("invalid telemetry IPP URI"))?,
            ),
        )
        .map_err(|_| anyhow!("could not construct telemetry metadata request"))?;
        let requested = IppValue::Array(
            REQUESTED_ATTRIBUTES
                .iter()
                .map(|key| keyword(key))
                .collect::<Result<_>>()?,
        );
        add(
            &mut request,
            DelimiterTag::OperationAttributes,
            "requested-attributes",
            requested,
        )?;
        let request_id = request.header().request_id;
        let response = client
            .post(self.http_uri.clone())
            .header("content-type", "application/ipp")
            .body(request.to_bytes())
            .send()
            .await
            .map_err(|error| {
                if error.is_timeout() {
                    anyhow!("telemetry request timed out")
                } else {
                    anyhow!("telemetry transport failed (including TLS verification)")
                }
            })?;
        ensure!(
            response.status().is_success(),
            "telemetry source returned HTTP status {}",
            response.status().as_u16()
        );
        ensure!(
            response
                .headers()
                .get("content-type")
                .and_then(|value| value.to_str().ok())
                .and_then(|value| value.split(';').next())
                .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/ipp")),
            "telemetry source did not return application/ipp"
        );
        ensure!(
            response
                .content_length()
                .is_none_or(|length| length <= RESPONSE_LIMIT as u64),
            "telemetry response exceeds 256 KiB"
        );
        let mut body = Vec::with_capacity(
            response
                .content_length()
                .unwrap_or(8192)
                .min(RESPONSE_LIMIT as u64) as usize,
        );
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(|error| {
                if error.is_timeout() {
                    anyhow!("telemetry response transfer timed out")
                } else {
                    anyhow!("telemetry response transfer failed")
                }
            })?;
            ensure!(
                chunk.len() <= RESPONSE_LIMIT - body.len(),
                "telemetry response exceeds 256 KiB"
            );
            body.extend_from_slice(&chunk);
        }
        let mut input = body.as_slice();
        let response = read_metadata(&mut input, RESPONSE_LIMIT)
            .await
            .map_err(|_| anyhow!("telemetry source returned invalid IPP metadata"))?;
        ensure!(
            input.is_empty(),
            "telemetry metadata contains trailing data"
        );
        ensure!(
            response.header().request_id == request_id,
            "telemetry source returned a mismatched request ID"
        );
        ensure!(
            response.header().status_code().is_success(),
            "telemetry source returned IPP status {}",
            response.header().operation_or_status
        );
        decode(&response)
    }
}

fn build_client(config: &TelemetryConfig, timeout: Duration) -> Result<reqwest::Client> {
    let mut builder = reqwest::Client::builder()
        .use_rustls_tls()
        .tls_built_in_root_certs(true)
        .tls_built_in_native_certs(true)
        .timeout(timeout)
        .connect_timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy();
    if let Some(path) = &config.ca_certificate {
        let file = std::fs::File::open(path)
            .map_err(|_| anyhow!("telemetry CA certificate could not be read"))?;
        ensure!(
            file.metadata().is_ok_and(|metadata| metadata.is_file()),
            "telemetry CA certificate must be a regular file"
        );
        let mut pem = Vec::new();
        file.take(CA_LIMIT + 1)
            .read_to_end(&mut pem)
            .map_err(|_| anyhow!("telemetry CA certificate could not be read"))?;
        ensure!(
            pem.len() as u64 <= CA_LIMIT,
            "telemetry CA certificate exceeds 1 MiB"
        );
        let certificates = reqwest::Certificate::from_pem_bundle(&pem).map_err(|_| {
            anyhow!("telemetry CA certificate is not a valid PEM certificate bundle")
        })?;
        ensure!(
            !certificates.is_empty(),
            "telemetry CA certificate bundle contains no certificates"
        );
        for certificate in certificates {
            builder = builder.add_root_certificate(certificate);
        }
    }
    builder
        .build()
        .map_err(|_| anyhow!("telemetry verified TLS client could not be initialized"))
}

fn at<'a>(response: &'a IppRequestResponse, key: &str, index: usize) -> Option<&'a IppValue> {
    attr(response, DelimiterTag::PrinterAttributes, key).and_then(|value| values(value).get(index))
}
fn string(value: &IppValue) -> Option<&str> {
    match value {
        IppValue::TextWithLanguage { text, .. } => Some(text),
        value => str_value(value),
    }
}
fn string_at(response: &IppRequestResponse, key: &str, index: usize) -> Option<String> {
    at(response, key, index).and_then(string).map(str::to_owned)
}
fn count(response: &IppRequestResponse, key: &str) -> usize {
    attr(response, DelimiterTag::PrinterAttributes, key)
        .map(values)
        .map_or(0, <[IppValue]>::len)
}
fn percentage_at(response: &IppRequestResponse, key: &str, index: usize) -> Option<Quantity> {
    at(response, key, index)
        .and_then(integer)
        .map(|raw| Quantity::new(raw, MeasurementUnit::Percent))
}
fn availability(value: Option<&IppValue>) -> Availability {
    match value {
        Some(IppValue::Other { tag: 0x10, .. }) => Availability::Unsupported,
        None | Some(IppValue::NoValue | IppValue::Other { tag: 0x12, .. }) => Availability::Unknown,
        _ => Availability::Reported,
    }
}

fn decode(response: &IppRequestResponse) -> Result<(Measurements, Vec<IppAttribute>)> {
    let group = DelimiterTag::PrinterAttributes;
    let mut measurements = Measurements::default();
    let unsupported = attr(
        response,
        DelimiterTag::UnsupportedAttributes,
        "requested-attributes",
    )
    .map(values)
    .unwrap_or(&[]);
    for (key, status) in &mut measurements.availability {
        let reported = attr(response, group, key);
        *status = if reported.is_some() {
            availability(reported)
        } else if attr(response, DelimiterTag::UnsupportedAttributes, key).is_some()
            || unsupported
                .iter()
                .any(|value| string(value) == Some(key.as_str()))
        {
            Availability::Unsupported
        } else {
            Availability::Unknown
        };
    }
    let marker_count = [
        "marker-names",
        "marker-types",
        "marker-colors",
        "marker-levels",
        "marker-low-levels",
        "marker-high-levels",
    ]
    .iter()
    .map(|key| count(response, key))
    .max()
    .unwrap_or(0);
    for index in 0..marker_count {
        measurements.markers.push(Marker {
            index: index + 1,
            name: string_at(response, "marker-names", index),
            marker_type: string_at(response, "marker-types", index),
            color: string_at(response, "marker-colors", index),
            level: percentage_at(response, "marker-levels", index),
            low_level: percentage_at(response, "marker-low-levels", index),
            high_level: percentage_at(response, "marker-high-levels", index),
        });
    }
    for index in
        0..count(response, "printer-supply").max(count(response, "printer-supply-description"))
    {
        let report = key_value_report(at(response, "printer-supply", index));
        let unit = MeasurementUnit::reported(report.fields.get("unit"));
        measurements.supplies.push(Supply {
            index: index + 1,
            description: string_at(response, "printer-supply-description", index),
            supply_type: report.fields.get("type").cloned(),
            colorant_name: report.fields.get("colorantname").cloned(),
            level: report_quantity(&report, "level", unit.clone()),
            max_capacity: report_quantity(&report, "maxcapacity", unit),
            report,
        });
    }
    measurements.input_trays = trays(response, "printer-input-tray");
    measurements.output_trays = trays(response, "printer-output-tray");
    measurements.marker_change_time = attr(response, group, "marker-change-time").and_then(integer);
    measurements.media_ready = string_list(attr(response, group, "media-ready"));
    if let Some(value) = attr(response, group, "media-col-ready") {
        for value in values(value) {
            if let IppValue::Collection(members) = value {
                let size = members.get("media-size").and_then(|value| match value {
                    IppValue::Collection(size) => Some(size),
                    _ => None,
                });
                measurements.media_col_ready.push(ReadyMedia {
                    media_size_name: members
                        .get("media-size-name")
                        .and_then(string)
                        .map(str::to_owned),
                    media_source: members
                        .get("media-source")
                        .and_then(string)
                        .map(str::to_owned),
                    media_type: members
                        .get("media-type")
                        .and_then(string)
                        .map(str::to_owned),
                    x_dimension: size
                        .and_then(|size| size.get("x-dimension"))
                        .and_then(integer)
                        .map(|raw| Quantity::new(raw, MeasurementUnit::HundredthsMillimeter)),
                    y_dimension: size
                        .and_then(|size| size.get("y-dimension"))
                        .and_then(integer)
                        .map(|raw| Quantity::new(raw, MeasurementUnit::HundredthsMillimeter)),
                    members: members
                        .iter()
                        .map(|(key, value)| (key.to_string(), reported_value(value)))
                        .collect(),
                });
            }
        }
    }
    measurements.printer_state = attr(response, group, "printer-state").and_then(integer);
    measurements.printer_state_reasons =
        string_list(attr(response, group, "printer-state-reasons"));
    measurements.printer_up_time = attr(response, group, "printer-up-time").and_then(integer);
    measurements.printer_current_time =
        attr(response, group, "printer-current-time").and_then(|value| match value {
            IppValue::DateTime(time) => Some(time.to_string()),
            _ => None,
        });
    let mut attributes = Vec::new();
    for printer in response
        .attributes()
        .groups()
        .iter()
        .filter(|g| g.tag() == group)
    {
        for attribute in printer.attributes() {
            if REQUESTED_ATTRIBUTES.contains(&attribute.name().as_str()) {
                ensure!(
                    !attributes
                        .iter()
                        .any(|prior: &IppAttribute| prior.name() == attribute.name()),
                    "telemetry source returned duplicate metadata attributes"
                );
                attributes.push(attribute.clone());
            }
        }
    }
    Ok((measurements, attributes))
}

fn key_value_report(value: Option<&IppValue>) -> KeyValueReport {
    let Some(value) = value else {
        return KeyValueReport::default();
    };
    let raw = match value {
        IppValue::OctetString(bytes) => match std::str::from_utf8(bytes) {
            Ok(raw) => raw,
            Err(_) => {
                return KeyValueReport {
                    raw_octets: Some(bytes.to_vec()),
                    decoding_error: Some("source report is not UTF-8".to_owned()),
                    ..KeyValueReport::default()
                };
            }
        },
        value => match string(value) {
            Some(raw) => raw,
            None => {
                return KeyValueReport {
                    decoding_error: Some("source report has no textual value".to_owned()),
                    ..KeyValueReport::default()
                };
            }
        },
    };
    let mut report = KeyValueReport {
        raw: Some(raw.to_owned()),
        ..KeyValueReport::default()
    };
    for field in raw.split(';').filter(|field| !field.is_empty()) {
        let Some((key, value)) = field.split_once('=') else {
            report.decoding_error = Some("source report has malformed key/value fields".to_owned());
            break;
        };
        if key.is_empty()
            || key
                .bytes()
                .any(|byte| !byte.is_ascii_alphanumeric() && byte != b'-')
            || value.chars().any(char::is_control)
            || report
                .fields
                .insert(key.to_owned(), value.to_owned())
                .is_some()
        {
            report.decoding_error =
                Some("source report has invalid or duplicate fields".to_owned());
            break;
        }
    }
    if report.decoding_error.is_some() {
        report.fields.clear();
    }
    report
}
fn report_quantity(report: &KeyValueReport, key: &str, unit: MeasurementUnit) -> Option<Quantity> {
    report
        .fields
        .get(key)
        .and_then(|value| value.parse().ok())
        .map(|raw| Quantity::new(raw, unit))
}
fn trays(response: &IppRequestResponse, key: &str) -> Vec<Tray> {
    (0..count(response, key))
        .map(|index| {
            let report = key_value_report(at(response, key, index));
            let unit = MeasurementUnit::reported(report.fields.get("unit"));
            let dimension_unit = MeasurementUnit::reported(report.fields.get("dimunit"));
            Tray {
                index: index + 1,
                name: report.fields.get("name").cloned(),
                tray_type: report.fields.get("type").cloned(),
                level: report_quantity(&report, "level", unit.clone()),
                max_capacity: report_quantity(&report, "maxcapacity", unit.clone()),
                remaining_capacity: report_quantity(&report, "remaining", unit),
                media_feed: report_quantity(&report, "mediafeed", dimension_unit.clone()),
                media_cross_feed: report_quantity(&report, "mediaxfeed", dimension_unit),
                status: report
                    .fields
                    .get("status")
                    .and_then(|value| value.parse().ok()),
                report,
            }
        })
        .collect()
}
fn reported_value(value: &IppValue) -> ReportedValue {
    match value {
        IppValue::Integer(value) => ReportedValue::Integer(*value),
        IppValue::Enum(value) => ReportedValue::Enum(*value),
        IppValue::Boolean(value) => ReportedValue::Boolean(*value),
        IppValue::Keyword(value) => ReportedValue::Keyword(value.to_string()),
        IppValue::NameWithoutLanguage(value) | IppValue::NameWithLanguage { name: value, .. } => {
            ReportedValue::Name(value.to_string())
        }
        IppValue::TextWithoutLanguage(value) | IppValue::TextWithLanguage { text: value, .. } => {
            ReportedValue::Text(value.to_string())
        }
        IppValue::Uri(value) => ReportedValue::Uri(value.to_string()),
        IppValue::OctetString(value) => ReportedValue::OctetString(value.to_vec()),
        IppValue::Array(values) => {
            ReportedValue::Array(values.iter().map(reported_value).collect())
        }
        IppValue::Collection(members) => ReportedValue::Collection(
            members
                .iter()
                .map(|(key, value)| (key.to_string(), reported_value(value)))
                .collect(),
        ),
        IppValue::RangeOfInteger { min, max } => ReportedValue::Range {
            min: *min,
            max: *max,
        },
        IppValue::Other { tag: 0x10, .. } => ReportedValue::Unsupported,
        IppValue::Other { tag: 0x12, .. } => ReportedValue::Unknown,
        IppValue::NoValue => ReportedValue::NoValue,
        value => {
            let encoded = value.to_bytes();
            ReportedValue::Other {
                tag: value.to_tag(),
                // The primitive encoder prefixes its value with a two-byte length.
                octets: encoded[2..].to_vec(),
            }
        }
    }
}

/// Queue reports and stale values use namespaced attributes exclusively. Fresh
/// direct-device readings may additionally use their original standard names.
/// Keeping each original typed value avoids JSON/text truncation and preserves
/// IPP's per-value wire limits, including large media collection arrays.
pub fn add_ipp(snapshot: &TelemetrySnapshot, reply: &mut IppRequestResponse) -> Result<()> {
    let group = DelimiterTag::PrinterAttributes;
    for (key, value) in [
        (
            "ferriprint-telemetry-freshness",
            snapshot.freshness.as_str(),
        ),
        (
            "ferriprint-telemetry-freshness-scope",
            snapshot.freshness_scope,
        ),
        (
            "ferriprint-telemetry-physical-freshness",
            snapshot.physical_freshness.as_str(),
        ),
    ] {
        add(reply, group, key, keyword(value)?)?;
    }
    if let Some(source) = &snapshot.source {
        add(
            reply,
            group,
            "ferriprint-telemetry-source",
            uri(&source.uri)?,
        )?;
        add(
            reply,
            group,
            "ferriprint-telemetry-source-kind",
            keyword(source.kind.as_str())?,
        )?;
    }
    if let Some(observed_at) = snapshot.observed_at {
        add(
            reply,
            group,
            "ferriprint-telemetry-observed-at",
            text(&observed_at.to_string())?,
        )?;
    }
    if let Some(error) = &snapshot.last_error {
        add(
            reply,
            group,
            "ferriprint-telemetry-last-error",
            text(bounded(error, 1023))?,
        )?;
    }
    if snapshot.measurements.marker_change_time.is_some() {
        add(
            reply,
            group,
            "ferriprint-telemetry-marker-change-time-clock",
            keyword(snapshot.measurements.marker_change_time_clock)?,
        )?;
    }
    let current_device = snapshot.freshness == Freshness::Fresh
        && snapshot
            .source
            .as_ref()
            .is_some_and(|source| source.kind == TelemetrySourceKind::DeviceIpp);
    for attribute in snapshot.attributes.iter() {
        let key = attribute.name().as_str();
        let custom = format!("ferriprint-telemetry-{key}");
        add(reply, group, &custom, attribute.value().clone())?;
        if current_device
            && !matches!(
                key,
                "printer-state"
                    | "printer-state-reasons"
                    | "printer-up-time"
                    | "printer-current-time"
            )
            && attr(reply, group, key).is_none()
        {
            add(reply, group, key, attribute.value().clone())?;
        }
    }
    Ok(())
}
