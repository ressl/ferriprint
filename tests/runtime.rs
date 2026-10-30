// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl
mod support;
use ferriprint::{
    domain::{JobTemplate, MediaRequest},
    jobs::JobEngine,
    profile::PrinterProfile,
    wire::*,
};
use futures_util::TryStreamExt;
use ipp::prelude::*;
use std::{io::Cursor, sync::atomic::Ordering};
use support::*;
const OP: DelimiterTag = DelimiterTag::OperationAttributes;
const JOB: DelimiterTag = DelimiterTag::JobAttributes;
fn job_id(reply: &IppRequestResponse) -> i32 {
    attr(reply, JOB, "job-id").and_then(integer).unwrap()
}
fn success(reply: &IppRequestResponse) {
    assert_eq!(
        reply.header().status_code(),
        StatusCode::SuccessfulOk,
        "{:?}",
        reply.attributes()
    );
}
fn borderless() -> JobTemplate {
    JobTemplate {
        media: Some("iso_a4_210x297mm".into()),
        media_col: Some(MediaRequest {
            width: Some(29700),
            height: Some(42000),
            left: Some(0),
            bottom: Some(0),
            right: Some(0),
            top: Some(0),
            ..Default::default()
        }),
        quality: Some(5),
        ..Default::default()
    }
}
fn resolve(config: &ferriprint::config::Config) -> ferriprint::domain::ResolvedJob {
    PrinterProfile::load(&config.printers[0], &settings(config))
        .unwrap()
        .resolve(&JobTemplate::default(), "application/pdf")
        .unwrap()
}

#[tokio::test]
async fn capabilities_numeric_media_quality_and_no_cups_identity() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(config(dir.path(), &receiver.uri(), 0)).await;
    let response = front
        .send(front.request(Operation::GetPrinterAttributes, None), &[])
        .await;
    success(&response);
    let g = DelimiterTag::PrinterAttributes;
    assert!(attr(&response, g, "cups-version").is_none());
    assert_eq!(
        values(attr(&response, g, "operations-supported").unwrap())
            .iter()
            .map(|v| integer(v).unwrap())
            .collect::<Vec<_>>(),
        [2, 4, 5, 6, 8, 9, 10, 11]
    );
    assert_eq!(
        attr(&response, g, "multiple-document-jobs-supported"),
        Some(&IppValue::Boolean(false))
    );
    assert!(
        string_list(attr(&response, g, "document-format-supported")).contains(&"image/urf".into())
    );
    assert!(string_list(attr(&response, g, "urf-supported")).contains(&"RS300".into()));
    let media = values(attr(&response, g, "media-col-database").unwrap());
    assert_eq!(media.len(), 4);
    assert!(media.iter().any(|v| if let IppValue::Collection(m) = v {
        m.get("media-size-name").and_then(str_value) == Some("iso_a3_297x420mm")
            && m.get("media-left-margin").and_then(integer) == Some(0)
    } else {
        false
    }));
    let printer = front.app.printer("demo").unwrap().read_owned().await;
    let profile = &printer.profile;
    let resolved = profile.resolve(&borderless(), "application/pdf").unwrap();
    assert_eq!(resolved.options["PageSize"], "TA3");
    assert_eq!(resolved.options["Quality"], "Fine");
    assert_eq!(resolved.resolution.x, 300);
    let mut request = front.request(Operation::ValidateJob, None);
    add(
        &mut request,
        JOB,
        "media",
        keyword("iso_a4_210x297mm").unwrap(),
    )
    .unwrap();
    add(
        &mut request,
        JOB,
        "media-col",
        ferriprint::protocol::media_collection(&resolved.media, None, None).unwrap(),
    )
    .unwrap();
    add(&mut request, JOB, "print-quality", IppValue::Enum(5)).unwrap();
    success(&front.send(request, &[]).await);
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn strict_media_margins_constraints_and_mapping_validation() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let cfg = config(dir.path(), &receiver.uri(), 0);
    let profile = PrinterProfile::load(&cfg.printers[0], &settings(&cfg)).unwrap();
    let mut request = borderless();
    request.media_col.as_mut().unwrap().width = Some(42000);
    assert_eq!(
        profile
            .resolve(&request, "application/pdf")
            .unwrap_err()
            .attribute,
        "media-col"
    );
    request = borderless();
    request.media_col.as_mut().unwrap().top = Some(1);
    assert!(profile.resolve(&request, "application/pdf").is_err());
    request = borderless();
    request.sides = Some("two-sided-long-edge".into());
    assert!(profile.resolve(&request, "application/pdf").is_err());
    request = JobTemplate {
        media_col: Some(MediaRequest {
            media_type: Some("photographic-glossy".into()),
            source: Some("tray-1".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert!(profile.resolve(&request, "application/pdf").is_err());
    request = JobTemplate {
        quality: Some(3),
        media_col: Some(MediaRequest {
            media_type: Some("photographic-glossy".into()),
            ..Default::default()
        }),
        ..Default::default()
    };
    assert_eq!(
        profile
            .resolve(&request, "application/pdf")
            .unwrap_err()
            .attribute,
        "print-quality"
    );
    let mut settings = settings(&cfg);
    settings
        .media_types
        .get_mut("stationery")
        .unwrap()
        .normal
        .insert("Quality".into(), "Imaginary".into());
    assert!(PrinterProfile::load(&cfg.printers[0], &settings).is_err());
}
#[tokio::test]
async fn print_is_processing_until_real_upstream_completion_and_cancel_uses_remote_id() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let cfg = config(dir.path(), &receiver.uri(), 0);
    let front = Frontend::start(cfg).await;
    let mut request = front.request(Operation::PrintJob, None);
    let resolved = front
        .app
        .printer("demo")
        .unwrap()
        .read_owned()
        .await
        .profile
        .resolve(&borderless(), "application/pdf")
        .unwrap();
    add(
        &mut request,
        JOB,
        "media-col",
        ferriprint::protocol::media_collection(&resolved.media, Some("main"), Some("stationery"))
            .unwrap(),
    )
    .unwrap();
    add(&mut request, JOB, "print-quality", IppValue::Enum(5)).unwrap();
    add(&mut request, OP, "job-name", name("Übergröße 🖨").unwrap()).unwrap();
    let response = front
        .send(request, b"%PDF-1.7\nnonprinting test document\n%%EOF")
        .await;
    success(&response);
    let id = job_id(&response);
    front.app.engine.tick().await.unwrap();
    let job = front.app.engine.get_job(id).await.unwrap().unwrap();
    assert_eq!(job.state, 5);
    assert_eq!(job.upstream_id, Some(700));
    assert_ne!(id, 700);
    assert!(job.completed_at.is_none());
    let received = receiver.device.jobs.lock()[0].clone();
    assert_eq!(received.options["PageSize"], "TA3");
    assert_eq!(received.options["Quality"], "Fine");
    assert_eq!(received.title, "Übergröße 🖨");
    assert_eq!(received.owner, "ferriprint");
    assert!(received.bytes > 0);
    let canceled = front
        .send(front.request(Operation::CancelJob, Some(id)), &[])
        .await;
    success(&canceled);
    assert_eq!(*receiver.device.cancellations.lock(), [700]);
    front.app.engine.tick().await.unwrap();
    assert_eq!(
        front.app.engine.get_job(id).await.unwrap().unwrap().state,
        7
    );
    assert!(!dir.path().join(format!("spool/{id}.document")).exists());
    let response = front
        .send(front.request(Operation::PrintJob, None), b"%PDF-1.4\n%%EOF")
        .await;
    success(&response);
    let second = job_id(&response);
    front.app.engine.tick().await.unwrap();
    assert_eq!(
        front
            .app
            .engine
            .get_job(second)
            .await
            .unwrap()
            .unwrap()
            .state,
        5
    );
    receiver.complete(701);
    front.app.engine.tick().await.unwrap();
    let job = front.app.engine.get_job(second).await.unwrap().unwrap();
    assert_eq!(job.state, 9);
    assert_eq!(job.impressions_completed, 2);
    assert_eq!(job.sheets_completed, 1);
}
#[tokio::test]
async fn create_without_format_then_jpeg_send_document_and_second_document_rejection() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(config(dir.path(), &receiver.uri(), 0)).await;
    let response = front
        .send(front.request(Operation::CreateJob, None), &[])
        .await;
    success(&response);
    let id = job_id(&response);
    assert_eq!(attr(&response, JOB, "job-state").and_then(integer), Some(4));
    let mut send = front.request(Operation::SendDocument, Some(id));
    add(
        &mut send,
        OP,
        "document-format",
        mime("image/jpeg").unwrap(),
    )
    .unwrap();
    add(&mut send, OP, "last-document", IppValue::Boolean(true)).unwrap();
    success(
        &front
            .send(send, b"\xff\xd8\xff nonprinting JPEG test")
            .await,
    );
    front.app.engine.tick().await.unwrap();
    assert_eq!(receiver.device.jobs.lock()[0].format, "image/jpeg");
    let mut send = front.request(Operation::SendDocument, Some(id));
    add(&mut send, OP, "last-document", IppValue::Boolean(true)).unwrap();
    assert_eq!(
        front.send(send, b"second").await.header().status_code(),
        StatusCode::ClientErrorNotPossible
    );
    let mut send = front.request(Operation::SendDocument, Some(id));
    add(&mut send, OP, "last-document", IppValue::Boolean(false)).unwrap();
    assert_eq!(
        front.send(send, b"second").await.header().status_code(),
        StatusCode::ServerErrorMultipleDocumentJobsNotSupported
    );
}
#[tokio::test]
async fn errors_report_unsupported_attributes_and_reject_foreign_targets() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(config(dir.path(), &receiver.uri(), 0)).await;
    let mut request = front.request(Operation::ValidateJob, None);
    add(
        &mut request,
        JOB,
        "media",
        keyword("iso_a2_420x594mm").unwrap(),
    )
    .unwrap();
    let response = front.send(request, &[]).await;
    assert_eq!(
        response.header().status_code(),
        StatusCode::ClientErrorAttributesOrValuesNotSupported
    );
    assert!(attr(&response, DelimiterTag::UnsupportedAttributes, "media").is_some());
    let mut request = front.request(Operation::ValidateJob, None);
    add(&mut request, JOB, "finishings", IppValue::Enum(4)).unwrap();
    let response = front.send(request, &[]).await;
    assert!(attr(&response, DelimiterTag::UnsupportedAttributes, "finishings").is_some());
    let request = IppRequestResponse::new(
        IppVersion::v2_0(),
        Operation::GetPrinterAttributes,
        Some("ipp://evil.invalid/ipp/print/demo".parse().unwrap()),
    )
    .unwrap();
    assert_eq!(
        front.send(request, &[]).await.header().status_code(),
        StatusCode::ClientErrorNotFound
    );
    let mut request = front.request(Operation::PrintJob, None);
    request.header_mut().request_id = 0;
    assert_eq!(
        front.send(request, b"data").await.header().status_code(),
        StatusCode::ClientErrorBadRequest
    );
    assert_eq!(
        front
            .send(front.request(Operation::PrintUri, None), &[])
            .await
            .header()
            .status_code(),
        StatusCode::ServerErrorOperationNotSupported
    );
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn durable_ids_admission_owner_query_and_cancel_during_upload() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = config(dir.path(), &receiver.uri(), 0);
    cfg.max_queued_jobs = 1;
    cfg.max_document_bytes = 100;
    let engine = JobEngine::open(&cfg).await.unwrap();
    let job = engine
        .create_job(
            "demo",
            &support::principal(&engine, "alice", support::FIXTURE_PASSWORD).await,
            "one",
            "application/pdf",
            resolve(&cfg),
        )
        .await
        .unwrap();
    assert!(
        engine
            .create_job(
                "demo",
                &support::principal(&engine, "alice", support::FIXTURE_PASSWORD).await,
                "full",
                "application/pdf",
                resolve(&cfg)
            )
            .await
            .is_err()
    );
    assert!(
        engine
            .cancel_job(
                job.id,
                &support::principal(&engine, "mallory", support::FIXTURE_PASSWORD).await
            )
            .await
            .is_err()
    );
    let (mut writer, reader) = tokio::io::duplex(8);
    let e = engine.clone();
    let uploading_principal = support::principal(&engine, "alice", support::FIXTURE_PASSWORD).await;
    let upload = tokio::spawn(async move {
        e.ingest_document(job.id, &uploading_principal, reader)
            .await
    });
    use tokio::io::AsyncWriteExt;
    writer
        .write_all(b"partial document beyond buffer capacity")
        .await
        .unwrap();
    engine
        .cancel_job(
            job.id,
            &support::principal(&engine, "alice", support::FIXTURE_PASSWORD).await,
        )
        .await
        .unwrap();
    drop(writer);
    assert!(upload.await.unwrap().is_err());
    assert_eq!(engine.get_job(job.id).await.unwrap().unwrap().state, 7);
    let origin = engine.time_origin();
    drop(engine);
    let engine = JobEngine::open(&cfg).await.unwrap();
    assert_eq!(engine.time_origin(), origin);
    let next = engine
        .create_job(
            "demo",
            &support::principal(&engine, "bob", support::FIXTURE_PASSWORD).await,
            "two",
            "application/pdf",
            resolve(&cfg),
        )
        .await
        .unwrap();
    assert!(next.id > job.id);
    assert_eq!(engine.active_count("demo").await.unwrap(), 1);
    assert!(
        engine
            .query_jobs("demo", Some("alice"), "not-completed", 10)
            .await
            .unwrap()
            .is_empty()
    );
    assert!(
        engine
            .ingest_document(
                next.id,
                &support::principal(&engine, "bob", support::FIXTURE_PASSWORD).await,
                Cursor::new(vec![0; 101])
            )
            .await
            .is_err()
    );
    assert_eq!(engine.get_job(next.id).await.unwrap().unwrap().state, 8);
    assert_eq!(engine.active_count("demo").await.unwrap(), 0);
}
#[tokio::test]
async fn explicit_rejection_aborts_but_timeout_never_resubmits_and_can_be_abandoned() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let cfg = config(dir.path(), &receiver.uri(), 0);
    let engine = JobEngine::open(&cfg).await.unwrap();
    receiver.device.mode.store(1, Ordering::SeqCst);
    let job = engine
        .create_job(
            "demo",
            &support::principal(&engine, "anonymous", support::FIXTURE_PASSWORD).await,
            "reject",
            "application/pdf",
            resolve(&cfg),
        )
        .await
        .unwrap();
    engine
        .ingest_document(
            job.id,
            &support::principal(&engine, "anonymous", support::FIXTURE_PASSWORD).await,
            Cursor::new(b"document"),
        )
        .await
        .unwrap();
    engine.tick().await.unwrap();
    let job = engine.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(job.state, 8);
    assert_eq!(job.reasons, ["job-rejected-by-upstream"]);
    receiver.device.mode.store(2, Ordering::SeqCst);
    let uncertain = engine
        .create_job(
            "demo",
            &support::principal(&engine, "anonymous", support::FIXTURE_PASSWORD).await,
            "timeout",
            "application/pdf",
            resolve(&cfg),
        )
        .await
        .unwrap();
    engine
        .ingest_document(
            uncertain.id,
            &support::principal(&engine, "anonymous", support::FIXTURE_PASSWORD).await,
            Cursor::new(b"document"),
        )
        .await
        .unwrap();
    engine.tick().await.unwrap();
    assert_eq!(
        engine.get_job(uncertain.id).await.unwrap().unwrap().phase,
        "uncertain"
    );
    for _ in 0..3 {
        engine.tick().await.unwrap();
    }
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 2);
    assert!(
        dir.path()
            .join(format!("spool/{}.document", uncertain.id))
            .exists()
    );
    drop(engine);
    let engine = JobEngine::open(&cfg).await.unwrap();
    engine.tick().await.unwrap();
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 2);
    assert!(
        engine
            .cancel_job(uncertain.id, &engine.auth.anonymous().unwrap())
            .await
            .unwrap_err()
            .to_string()
            .contains("unknown upstream")
    );
    let abandoned = engine.abandon_uncertain(uncertain.id).await.unwrap();
    assert_eq!(abandoned.state, 8);
    assert!(abandoned.message.contains("MAY STILL PRINT"));
    assert_eq!(engine.active_count("demo").await.unwrap(), 0);
}
#[tokio::test]
async fn cancel_while_submitting_is_forwarded_after_remote_identity_arrives() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    receiver.device.mode.store(3, Ordering::SeqCst);
    let mut cfg = config(dir.path(), &receiver.uri(), 0);
    cfg.backend_timeout_secs = 5;
    let engine = JobEngine::open(&cfg).await.unwrap();
    let job = engine
        .create_job(
            "demo",
            &support::principal(&engine, "alice", support::FIXTURE_PASSWORD).await,
            "race",
            "application/pdf",
            resolve(&cfg),
        )
        .await
        .unwrap();
    engine
        .ingest_document(
            job.id,
            &support::principal(&engine, "alice", support::FIXTURE_PASSWORD).await,
            Cursor::new(b"document"),
        )
        .await
        .unwrap();
    let e = engine.clone();
    let tick = tokio::spawn(async move { e.tick().await });
    receiver.device.accepted.notified().await;
    assert_eq!(
        engine
            .cancel_job(
                job.id,
                &support::principal(&engine, "alice", support::FIXTURE_PASSWORD).await
            )
            .await
            .unwrap()
            .state,
        6
    );
    receiver.device.gate.notify_one();
    tick.await.unwrap().unwrap();
    assert_eq!(*receiver.device.cancellations.lock(), [700]);
    engine.tick().await.unwrap();
    assert_eq!(engine.get_job(job.id).await.unwrap().unwrap().state, 7);
}
#[tokio::test]
async fn crash_during_submission_stops_without_duplicate_and_known_jobs_reconcile() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    receiver.device.mode.store(3, Ordering::SeqCst);
    let cfg = config(dir.path(), &receiver.uri(), 0);
    let engine = JobEngine::open(&cfg).await.unwrap();
    let job = engine
        .create_job(
            "demo",
            &support::principal(&engine, "anonymous", support::FIXTURE_PASSWORD).await,
            "crash",
            "application/pdf",
            resolve(&cfg),
        )
        .await
        .unwrap();
    engine
        .ingest_document(
            job.id,
            &support::principal(&engine, "anonymous", support::FIXTURE_PASSWORD).await,
            Cursor::new(b"document"),
        )
        .await
        .unwrap();
    let e = engine.clone();
    let tick = tokio::spawn(async move { e.tick().await });
    receiver.device.accepted.notified().await;
    tick.abort();
    let _ = tick.await;
    drop(engine);
    let engine = JobEngine::open(&cfg).await.unwrap();
    let recovered = engine.get_job(job.id).await.unwrap().unwrap();
    assert_eq!(recovered.phase, "uncertain");
    engine.tick().await.unwrap();
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 1);
    receiver.device.gate.notify_one();
    engine.abandon_uncertain(job.id).await.unwrap();
    receiver.device.mode.store(0, Ordering::SeqCst);
    let second = engine
        .create_job(
            "demo",
            &support::principal(&engine, "anonymous", support::FIXTURE_PASSWORD).await,
            "known",
            "application/pdf",
            resolve(&cfg),
        )
        .await
        .unwrap();
    engine
        .ingest_document(
            second.id,
            &support::principal(&engine, "anonymous", support::FIXTURE_PASSWORD).await,
            Cursor::new(b"document"),
        )
        .await
        .unwrap();
    engine.tick().await.unwrap();
    drop(engine);
    receiver.complete(701);
    let engine = JobEngine::open(&cfg).await.unwrap();
    engine.tick().await.unwrap();
    assert_eq!(engine.get_job(second.id).await.unwrap().unwrap().state, 9);
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn dropped_upload_aborts_and_restart_removes_orphan_payloads() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let cfg = config(dir.path(), &receiver.uri(), 0);
    let engine = JobEngine::open(&cfg).await.unwrap();
    let job = engine
        .create_job(
            "demo",
            &support::principal(&engine, "alice", support::FIXTURE_PASSWORD).await,
            "upload",
            "application/pdf",
            resolve(&cfg),
        )
        .await
        .unwrap();
    let (mut writer, reader) = tokio::io::duplex(8);
    let e = engine.clone();
    let uploading_principal = support::principal(&engine, "alice", support::FIXTURE_PASSWORD).await;
    let upload = tokio::spawn(async move {
        e.ingest_document(job.id, &uploading_principal, reader)
            .await
    });
    use tokio::io::AsyncWriteExt;
    writer
        .write_all(b"partial document beyond buffer capacity")
        .await
        .unwrap();
    upload.abort();
    let _ = upload.await;
    assert_eq!(engine.get_job(job.id).await.unwrap().unwrap().state, 8);
    drop(writer);
    drop(engine);
    std::fs::write(dir.path().join("spool/99999.part"), b"partial").unwrap();
    std::fs::write(
        dir.path().join(format!("spool/{}.document", job.id)),
        b"orphan terminal",
    )
    .unwrap();
    let engine = JobEngine::open(&cfg).await.unwrap();
    assert!(!dir.path().join("spool/99999.part").exists());
    assert!(
        !dir.path()
            .join(format!("spool/{}.document", job.id))
            .exists()
    );
    engine.tick().await.unwrap();
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn metadata_bounds_language_strings_unicode_and_transport_urls() {
    assert_eq!(
        http_url(&"ipp://[::1]/printers/x".parse().unwrap())
            .unwrap()
            .as_str(),
        "http://[::1]:631/printers/x"
    );
    assert_eq!(
        http_url(&"ipps://host:443/a%20b".parse().unwrap())
            .unwrap()
            .as_str(),
        "https://host/a%20b"
    );
    assert_eq!(bounded("ééé", 5), "éé");
    let request = IppRequestResponse::new(
        IppVersion::v2_0(),
        Operation::GetPrinterAttributes,
        Some("ipp://localhost/a".parse().unwrap()),
    )
    .unwrap();
    let mut bytes = request.to_bytes().to_vec();
    bytes.pop();
    bytes.extend_from_slice(&[0x36, 0, 4, b'n', b'a', b'm', b'e', 0, 4, 0, 255, 0, 0, 3]);
    assert!(read_metadata(&mut Cursor::new(bytes), 1024).await.is_err());
    let mut bytes = request.to_bytes().to_vec();
    bytes.pop();
    bytes.extend_from_slice(&[0x41, 0, 4, b'n', b'a', b'm', b'e', 0xff, 0xff]);
    assert!(read_metadata(&mut Cursor::new(bytes), 1024).await.is_err());
}
#[tokio::test]
async fn authenticated_principal_cannot_be_spoofed_by_requesting_user_name() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = config(dir.path(), &receiver.uri(), 0);
    let secret = "isolated-test-password";
    cfg.access.anonymous_trusted_lan = false;
    cfg.access.anonymous_owner = None;
    cfg.access.anonymous_clients.clear();
    add_user(&mut cfg, "operator", ferriprint::config::Role::User, secret);
    let front = Frontend::start(cfg).await;
    let mut request = front.request(Operation::CreateJob, None);
    add(
        &mut request,
        OP,
        "requesting-user-name",
        name("victim").unwrap(),
    )
    .unwrap();
    let client = reqwest::Client::new();
    let endpoint = format!("http://{}/ipp/print/demo", front.address);
    assert_eq!(
        client
            .post(&endpoint)
            .header("content-type", "application/ipp")
            .body(request.to_bytes())
            .send()
            .await
            .unwrap()
            .status(),
        401
    );
    let response = client
        .post(&endpoint)
        .header("content-type", "application/ipp")
        .basic_auth("operator", Some(secret))
        .body(request.to_bytes())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut reader =
        tokio_util::io::StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
    let response = read_metadata(&mut reader, 1024 * 1024).await.unwrap();
    success(&response);
    let job = front
        .app
        .engine
        .get_job(job_id(&response))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(job.owner, "operator");
    assert!(
        front
            .app
            .engine
            .cancel_job(
                job.id,
                &support::principal(&front.app.engine, "mallory", support::FIXTURE_PASSWORD).await
            )
            .await
            .is_err()
    );
}

/// RFC 8011 Appendix C.1: with "ipp-attribute-fidelity" 'false' (also the
/// default) "the Printer MUST always accept the request by ignoring unsupported
/// Job Template attributes"; with 'true' it "MUST reject the request".
/// These exercise real operation responses, not just the parser.
#[tokio::test]
async fn best_effort_fidelity_ignores_unsupported_attributes_and_strict_fidelity_rejects() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(config(dir.path(), &receiver.uri(), 0)).await;

    // (a) Unknown/substitutable attribute + fidelity=false: accepted, and the
    // ignored attribute is reported in the Unsupported Attributes group.
    let mut request = front.request(Operation::ValidateJob, None);
    add(
        &mut request,
        OP,
        "ipp-attribute-fidelity",
        IppValue::Boolean(false),
    )
    .unwrap();
    add(&mut request, JOB, "job-priority", IppValue::Integer(50)).unwrap();
    add(&mut request, JOB, "finishings", IppValue::Enum(3)).unwrap();
    add(
        &mut request,
        JOB,
        "media",
        keyword("iso_a4_210x297mm").unwrap(),
    )
    .unwrap();
    let response = front.send(request, &[]).await;
    assert_eq!(
        response.header().status_code(),
        StatusCode::SuccessfulOkIgnoredOrSubstitutedAttributes,
        "{:?}",
        response.attributes()
    );
    assert!(
        attr(
            &response,
            DelimiterTag::UnsupportedAttributes,
            "job-priority"
        )
        .is_some()
    );
    assert!(attr(&response, DelimiterTag::UnsupportedAttributes, "finishings").is_some());

    // (b) The same attributes with fidelity=true: rejected as unsupported.
    let mut request = front.request(Operation::ValidateJob, None);
    add(
        &mut request,
        OP,
        "ipp-attribute-fidelity",
        IppValue::Boolean(true),
    )
    .unwrap();
    add(&mut request, JOB, "job-priority", IppValue::Integer(50)).unwrap();
    let response = front.send(request, &[]).await;
    assert_eq!(
        response.header().status_code(),
        StatusCode::ClientErrorAttributesOrValuesNotSupported
    );
    assert!(
        attr(
            &response,
            DelimiterTag::UnsupportedAttributes,
            "job-priority"
        )
        .is_some()
    );

    // (c) number-up=2 is not implemented; best-effort means one-up, not a rejection.
    let mut request = front.request(Operation::ValidateJob, None);
    add(
        &mut request,
        OP,
        "ipp-attribute-fidelity",
        IppValue::Boolean(false),
    )
    .unwrap();
    add(&mut request, JOB, "number-up", IppValue::Integer(2)).unwrap();
    let response = front.send(request, &[]).await;
    assert_eq!(
        response.header().status_code(),
        StatusCode::SuccessfulOkIgnoredOrSubstitutedAttributes,
        "{:?}",
        response.attributes()
    );
    assert!(attr(&response, DelimiterTag::UnsupportedAttributes, "number-up").is_some());

    // (d) Paper identity is never substituted, even for best-effort jobs: an
    // unknown size must fail rather than silently printing on other paper.
    let mut request = front.request(Operation::ValidateJob, None);
    add(
        &mut request,
        OP,
        "ipp-attribute-fidelity",
        IppValue::Boolean(false),
    )
    .unwrap();
    add(
        &mut request,
        JOB,
        "media",
        keyword("iso_a2_420x594mm").unwrap(),
    )
    .unwrap();
    assert_eq!(
        front.send(request, &[]).await.header().status_code(),
        StatusCode::ClientErrorAttributesOrValuesNotSupported
    );

    // The same applies to copies outside the supported range.
    let mut request = front.request(Operation::ValidateJob, None);
    add(
        &mut request,
        OP,
        "ipp-attribute-fidelity",
        IppValue::Boolean(false),
    )
    .unwrap();
    add(&mut request, JOB, "copies", IppValue::Integer(1000)).unwrap();
    assert_eq!(
        front.send(request, &[]).await.header().status_code(),
        StatusCode::ClientErrorAttributesOrValuesNotSupported
    );
}

/// A best-effort Print-Job must actually create and submit the job with the
/// substituted values, not merely return a success status.
#[tokio::test]
async fn best_effort_print_job_is_created_and_submitted_with_substituted_values() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(config(dir.path(), &receiver.uri(), 0)).await;
    let mut request = front.request(Operation::PrintJob, None);
    add(
        &mut request,
        OP,
        "document-format",
        mime("application/pdf").unwrap(),
    )
    .unwrap();
    add(
        &mut request,
        OP,
        "ipp-attribute-fidelity",
        IppValue::Boolean(false),
    )
    .unwrap();
    add(&mut request, JOB, "job-priority", IppValue::Integer(50)).unwrap();
    add(&mut request, JOB, "finishings", IppValue::Enum(3)).unwrap();
    add(&mut request, JOB, "number-up", IppValue::Integer(2)).unwrap();
    add(&mut request, JOB, "copies", IppValue::Integer(2)).unwrap();
    add(
        &mut request,
        JOB,
        "sides",
        keyword("two-sided-long-edge").unwrap(),
    )
    .unwrap();
    let response = front
        .send(request, b"%PDF-1.4 nonprinting best-effort")
        .await;
    assert_eq!(
        response.header().status_code(),
        StatusCode::SuccessfulOkIgnoredOrSubstitutedAttributes,
        "{:?}",
        response.attributes()
    );
    let id = job_id(&response);
    front.app.engine.tick().await.unwrap();
    let received = receiver.device.jobs.lock().clone();
    assert_eq!(received.len(), 1, "exactly one upstream job");
    assert_eq!(received[0].copies, 2, "supported copies are still applied");
    assert_eq!(
        received[0].options.get("Duplex").map(String::as_str),
        Some("DuplexNoTumble")
    );
    assert!(
        !received[0].options.contains_key("number-up"),
        "number-up is not forwarded as a PPD option"
    );
    let job = front.app.engine.get_job(id).await.unwrap().unwrap();
    assert_eq!(job.resolved.copies, 2);
}
