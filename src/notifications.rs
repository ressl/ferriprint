// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

//! Persisted, privacy-minimal operational transitions and an at-least-once webhook outbox.
//! The protected startup configuration is the only authority for transport settings.

use crate::{
    auth::read_private_file,
    config::validate_printer_name,
    jobs::{JobEngine, JobRecord},
};
use anyhow::{Context, Result, anyhow, ensure};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderValue};
use rusqlite::{Connection, OptionalExtension, params};
use serde::{Deserialize, Serialize};
use std::{
    fmt,
    net::IpAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use zeroize::Zeroizing;

const SECRET_LIMIT: usize = 4096;
const DELIVERIES_PER_TICK: usize = 16;

/// A fixed operator-controlled destination, never supplied by an admin request.
#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationConfig {
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub webhook_url: String,
    #[serde(default)]
    pub bearer_secret_file: Option<PathBuf>,
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
    #[serde(default = "default_attempts")]
    pub max_attempts: u32,
    #[serde(default = "default_initial_backoff")]
    pub backoff_initial_secs: u64,
    #[serde(default = "default_max_backoff")]
    pub backoff_max_secs: u64,
}

fn default_enabled() -> bool {
    true
}
fn default_timeout() -> u64 {
    10
}
fn default_attempts() -> u32 {
    5
}
fn default_initial_backoff() -> u64 {
    5
}
fn default_max_backoff() -> u64 {
    3600
}

impl Default for NotificationConfig {
    fn default() -> Self {
        Self {
            enabled: default_enabled(),
            webhook_url: String::new(),
            bearer_secret_file: None,
            timeout_secs: default_timeout(),
            max_attempts: default_attempts(),
            backoff_initial_secs: default_initial_backoff(),
            backoff_max_secs: default_max_backoff(),
        }
    }
}

impl fmt::Debug for NotificationConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NotificationConfig")
            .field("enabled", &self.enabled)
            .field("webhook_url", &"[redacted]")
            .field("bearer_secret_file", &self.bearer_secret_file.is_some())
            .field("timeout_secs", &self.timeout_secs)
            .field("max_attempts", &self.max_attempts)
            .field("backoff_initial_secs", &self.backoff_initial_secs)
            .field("backoff_max_secs", &self.backoff_max_secs)
            .finish()
    }
}

impl NotificationConfig {
    pub fn resolve_paths(&mut self, base: &Path) {
        if let Some(path) = &mut self.bearer_secret_file
            && path.is_relative()
        {
            *path = base.join(&*path);
        }
    }

    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=120).contains(&self.timeout_secs),
            "notification timeout_secs must be between 1 and 120"
        );
        ensure!(
            (1..=20).contains(&self.max_attempts),
            "notification max_attempts must be between 1 and 20"
        );
        ensure!(
            (1..=3600).contains(&self.backoff_initial_secs),
            "notification backoff_initial_secs must be between 1 and 3600"
        );
        ensure!(
            self.backoff_max_secs >= self.backoff_initial_secs && self.backoff_max_secs <= 86400,
            "notification backoff_max_secs must be at least backoff_initial_secs and at most 86400"
        );
        if self.enabled || !self.webhook_url.is_empty() {
            self.destination()?;
        }
        Ok(())
    }

    fn destination(&self) -> Result<url::Url> {
        let value = &self.webhook_url;
        ensure!(
            !value.is_empty()
                && value.len() <= 4096
                && !value.bytes().any(|b| b <= b' ' || b == 0x7f || b == b'\\'),
            "notification webhook URL is invalid"
        );
        let url =
            url::Url::parse(value).map_err(|_| anyhow!("notification webhook URL is invalid"))?;
        let (_, rest) = value
            .split_once("://")
            .context("notification webhook URL requires an explicit scheme and authority")?;
        let authority = rest.split(['/', '?', '#']).next().unwrap_or_default();
        ensure!(
            !authority.is_empty()
                && !authority.contains('@')
                && url.username().is_empty()
                && url.password().is_none()
                && url.query().is_none()
                && url.fragment().is_none()
                && url.host().is_some()
                && url.port() != Some(0),
            "notification webhook URL must have a host and no credentials, query or fragment"
        );
        match url.scheme() {
            "https" => {}
            "http" => {
                // Validate the original spelling too: DNS names and WHATWG's numeric
                // host aliases are not literal loopback addresses.
                let host = if let Some(bracketed) = authority.strip_prefix('[') {
                    bracketed.split_once(']').map(|(host, _)| host)
                } else {
                    authority.split(':').next()
                };
                ensure!(
                    host.and_then(|host| host.parse::<IpAddr>().ok())
                        .is_some_and(|ip| ip.is_loopback()),
                    "notification HTTP is allowed only for a literal loopback address"
                );
            }
            _ => anyhow::bail!("notification webhook URL must use HTTPS or literal-loopback HTTP"),
        }
        Ok(url)
    }
}

/// Safe administration metadata. This deliberately has no destination, payload,
/// credential, remote message, response body, owner, document or title field.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Delivery {
    pub id: String,
    pub printer: String,
    pub event_type: String,
    pub condition: String,
    pub reason_code: String,
    pub job_id: Option<i32>,
    pub source: String,
    pub timestamp: i64,
    pub problem: Option<bool>,
    pub status: String,
    /// Attempts in the current bounded delivery cycle.
    pub attempts: u32,
    /// Attempts across all explicit retries, retained as evidence.
    pub total_attempts: u64,
    pub retry_count: u32,
    pub last_attempt_at: Option<i64>,
    pub next_attempt_at: Option<i64>,
    pub delivered_at: Option<i64>,
    pub exhausted_at: Option<i64>,
    pub failure_class: Option<String>,
    pub http_status: Option<u16>,
}

#[derive(Serialize)]
struct Payload<'a> {
    event_id: &'a str,
    #[serde(rename = "type")]
    event_type: &'a str,
    printer: &'a str,
    condition: &'a str,
    reason_code: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    job_id: Option<i32>,
    source: &'a str,
    timestamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    problem: Option<bool>,
}

struct Transport {
    client: reqwest::Client,
    destination: url::Url,
    timeout_secs: u64,
    max_attempts: u32,
    backoff_initial_secs: u64,
    backoff_max_secs: u64,
}

pub struct Notifications {
    engine: Arc<JobEngine>,
    transport: Option<Transport>,
    delivery_lock: tokio::sync::Mutex<()>,
}

/// Initialize before JobEngine's startup recovery saves, so a disabled startup
/// never inherits the previous process's enabled transport gate.
pub(crate) fn initialize_database(db: &Connection, configured: bool) -> Result<()> {
    transaction(db, |db| {
        db.execute_batch(
            "CREATE TABLE IF NOT EXISTS notification_runtime (
                id INTEGER PRIMARY KEY CHECK(id=1),
                enabled INTEGER NOT NULL CHECK(enabled IN (0,1))
            );
            CREATE TABLE IF NOT EXISTS notification_printers (
                printer TEXT PRIMARY KEY,
                enabled INTEGER NOT NULL CHECK(enabled IN (0,1)),
                updated_at INTEGER NOT NULL
            );
            CREATE TABLE IF NOT EXISTS notification_conditions (
                printer TEXT NOT NULL,
                condition_key TEXT NOT NULL,
                condition TEXT NOT NULL,
                source TEXT NOT NULL,
                job_id INTEGER,
                problem INTEGER,
                last_observation INTEGER,
                observed_at INTEGER NOT NULL,
                PRIMARY KEY(printer,condition_key)
            );
            CREATE TABLE IF NOT EXISTS notification_outbox (
                id TEXT PRIMARY KEY,
                printer TEXT NOT NULL,
                event_type TEXT NOT NULL,
                condition TEXT NOT NULL,
                reason_code TEXT NOT NULL,
                job_id INTEGER,
                source TEXT NOT NULL,
                timestamp INTEGER NOT NULL,
                problem INTEGER,
                payload TEXT NOT NULL,
                status TEXT NOT NULL CHECK(status IN ('pending','sending','delivered','exhausted')),
                attempts INTEGER NOT NULL DEFAULT 0,
                total_attempts INTEGER NOT NULL DEFAULT 0,
                retry_count INTEGER NOT NULL DEFAULT 0,
                last_attempt_at INTEGER,
                next_attempt_at INTEGER,
                delivered_at INTEGER,
                exhausted_at INTEGER,
                failure_class TEXT,
                http_status INTEGER
            );
            CREATE INDEX IF NOT EXISTS notification_outbox_due
                ON notification_outbox(status,next_attempt_at);",
        )?;
        let previous = db
            .query_row(
                "SELECT enabled FROM notification_runtime WHERE id=1",
                [],
                |row| row.get::<_, bool>(0),
            )
            .optional()?;
        if configured && previous != Some(true) {
            let mut statement =
                db.prepare("SELECT printer FROM notification_printers WHERE enabled=1")?;
            let printers = statement.query_map([], |row| row.get::<_, String>(0))?;
            for printer in printers {
                baseline_jobs(db, &printer?)?;
            }
        }
        db.execute(
            "INSERT INTO notification_runtime(id,enabled) VALUES(1,?1)
             ON CONFLICT(id) DO UPDATE SET enabled=excluded.enabled",
            [configured],
        )?;
        Ok(())
    })
}

impl Notifications {
    pub fn open(config: Option<&NotificationConfig>, engine: Arc<JobEngine>) -> Result<Arc<Self>> {
        if let Some(config) = config {
            config.validate()?;
        }
        let transport = config
            .filter(|config| config.enabled)
            .map(build_transport)
            .transpose()?;
        initialize_database(&engine.database(), transport.is_some())?;
        Ok(Arc::new(Self {
            engine,
            transport,
            delivery_lock: tokio::sync::Mutex::new(()),
        }))
    }

    pub fn configured(&self) -> bool {
        self.transport.is_some()
    }

    /// The persisted printer preference; `configured()` separately reports whether
    /// the protected startup configuration currently permits delivery.
    pub fn is_enabled(&self, printer: &str) -> Result<bool> {
        validate_printer_name(printer)?;
        Ok(self
            .engine
            .database()
            .query_row(
                "SELECT enabled FROM notification_printers WHERE printer=?1",
                [printer],
                |row| row.get(0),
            )
            .optional()?
            .unwrap_or(false))
    }

    pub fn set_enabled(&self, printer: &str, enabled: bool) -> Result<()> {
        validate_printer_name(printer)?;
        ensure!(
            !enabled || self.configured(),
            "notification transport is disabled in protected startup configuration"
        );
        let db = self.engine.database();
        transaction(&db, |db| {
            let previous = db
                .query_row(
                    "SELECT enabled FROM notification_printers WHERE printer=?1",
                    [printer],
                    |row| row.get::<_, bool>(0),
                )
                .optional()?
                .unwrap_or(false);
            if enabled && !previous {
                // This is a snapshot, not an observation: old failures must not
                // become retroactive alerts on first enable or re-enable.
                baseline_jobs(db, printer)?;
            }
            db.execute(
                "INSERT INTO notification_printers(printer,enabled,updated_at) VALUES(?1,?2,?3)
                 ON CONFLICT(printer) DO UPDATE SET enabled=excluded.enabled,updated_at=excluded.updated_at",
                params![printer, enabled, now()],
            )?;
            Ok(())
        })
    }

    /// `None` is unknown/stale, not healthy. In particular it cannot recover a
    /// previously observed paper shortage. Only enumerated codes enter payloads.
    pub fn observe(
        &self,
        printer: &str,
        condition: &str,
        problem: Option<bool>,
        source: &str,
    ) -> Result<()> {
        validate_printer_name(printer)?;
        validate_observation(condition, source)?;
        let db = self.engine.database();
        if !enabled_in_database(&db, printer)? {
            return Ok(());
        }
        let key = format!("{source}:{condition}");
        transaction(&db, |db| {
            store_observation(
                db,
                &Observation {
                    printer,
                    key: &key,
                    condition,
                    problem,
                    source,
                    job_id: None,
                    reason_code: if condition == "paper-shortage" {
                        "media-empty"
                    } else {
                        condition
                    },
                    timestamp: now(),
                },
            )
        })
    }

    /// Queue an actual delivery even when this printer's automatic events are off.
    /// Returning an ID means queued, never that the receiver has acknowledged it.
    pub fn diagnostic(&self, printer: &str) -> Result<String> {
        validate_printer_name(printer)?;
        ensure!(
            self.configured(),
            "notification transport is disabled in protected startup configuration"
        );
        let db = self.engine.database();
        transaction(&db, |db| {
            insert_event(
                db,
                &Observation {
                    printer,
                    key: "diagnostic",
                    condition: "diagnostic",
                    problem: None,
                    source: "diagnostic",
                    job_id: None,
                    reason_code: "diagnostic-requested",
                    timestamp: now(),
                },
                "diagnostic",
            )
        })
    }

    /// Deliver at most 16 due entries. No database guard or transaction survives
    /// an await. A persisted sending lease also recovers canceled/crashed attempts.
    pub async fn tick(&self) -> Result<()> {
        let Some(transport) = &self.transport else {
            return Ok(());
        };
        let _delivery_guard = self.delivery_lock.lock().await;
        for _ in 0..DELIVERIES_PER_TICK {
            let claimed = {
                let db = self.engine.database();
                claim_due(&db, transport)?
            };
            let Some(claimed) = claimed else {
                break;
            };
            let Claim::Event(mut event) = claimed else {
                continue;
            };
            let response = transport
                .client
                .post(transport.destination.clone())
                .header("content-type", "application/json")
                .header("accept", "application/json")
                .header("idempotency-key", event.id.as_str())
                .body(std::mem::take(&mut event.payload))
                .send()
                .await;
            let outcome = match response {
                Ok(response) => {
                    let status = response.status();
                    // Deliberately retain/read zero response-body bytes. Neither
                    // successful nor error bodies have any role in delivery state.
                    drop(response);
                    if status.is_success() {
                        Outcome::Delivered(status.as_u16())
                    } else {
                        Outcome::Failed {
                            class: if status.is_redirection() {
                                "http-3xx"
                            } else if status.is_client_error() {
                                "http-4xx"
                            } else if status.is_server_error() {
                                "http-5xx"
                            } else {
                                "http-non-success"
                            },
                            status: Some(status.as_u16()),
                        }
                    }
                }
                Err(error) => Outcome::Failed {
                    class: if error.is_timeout() {
                        "timeout"
                    } else {
                        "transport"
                    },
                    status: None,
                },
            };
            finish_attempt(&self.engine.database(), &event, transport, outcome)?;
        }
        Ok(())
    }

    pub fn deliveries(&self) -> Result<Vec<Delivery>> {
        let db = self.engine.database();
        let mut statement = db.prepare(
            "SELECT id,printer,event_type,condition,reason_code,job_id,source,timestamp,problem,
                    status,attempts,total_attempts,retry_count,last_attempt_at,next_attempt_at,
                    delivered_at,exhausted_at,failure_class,http_status
             FROM notification_outbox ORDER BY timestamp DESC,rowid DESC",
        )?;
        Ok(statement
            .query_map([], |row| {
                Ok(Delivery {
                    id: row.get(0)?,
                    printer: row.get(1)?,
                    event_type: row.get(2)?,
                    condition: row.get(3)?,
                    reason_code: row.get(4)?,
                    job_id: row.get(5)?,
                    source: row.get(6)?,
                    timestamp: row.get(7)?,
                    problem: row.get(8)?,
                    status: row.get(9)?,
                    attempts: row.get(10)?,
                    total_attempts: row.get(11)?,
                    retry_count: row.get(12)?,
                    last_attempt_at: row.get(13)?,
                    next_attempt_at: row.get(14)?,
                    delivered_at: row.get(15)?,
                    exhausted_at: row.get(16)?,
                    failure_class: row.get(17)?,
                    http_status: row.get(18)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?)
    }

    /// Restart a bounded attempt cycle without changing event ID or payload.
    /// Acknowledged and currently in-flight deliveries cannot be retried.
    pub fn retry(&self, id: &str) -> Result<()> {
        ensure!(
            self.configured(),
            "notification transport is disabled in protected startup configuration"
        );
        let changed = self.engine.database().execute(
            "UPDATE notification_outbox
             SET status='pending',attempts=0,retry_count=retry_count+1,
                 next_attempt_at=?2,exhausted_at=NULL
             WHERE id=?1 AND status IN ('pending','exhausted') AND total_attempts>0",
            params![id, now()],
        )?;
        ensure!(
            changed == 1,
            "notification delivery is not retryable (missing, unattempted, delivered or in flight)"
        );
        Ok(())
    }
}

fn build_transport(config: &NotificationConfig) -> Result<Transport> {
    let destination = config.destination()?;
    let mut headers = HeaderMap::new();
    if let Some(path) = &config.bearer_secret_file {
        let bytes = Zeroizing::new(
            read_private_file(path, SECRET_LIMIT)
                .map_err(|_| anyhow!("notification bearer secret could not be read securely"))?,
        );
        let token = bytes.strip_suffix(b"\n").unwrap_or(&bytes);
        let token = token.strip_suffix(b"\r").unwrap_or(token);
        let unpadded = token.iter().position(|b| *b == b'=').unwrap_or(token.len());
        ensure!(
            unpadded > 0
                && token[..unpadded].iter().all(|b| {
                    b.is_ascii_alphanumeric()
                        || matches!(*b, b'-' | b'.' | b'_' | b'~' | b'+' | b'/')
                })
                && token[unpadded..].iter().all(|b| *b == b'='),
            "notification bearer secret must be a single nonempty bearer token"
        );
        let mut authorization = Zeroizing::new(Vec::with_capacity(7 + token.len()));
        authorization.extend_from_slice(b"Bearer ");
        authorization.extend_from_slice(token);
        let mut value = HeaderValue::from_bytes(&authorization)
            .map_err(|_| anyhow!("notification bearer secret is not a valid header"))?;
        value.set_sensitive(true);
        headers.insert(AUTHORIZATION, value);
    }
    let timeout = Duration::from_secs(config.timeout_secs);
    let client = reqwest::Client::builder()
        .use_rustls_tls()
        .tls_built_in_root_certs(true)
        .tls_built_in_native_certs(true)
        .timeout(timeout)
        .connect_timeout(timeout)
        .redirect(reqwest::redirect::Policy::none())
        .referer(false)
        .no_proxy()
        .default_headers(headers)
        .build()
        .map_err(|_| anyhow!("notification verified TLS client could not be initialized"))?;
    Ok(Transport {
        client,
        destination,
        timeout_secs: config.timeout_secs,
        max_attempts: config.max_attempts,
        backoff_initial_secs: config.backoff_initial_secs,
        backoff_max_secs: config.backoff_max_secs,
    })
}

fn validate_observation(condition: &str, source: &str) -> Result<()> {
    ensure!(
        matches!(
            (condition, source),
            ("native-queue-unreachable", "native-queue")
                | ("telemetry-source-unreachable", "telemetry")
                | ("paper-shortage", "native-queue" | "telemetry")
        ),
        "notification observation requires an enumerated condition and source kind"
    );
    Ok(())
}

fn enabled_in_database(db: &Connection, printer: &str) -> Result<bool> {
    Ok(db.query_row(
        "SELECT COALESCE((SELECT enabled FROM notification_runtime WHERE id=1),0)
                AND COALESCE((SELECT enabled FROM notification_printers WHERE printer=?1),0)",
        [printer],
        |row| row.get(0),
    )?)
}

struct Observation<'a> {
    printer: &'a str,
    key: &'a str,
    condition: &'a str,
    problem: Option<bool>,
    source: &'a str,
    job_id: Option<i32>,
    reason_code: &'a str,
    timestamp: i64,
}

fn store_observation(db: &Connection, observation: &Observation<'_>) -> Result<()> {
    let previous = db
        .query_row(
            "SELECT problem FROM notification_conditions WHERE printer=?1 AND condition_key=?2",
            params![observation.printer, observation.key],
            |row| row.get::<_, Option<bool>>(0),
        )
        .optional()?
        .flatten();
    db.execute(
        "INSERT INTO notification_conditions
            (printer,condition_key,condition,source,job_id,problem,last_observation,observed_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8)
         ON CONFLICT(printer,condition_key) DO UPDATE SET
            problem=excluded.problem,last_observation=excluded.last_observation,
            observed_at=excluded.observed_at",
        params![
            observation.printer,
            observation.key,
            observation.condition,
            observation.source,
            observation.job_id,
            observation.problem.or(previous),
            observation.problem,
            observation.timestamp,
        ],
    )?;
    match observation.problem {
        Some(true) if previous != Some(true) => {
            insert_event(db, observation, "problem")?;
        }
        Some(false) if previous == Some(true) => {
            insert_event(db, observation, "recovery")?;
        }
        _ => {}
    }
    Ok(())
}

fn insert_event(
    db: &Connection,
    observation: &Observation<'_>,
    event_type: &str,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().to_string();
    let reason = if event_type == "recovery" {
        "observed-healthy"
    } else {
        observation.reason_code
    };
    let payload = serde_json::to_string(&Payload {
        event_id: &id,
        event_type,
        printer: observation.printer,
        condition: observation.condition,
        reason_code: reason,
        job_id: observation.job_id,
        source: observation.source,
        timestamp: observation.timestamp,
        problem: observation.problem,
    })?;
    db.execute(
        "INSERT INTO notification_outbox
            (id,printer,event_type,condition,reason_code,job_id,source,timestamp,problem,payload,status,next_attempt_at)
         VALUES(?1,?2,?3,?4,?5,?6,?7,?8,?9,?10,'pending',?8)",
        params![
            id,
            observation.printer,
            event_type,
            observation.condition,
            reason,
            observation.job_id,
            observation.source,
            observation.timestamp,
            observation.problem,
            payload,
        ],
    )?;
    Ok(id)
}

/// Called by jobs::save after its record update, under the existing database lock.
/// A savepoint makes transition state and outbox insertion atomic both inside and
/// outside the caller's job transaction. Never include arbitrary job metadata.
pub(crate) fn record_job_transition(db: &Connection, job: &JobRecord) -> Result<()> {
    if !enabled_in_database(db, &job.printer)? {
        return Ok(());
    }
    transaction(db, |db| {
        let healthy = matches!(job.state, 3..=5 | 9);
        for (condition, problem, reason_code) in [
            (
                "job-aborted",
                if job.state == 8 {
                    Some(true)
                } else if healthy {
                    Some(false)
                } else {
                    None
                },
                "job-aborted",
            ),
            (
                "job-stopped-uncertain",
                if job.state == 6 && (!job.cancel_requested || job.phase == "uncertain") {
                    Some(true)
                } else if healthy {
                    Some(false)
                } else {
                    None
                },
                if job.phase == "uncertain" {
                    "submission-outcome-unknown"
                } else {
                    "job-stopped"
                },
            ),
        ] {
            let key = format!("job:{}:{condition}", job.id);
            store_observation(
                db,
                &Observation {
                    printer: &job.printer,
                    key: &key,
                    condition,
                    problem,
                    source: "local-job",
                    job_id: Some(job.id),
                    reason_code,
                    timestamp: now(),
                },
            )?;
        }
        Ok(())
    })
}

fn baseline_jobs(db: &Connection, printer: &str) -> Result<()> {
    let mut statement = db.prepare("SELECT id,state FROM jobs WHERE printer=?1")?;
    let jobs = statement.query_map([printer], |row| {
        Ok((row.get::<_, i32>(0)?, row.get::<_, i32>(1)?))
    })?;
    let timestamp = now();
    for job in jobs {
        let (id, state) = job?;
        for (condition, problem_state) in [("job-aborted", 8), ("job-stopped-uncertain", 6)] {
            let problem = if state == problem_state {
                Some(true)
            } else if matches!(state, 3..=5 | 9) {
                Some(false)
            } else {
                None
            };
            db.execute(
                "INSERT INTO notification_conditions
                    (printer,condition_key,condition,source,job_id,problem,last_observation,observed_at)
                 VALUES(?1,?2,?3,'local-job',?4,?5,?5,?6)
                 ON CONFLICT(printer,condition_key) DO UPDATE SET
                    problem=excluded.problem,last_observation=excluded.last_observation,
                    observed_at=excluded.observed_at",
                params![printer, format!("job:{id}:{condition}"), condition, id, problem, timestamp],
            )?;
        }
    }
    Ok(())
}

struct Outbound {
    id: String,
    payload: String,
    attempts: u32,
    total_attempts: u64,
}
enum Claim {
    Event(Outbound),
    Exhausted,
}

fn claim_due(db: &Connection, transport: &Transport) -> Result<Option<Claim>> {
    transaction(db, |db| {
        let timestamp = now();
        let due = db
            .query_row(
                "SELECT id,payload,attempts,total_attempts,status FROM notification_outbox
                 WHERE status IN ('pending','sending') AND next_attempt_at<=?1
                 ORDER BY next_attempt_at,rowid LIMIT 1",
                [timestamp],
                |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, u32>(2)?,
                        row.get::<_, u64>(3)?,
                        row.get::<_, String>(4)?,
                    ))
                },
            )
            .optional()?;
        let Some((id, payload, attempts, total_attempts, status)) = due else {
            return Ok(None);
        };
        if attempts >= transport.max_attempts {
            db.execute(
                "UPDATE notification_outbox SET status='exhausted',exhausted_at=?2,next_attempt_at=NULL,
                    failure_class=CASE WHEN status='sending' THEN 'delivery-interrupted' ELSE failure_class END,
                    http_status=CASE WHEN status='sending' THEN NULL ELSE http_status END
                 WHERE id=?1",
                params![id, timestamp],
            )?;
            return Ok(Some(Claim::Exhausted));
        }
        db.execute(
            "UPDATE notification_outbox SET status='sending',attempts=attempts+1,
                total_attempts=total_attempts+1,last_attempt_at=?2,next_attempt_at=?3,
                failure_class=CASE WHEN status='sending' THEN 'delivery-interrupted' ELSE failure_class END,
                http_status=CASE WHEN status='sending' THEN NULL ELSE http_status END
             WHERE id=?1",
            params![id, timestamp, timestamp.saturating_add(transport.timeout_secs as i64 + 1)],
        )?;
        debug_assert!(matches!(status.as_str(), "pending" | "sending"));
        Ok(Some(Claim::Event(Outbound {
            id,
            payload,
            attempts: attempts + 1,
            total_attempts: total_attempts + 1,
        })))
    })
}

enum Outcome {
    Delivered(u16),
    Failed {
        class: &'static str,
        status: Option<u16>,
    },
}

fn finish_attempt(
    db: &Connection,
    event: &Outbound,
    transport: &Transport,
    outcome: Outcome,
) -> Result<()> {
    let timestamp = now();
    let changed = match outcome {
        Outcome::Delivered(status) => db.execute(
            "UPDATE notification_outbox SET status='delivered',delivered_at=?2,
                next_attempt_at=NULL,exhausted_at=NULL,failure_class=NULL,http_status=?3
             WHERE id=?1 AND status='sending' AND total_attempts=?4",
            params![event.id, timestamp, status, event.total_attempts],
        )?,
        Outcome::Failed { class, status } => {
            let exhausted = event.attempts >= transport.max_attempts;
            let backoff = transport
                .backoff_initial_secs
                .saturating_mul(1u64 << (event.attempts - 1))
                .min(transport.backoff_max_secs);
            db.execute(
                "UPDATE notification_outbox SET status=?2,next_attempt_at=?3,exhausted_at=?4,
                    failure_class=?5,http_status=?6
                 WHERE id=?1 AND status='sending' AND total_attempts=?7",
                params![
                    event.id,
                    if exhausted { "exhausted" } else { "pending" },
                    if exhausted {
                        None
                    } else {
                        Some(timestamp.saturating_add(backoff as i64))
                    },
                    if exhausted { Some(timestamp) } else { None },
                    class,
                    status,
                    event.total_attempts,
                ],
            )?
        }
    };
    ensure!(
        changed == 1,
        "notification delivery attempt lost its persisted claim"
    );
    Ok(())
}

// rusqlite's &mut savepoint API cannot be used from jobs::save's &Connection.
// A fixed-name RAII savepoint also nests correctly in create_job's transaction.
struct NotificationSavepoint<'a> {
    db: &'a Connection,
    committed: bool,
}
impl Drop for NotificationSavepoint<'_> {
    fn drop(&mut self) {
        if !self.committed {
            let _ = self.db.execute_batch(
                "ROLLBACK TO SAVEPOINT ferriprint_notification; RELEASE SAVEPOINT ferriprint_notification",
            );
        }
    }
}
fn transaction<T>(db: &Connection, operation: impl FnOnce(&Connection) -> Result<T>) -> Result<T> {
    db.execute_batch("SAVEPOINT ferriprint_notification")?;
    let mut savepoint = NotificationSavepoint {
        db,
        committed: false,
    };
    let result = operation(db)?;
    db.execute_batch("RELEASE SAVEPOINT ferriprint_notification")?;
    savepoint.committed = true;
    Ok(result)
}

fn now() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
        .min(i64::MAX as u64) as i64
}
