// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl
mod support;
use ferriprint::{
    config::{Config, Role},
    domain::{JobTemplate, MediaRequest},
    jobs::JobEngine,
    profile::PrinterProfile,
    setup::SetupChoices,
    wire::*,
};
use futures_util::TryStreamExt;
use ipp::prelude::*;
use std::{
    collections::BTreeMap,
    io::Write,
    process::{Command, Stdio},
    time::Duration,
};
use support::*;
const OP: DelimiterTag = DelimiterTag::OperationAttributes;
const JOB: DelimiterTag = DelimiterTag::JobAttributes;
const PASSWORD: &str = "isolated-consumer-password";
fn id(reply: &IppRequestResponse) -> i32 {
    attr(reply, JOB, "job-id").and_then(integer).unwrap()
}
fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}
async fn ipp(
    front: &Frontend,
    actor: &str,
    request: IppRequestResponse,
    payload: &[u8],
) -> IppRequestResponse {
    let mut bytes = request.to_bytes().to_vec();
    bytes.extend_from_slice(payload);
    let response = client()
        .post(format!("http://{}/ipp/print/demo", front.address))
        .basic_auth(actor, Some(PASSWORD))
        .header("content-type", "application/ipp")
        .body(bytes)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut reader =
        tokio_util::io::StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
    read_metadata(&mut reader, 1024 * 1024).await.unwrap()
}
fn origin(front: &Frontend) -> String {
    http_url(&front.app.config.public_url)
        .unwrap()
        .origin()
        .ascii_serialization()
}
async fn configuration_time(front: &Frontend) -> (i32, i32) {
    let reply = ipp(
        front,
        "operator",
        front.request(Operation::GetPrinterAttributes, None),
        &[],
    )
    .await;
    let group = DelimiterTag::PrinterAttributes;
    (
        attr(&reply, group, "printer-config-change-time")
            .and_then(integer)
            .unwrap(),
        attr(&reply, group, "printer-up-time")
            .and_then(integer)
            .unwrap(),
    )
}
async fn csrf(front: &Frontend, actor: &str, path: &str) -> String {
    let response = client()
        .get(format!("http://{}{path}", front.address))
        .basic_auth(actor, Some(PASSWORD))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("frame-ancestors 'none'")
    );
    assert_eq!(response.headers()["cache-control"], "no-store");
    response
        .text()
        .await
        .unwrap()
        .split("name=\"csrf\" value=\"")
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}
fn actors(cfg: &mut Config) {
    add_user(cfg, "alice", Role::User, PASSWORD);
    add_user(cfg, "bob", Role::User, PASSWORD);
    add_user(cfg, "operator", Role::Admin, PASSWORD);
}
fn form(
    settings: &ferriprint::config::ProfileSettings,
    token: &str,
    revision: u64,
    intent: &str,
) -> BTreeMap<String, String> {
    let mut advanced = toml::Table::try_from(settings).unwrap();
    advanced.remove("defaults");
    let d = &settings.defaults;
    let mut form = BTreeMap::from([
        ("csrf".into(), token.into()),
        ("revision".into(), revision.to_string()),
        ("intent".into(), intent.into()),
        (
            "settings".into(),
            toml::to_string_pretty(&advanced).unwrap(),
        ),
        ("default_media".into(), d.media.clone()),
        ("default_source".into(), d.source.clone()),
        ("default_media_type".into(), d.media_type.clone()),
        ("default_quality".into(), d.quality.to_string()),
        ("default_color_mode".into(), d.color_mode.clone()),
        ("default_sides".into(), d.sides.clone()),
    ]);
    if d.borderless {
        form.insert("default_borderless".into(), "true".into());
    }
    form
}

#[tokio::test]
async fn verified_users_isolate_ipp_html_json_and_real_upstream_cancellation() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = config(dir.path(), &receiver.uri(), 0);
    actors(&mut cfg);
    cfg.access.anonymous_trusted_lan = false;
    cfg.access.anonymous_owner = None;
    cfg.access.anonymous_clients.clear();
    let front = Frontend::start(cfg).await;
    let mut request = front.request(Operation::PrintJob, None);
    add(
        &mut request,
        OP,
        "job-name",
        name("alice-private <script>alert(1)</script>").unwrap(),
    )
    .unwrap();
    add(
        &mut request,
        OP,
        "requesting-user-name",
        name("bob").unwrap(),
    )
    .unwrap();
    let alice = id(&ipp(&front, "alice", request, b"%PDF-1.4\n%%EOF").await);
    let mut request = front.request(Operation::PrintJob, None);
    add(&mut request, OP, "job-name", name("bob-private").unwrap()).unwrap();
    let bob = id(&ipp(&front, "bob", request, b"%PDF-1.4\n%%EOF").await);
    front.app.engine.tick().await.unwrap();
    let guest = client();
    let endpoint = format!("http://{}/ipp/print/demo", front.address);
    let gpa = front
        .request(Operation::GetPrinterAttributes, None)
        .to_bytes();
    let response = guest
        .post(&endpoint)
        .header("content-type", "application/ipp")
        .body(gpa.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut reader =
        tokio_util::io::StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
    let public = read_metadata(&mut reader, 1024 * 1024).await.unwrap();
    assert_eq!(public.header().status_code(), StatusCode::SuccessfulOk);
    assert_eq!(public.attributes().groups_of(JOB).count(), 0);
    let public_bytes = public.to_bytes();
    for title in ["alice-private", "bob-private"] {
        assert!(!String::from_utf8_lossy(&public_bytes).contains(title));
    }
    for operation in [
        Operation::PrintJob,
        Operation::ValidateJob,
        Operation::CreateJob,
        Operation::SendDocument,
        Operation::CancelJob,
        Operation::GetJobAttributes,
        Operation::GetJobs,
    ] {
        let target = matches!(
            operation,
            Operation::SendDocument | Operation::CancelJob | Operation::GetJobAttributes
        )
        .then_some(alice);
        let mut request = front.request(operation, target);
        add(
            &mut request,
            OP,
            "requesting-user-name",
            name("alice").unwrap(),
        )
        .unwrap();
        assert_eq!(
            guest
                .post(&endpoint)
                .header("content-type", "application/ipp")
                .body(request.to_bytes())
                .send()
                .await
                .unwrap()
                .status(),
            401,
            "{operation:?} must not acquire a job owner from capability access"
        );
    }
    for path in [
        "/".to_owned(),
        "/api/jobs".to_owned(),
        format!("/api/jobs/{alice}"),
        "/api/printers".to_owned(),
        "/admin".to_owned(),
        "/admin/printers/demo".to_owned(),
    ] {
        assert_eq!(
            guest
                .get(format!("http://{}{path}", front.address))
                .send()
                .await
                .unwrap()
                .status(),
            401
        );
    }
    let source = format!("http://{}/source", front.address);
    assert_eq!(guest.get(&source).send().await.unwrap().status(), 200);
    assert_eq!(
        guest
            .post(&endpoint)
            .header("content-type", "application/ipp")
            .basic_auth("alice", Some("wrong-password"))
            .body(gpa)
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        guest
            .get(source)
            .header("authorization", "Bearer invalid-fixture-authorization")
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        front
            .app
            .engine
            .get_job(alice)
            .await
            .unwrap()
            .unwrap()
            .owner,
        "alice"
    );
    for operation in [Operation::GetJobAttributes, Operation::CancelJob] {
        let reply = ipp(&front, "bob", front.request(operation, Some(alice)), &[]).await;
        assert_eq!(
            reply.header().status_code(),
            StatusCode::ClientErrorNotAuthorized
        );
    }
    let mut query = front.request(Operation::GetJobs, None);
    add(&mut query, OP, "my-jobs", IppValue::Boolean(false)).unwrap();
    let reply = ipp(&front, "bob", query, &[]).await;
    assert_eq!(reply.attributes().groups_of(JOB).count(), 1);
    assert_eq!(id(&reply), bob);
    for path in ["/", "/api/jobs"] {
        let body = client()
            .get(format!("http://{}{path}", front.address))
            .basic_auth("bob", Some(PASSWORD))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(body.contains("bob-private"));
        assert!(!body.contains("alice-private"));
        let shared = client()
            .get(format!("http://{}{path}", front.address))
            .send()
            .await
            .unwrap()
            .text()
            .await
            .unwrap();
        assert!(!shared.contains("alice-private") && !shared.contains("bob-private"));
    }
    assert_eq!(
        client()
            .get(format!("http://{}/api/jobs/{alice}", front.address))
            .basic_auth("bob", Some(PASSWORD))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client()
            .get(format!("http://{}/admin", front.address))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        client()
            .get(format!("http://{}/admin", front.address))
            .basic_auth("alice", Some(PASSWORD))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client()
            .get(format!("http://{}/", front.address))
            .basic_auth("bob", Some("wrong-password"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    // Authentication scheme is case-insensitive, as required for HTTP Basic.
    use base64::Engine;
    let authorization = format!(
        "bAsIc {}",
        base64::engine::general_purpose::STANDARD.encode(format!("alice:{PASSWORD}"))
    );
    let html = client()
        .get(format!("http://{}/", front.address))
        .header("authorization", authorization)
        .send()
        .await
        .unwrap()
        .text()
        .await
        .unwrap();
    assert!(html.contains("alice-private &lt;script&gt;"));
    assert!(!html.contains("<script>alert"));
    let token = csrf(&front, "alice", "/").await;
    let endpoint = format!("http://{}/api/jobs/{alice}/cancel", front.address);
    assert_eq!(
        client()
            .post(&endpoint)
            .basic_auth("alice", Some(PASSWORD))
            .header("origin", origin(&front))
            .form(&[("csrf", "wrong")])
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client()
            .post(&endpoint)
            .basic_auth("alice", Some(PASSWORD))
            .header("origin", "https://attacker.example")
            .form(&[("csrf", token.as_str())])
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client()
            .post(&endpoint)
            .basic_auth("alice", Some(PASSWORD))
            .header("origin", "null")
            .form(&[("csrf", token.as_str())])
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert!(receiver.device.cancellations.lock().is_empty());
    assert_eq!(
        client()
            .post(&endpoint)
            .basic_auth("alice", Some(PASSWORD))
            .header("origin", origin(&front))
            .form(&[("csrf", token.as_str())])
            .send()
            .await
            .unwrap()
            .status(),
        303
    );
    let admin = csrf(&front, "operator", "/admin").await;
    assert_eq!(
        client()
            .post(format!("http://{}/api/jobs/{bob}/cancel", front.address))
            .basic_auth("operator", Some(PASSWORD))
            .header("origin", origin(&front))
            .form(&[("csrf", admin.as_str())])
            .send()
            .await
            .unwrap()
            .status(),
        303
    );
    front.app.engine.tick().await.unwrap();
    // Reconciliation may repeat an idempotent cancel while observing the native
    // terminal state; the security contract is that only authorized IDs are hit.
    let cancelled: std::collections::BTreeSet<_> = receiver
        .device
        .cancellations
        .lock()
        .iter()
        .copied()
        .collect();
    assert_eq!(cancelled, std::collections::BTreeSet::from([700, 701]));
    assert_eq!(
        front
            .app
            .engine
            .get_job(alice)
            .await
            .unwrap()
            .unwrap()
            .state,
        7
    );
    assert_eq!(
        front.app.engine.get_job(bob).await.unwrap().unwrap().state,
        7
    );
}

#[tokio::test]
async fn profile_validation_gate_durable_apply_and_immutable_job_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = config(dir.path(), &receiver.uri(), 0);
    actors(&mut cfg);
    // Real file metadata and a controlled durable origin make configuration
    // transitions observable without sleeping or inventing time on each query.
    let baseline = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs()
        - 120;
    let time_origin = baseline as i64 - 30;
    let private_ppd = dir.path().join("provisioned.ppd");
    std::fs::copy(&cfg.printers[0].ppd, &private_ppd).unwrap();
    cfg.printers[0].ppd = private_ppd;
    for (path, seconds) in [
        (cfg.profile_path("demo"), baseline),
        (cfg.printers[0].ppd.clone(), baseline + 10),
    ] {
        std::fs::File::options()
            .write(true)
            .open(path)
            .unwrap()
            .set_times(
                std::fs::FileTimes::new()
                    .set_modified(std::time::UNIX_EPOCH + Duration::from_secs(seconds)),
            )
            .unwrap();
    }
    let engine = JobEngine::open(&cfg).await.unwrap();
    drop(engine);
    let database = rusqlite::Connection::open(cfg.state_dir.join("jobs.sqlite3")).unwrap();
    database
        .execute(
            "UPDATE service_meta SET value=?1 WHERE key='time-origin'",
            [time_origin],
        )
        .unwrap();
    drop(database);
    let restart = cfg.clone();
    let front = Frontend::start(cfg).await;
    let (before, _) = configuration_time(&front).await;
    assert_eq!(before, ((baseline + 10) as i64 - time_origin) as i32);
    assert_eq!(configuration_time(&front).await.0, before);
    let token = csrf(&front, "operator", "/admin/printers/demo").await;
    let endpoint = format!("http://{}/admin/printers/demo", front.address);
    let mut desired = settings(&front.app.config);
    desired.defaults.quality = 5;
    desired.defaults.color_mode = "monochrome".into();
    let send = |form: BTreeMap<String, String>| {
        client()
            .post(&endpoint)
            .basic_auth("operator", Some(PASSWORD))
            .header("origin", origin(&front))
            .form(&form)
            .send()
    };
    let validation = send(form(&desired, &token, 0, "validate")).await.unwrap();
    assert_eq!(validation.status(), 200);
    assert_eq!(settings(&front.app.config).defaults.quality, 4);
    let held = id(&ipp(
        &front,
        "alice",
        front.request(Operation::CreateJob, None),
        &[],
    )
    .await);
    assert_eq!(
        send(form(&desired, &token, 0, "save"))
            .await
            .unwrap()
            .status(),
        409
    );
    assert_eq!(settings(&front.app.config).defaults.quality, 4);
    assert_eq!(configuration_time(&front).await.0, before);
    assert_eq!(
        front
            .app
            .engine
            .get_job(held)
            .await
            .unwrap()
            .unwrap()
            .resolved
            .quality,
        4
    );
    front.app.engine.cancel_job(held, "alice").await.unwrap();
    let mut invalid = desired.clone();
    invalid
        .media_types
        .get_mut("stationery")
        .unwrap()
        .normal
        .insert("Quality".into(), "Invented".into());
    assert_eq!(
        send(form(&invalid, &token, 0, "save"))
            .await
            .unwrap()
            .status(),
        400
    );
    let mut malicious = form(&desired, &token, 0, "save");
    malicious
        .get_mut("settings")
        .unwrap()
        .push_str("\nppd = '/etc/shadow'\n");
    assert_eq!(send(malicious).await.unwrap().status(), 400);
    assert_eq!(
        send(form(&desired, &token, 0, "save"))
            .await
            .unwrap()
            .status(),
        303
    );
    let (applied_time, uptime) = configuration_time(&front).await;
    assert!(applied_time > before && applied_time <= uptime);
    assert_eq!(configuration_time(&front).await.0, applied_time);
    assert_eq!(settings(&front.app.config).defaults.quality, 5);
    assert_eq!(
        front.app.printers["demo"]
            .read()
            .await
            .profile
            .capabilities()
            .defaults
            .quality,
        5
    );
    let new_job = id(&ipp(
        &front,
        "bob",
        front.request(Operation::CreateJob, None),
        &[],
    )
    .await);
    let snapshot = front.app.engine.get_job(new_job).await.unwrap().unwrap();
    assert_eq!(snapshot.resolved.options["Quality"], "Fine");
    assert_eq!(snapshot.resolved.color_mode, "monochrome");
    assert_eq!(
        send(form(&desired, &token, 0, "save"))
            .await
            .unwrap()
            .status(),
        409
    );
    front.app.engine.cancel_job(new_job, "bob").await.unwrap();
    drop(front);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let reopened = ferriprint::server::App::open(restart).await.unwrap();
    let restarted = Frontend::from_app(reopened.clone()).await;
    assert_eq!(configuration_time(&restarted).await.0, applied_time);
    assert_eq!(
        reopened.printers["demo"]
            .read()
            .await
            .profile
            .capabilities()
            .defaults
            .quality,
        5
    );
    assert_eq!(
        reopened
            .engine
            .get_job(held)
            .await
            .unwrap()
            .unwrap()
            .resolved
            .quality,
        4
    );
    assert_eq!(
        reopened
            .engine
            .get_job(new_job)
            .await
            .unwrap()
            .unwrap()
            .resolved
            .quality,
        5
    );
}

#[tokio::test]
async fn active_backend_identity_cannot_be_removed_changed_or_silently_migrated() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let cfg = config(dir.path(), &receiver.uri(), 0);
    let engine = JobEngine::open(&cfg).await.unwrap();
    let resolved = PrinterProfile::load(&cfg.printers[0], &settings(&cfg))
        .unwrap()
        .resolve(&JobTemplate::default(), "application/pdf")
        .unwrap();
    let held = engine
        .create_job(
            "demo",
            "alice",
            "immutable backend",
            "application/pdf",
            resolved,
        )
        .await
        .unwrap();
    drop(engine);
    let mut changed = cfg.clone();
    changed.printers[0].upstream_uri = "ipp://127.0.0.1:9/printers/elsewhere".parse().unwrap();
    assert!(JobEngine::open(&changed).await.is_err());
    let engine = JobEngine::open(&cfg).await.unwrap();
    assert_eq!(
        engine.get_job(held.id).await.unwrap().unwrap().phase,
        "held"
    );
    engine.cancel_job(held.id, "alice").await.unwrap();
    drop(engine);
    // Terminal history remains readable after a protected queue identity update.
    let engine = JobEngine::open(&changed).await.unwrap();
    assert_eq!(
        engine
            .get_job(held.id)
            .await
            .unwrap()
            .unwrap()
            .resolved
            .options["PageSize"],
        "A4"
    );
}

#[tokio::test]
async fn invalid_stored_profile_never_falls_back_to_bootstrap_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let cfg = config(dir.path(), &receiver.uri(), 0);
    let mut profile = settings(&cfg);
    profile.schema_version = 9;
    save_settings(&cfg, &profile);
    assert!(ferriprint::server::App::open(cfg).await.is_err());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn discovery_authentication_requirements_follow_source_policy_without_implicit_admission() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = config(dir.path(), &receiver.uri(), 0);
    cfg.allowed_clients = vec![
        "127.0.0.1/32".parse().unwrap(),
        "127.0.0.2/32".parse().unwrap(),
    ];
    cfg.access.anonymous_clients = vec!["127.0.0.1/32".parse().unwrap()];
    add_user(&mut cfg, "alice", Role::User, PASSWORD);
    let front = Frontend::start(cfg).await;
    let pilot = client();
    let admitted = reqwest::Client::builder()
        .local_address("127.0.0.2".parse::<std::net::IpAddr>().unwrap())
        .build()
        .unwrap();
    let denied = reqwest::Client::builder()
        .local_address("127.0.0.3".parse::<std::net::IpAddr>().unwrap())
        .build()
        .unwrap();
    let endpoint = format!("http://{}/ipp/print/demo", front.address);
    let root = format!("http://{}/", front.address);
    assert_eq!(pilot.get(&root).send().await.unwrap().status(), 200);
    assert_eq!(admitted.get(&root).send().await.unwrap().status(), 401);
    assert_eq!(
        admitted
            .get(&root)
            .basic_auth("alice", Some(PASSWORD))
            .send()
            .await
            .unwrap()
            .status(),
        200
    );
    let gpa = front
        .request(Operation::GetPrinterAttributes, None)
        .to_bytes();
    for (actor, authentication, required) in [
        (&pilot, "none", &["none"][..]),
        (&admitted, "basic", &["username", "password"][..]),
    ] {
        let response = actor
            .post(&endpoint)
            .header("content-type", "application/ipp")
            .body(gpa.clone())
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        let mut reader = tokio_util::io::StreamReader::new(
            response.bytes_stream().map_err(std::io::Error::other),
        );
        let reply = read_metadata(&mut reader, 1024 * 1024).await.unwrap();
        assert_eq!(reply.header().status_code(), StatusCode::SuccessfulOk);
        assert_eq!(
            attr(
                &reply,
                DelimiterTag::PrinterAttributes,
                "uri-authentication-supported"
            )
            .and_then(str_value),
            Some(authentication)
        );
        let value = attr(
            &reply,
            DelimiterTag::PrinterAttributes,
            "auth-info-required",
        )
        .unwrap();
        let requirements: Vec<_> = match value {
            IppValue::Array(values) => values
                .iter()
                .map(|value| str_value(value).unwrap())
                .collect(),
            value => vec![str_value(value).unwrap()],
        };
        assert_eq!(requirements, required);
    }
    let source = format!("http://{}/source", front.address);
    assert_eq!(
        pilot
            .post(&endpoint)
            .header("content-type", "application/ipp")
            .basic_auth("alice", Some("wrong-password"))
            .body(gpa.clone())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        pilot
            .get(&source)
            .basic_auth("alice", Some("wrong-password"))
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    assert_eq!(
        denied
            .post(&endpoint)
            .header("content-type", "application/ipp")
            .body(gpa)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(denied.get(source).send().await.unwrap().status(), 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn real_setup_cli_installs_confirmed_photo_profile_and_rejects_invalid_choices() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let cfg = config(dir.path(), &receiver.uri(), 0);
    let settings = settings(&cfg);
    std::fs::remove_file(cfg.profile_path("demo")).unwrap();
    let bootstrap = dir.path().join("bootstrap.toml");
    std::fs::write(&bootstrap, toml::to_string_pretty(&cfg).unwrap()).unwrap();
    let mut choices = SetupChoices {
        confirmed_native_mappings: true,
        settings,
        previews: vec![JobTemplate {
            media_col: Some(MediaRequest {
                media_type: Some("photographic-glossy".into()),
                source: Some("main".into()),
                ..Default::default()
            }),
            quality: Some(5),
            ..Default::default()
        }],
    };
    let choices_path = dir.path().join("choices.toml");
    std::fs::write(&choices_path, toml::to_string_pretty(&choices).unwrap()).unwrap();
    let output = dir.path().join("installed.toml");
    let binary = env!("CARGO_BIN_EXE_ferriprint");
    let args = vec![
        "setup".to_owned(),
        "install".into(),
        "--bootstrap".into(),
        bootstrap.display().to_string(),
        "--printer".into(),
        "demo".into(),
        "--choices".into(),
        choices_path.display().to_string(),
        "--output".into(),
        output.display().to_string(),
        "--confirm".into(),
    ];
    let original = choices
        .settings
        .sources
        .insert("main".into(), "InventedTray".into())
        .unwrap();
    std::fs::write(&choices_path, toml::to_string_pretty(&choices).unwrap()).unwrap();
    let invalid_args = args.clone();
    let invalid = tokio::task::spawn_blocking(move || {
        Command::new(binary).args(invalid_args).output().unwrap()
    })
    .await
    .unwrap();
    assert!(!invalid.status.success());
    assert!(!output.exists() && !cfg.profile_path("demo").exists());
    choices.settings.sources.insert("main".into(), original);
    std::fs::write(&choices_path, toml::to_string_pretty(&choices).unwrap()).unwrap();
    let installed =
        tokio::task::spawn_blocking(move || Command::new(binary).args(args).output().unwrap())
            .await
            .unwrap();
    assert!(
        installed.status.success(),
        "{}",
        String::from_utf8_lossy(&installed.stderr)
    );
    let checked = Command::new(binary)
        .arg("--config")
        .arg(&output)
        .arg("check")
        .output()
        .unwrap();
    assert!(
        checked.status.success(),
        "{}",
        String::from_utf8_lossy(&checked.stderr)
    );
    let installed_cfg = Config::load(&output).unwrap();
    let front = Frontend::start(installed_cfg).await;
    let mut request = front.request(Operation::PrintJob, None);
    add(
        &mut request,
        JOB,
        "media-col",
        collection(vec![
            ("media-type", keyword("photographic-glossy").unwrap()),
            ("media-source", keyword("main").unwrap()),
        ])
        .unwrap(),
    )
    .unwrap();
    add(&mut request, JOB, "print-quality", IppValue::Enum(5)).unwrap();
    let reply = front.send(request, b"%PDF-1.4\n%%EOF").await;
    assert_eq!(reply.header().status_code(), StatusCode::SuccessfulOk);
    front.app.engine.tick().await.unwrap();
    assert_eq!(receiver.device.jobs.lock()[0].options["MediaType"], "Photo");
    assert_eq!(receiver.device.jobs.lock()[0].options["Quality"], "Fine");
    // Setup cannot overwrite either file, even with --confirm.
    let duplicate = Command::new(binary)
        .args(["setup", "install", "--bootstrap"])
        .arg(&bootstrap)
        .args(["--printer", "demo", "--choices"])
        .arg(&choices_path)
        .arg("--output")
        .arg(&output)
        .arg("--confirm")
        .output()
        .unwrap();
    assert!(!duplicate.status.success());
    let mut bad = choices;
    bad.settings.schema_version = 9;
    let badfile = dir.path().join("bad-profile.toml");
    std::fs::write(&badfile, toml::to_string_pretty(&bad.settings).unwrap()).unwrap();
    assert!(ferriprint::config::ProfileSettings::load(&badfile).is_err());
}

#[tokio::test]
async fn password_cli_never_uses_argv_and_hash_file_has_private_permissions() {
    use base64::Engine;
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let output = dir.path().join("operator.argon2");
    let mut process = Command::new(env!("CARGO_BIN_EXE_ferriprint"))
        .args(["user", "password-hash", "--output"])
        .arg(&output)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    process
        .stdin
        .take()
        .unwrap()
        .write_all(format!("{PASSWORD}\n").as_bytes())
        .unwrap();
    let result = process.wait_with_output().unwrap();
    assert!(result.status.success());
    assert!(!String::from_utf8_lossy(&result.stdout).contains(PASSWORD));
    assert!(!String::from_utf8_lossy(&result.stderr).contains(PASSWORD));
    assert_eq!(
        std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
        0o600
    );
    let access = ferriprint::config::AccessConfig {
        anonymous_trusted_lan: false,
        anonymous_owner: None,
        anonymous_clients: Vec::new(),
        users: vec![ferriprint::config::UserConfig {
            username: "operator".into(),
            role: Role::User,
            password_hash_file: output,
        }],
    };
    let authenticator = ferriprint::auth::Authenticator::open(&access).unwrap();
    let header = |password: &str| {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("operator:{password}"))
        )
    };
    let principal = authenticator.authenticate(&header(PASSWORD)).await.unwrap();
    assert_eq!(principal.username, "operator");
    assert!(
        authenticator
            .authenticate(&header("wrong-isolated-password"))
            .await
            .is_err()
    );
}

#[cfg(target_os = "macos")]
#[tokio::test]
async fn macos_extended_acl_cannot_bypass_private_credential_mode() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), "ipp://127.0.0.1:9/printers/receiver", 0);
    add_user(&mut cfg, "office-user", Role::User, PASSWORD);
    assert!(ferriprint::auth::Authenticator::open(&cfg.access).is_ok());
    let path = &cfg.access.users[0].password_hash_file;
    assert!(
        Command::new("/bin/chmod")
            .args(["+a", "everyone allow read"])
            .arg(path)
            .status()
            .unwrap()
            .success()
    );
    // Darwin extended ACEs can grant access without changing the private mode.
    assert_eq!(
        std::fs::metadata(path).unwrap().permissions().mode() & 0o077,
        0
    );
    assert!(ferriprint::auth::Authenticator::open(&cfg.access).is_err());
}

#[cfg(target_os = "linux")]
#[tokio::test]
async fn service_specific_credential_acl_authenticates_but_untrusted_access_is_rejected() {
    use base64::Engine;
    use std::os::{fd::AsRawFd, unix::fs::PermissionsExt};
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), "ipp://127.0.0.1:9/printers/receiver", 0);
    add_user(&mut cfg, "office-user", Role::User, PASSWORD);
    let file = std::fs::File::open(&cfg.access.users[0].password_hash_file).unwrap();
    let service_uid = unsafe { libc::geteuid() };
    let set_acl = |named_uid: u32,
                   group_permission: u16,
                   other_permission: u16,
                   named_group_permission: u16,
                   named_user_permission: u16| {
        let entries = [
            (1u16, 4u16, u32::MAX),
            (2, named_user_permission, named_uid),
            (4, group_permission, u32::MAX),
            (
                8,
                named_group_permission,
                unsafe { libc::getegid() }.saturating_add(1),
            ),
            (16, 4, u32::MAX),
            (32, other_permission, u32::MAX),
        ];
        let mut acl = [0u8; 52];
        acl[..4].copy_from_slice(&2u32.to_le_bytes());
        for (index, (tag, permission, uid)) in entries.into_iter().enumerate() {
            let offset = 4 + index * 8;
            acl[offset..offset + 2].copy_from_slice(&tag.to_le_bytes());
            acl[offset + 2..offset + 4].copy_from_slice(&permission.to_le_bytes());
            acl[offset + 4..offset + 8].copy_from_slice(&uid.to_le_bytes());
        }
        let result = unsafe {
            libc::fsetxattr(
                file.as_raw_fd(),
                c"system.posix_acl_access".as_ptr(),
                acl.as_ptr().cast(),
                acl.len(),
                0,
            )
        };
        assert_eq!(
            result,
            0,
            "set actual filesystem ACL: {}",
            std::io::Error::last_os_error()
        );
    };
    set_acl(service_uid, 0, 0, 0, 4);
    // The group-class mode contains an ACL mask despite group:: having no access.
    assert_ne!(file.metadata().unwrap().permissions().mode() & 0o070, 0);
    let authenticator = ferriprint::auth::Authenticator::open(&cfg.access).unwrap();
    let header = format!(
        "Basic {}",
        base64::engine::general_purpose::STANDARD.encode(format!("office-user:{PASSWORD}"))
    );
    assert_eq!(
        authenticator.authenticate(&header).await.unwrap().username,
        "office-user"
    );
    set_acl(service_uid.saturating_add(1), 0, 0, 0, 4);
    assert!(ferriprint::auth::Authenticator::open(&cfg.access).is_err());
    set_acl(service_uid, 4, 0, 0, 4);
    assert!(ferriprint::auth::Authenticator::open(&cfg.access).is_err());
    set_acl(service_uid, 0, 4, 0, 4);
    assert!(ferriprint::auth::Authenticator::open(&cfg.access).is_err());
    set_acl(service_uid, 0, 0, 4, 4);
    assert!(ferriprint::auth::Authenticator::open(&cfg.access).is_err());
    set_acl(service_uid.saturating_add(1), 0, 0, 0, 2);
    assert!(ferriprint::auth::Authenticator::open(&cfg.access).is_ok());
    assert_eq!(
        unsafe { libc::fremovexattr(file.as_raw_fd(), c"system.posix_acl_access".as_ptr()) },
        0
    );
    std::fs::set_permissions(
        &cfg.access.users[0].password_hash_file,
        std::fs::Permissions::from_mode(0o440),
    )
    .unwrap();
    assert!(ferriprint::auth::Authenticator::open(&cfg.access).is_err());
}

#[test]
fn anonymous_admission_requires_explicit_subnet_policy_and_owner_distinct_from_users() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), "ipp://127.0.0.1:9/printers/receiver", 0);
    cfg.allowed_clients = vec!["192.0.2.0/24".parse().unwrap()];
    cfg.access.anonymous_clients = vec!["192.0.2.7/32".parse().unwrap()];
    assert!(cfg.validate().is_ok());
    cfg.access.anonymous_clients = vec!["198.51.100.0/24".parse().unwrap()];
    assert!(cfg.validate().is_err());
    cfg.access.anonymous_clients.clear();
    assert!(cfg.validate().is_err());
    cfg.access.anonymous_clients = vec!["192.0.2.7/32".parse().unwrap()];
    cfg.access.users.push(ferriprint::config::UserConfig {
        username: cfg.access.anonymous_owner.clone().unwrap(),
        role: Role::User,
        password_hash_file: dir.path().join("not-read-by-policy-validation"),
    });
    assert!(cfg.validate().is_err());
}
