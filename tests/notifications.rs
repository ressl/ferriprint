// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl
// All transports in this file terminate at isolated, nonprinting loopback receivers.

mod support;

use axum::{
    Router,
    body::Bytes,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::post,
};
use base64::Engine as _;
use ferriprint::{
    auth::{Authenticator, Principal},
    config::{Config, Role},
    domain::{JobTemplate, ResolvedJob},
    jobs::{JobEngine, JobRecord},
    notifications::{Delivery, NotificationConfig, Notifications},
    profile::PrinterProfile,
};
use parking_lot::Mutex;
use serde_json::Value;
use std::{
    collections::BTreeSet,
    io::Cursor,
    os::unix::fs::{PermissionsExt, symlink},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicU8, Ordering},
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use support::{Receiver as IppReceiver, add_user, config, settings};
use tokio::sync::Notify;

const PRIVATE_RESPONSE: &str = "private-remote-response-token-and-message";
const PRIVATE_TITLE: &str = "private-document-title.pdf";
const PRIVATE_OWNER: &str = "private-owner-identity";
const PRIVATE_DOCUMENT: &[u8] = b"private-document-content-never-in-notifications";

#[derive(Clone)]
struct Received {
    headers: HeaderMap,
    payload: Value,
    body: Bytes,
}
struct WebhookState {
    received: Mutex<Vec<Received>>,
    // 0 acknowledge, 1 unavailable, 2 redirect, 3 lose/delay acknowledgement.
    mode: AtomicU8,
    redirect: Mutex<Option<String>>,
    accepted: Notify,
}
struct Webhook {
    url: String,
    state: Arc<WebhookState>,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Webhook {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl Webhook {
    async fn start() -> Self {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = Arc::new(WebhookState {
            received: Mutex::new(Vec::new()),
            mode: AtomicU8::new(0),
            redirect: Mutex::new(None),
            accepted: Notify::new(),
        });
        let router = Router::new()
            .route("/notify", post(receive))
            .with_state(state.clone());
        let task = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        Self {
            url: format!("http://{address}/notify"),
            state,
            task,
        }
    }
    fn config(&self) -> NotificationConfig {
        NotificationConfig {
            webhook_url: self.url.clone(),
            timeout_secs: 1,
            max_attempts: 2,
            backoff_initial_secs: 2,
            backoff_max_secs: 2,
            ..Default::default()
        }
    }
    fn received(&self) -> Vec<Received> {
        self.state.received.lock().clone()
    }
}
async fn receive(
    State(state): State<Arc<WebhookState>>,
    headers: HeaderMap,
    body: Bytes,
) -> Response {
    state.received.lock().push(Received {
        headers,
        payload: serde_json::from_slice(&body).unwrap(),
        body,
    });
    state.accepted.notify_one();
    match state.mode.load(Ordering::SeqCst) {
        1 => (StatusCode::SERVICE_UNAVAILABLE, PRIVATE_RESPONSE).into_response(),
        2 => {
            let redirect = state.redirect.lock().clone().unwrap();
            (
                StatusCode::TEMPORARY_REDIRECT,
                [("location", redirect)],
                PRIVATE_RESPONSE,
            )
                .into_response()
        }
        3 => {
            tokio::time::sleep(Duration::from_secs(3)).await;
            (StatusCode::NO_CONTENT, "").into_response()
        }
        _ => (StatusCode::NO_CONTENT, "").into_response(),
    }
}

fn fixture_config(path: &Path, upstream: &str, notification: &NotificationConfig) -> Config {
    let mut config = config(path, upstream, 0);
    config.notifications = Some(notification.clone());
    config
}
fn resolve(config: &Config) -> ResolvedJob {
    PrinterProfile::load(&config.printers[0], &settings(config))
        .unwrap()
        .resolve(&JobTemplate::default(), "application/pdf")
        .unwrap()
}
async fn attach_job(engine: &JobEngine, config: &Config, principal: &Principal) -> JobRecord {
    let job = engine
        .create_job(
            "demo",
            principal,
            PRIVATE_TITLE,
            "application/pdf",
            resolve(config),
        )
        .await
        .unwrap();
    engine
        .ingest_document(job.id, principal, Cursor::new(PRIVATE_DOCUMENT))
        .await
        .unwrap()
}
fn delivery(notifications: &Notifications, id: &str) -> Delivery {
    notifications
        .deliveries()
        .unwrap()
        .into_iter()
        .find(|delivery| delivery.id == id)
        .unwrap()
}
async fn wait_until_due(delivery: &Delivery) {
    let due = delivery.next_attempt_at.unwrap() as u128 * 1000;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis();
    tokio::time::sleep(Duration::from_millis(
        due.saturating_sub(now).min(u64::MAX as u128) as u64 + 20,
    ))
    .await;
}
fn assert_private(received: &Received) {
    let payload = received.payload.as_object().unwrap();
    let mut allowed = BTreeSet::from([
        "event_id",
        "type",
        "printer",
        "condition",
        "reason_code",
        "source",
        "timestamp",
    ]);
    if payload.contains_key("job_id") {
        allowed.insert("job_id");
        assert!(payload["job_id"].as_i64().unwrap() > 0);
    }
    if payload.contains_key("problem") {
        allowed.insert("problem");
        assert!(payload["problem"].is_boolean());
    }
    assert_eq!(
        payload.keys().map(String::as_str).collect::<BTreeSet<_>>(),
        allowed
    );
    let id = payload["event_id"].as_str().unwrap();
    uuid::Uuid::parse_str(id).unwrap();
    assert_eq!(received.headers["idempotency-key"].to_str().unwrap(), id);
    assert_eq!(received.headers["content-type"], "application/json");
    assert_eq!(payload["printer"], "demo");
    assert!(payload["timestamp"].as_i64().unwrap() > 0);
    let body = std::str::from_utf8(&received.body).unwrap();
    for forbidden in [
        PRIVATE_TITLE,
        PRIVATE_OWNER,
        std::str::from_utf8(PRIVATE_DOCUMENT).unwrap(),
        PRIVATE_RESPONSE,
        "backend_identity",
        "upstream_uri",
        "password",
        "argon2",
        "bearer",
        "http://",
        "ipp://",
    ] {
        assert!(!body.contains(forbidden));
    }
}

#[test]
fn protected_configuration_has_strict_bounds_and_unambiguous_authority() {
    let config: NotificationConfig =
        toml::from_str("webhook_url = 'https://notify.example.test/events'").unwrap();
    config.validate().unwrap();
    assert_eq!(config.timeout_secs, 10);
    assert_eq!(config.max_attempts, 5);
    assert_eq!(config.backoff_initial_secs, 5);
    assert_eq!(config.backoff_max_secs, 3600);
    assert!(!format!("{config:?}").contains("notify.example.test"));
    assert!(
        toml::from_str::<NotificationConfig>(
            "webhook_url = 'https://notify.example.test/events'\nmax_attemps = 2"
        )
        .is_err()
    );
    for url in [
        "http://localhost/notify",
        "http://192.0.2.1/notify",
        "http://127.0.0.1@192.0.2.1/notify",
        "https://user:password@notify.example.test/notify",
        "https://@notify.example.test/notify",
        "https://notify.example.test/notify?token=secret",
        "https://notify.example.test/notify#fragment",
        "file:///private/notify",
        "ftp://notify.example.test/notify",
        "http://2130706433/notify",
        "http://127.1/notify",
        "http://0177.0.0.1/notify",
        "http://[::ffff:127.0.0.1]/notify",
        "http://127.0.0.1:0/notify",
        " https://notify.example.test/notify",
        "https://notify.example.test\\@127.0.0.1/notify",
    ] {
        let candidate = NotificationConfig {
            webhook_url: url.into(),
            ..config.clone()
        };
        assert!(candidate.validate().is_err(), "accepted unsafe URL {url}");
    }
    for url in [
        "http://127.0.0.1:12345/notify",
        "http://127.100.20.1/notify",
        "http://[::1]:12345/notify",
        "https://notify.example.test/notify",
    ] {
        NotificationConfig {
            webhook_url: url.into(),
            ..config.clone()
        }
        .validate()
        .unwrap();
    }
    for invalid in [
        NotificationConfig {
            timeout_secs: 0,
            ..config.clone()
        },
        NotificationConfig {
            timeout_secs: 121,
            ..config.clone()
        },
        NotificationConfig {
            max_attempts: 0,
            ..config.clone()
        },
        NotificationConfig {
            max_attempts: 21,
            ..config.clone()
        },
        NotificationConfig {
            backoff_initial_secs: 0,
            ..config.clone()
        },
        NotificationConfig {
            backoff_max_secs: 4,
            ..config.clone()
        },
        NotificationConfig {
            backoff_max_secs: 86401,
            ..config.clone()
        },
    ] {
        assert!(invalid.validate().is_err());
    }
    let mut disabled: NotificationConfig =
        toml::from_str("enabled = false\nbearer_secret_file = 'notify.secret'").unwrap();
    disabled.validate().unwrap();
    disabled.resolve_paths(Path::new("/private/config"));
    assert_eq!(
        disabled.bearer_secret_file.unwrap(),
        Path::new("/private/config/notify.secret")
    );
}

#[tokio::test]
async fn unconfigured_and_explicitly_disabled_services_never_fake_a_diagnostic() {
    let dir = tempfile::tempdir().unwrap();
    let config = config(dir.path(), "ipp://127.0.0.1:9/printers/receiver", 0);
    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(None, engine.clone()).unwrap();
    assert!(!notifications.configured());
    assert!(!notifications.is_enabled("demo").unwrap());
    assert!(notifications.set_enabled("demo", true).is_err());
    assert!(notifications.diagnostic("demo").is_err());
    assert!(notifications.retry("missing").is_err());
    notifications
        .observe("demo", "paper-shortage", Some(true), "native-queue")
        .unwrap();
    notifications.tick().await.unwrap();
    assert!(notifications.deliveries().unwrap().is_empty());
    let disabled = NotificationConfig {
        enabled: false,
        ..Default::default()
    };
    let notifications = Notifications::open(Some(&disabled), engine).unwrap();
    assert!(!notifications.configured());
    assert!(notifications.diagnostic("demo").is_err());
    assert!(notifications.deliveries().unwrap().is_empty());
}

#[tokio::test]
async fn transitions_are_source_scoped_unknown_safe_deduplicated_and_restart_durable() {
    let dir = tempfile::tempdir().unwrap();
    let webhook = Webhook::start().await;
    let notification_config = webhook.config();
    let config = fixture_config(
        dir.path(),
        "ipp://127.0.0.1:9/printers/receiver",
        &notification_config,
    );
    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(Some(&notification_config), engine.clone()).unwrap();
    notifications.set_enabled("demo", true).unwrap();
    let diagnostic = notifications.diagnostic("demo").unwrap();
    for (condition, source) in [
        ("native-queue-unreachable", "native-queue"),
        ("telemetry-source-unreachable", "telemetry"),
        ("paper-shortage", "telemetry"),
    ] {
        notifications
            .observe("demo", condition, None, source)
            .unwrap();
        notifications
            .observe("demo", condition, Some(false), source)
            .unwrap();
        notifications
            .observe("demo", condition, Some(true), source)
            .unwrap();
        notifications
            .observe("demo", condition, Some(true), source)
            .unwrap();
    }
    notifications
        .observe("demo", "paper-shortage", None, "telemetry")
        .unwrap();
    notifications
        .observe("demo", "paper-shortage", Some(false), "native-queue")
        .unwrap();
    assert_eq!(notifications.deliveries().unwrap().len(), 4);
    notifications
        .observe(
            "demo",
            "native-queue-unreachable",
            Some(false),
            "native-queue",
        )
        .unwrap();
    notifications
        .observe(
            "demo",
            "telemetry-source-unreachable",
            Some(false),
            "telemetry",
        )
        .unwrap();
    assert!(
        notifications
            .observe("demo", PRIVATE_RESPONSE, Some(true), "telemetry")
            .is_err()
    );
    assert!(
        notifications
            .observe("demo", "paper-shortage", Some(true), &webhook.url)
            .is_err()
    );
    let (first, concurrent) = tokio::join!(notifications.tick(), notifications.tick());
    first.unwrap();
    concurrent.unwrap();
    assert_eq!(webhook.received().len(), 6);
    assert_eq!(delivery(&notifications, &diagnostic).status, "delivered");
    drop(notifications);
    drop(engine);

    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(Some(&notification_config), engine).unwrap();
    assert!(notifications.is_enabled("demo").unwrap());
    notifications
        .observe("demo", "paper-shortage", None, "telemetry")
        .unwrap();
    notifications
        .observe("demo", "paper-shortage", Some(true), "telemetry")
        .unwrap();
    notifications
        .observe("demo", "paper-shortage", Some(false), "native-queue")
        .unwrap();
    assert_eq!(notifications.deliveries().unwrap().len(), 6);
    notifications
        .observe("demo", "paper-shortage", Some(false), "telemetry")
        .unwrap();
    notifications
        .observe("demo", "paper-shortage", Some(false), "telemetry")
        .unwrap();
    notifications
        .observe("demo", "paper-shortage", Some(true), "telemetry")
        .unwrap();
    notifications.set_enabled("demo", false).unwrap();
    notifications
        .observe(
            "demo",
            "native-queue-unreachable",
            Some(true),
            "native-queue",
        )
        .unwrap();
    notifications.tick().await.unwrap();
    notifications.tick().await.unwrap();
    let received = webhook.received();
    assert_eq!(received.len(), 8);
    assert_eq!(
        received
            .iter()
            .map(|event| event.payload["event_id"].as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        received.len()
    );
    for event in &received {
        assert_private(event);
        assert!(!event.headers.contains_key("authorization"));
    }
    assert_eq!(
        received
            .iter()
            .filter(|event| event.payload["type"] == "recovery")
            .count(),
        3
    );
    assert!(!notifications.is_enabled("demo").unwrap());
}

#[tokio::test]
async fn pending_failures_survive_restart_and_exhausted_events_require_explicit_retry() {
    let dir = tempfile::tempdir().unwrap();
    let webhook = Webhook::start().await;
    webhook.state.mode.store(1, Ordering::SeqCst);
    let notification_config = webhook.config();
    let config = fixture_config(
        dir.path(),
        "ipp://127.0.0.1:9/printers/receiver",
        &notification_config,
    );
    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(Some(&notification_config), engine.clone()).unwrap();
    let id = notifications.diagnostic("demo").unwrap();
    notifications.tick().await.unwrap();
    let pending = delivery(&notifications, &id);
    assert_eq!(pending.status, "pending");
    assert_eq!(pending.attempts, 1);
    assert_eq!(pending.failure_class.as_deref(), Some("http-5xx"));
    assert_eq!(pending.http_status, Some(503));
    assert!(pending.next_attempt_at.unwrap() >= pending.last_attempt_at.unwrap() + 2);
    notifications.tick().await.unwrap();
    assert_eq!(webhook.received().len(), 1);
    drop(notifications);
    drop(engine);

    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(Some(&notification_config), engine).unwrap();
    wait_until_due(&delivery(&notifications, &id)).await;
    notifications.tick().await.unwrap();
    let exhausted = delivery(&notifications, &id);
    assert_eq!(exhausted.status, "exhausted");
    assert_eq!(exhausted.attempts, 2);
    assert_eq!(exhausted.total_attempts, 2);
    assert!(exhausted.next_attempt_at.is_none());
    assert!(exhausted.exhausted_at.is_some());
    notifications.tick().await.unwrap();
    assert_eq!(webhook.received().len(), 2);
    assert!(
        !serde_json::to_string(&notifications.deliveries().unwrap())
            .unwrap()
            .contains(PRIVATE_RESPONSE)
    );

    webhook.state.mode.store(0, Ordering::SeqCst);
    notifications.retry(&id).unwrap();
    notifications.tick().await.unwrap();
    let delivered = delivery(&notifications, &id);
    assert_eq!(delivered.status, "delivered");
    assert_eq!(delivered.attempts, 1);
    assert_eq!(delivered.total_attempts, 3);
    assert_eq!(delivered.retry_count, 1);
    assert!(delivered.delivered_at.is_some());
    assert!(delivered.failure_class.is_none());
    assert!(delivered.exhausted_at.is_none());
    assert!(notifications.retry(&id).is_err());
    let received = webhook.received();
    assert_eq!(received.len(), 3);
    assert!(received.iter().all(|event| event.payload["event_id"] == id));
    assert!(received.iter().all(|event| event.body == received[0].body));
    for event in &received {
        assert_private(event);
    }
}

#[tokio::test]
async fn timeout_and_redirect_failures_are_real_redacted_and_private_bearer_is_not_forwarded() {
    let dir = tempfile::tempdir().unwrap();
    let webhook = Webhook::start().await;
    let redirect_target = Webhook::start().await;
    let secret_path = dir.path().join("notification.secret");
    let secret = "private-bearer-token";
    std::fs::write(&secret_path, format!("{secret}\n")).unwrap();
    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let mut notification_config = webhook.config();
    notification_config.max_attempts = 1;
    notification_config.bearer_secret_file = Some(secret_path.clone());
    let config = fixture_config(
        dir.path(),
        "ipp://127.0.0.1:9/printers/receiver",
        &notification_config,
    );
    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(Some(&notification_config), engine.clone()).unwrap();
    webhook.state.mode.store(3, Ordering::SeqCst);
    let timeout_id = notifications.diagnostic("demo").unwrap();
    notifications.tick().await.unwrap();
    let timed_out = delivery(&notifications, &timeout_id);
    assert_eq!(timed_out.status, "exhausted");
    assert_eq!(timed_out.failure_class.as_deref(), Some("timeout"));
    assert_eq!(timed_out.attempts, 1);
    assert!(timed_out.http_status.is_none());
    webhook.state.mode.store(0, Ordering::SeqCst);
    notifications.retry(&timeout_id).unwrap();
    notifications.tick().await.unwrap();
    assert_eq!(delivery(&notifications, &timeout_id).status, "delivered");

    *webhook.state.redirect.lock() = Some(redirect_target.url.clone());
    webhook.state.mode.store(2, Ordering::SeqCst);
    let redirect_id = notifications.diagnostic("demo").unwrap();
    notifications.tick().await.unwrap();
    let redirected = delivery(&notifications, &redirect_id);
    assert_eq!(redirected.status, "exhausted");
    assert_eq!(redirected.failure_class.as_deref(), Some("http-3xx"));
    assert_eq!(redirected.http_status, Some(307));
    assert!(redirect_target.received().is_empty());
    for event in webhook.received() {
        assert_eq!(
            event.headers["authorization"].to_str().unwrap(),
            format!("Bearer {secret}")
        );
        assert!(!std::str::from_utf8(&event.body).unwrap().contains(secret));
        assert_private(&event);
    }
    let metadata = serde_json::to_string(&notifications.deliveries().unwrap()).unwrap();
    assert!(!metadata.contains(secret));
    assert!(!metadata.contains(PRIVATE_RESPONSE));
    assert!(!metadata.contains(&webhook.url));
    assert!(!format!("{notification_config:?}").contains(secret));

    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(Notifications::open(Some(&notification_config), engine.clone()).is_err());
    std::fs::set_permissions(&secret_path, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.path().join("notification.link");
    symlink(&secret_path, &link).unwrap();
    notification_config.bearer_secret_file = Some(link);
    assert!(Notifications::open(Some(&notification_config), engine).is_err());
}

#[tokio::test]
async fn canceled_delivery_is_reclaimed_after_restart_with_identical_idempotency_key() {
    let dir = tempfile::tempdir().unwrap();
    let webhook = Webhook::start().await;
    webhook.state.mode.store(3, Ordering::SeqCst);
    let notification_config = webhook.config();
    let config = fixture_config(
        dir.path(),
        "ipp://127.0.0.1:9/printers/receiver",
        &notification_config,
    );
    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(Some(&notification_config), engine.clone()).unwrap();
    let id = notifications.diagnostic("demo").unwrap();
    let worker = notifications.clone();
    let task = tokio::spawn(async move { worker.tick().await });
    webhook.state.accepted.notified().await;
    assert_eq!(delivery(&notifications, &id).status, "sending");
    assert!(notifications.retry(&id).is_err());
    task.abort();
    assert!(task.await.unwrap_err().is_cancelled());
    drop(notifications);
    drop(engine);

    webhook.state.mode.store(0, Ordering::SeqCst);
    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(Some(&notification_config), engine).unwrap();
    notifications.tick().await.unwrap();
    assert_eq!(webhook.received().len(), 1);
    wait_until_due(&delivery(&notifications, &id)).await;
    notifications.tick().await.unwrap();
    let delivered = delivery(&notifications, &id);
    assert_eq!(delivered.status, "delivered");
    assert_eq!(delivered.total_attempts, 2);
    let received = webhook.received();
    assert_eq!(received.len(), 2);
    assert_eq!(received[0].body, received[1].body);
    assert_eq!(
        received[0].headers["idempotency-key"],
        received[1].headers["idempotency-key"]
    );
    assert_eq!(received[0].payload["event_id"], id);
}

#[tokio::test]
async fn job_failures_are_baselined_once_and_live_stopped_aborted_uncertain_transitions_are_private()
 {
    let dir = tempfile::tempdir().unwrap();
    let webhook = Webhook::start().await;
    let ipp = IppReceiver::start().await;
    let notification_config = webhook.config();
    let mut config = fixture_config(dir.path(), &ipp.uri(), &notification_config);
    add_user(
        &mut config,
        PRIVATE_OWNER,
        Role::User,
        "private-test-password",
    );
    let authorization = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD
            .encode(format!("{PRIVATE_OWNER}:private-test-password"))
    );
    let principal = Authenticator::open(&config)
        .unwrap()
        .authenticate(&authorization)
        .await
        .unwrap();
    let engine = JobEngine::open(&config).await.unwrap();

    ipp.device.mode.store(1, Ordering::SeqCst);
    let historical_aborted = attach_job(&engine, &config, &principal).await;
    engine.tick().await.unwrap();
    assert_eq!(
        engine
            .get_job(historical_aborted.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        8
    );
    ipp.device.mode.store(0, Ordering::SeqCst);
    let historical_stopped = attach_job(&engine, &config, &principal).await;
    engine.tick().await.unwrap();
    ipp.device.jobs.lock()[0].state = 6;
    engine.tick().await.unwrap();
    assert_eq!(
        engine
            .get_job(historical_stopped.id)
            .await
            .unwrap()
            .unwrap()
            .state,
        6
    );

    let notifications = Notifications::open(Some(&notification_config), engine.clone()).unwrap();
    notifications.set_enabled("demo", true).unwrap();
    engine.tick().await.unwrap();
    assert!(notifications.deliveries().unwrap().is_empty());
    ipp.device.mode.store(1, Ordering::SeqCst);
    let aborted = attach_job(&engine, &config, &principal).await;
    engine.tick().await.unwrap();
    assert_eq!(engine.get_job(aborted.id).await.unwrap().unwrap().state, 8);
    assert_eq!(notifications.deliveries().unwrap().len(), 1);

    ipp.device.mode.store(0, Ordering::SeqCst);
    let stopped = attach_job(&engine, &config, &principal).await;
    engine.tick().await.unwrap();
    ipp.device.jobs.lock()[1].state = 6;
    engine.tick().await.unwrap();
    engine.tick().await.unwrap();
    assert_eq!(notifications.deliveries().unwrap().len(), 2);
    let canceled = attach_job(&engine, &config, &principal).await;
    engine.tick().await.unwrap();
    engine.cancel_job(canceled.id, &principal).await.unwrap();
    engine.tick().await.unwrap();
    assert_eq!(engine.get_job(canceled.id).await.unwrap().unwrap().state, 7);
    assert_eq!(
        notifications.deliveries().unwrap().len(),
        2,
        "ordinary confirmed user cancellation is not a job-failure alert"
    );
    drop(notifications);
    drop(engine);

    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(Some(&notification_config), engine.clone()).unwrap();
    engine.tick().await.unwrap();
    assert_eq!(notifications.deliveries().unwrap().len(), 2);
    ipp.device.jobs.lock()[1].state = 5;
    engine.tick().await.unwrap();
    engine.tick().await.unwrap();
    assert_eq!(notifications.deliveries().unwrap().len(), 3);

    ipp.device.mode.store(2, Ordering::SeqCst);
    let uncertain = attach_job(&engine, &config, &principal).await;
    engine.tick().await.unwrap();
    assert_eq!(
        engine.get_job(uncertain.id).await.unwrap().unwrap().phase,
        "uncertain"
    );
    engine.tick().await.unwrap();
    assert_eq!(notifications.deliveries().unwrap().len(), 4);
    notifications.tick().await.unwrap();
    let received = webhook.received();
    assert_eq!(received.len(), 4);
    assert!(received.iter().all(|event| {
        let id = event.payload["job_id"].as_i64().unwrap();
        id != i64::from(historical_aborted.id) && id != i64::from(historical_stopped.id)
    }));
    for event in &received {
        assert_private(event);
        assert_eq!(event.payload["source"], "local-job");
        assert!(
            !std::str::from_utf8(&event.body)
                .unwrap()
                .contains(&ipp.uri())
        );
    }
    assert!(
        received
            .iter()
            .any(|event| event.payload["job_id"] == aborted.id
                && event.payload["condition"] == "job-aborted")
    );
    assert!(
        received
            .iter()
            .any(|event| event.payload["job_id"] == stopped.id
                && event.payload["type"] == "recovery")
    );
    assert!(
        received
            .iter()
            .any(|event| event.payload["job_id"] == uncertain.id
                && event.payload["reason_code"] == "submission-outcome-unknown")
    );

    notifications.set_enabled("demo", false).unwrap();
    ipp.device.mode.store(1, Ordering::SeqCst);
    attach_job(&engine, &config, &principal).await;
    engine.tick().await.unwrap();
    notifications.set_enabled("demo", true).unwrap();
    engine.tick().await.unwrap();
    assert_eq!(notifications.deliveries().unwrap().len(), 4);
}

#[tokio::test]
async fn bounded_ticks_and_disabled_startup_preserve_pending_delivery_evidence() {
    let dir = tempfile::tempdir().unwrap();
    let webhook = Webhook::start().await;
    let notification_config = webhook.config();
    let mut config = fixture_config(
        dir.path(),
        "ipp://127.0.0.1:9/printers/receiver",
        &notification_config,
    );
    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(Some(&notification_config), engine.clone()).unwrap();
    notifications.set_enabled("demo", true).unwrap();
    for _ in 0..20 {
        notifications.diagnostic("demo").unwrap();
    }
    notifications.tick().await.unwrap();
    assert_eq!(webhook.received().len(), 16);
    assert_eq!(
        notifications
            .deliveries()
            .unwrap()
            .iter()
            .filter(|event| event.status == "pending")
            .count(),
        4
    );
    drop(notifications);
    drop(engine);

    config.notifications = None;
    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(None, engine.clone()).unwrap();
    assert!(!notifications.configured());
    assert!(notifications.is_enabled("demo").unwrap());
    assert!(notifications.diagnostic("demo").is_err());
    notifications
        .observe("demo", "paper-shortage", Some(true), "telemetry")
        .unwrap();
    notifications.tick().await.unwrap();
    assert_eq!(webhook.received().len(), 16);
    assert_eq!(notifications.deliveries().unwrap().len(), 20);
    drop(notifications);
    drop(engine);

    config.notifications = Some(notification_config.clone());
    let engine = JobEngine::open(&config).await.unwrap();
    let notifications = Notifications::open(Some(&notification_config), engine).unwrap();
    notifications.tick().await.unwrap();
    assert_eq!(webhook.received().len(), 20);
    assert!(
        notifications
            .deliveries()
            .unwrap()
            .iter()
            .all(|event| event.status == "delivered")
    );
    notifications
        .observe("demo", "paper-shortage", Some(true), "telemetry")
        .unwrap();
    notifications.tick().await.unwrap();
    assert_eq!(webhook.received().len(), 21);
    let received = webhook.received();
    assert_eq!(
        received
            .iter()
            .map(|event| event.payload["event_id"].as_str().unwrap())
            .collect::<BTreeSet<_>>()
            .len(),
        21
    );
}
