// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl
mod support;
use ferriprint::{profile::PrinterProfile, protocol, wire::*};
use futures_util::{StreamExt, TryStreamExt};
use ipp::prelude::*;
use std::{
    io::{Cursor, Write},
    sync::atomic::Ordering,
};
use support::*;
use tokio::io::AsyncReadExt;

fn target_request(operation: Operation, attribute: &str, target: &str) -> IppRequestResponse {
    let mut request = IppRequestResponse::new(IppVersion::v2_0(), operation, None).unwrap();
    add(
        &mut request,
        DelimiterTag::OperationAttributes,
        attribute,
        uri(target).unwrap(),
    )
    .unwrap();
    request
}

#[tokio::test]
async fn native_dns_root_dot_routes_printer_and_job_targets_without_changing_identity() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = config(dir.path(), &receiver.uri(), 0);
    cfg.public_url = "ipp://print.example:8631/".parse().unwrap();
    let front = Frontend::start(cfg).await;
    let canonical = front.printer_uri();
    let expected_uuid = format!(
        "urn:uuid:{}",
        uuid::Uuid::new_v5(&uuid::Uuid::NAMESPACE_URL, canonical.as_bytes())
    );
    for target in [
        canonical.as_str(),
        "ipp://print.example.:8631/ipp/print/demo",
        "ipp://PRINT.EXAMPLE.:8631/ipp/print/demo",
    ] {
        let reply = front
            .send(
                target_request(Operation::GetPrinterAttributes, "printer-uri", target),
                &[],
            )
            .await;
        assert_eq!(reply.header().status_code(), StatusCode::SuccessfulOk);
        assert_eq!(
            attr(
                &reply,
                DelimiterTag::PrinterAttributes,
                "printer-uri-supported"
            )
            .and_then(str_value),
            Some(canonical.as_str())
        );
        assert_eq!(
            attr(&reply, DelimiterTag::PrinterAttributes, "printer-uuid").and_then(str_value),
            Some(expected_uuid.as_str())
        );
        assert_eq!(
            front
                .send(
                    target_request(Operation::ValidateJob, "printer-uri", target),
                    &[],
                )
                .await
                .header()
                .status_code(),
            StatusCode::SuccessfulOk
        );
    }
    // The local receiver never prints; create only an empty held frontend job.
    let created = front
        .send(front.request(Operation::CreateJob, None), &[])
        .await;
    let id = attr(&created, DelimiterTag::JobAttributes, "job-id")
        .and_then(integer)
        .unwrap();
    let canonical_job = format!("{canonical}/jobs/{id}");
    for target in [
        canonical_job.clone(),
        format!("ipp://PRINT.EXAMPLE.:8631/ipp/print/demo/jobs/{id}"),
    ] {
        let reply = front
            .send(
                target_request(Operation::GetJobAttributes, "job-uri", &target),
                &[],
            )
            .await;
        assert_eq!(reply.header().status_code(), StatusCode::SuccessfulOk);
        assert_eq!(
            attr(&reply, DelimiterTag::JobAttributes, "job-uri").and_then(str_value),
            Some(canonical_job.as_str())
        );
        assert_eq!(
            attr(&reply, DelimiterTag::JobAttributes, "job-printer-uri").and_then(str_value),
            Some(canonical.as_str())
        );
    }
    let request = target_request(
        Operation::GetJobAttributes,
        "job-uri",
        &format!("ipp://print.example.:8631/ipp/print/demo/jobs/{id}"),
    );
    let response = reqwest::Client::new()
        .post(format!(
            "http://{}/ipp/print/demo/jobs/{}",
            front.address,
            id + 1
        ))
        .header("content-type", "application/ipp")
        .body(request.to_bytes())
        .send()
        .await
        .unwrap();
    let reply = read_metadata(&mut Cursor::new(response.bytes().await.unwrap()), 8192)
        .await
        .unwrap();
    assert_eq!(
        reply.header().status_code(),
        StatusCode::ClientErrorBadRequest
    );
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn dns_uri_identity_preserves_scheme_authority_and_raw_resource_boundaries() {
    const CANONICAL: &str = "ipps://print.example:8631/ipp/print/demo";
    for target in [
        "ipp://print.example.:8631/ipp/print/demo",
        "https://print.example.:8631/ipp/print/demo",
        "ipps://evil.example.:8631/ipp/print/demo",
        "ipps://print.example.evil.invalid:8631/ipp/print/demo",
        "ipps://192.0.2.1:8631/ipp/print/demo",
        "ipps://user@print.example.:8631/ipp/print/demo",
        "ipps://@print.example.:8631/ipp/print/demo",
        "ipps://print.example@evil.example:8631/ipp/print/demo",
        "ipps://print.example.:8632/ipp/print/demo",
        "ipps://print.example./ipp/print/demo",
        "ipps://print.example.:/ipp/print/demo",
        "ipps://print.example.:08631/ipp/print/demo",
        "ipps://print.example..:8631/ipp/print/demo",
        "ipps://print%2eexample:8631/ipp/print/demo",
        "ipps://print.example.:8631/ipp/print/other",
        "ipps://print.example.:8631/ipp/print/demo/",
        "ipps://print.example.:8631/ipp/print/x/../demo",
        "ipps://print.example.:8631/ipp/print/%64emo",
        "ipps://print.example.:8631/ipp/print/Demo",
        "ipps://print.example.:8631/ipp/print/demo?x=1",
        "ipps://print.example.:8631/ipp/print/demo?",
        "ipps://print.example.:8631/ipp/print/demo#fragment",
        "ipps://print.example.:8631/ipp/print/demo#",
    ] {
        for (operation, attribute, target) in [
            (
                Operation::GetPrinterAttributes,
                "printer-uri",
                target.to_owned(),
            ),
            (Operation::ValidateJob, "printer-uri", target.to_owned()),
            (
                Operation::GetJobAttributes,
                "job-uri",
                format!("{target}/jobs/1"),
            ),
        ] {
            let request = target_request(operation, attribute, &target);
            let decoded = read_metadata(&mut Cursor::new(request.to_bytes()), 8192)
                .await
                .unwrap();
            let error =
                protocol::validate_envelope(&Metadata::from(decoded), CANONICAL, None).unwrap_err();
            assert_eq!(error.status, StatusCode::ClientErrorNotFound, "{target}");
        }
    }
    for (attribute, operation, target, expected) in [
        (
            "printer-uri",
            Operation::ValidateJob,
            "ipps://PRINT.EXAMPLE.:8631/ipp/print/demo",
            None,
        ),
        (
            "job-uri",
            Operation::GetJobAttributes,
            "ipps://PRINT.EXAMPLE.:8631/ipp/print/demo/jobs/1",
            Some(1),
        ),
    ] {
        let request = target_request(operation, attribute, target);
        assert_eq!(
            protocol::validate_envelope(&Metadata::from(request), CANONICAL, None).unwrap(),
            expected
        );
    }
    for number in ["01", "+1"] {
        let request = target_request(
            Operation::GetJobAttributes,
            "job-uri",
            &format!("ipps://print.example.:8631/ipp/print/demo/jobs/{number}"),
        );
        let error =
            protocol::validate_envelope(&Metadata::from(request), CANONICAL, None).unwrap_err();
        assert_eq!(error.status, StatusCode::ClientErrorBadRequest);
    }
    let request = target_request(
        Operation::ValidateJob,
        "printer-uri",
        "ipps://192.0.2.1.:8631/ipp/print/demo",
    );
    let error = protocol::validate_envelope(
        &Metadata::from(request),
        "ipps://192.0.2.1:8631/ipp/print/demo",
        None,
    )
    .unwrap_err();
    assert_eq!(error.status, StatusCode::ClientErrorNotFound);
}

#[tokio::test]
async fn malformed_collection_cannot_turn_into_default_media() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(config(dir.path(), &receiver.uri(), 0)).await;
    let request = front.request(Operation::PrintJob, None);
    let mut prefix = request.to_bytes().to_vec();
    prefix.pop();
    prefix.push(2);
    // media-col: beginCollection; memberAttrName(media-size); endCollection, with no value.
    prefix.extend_from_slice(&[0x34, 0, 9]);
    prefix.extend_from_slice(b"media-col");
    prefix.extend_from_slice(&[0, 0, 0x4a, 0, 0, 0, 10]);
    prefix.extend_from_slice(b"media-size");
    prefix.extend_from_slice(&[0x37, 0, 0, 0, 0, 3]);
    assert!(
        read_metadata(&mut Cursor::new(prefix.clone()), 1024)
            .await
            .is_err()
    );
    let response = reqwest::Client::new()
        .post(format!("http://{}/ipp/print/demo", front.address))
        .header("content-type", "application/ipp")
        .body(prefix)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 400);
    assert_eq!(front.app.engine.active_count("demo").await.unwrap(), 0);
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 0);
    // Member name outside a collection and a nonzero member attribute name are also malformed.
    for bytes in [
        vec![0x4a, 0, 0, 0, 1, b'x', 3],
        vec![
            0x34, 0, 1, b'c', 0, 0, 0x4a, 0, 1, b'x', 0, 1, b'y', 0x37, 0, 0, 0, 0, 3,
        ],
    ] {
        let request = front.request(Operation::ValidateJob, None);
        let mut prefix = request.to_bytes().to_vec();
        prefix.pop();
        prefix.extend(bytes);
        assert!(read_metadata(&mut Cursor::new(prefix), 1024).await.is_err());
    }
}
#[tokio::test]
async fn nested_and_array_collection_values_survive_codec_boundary() {
    let mut request = IppRequestResponse::new(
        IppVersion::v2_0(),
        Operation::ValidateJob,
        Some("ipp://localhost/printer".parse().unwrap()),
    )
    .unwrap();
    let nested = collection(vec![
        ("x-dimension", IppValue::Integer(29700)),
        ("y-dimension", IppValue::Integer(42000)),
    ])
    .unwrap();
    let value = collection(vec![
        ("media-size", nested),
        (
            "media-left-margin",
            IppValue::Array(vec![IppValue::Integer(0), IppValue::Integer(300)]),
        ),
        (
            "alternates",
            IppValue::Array(vec![
                collection(vec![("n", IppValue::Integer(1))]).unwrap(),
                collection(vec![("n", IppValue::Integer(2))]).unwrap(),
            ]),
        ),
    ])
    .unwrap();
    add(
        &mut request,
        DelimiterTag::JobAttributes,
        "media-col",
        value.clone(),
    )
    .unwrap();
    let decoded = read_metadata(&mut Cursor::new(request.to_bytes()), 8192)
        .await
        .unwrap();
    assert_eq!(
        attr(&decoded, DelimiterTag::JobAttributes, "media-col"),
        Some(&value)
    );
    // A legal setOf integer is not a legal scalar margin; it must fail, not retain only zero.
    let metadata = Metadata::from(decoded);
    assert!(ferriprint::protocol::template(&metadata).is_err());
}
#[tokio::test]
async fn http_and_native_transfer_stay_streaming_for_large_documents() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(config(dir.path(), &receiver.uri(), 0)).await;
    let request = front.request(Operation::PrintJob, None);
    let prefix = request.to_bytes();
    let length = 3 * 1024 * 1024 + 17;
    let document = tokio_util::io::ReaderStream::new(tokio::io::repeat(0x42).take(length));
    let stream =
        futures_util::stream::once(async move { Ok::<_, std::io::Error>(prefix) }).chain(document);
    let response = reqwest::Client::new()
        .post(format!("http://{}/ipp/print/demo", front.address))
        .header("content-type", "application/ipp")
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut reader =
        tokio_util::io::StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
    let response = read_metadata(&mut reader, 65536).await.unwrap();
    let id = attr(&response, DelimiterTag::JobAttributes, "job-id")
        .and_then(integer)
        .unwrap();
    assert_eq!(
        front
            .app
            .engine
            .get_job(id)
            .await
            .unwrap()
            .unwrap()
            .document_bytes,
        length
    );
    front.app.engine.tick().await.unwrap();
    assert_eq!(receiver.device.jobs.lock()[0].bytes, length);
}
#[tokio::test]
async fn startup_intersects_formats_and_frontend_quality_owns_urf_pq() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    receiver.device.mode.store(4, Ordering::SeqCst);
    let front = Frontend::start(config(dir.path(), &receiver.uri(), 0)).await;
    let reply = front
        .send(front.request(Operation::GetPrinterAttributes, None), &[])
        .await;
    assert_eq!(
        string_list(attr(
            &reply,
            DelimiterTag::PrinterAttributes,
            "document-format-supported"
        )),
        ["application/pdf"]
    );
    assert!(attr(&reply, DelimiterTag::PrinterAttributes, "urf-supported").is_none());
    drop(front);
    receiver.device.mode.store(0, Ordering::SeqCst);
    let dir = tempfile::tempdir().unwrap();
    let front = Frontend::start(config(dir.path(), &receiver.uri(), 0)).await;
    let reply = front
        .send(front.request(Operation::GetPrinterAttributes, None), &[])
        .await;
    let urf = string_list(attr(
        &reply,
        DelimiterTag::PrinterAttributes,
        "urf-supported",
    ));
    assert!(urf.contains(&"PQ3-4-5".into()));
    assert!(!urf.contains(&"PQ4".into()));
    assert!(urf.contains(&"RS300".into()));
}
#[test]
fn gzip_classic_ppd_and_transport_authentication_policy() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = config(dir.path(), "ipp://127.0.0.1:631/printers/test", 0);
    let mut ppd = include_bytes!("fixtures/native.ppd").to_vec();
    ppd.extend_from_slice(b"\n*NickName: \"Caf\xe9 test\"\n");
    let file = dir.path().join("latin.ppd.gz");
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::default());
    gzip.write_all(&ppd).unwrap();
    std::fs::write(&file, gzip.finish().unwrap()).unwrap();
    cfg.printers[0].ppd = file;
    assert_eq!(
        PrinterProfile::load(&cfg.printers[0], &settings(&cfg))
            .unwrap()
            .capabilities()
            .make_and_model,
        "Café test"
    );
    cfg.printers[0].upstream_auth = Some(ferriprint::config::AuthConfig {
        username: "native".into(),
        password_env: "PATH".into(),
    });
    assert!(cfg.validate().is_ok());
    cfg.printers[0].upstream_uri = "ipp://remote.example/printers/test".parse().unwrap();
    assert!(cfg.validate().is_err());
    cfg.printers[0].upstream_uri = "ipps://remote.example/printers/test".parse().unwrap();
    assert!(cfg.validate().is_ok());
    cfg.listen = "0.0.0.0:8631".parse().unwrap();
    assert!(cfg.validate().is_err());
}
#[tokio::test]
async fn job_and_printer_times_share_the_durable_origin() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(config(dir.path(), &receiver.uri(), 0)).await;
    let created = front
        .send(front.request(Operation::CreateJob, None), &[])
        .await;
    let creation = attr(&created, DelimiterTag::JobAttributes, "time-at-creation")
        .and_then(integer)
        .unwrap();
    let job_up = attr(&created, DelimiterTag::JobAttributes, "job-printer-up-time")
        .and_then(integer)
        .unwrap();
    let printer = front
        .send(front.request(Operation::GetPrinterAttributes, None), &[])
        .await;
    let printer_up = attr(&printer, DelimiterTag::PrinterAttributes, "printer-up-time")
        .and_then(integer)
        .unwrap();
    assert!(creation <= job_up && job_up <= printer_up);
    assert!(creation < 60);
}

#[tokio::test]
async fn listing_filters_job_uris_nonutf8_titles_and_private_source_offer() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(config(dir.path(), &receiver.uri(), 0)).await;
    let mut old = front.request(Operation::GetPrinterAttributes, None);
    old.header_mut().version = IppVersion::v1_1();
    assert_eq!(
        front.send(old, &[]).await.header().version,
        IppVersion::v1_1()
    );
    let first = front
        .send(front.request(Operation::CreateJob, None), &[])
        .await;
    let first_id = attr(&first, DelimiterTag::JobAttributes, "job-id")
        .and_then(integer)
        .unwrap();
    front
        .send(front.request(Operation::CancelJob, Some(first_id)), &[])
        .await;
    let second = front
        .send(front.request(Operation::CreateJob, None), &[])
        .await;
    let second_id = attr(&second, DelimiterTag::JobAttributes, "job-id")
        .and_then(integer)
        .unwrap();
    let jobs = front
        .send(front.request(Operation::GetJobs, None), &[])
        .await;
    assert_eq!(
        jobs.attributes()
            .groups_of(DelimiterTag::JobAttributes)
            .count(),
        1
    );
    assert_eq!(
        attr(&jobs, DelimiterTag::JobAttributes, "job-id").and_then(integer),
        Some(second_id)
    );
    let mut completed = front.request(Operation::GetJobs, None);
    add(
        &mut completed,
        DelimiterTag::OperationAttributes,
        "which-jobs",
        keyword("completed").unwrap(),
    )
    .unwrap();
    let completed = front.send(completed, &[]).await;
    assert_eq!(
        attr(&completed, DelimiterTag::JobAttributes, "job-id").and_then(integer),
        Some(first_id)
    );
    let job = front
        .send(
            front.request(Operation::GetJobAttributes, Some(second_id)),
            &[],
        )
        .await;
    assert_eq!(
        attr(&job, DelimiterTag::JobAttributes, "job-state").and_then(integer),
        Some(4)
    );
    let job_uri = attr(&job, DelimiterTag::JobAttributes, "job-uri")
        .and_then(str_value)
        .unwrap();
    let mut target =
        IppRequestResponse::new(IppVersion::v2_0(), Operation::GetJobAttributes, None).unwrap();
    add(
        &mut target,
        DelimiterTag::OperationAttributes,
        "job-uri",
        uri(job_uri).unwrap(),
    )
    .unwrap();
    assert_eq!(
        front.send(target, &[]).await.header().status_code(),
        StatusCode::SuccessfulOk
    );
    let mut request = front.request(Operation::CreateJob, None);
    add(
        &mut request,
        DelimiterTag::OperationAttributes,
        "job-name",
        name("x").unwrap(),
    )
    .unwrap();
    let mut raw = request.to_bytes().to_vec();
    // The final name value is a one-byte non-UTF8 title. Return an IPP error, never panic.
    let last = raw.len() - 2;
    raw[last] = 0xff;
    let response = reqwest::Client::new()
        .post(format!("http://{}/ipp/print/demo", front.address))
        .header("content-type", "application/ipp")
        .body(raw)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut reader =
        tokio_util::io::StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
    let response = read_metadata(&mut reader, 65536).await.unwrap();
    assert_eq!(
        response.header().status_code(),
        StatusCode::ClientErrorAttributesOrValuesNotSupported
    );
    let response = reqwest::get(format!("http://{}/source", front.address))
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let bytes = response.bytes().await.unwrap();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(bytes.as_ref()));
    let paths: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().into_owned())
        .collect();
    for expected in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "src/main.rs",
        "LICENSE",
        "NOTICE",
        "THIRD_PARTY_NOTICES.md",
        "docs/reference-design.md",
        ".gitlab-ci.yml",
    ] {
        assert!(
            paths.contains(&std::path::PathBuf::from(format!("ferriprint/{expected}"))),
            "{expected} missing from built source offer"
        );
    }
    assert!(paths.iter().all(|p| !p.components().any(|c| matches!(
        c.as_os_str().to_str(),
        Some(".git" | "target" | "spool" | "var")
    ))));
    assert_eq!(front.app.engine.active_count("demo").await.unwrap(), 1);
}

#[tokio::test]
async fn native_defaults_validate_constraints_without_overriding_selected_resolution() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let ppd = dir.path().join("native.ppd");
    let mut text = include_str!("fixtures/native.ppd").to_owned();
    text.push_str("\n*OpenUI *Resolution/Input resolution: PickOne\n*DefaultResolution: 300dpi\n*Resolution 300dpi/300 DPI: \"\"\n*Resolution 600dpi/600 DPI: \"\"\n*CloseUI: *Resolution\n*OpenUI *DefaultOnly/Native-only switch: PickOne\n*DefaultDefaultOnly: On\n*DefaultOnly On/On: \"\"\n*DefaultOnly Off/Off: \"\"\n*CloseUI: *DefaultOnly\n*UIConstraints: *DefaultOnly On *PageSize Letter\n");
    std::fs::write(&ppd, text).unwrap();
    let mut cfg = config(dir.path(), &receiver.uri(), 0);
    cfg.printers[0].ppd = ppd;
    let mut mapped = settings(&cfg);
    mapped
        .resolutions
        .push(ferriprint::domain::Resolution { x: 600, y: 600 });
    save_settings(&cfg, &mapped);
    let front = Frontend::start(cfg).await;
    let mut request = front.request(Operation::PrintJob, None);
    add(
        &mut request,
        DelimiterTag::JobAttributes,
        "printer-resolution",
        IppValue::Resolution {
            cross_feed: 600,
            feed: 600,
            units: 3,
        },
    )
    .unwrap();
    let reply = front.send(request, b"%PDF-1.4\n%%EOF").await;
    assert_eq!(reply.header().status_code(), StatusCode::SuccessfulOk);
    front.app.engine.tick().await.unwrap();
    let received = receiver.device.jobs.lock()[0].clone();
    assert_eq!(received.resolution, Some((600, 600)));
    assert!(!received.options.contains_key("Resolution"));
    assert!(!received.options.contains_key("DefaultOnly"));
    let request = ferriprint::domain::JobTemplate {
        media: Some("na_letter_8.5x11in".into()),
        ..Default::default()
    };
    assert!(
        front.app.printers["demo"]
            .read()
            .await
            .profile
            .resolve(&request, "application/pdf")
            .is_err(),
        "an unsubmitted native default must still participate in PPD constraints"
    );
}
