// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl
// These tests use only the synthetic HTTP CUPS receiver, never a native queue.
mod support;

use ferriprint::{
    auth::AccessState,
    config::{Config, ProfileSettings, Role},
    domain::Resolution,
    managed::PrinterCatalog,
    profile::PpdInventory,
    recipes::Recipe,
    wire::*,
};
use futures_util::TryStreamExt;
use ipp::prelude::*;
use std::{collections::BTreeMap, io::Write, path::Path, sync::atomic::Ordering};
use support::*;
use tokio_util::io::StreamReader;

const OP: DelimiterTag = DelimiterTag::OperationAttributes;
const JOB: DelimiterTag = DelimiterTag::JobAttributes;
const PRINTER: DelimiterTag = DelimiterTag::PrinterAttributes;
const PPD: &[u8] = include_bytes!("fixtures/native.ppd");
const PDF: &[u8] = b"%PDF-1.7\nsynthetic nonprinting onboarding fixture\n%%EOF\n";
type Fields = BTreeMap<String, String>;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}
fn endpoint(front: &Frontend, path: &str) -> String {
    format!("http://{}{path}", front.address)
}
fn origin(front: &Frontend) -> String {
    http_url(&front.app.config.public_url)
        .unwrap()
        .origin()
        .ascii_serialization()
}
fn hidden(html: &str, name: &str) -> String {
    let needle = format!("name=\"{name}\" value=\"");
    html.split_once(&needle)
        .unwrap_or_else(|| panic!("Missing form field {name}: {html}"))
        .1
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}
fn fields(pairs: &[(&str, &str)]) -> Fields {
    pairs
        .iter()
        .map(|(key, value)| ((*key).into(), (*value).into()))
        .collect()
}
async fn get(front: &Frontend, client: &reqwest::Client, path: &str) -> reqwest::Response {
    client
        .get(endpoint(front, path))
        .basic_auth(FIXTURE_ADMIN, Some(FIXTURE_PASSWORD))
        .send()
        .await
        .unwrap()
}
async fn post(
    front: &Frontend,
    client: &reqwest::Client,
    path: &str,
    fields: &Fields,
) -> reqwest::Response {
    client
        .post(endpoint(front, path))
        .basic_auth(FIXTURE_ADMIN, Some(FIXTURE_PASSWORD))
        .header("origin", origin(front))
        .header("sec-fetch-site", "same-origin")
        .form(fields)
        .send()
        .await
        .unwrap()
}
fn empty_configuration(directory: &Path, receiver: &Receiver) -> Config {
    let mut cfg = config(directory, &receiver.uri(), 0);
    std::fs::remove_file(cfg.profile_path("demo")).unwrap();
    cfg.printers.clear();
    std::fs::write(
        directory.join("printers.toml"),
        toml::to_string_pretty(&PrinterCatalog::default()).unwrap(),
    )
    .unwrap();
    let path = directory.join("access.toml");
    let mut access: AccessState = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    access.anonymous_printers.clear();
    for account in access.accounts.values_mut() {
        account.printers.clear();
    }
    std::fs::write(path, toml::to_string_pretty(&access).unwrap()).unwrap();
    cfg
}
async fn assert_empty(front: &Frontend, receiver: &Receiver) {
    assert!(front.app.printer_snapshot().is_empty());
    assert!(front.app.catalog.lock().await.printers.is_empty());
    let catalog = ferriprint::managed::load_catalog(&front.app.config).unwrap();
    assert!(catalog.printers.is_empty());
    assert_eq!(catalog.revision, 0);
    assert_eq!(
        std::fs::read_dir(front.app.config.state_dir.join("profiles"))
            .unwrap()
            .count(),
        0
    );
    assert!(!front.app.config.state_dir.join("ppds").exists());
    assert!(receiver.device.jobs.lock().is_empty());
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 0);
    assert!(receiver.device.cancellations.lock().is_empty());
}

struct Wizard {
    path: String,
    html: String,
}
impl Wizard {
    async fn start(front: &Frontend, client: &reqwest::Client) -> Self {
        let page = get(front, client, "/admin/onboarding?source=local").await;
        assert_eq!(page.status(), 200);
        let html = page.text().await.unwrap();
        let response = post(
            front,
            client,
            "/admin/onboarding/start",
            &fields(&[
                ("csrf", &hidden(&html, "csrf")),
                ("source", "local"),
                ("queue", "receiver"),
            ]),
        )
        .await;
        assert_eq!(response.status(), 303);
        let path = response.headers()["location"].to_str().unwrap().to_owned();
        let page = get(front, client, &path).await;
        assert_eq!(page.status(), 200);
        Self {
            path,
            html: page.text().await.unwrap(),
        }
    }
    fn form(&self, intent: &str, values: Fields) -> Fields {
        let mut values = values;
        values.insert("csrf".into(), hidden(&self.html, "csrf"));
        values.insert("revision".into(), hidden(&self.html, "revision"));
        values.insert("intent".into(), intent.into());
        values
    }
    async fn submit(
        &mut self,
        front: &Frontend,
        client: &reqwest::Client,
        intent: &str,
        values: Fields,
    ) -> u16 {
        let response = post(front, client, &self.path, &self.form(intent, values)).await;
        let status = response.status().as_u16();
        self.html = response.text().await.unwrap();
        status
    }
    async fn accepted(
        &mut self,
        front: &Frontend,
        client: &reqwest::Client,
        intent: &str,
        values: Fields,
    ) {
        let status = self.submit(front, client, intent, values).await;
        assert_eq!(status, 200, "{intent}: {}", self.html);
    }
}
fn native(prefix: &str, choices: &[(&str, &str)]) -> Fields {
    let inventory = PpdInventory::from_bytes(PPD).unwrap();
    let options: Vec<_> = inventory
        .choices
        .keys()
        .filter(|option| !matches!(option.as_str(), "PageSize" | "PageRegion" | "InputSlot"))
        .collect();
    choices
        .iter()
        .map(|(option, choice)| {
            let index = options
                .iter()
                .position(|actual| actual.as_str() == *option)
                .unwrap();
            (format!("{prefix}_{index}"), (*choice).into())
        })
        .collect()
}
fn mapping(key: &str, prefix: &str, choices: &[(&str, &str)]) -> Fields {
    let mut value = native(prefix, choices);
    value.insert("key".into(), key.into());
    value.insert("confirmed".into(), "true".into());
    value
}
fn recipe_form(key: &str, recipe: &Recipe) -> Fields {
    fields(&[
        ("key", key),
        ("display_name", &recipe.display_name),
        (
            "media_variant",
            &format!(
                "{}|{}",
                recipe.media,
                if recipe.borderless {
                    "borderless"
                } else {
                    "bordered"
                }
            ),
        ),
        ("source", &recipe.source),
        ("media_type", &recipe.media_type),
        ("quality", &recipe.quality.to_string()),
        ("color_mode", &recipe.color_mode),
        ("sides", &recipe.sides),
        ("copies", &recipe.copies.to_string()),
        ("orientation", &recipe.orientation.to_string()),
        (
            "resolution",
            &format!("{}x{}", recipe.resolution.x, recipe.resolution.y),
        ),
        ("print_scaling", &recipe.print_scaling),
    ])
}
fn normal_recipe() -> Recipe {
    Recipe {
        display_name: "A4 normal bordered".into(),
        media: "iso_a4_210x297mm".into(),
        borderless: false,
        source: "main".into(),
        media_type: "stationery".into(),
        quality: 4,
        color_mode: "color".into(),
        sides: "one-sided".into(),
        copies: 2,
        orientation: 4,
        resolution: Resolution { x: 300, y: 300 },
        print_scaling: "fit".into(),
    }
}
fn photo_recipe() -> Recipe {
    Recipe {
        display_name: "A3 borderless glossy high".into(),
        media: "iso_a3_297x420mm".into(),
        borderless: true,
        media_type: "photographic-glossy".into(),
        quality: 5,
        copies: 3,
        orientation: 6,
        print_scaling: "fill".into(),
        ..normal_recipe()
    }
}
fn duplex_recipe() -> Recipe {
    Recipe {
        display_name: "A4 monochrome duplex draft".into(),
        source: "tray-1".into(),
        quality: 3,
        color_mode: "monochrome".into(),
        sides: "two-sided-long-edge".into(),
        copies: 4,
        orientation: 5,
        print_scaling: "auto-fit".into(),
        ..normal_recipe()
    }
}
async fn through_mappings(front: &Frontend, client: &reqwest::Client, name: &str) -> Wizard {
    let mut wizard = Wizard::start(front, client).await;
    wizard
        .accepted(
            front,
            client,
            "identity",
            fields(&[
                ("name", name),
                ("display_name", "Synthetic browser printer"),
                ("service_name", &format!("Synthetic {name}")),
                ("location", "Isolated receiver; never physical"),
            ]),
        )
        .await;
    for (key, choice) in [("main", "Rear"), ("tray-1", "Cassette")] {
        wizard
            .accepted(
                front,
                client,
                "source",
                fields(&[
                    ("key", key),
                    ("source_native", choice),
                    ("confirmed", "true"),
                ]),
            )
            .await;
    }
    let mut stationery = mapping(
        "stationery",
        "normal",
        &[("MediaType", "Plain"), ("Quality", "Standard")],
    );
    stationery.extend(native(
        "draft",
        &[("MediaType", "Plain"), ("Quality", "Economy")],
    ));
    stationery.extend(native(
        "high",
        &[("MediaType", "Plain"), ("Quality", "Fine")],
    ));
    stationery.insert("draft_enabled".into(), "true".into());
    stationery.insert("high_enabled".into(), "true".into());
    wizard.accepted(front, client, "material", stationery).await;
    let mut glossy = mapping(
        "photographic-glossy",
        "normal",
        &[("MediaType", "Photo"), ("Quality", "Standard")],
    );
    glossy.extend(native(
        "high",
        &[("MediaType", "Photo"), ("Quality", "Fine")],
    ));
    glossy.insert("high_enabled".into(), "true".into());
    wizard.accepted(front, client, "material", glossy).await;
    for (key, choice) in [("color", "RGB"), ("monochrome", "Gray")] {
        wizard
            .accepted(
                front,
                client,
                "color",
                mapping(key, "native", &[("ColorModel", choice)]),
            )
            .await;
    }
    for (key, choice) in [
        ("one-sided", "None"),
        ("two-sided-long-edge", "DuplexNoTumble"),
        ("two-sided-short-edge", "DuplexTumble"),
    ] {
        wizard
            .accepted(
                front,
                client,
                "sides",
                mapping(key, "native", &[("Duplex", choice)]),
            )
            .await;
    }
    wizard
        .accepted(
            front,
            client,
            "mappings_done",
            fields(&[("confirmed", "true")]),
        )
        .await;
    wizard
}
fn default_fields() -> Fields {
    fields(&[
        ("media_variant", "iso_a4_210x297mm|bordered"),
        ("source", "main"),
        ("media_type", "stationery"),
        ("quality", "4"),
        ("color_mode", "color"),
        ("sides", "one-sided"),
        ("format_0", "true"),
    ])
}
async fn reviewed(front: &Frontend, client: &reqwest::Client, name: &str) -> Wizard {
    let mut wizard = through_mappings(front, client, name).await;
    wizard
        .accepted(front, client, "defaults", default_fields())
        .await;
    for (key, recipe) in [
        ("A4normal", normal_recipe()),
        ("A3borderlessHigh", photo_recipe()),
        ("duplex", duplex_recipe()),
    ] {
        wizard
            .accepted(front, client, "recipe", recipe_form(key, &recipe))
            .await;
    }
    wizard
        .accepted(front, client, "review", Fields::new())
        .await;
    wizard
}
async fn ipp(
    front: &Frontend,
    printer: &str,
    request: IppRequestResponse,
    document: &[u8],
) -> IppRequestResponse {
    let stream = futures_util::stream::iter([
        Ok::<_, std::io::Error>(request.to_bytes()),
        Ok(bytes::Bytes::copy_from_slice(document)),
    ]);
    let response = reqwest::Client::new()
        .post(endpoint(front, &format!("/ipp/print/{printer}")))
        .basic_auth(FIXTURE_ADMIN, Some(FIXTURE_PASSWORD))
        .header("content-type", "application/ipp")
        .body(reqwest::Body::wrap_stream(stream))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
    let mut reader = StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
    read_metadata(&mut reader, 1024 * 1024).await.unwrap()
}
fn request(front: &Frontend, printer: &str, operation: Operation) -> IppRequestResponse {
    IppRequestResponse::new(
        IppVersion::v2_0(),
        operation,
        Some(front.app.config.printer_uri(printer).parse().unwrap()),
    )
    .unwrap()
}

#[tokio::test]
async fn empty_catalog_real_http_wizard_activates_persisted_routes_and_complete_tickets() {
    let directory = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = empty_configuration(directory.path(), &receiver);
    add_user(&mut cfg, "reader", Role::User, "isolated-reader-password");
    let front = Frontend::start(cfg.clone()).await;
    let client = client();
    let mut wizard = through_mappings(&front, &client, "office").await;
    let stale = wizard.form("defaults", default_fields());
    let mut invalid = default_fields();
    invalid.insert("media_type".into(), "photographic-glossy".into());
    invalid.insert("source".into(), "tray-1".into());
    assert_eq!(
        wizard.submit(&front, &client, "defaults", invalid).await,
        400
    );
    assert_empty(&front, &receiver).await;
    wizard
        .accepted(&front, &client, "defaults", default_fields())
        .await;
    assert_eq!(
        post(&front, &client, &wizard.path, &stale).await.status(),
        409
    );
    for (key, recipe) in [
        ("A4normal", normal_recipe()),
        ("A3borderlessHigh", photo_recipe()),
        ("duplex", duplex_recipe()),
    ] {
        wizard
            .accepted(&front, &client, "recipe", recipe_form(key, &recipe))
            .await;
    }
    let mut impossible = photo_recipe();
    impossible.sides = "two-sided-long-edge".into();
    assert_eq!(
        wizard
            .submit(
                &front,
                &client,
                "recipe",
                recipe_form("invalid", &impossible)
            )
            .await,
        400
    );
    wizard
        .accepted(&front, &client, "review", Fields::new())
        .await;
    assert!(
        wizard.html.contains("TA3")
            && wizard.html.contains("Fine")
            && wizard.html.contains("photographic-glossy")
    );
    assert_empty(&front, &receiver).await;
    wizard
        .accepted(&front, &client, "commit", fields(&[("confirmed", "true")]))
        .await;
    assert!(wizard.html.contains("Printer saved and HTTP-active"));
    assert!(wizard.html.contains("disabled"));
    assert!(wizard.html.contains("/admin/users#anonymous-grants"));
    assert!(
        wizard
            .html
            .contains(&front.app.config.printer_uri("office"))
    );
    assert_eq!(front.app.printer_snapshot().len(), 1);
    assert_eq!(front.app.catalog.lock().await.revision, 1);
    assert_eq!(get(&front, &client, &wizard.path).await.status(), 404);
    assert!(receiver.device.jobs.lock().is_empty());
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 0);
    let export_path = "/api/printers/office/recipes/A3borderlessHigh";
    let export: serde_json::Value = serde_json::from_slice(
        &get(&front, &client, export_path)
            .await
            .bytes()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(export["key"], "A3borderlessHigh");
    assert_eq!(export["resolved"]["options"]["PageSize"], "TA3");
    assert_eq!(export["resolved"]["media"]["margins"]["left"], 0);
    assert_eq!(
        client
            .get(endpoint(&front, export_path))
            .basic_auth("reader", Some("isolated-reader-password"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .get(endpoint(&front, export_path))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let attributes = ipp(
        &front,
        "office",
        request(&front, "office", Operation::GetPrinterAttributes),
        &[],
    )
    .await;
    assert_eq!(attributes.header().status_code(), StatusCode::SuccessfulOk);
    let presets = values(attr(&attributes, PRINTER, "job-presets-supported").unwrap());
    assert_eq!(presets.len(), 3);
    for preset in presets {
        let IppValue::Collection(members) = preset else {
            panic!("standard preset is not a collection")
        };
        let key = members.get("preset-name").and_then(str_value).unwrap();
        let mut print = request(&front, "office", Operation::PrintJob);
        for (name, value) in members {
            if !matches!(name.as_str(), "preset-name" | "preset-category") {
                add(&mut print, JOB, name, value.clone()).unwrap();
            }
        }
        add(
            &mut print,
            OP,
            "document-format",
            mime("application/pdf").unwrap(),
        )
        .unwrap();
        let reply = ipp(&front, "office", print, PDF).await;
        assert_eq!(reply.header().status_code(), StatusCode::SuccessfulOk);
        front.app.engine.tick().await.unwrap();
        let received = receiver.device.jobs.lock().last().unwrap().clone();
        let expected = front
            .app
            .printer("office")
            .unwrap()
            .read()
            .await
            .profile
            .recipes()[key]
            .resolved
            .clone();
        for (native, value) in &expected.options {
            assert_eq!(received.options.get(native), Some(value));
        }
        assert_eq!(received.copies, expected.copies);
        assert_eq!(received.orientation, expected.orientation);
        assert_eq!(
            received.print_scaling.as_deref(),
            Some(expected.print_scaling.as_str())
        );
        assert_eq!(
            received.resolution,
            Some((expected.resolution.x, expected.resolution.y))
        );
        receiver.complete(received.id);
        front.app.engine.tick().await.unwrap();
    }
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 3);
    let protected = directory.path().join("protected.toml");
    std::fs::write(&protected, toml::to_string_pretty(&cfg).unwrap()).unwrap();
    drop(front);
    tokio::task::yield_now().await;
    let reloaded = Config::load(&protected).unwrap();
    assert_eq!(reloaded.printers.len(), 1);
    let restart = Frontend::start(reloaded).await;
    let response = ipp(
        &restart,
        "office",
        request(&restart, "office", Operation::GetPrinterAttributes),
        &[],
    )
    .await;
    assert_eq!(response.header().status_code(), StatusCode::SuccessfulOk);
    assert_eq!(
        values(attr(&response, PRINTER, "job-presets-supported").unwrap()).len(),
        3
    );
    let persisted: serde_json::Value = serde_json::from_slice(
        &get(&restart, &client, export_path)
            .await
            .bytes()
            .await
            .unwrap(),
    )
    .unwrap();
    assert_eq!(persisted, export);
    assert_eq!(
        client
            .get(endpoint(&restart, export_path))
            .basic_auth("reader", Some("isolated-reader-password"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 3);
}

#[tokio::test]
async fn untrusted_queue_source_csrf_nonadmin_and_failed_acquisition_never_install() {
    let directory = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = empty_configuration(directory.path(), &receiver);
    add_user(
        &mut cfg,
        "ordinary",
        Role::User,
        "ordinary-fixture-password",
    );
    add_user(
        &mut cfg,
        "other-admin",
        Role::Admin,
        "second-admin-fixture-password",
    );
    let front = Frontend::start(cfg).await;
    let client = client();
    let wizard = Wizard::start(&front, &client).await;
    let csrf = hidden(&wizard.html, "csrf");
    for (source, queue) in [
        ("unknown", "receiver"),
        ("http://127.0.0.1/", "receiver"),
        ("local", "../receiver"),
        ("local", "missing"),
    ] {
        let response = post(
            &front,
            &client,
            "/admin/onboarding/start",
            &fields(&[("csrf", &csrf), ("source", source), ("queue", queue)]),
        )
        .await;
        assert!(response.status().is_client_error() || response.status().is_server_error());
        assert_empty(&front, &receiver).await;
    }
    let form = fields(&[("csrf", &csrf), ("source", "local"), ("queue", "receiver")]);
    let trusted_origin = origin(&front);
    for (origin_header, fetch) in [
        ("http://foreign.invalid", "same-origin"),
        (trusted_origin.as_str(), "cross-site"),
    ] {
        assert_eq!(
            client
                .post(endpoint(&front, "/admin/onboarding/start"))
                .basic_auth(FIXTURE_ADMIN, Some(FIXTURE_PASSWORD))
                .header("origin", origin_header)
                .header("sec-fetch-site", fetch)
                .form(&form)
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
    }
    let mut bad_csrf = form.clone();
    bad_csrf.insert("csrf".into(), "not-the-token".into());
    assert_eq!(
        post(&front, &client, "/admin/onboarding/start", &bad_csrf)
            .await
            .status(),
        403
    );
    assert_eq!(
        client
            .post(endpoint(&front, "/admin/onboarding/start"))
            .basic_auth("ordinary", Some("ordinary-fixture-password"))
            .header("origin", origin(&front))
            .form(&form)
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .get(endpoint(&front, "/admin/onboarding"))
            .basic_auth("ordinary", Some("ordinary-fixture-password"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client
            .get(endpoint(&front, &wizard.path))
            .basic_auth("other-admin", Some("second-admin-fixture-password"))
            .send()
            .await
            .unwrap()
            .status(),
        404
    );
    assert_eq!(
        client
            .post(endpoint(&front, "/admin/onboarding/start"))
            .basic_auth(FIXTURE_ADMIN, Some(FIXTURE_PASSWORD))
            .header("content-type", "application/x-www-form-urlencoded")
            .body(vec![b'x'; 1024 * 1024 + 1])
            .send()
            .await
            .unwrap()
            .status(),
        413
    );
    for uri in [
        "ipp://foreign.invalid/printers/receiver".to_owned(),
        format!("ipp://{}/classes/receiver", receiver.address),
        format!("ipp://{}/printers/receiver?query=1", receiver.address),
        format!("ipp://{}/printers/receiver#fragment", receiver.address),
    ] {
        *receiver.device.inventory_uri.lock() = Some(uri);
        assert_eq!(
            post(&front, &client, "/admin/onboarding/start", &form)
                .await
                .status(),
            502
        );
        assert_empty(&front, &receiver).await;
    }
    *receiver.device.inventory_uri.lock() = None;
    receiver.device.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        post(&front, &client, "/admin/onboarding/start", &form)
            .await
            .status(),
        502
    );
    receiver.device.unavailable.store(false, Ordering::SeqCst);
    for ppd in [
        b"not a native PPD".to_vec(),
        vec![b' '; 16 * 1024 * 1024 + 1],
    ] {
        *receiver.device.ppd.lock() = ppd;
        assert_eq!(
            post(&front, &client, "/admin/onboarding/start", &form)
                .await
                .status(),
            502
        );
        assert_empty(&front, &receiver).await;
    }
    let mut compressed = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    compressed
        .write_all(&vec![b' '; 16 * 1024 * 1024 + 1])
        .unwrap();
    *receiver.device.ppd.lock() = compressed.finish().unwrap();
    assert_eq!(
        post(&front, &client, "/admin/onboarding/start", &form)
            .await
            .status(),
        502
    );
    assert_empty(&front, &receiver).await;
}

#[tokio::test]
async fn exact_review_ppd_confirmation_and_account_revision_are_commit_boundaries() {
    let directory = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let cfg = empty_configuration(directory.path(), &receiver);
    let front = Frontend::start(cfg).await;
    let client = client();
    let mut wizard = reviewed(&front, &client, "reviewed").await;
    assert_eq!(
        wizard
            .submit(&front, &client, "commit", Fields::new())
            .await,
        400
    );
    let mut foreign = wizard.form("commit", fields(&[("confirmed", "true")]));
    foreign.insert("csrf".into(), "wrong".into());
    assert_eq!(
        post(&front, &client, &wizard.path, &foreign).await.status(),
        403
    );
    receiver
        .device
        .ppd
        .lock()
        .extend_from_slice(b"\n*% Installed driver changed after review\n");
    assert_eq!(
        wizard
            .submit(&front, &client, "commit", fields(&[("confirmed", "true")]))
            .await,
        400
    );
    assert_empty(&front, &receiver).await;
    *receiver.device.ppd.lock() = PPD.to_vec();
    receiver.device.unavailable.store(true, Ordering::SeqCst);
    assert_eq!(
        wizard
            .submit(&front, &client, "commit", fields(&[("confirmed", "true")]))
            .await,
        400
    );
    receiver.device.unavailable.store(false, Ordering::SeqCst);
    assert_empty(&front, &receiver).await;
    let actor = principal(&front.app.engine, FIXTURE_ADMIN, FIXTURE_PASSWORD).await;
    let account = front
        .app
        .auth
        .accounts()
        .into_iter()
        .find(|account| account.username == FIXTURE_ADMIN)
        .unwrap();
    front
        .app
        .auth
        .update_account(
            &actor,
            FIXTURE_ADMIN,
            ferriprint::auth::AccountUpdate {
                revision: account.revision,
                role: Role::Admin,
                enabled: true,
                printers: account.printers,
                hash: None,
            },
        )
        .unwrap();
    assert_eq!(get(&front, &client, &wizard.path).await.status(), 404);
    assert_eq!(
        post(
            &front,
            &client,
            &wizard.path,
            &wizard.form("commit", fields(&[("confirmed", "true")]))
        )
        .await
        .status(),
        403
    );
    assert_empty(&front, &receiver).await;
}

#[tokio::test]
async fn stale_catalog_and_duplicate_names_do_not_leave_partial_profiles_or_routes() {
    let directory = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(empty_configuration(directory.path(), &receiver)).await;
    let client = client();
    let mut first = reviewed(&front, &client, "same-name").await;
    let mut second = reviewed(&front, &client, "same-name").await;
    first
        .accepted(&front, &client, "commit", fields(&[("confirmed", "true")]))
        .await;
    let catalog = std::fs::read(directory.path().join("printers.toml")).unwrap();
    let profile = std::fs::read(front.app.config.profile_path("same-name")).unwrap();
    assert_eq!(
        second
            .submit(&front, &client, "commit", fields(&[("confirmed", "true")]))
            .await,
        409
    );
    second
        .accepted(&front, &client, "back", Fields::new())
        .await;
    second
        .accepted(&front, &client, "review", Fields::new())
        .await;
    assert_eq!(
        second
            .submit(&front, &client, "commit", fields(&[("confirmed", "true")]))
            .await,
        409
    );
    assert_eq!(
        std::fs::read(directory.path().join("printers.toml")).unwrap(),
        catalog
    );
    assert_eq!(
        std::fs::read(front.app.config.profile_path("same-name")).unwrap(),
        profile
    );
    assert_eq!(
        std::fs::read_dir(directory.path().join("ppds"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(
        std::fs::read_dir(directory.path().join("profiles"))
            .unwrap()
            .count(),
        1
    );
    assert_eq!(front.app.printer_snapshot().len(), 1);
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn structured_recipe_editor_validates_saves_exports_and_deletes_one_recipe_atomically() {
    let directory = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = config(directory.path(), &receiver.uri(), 0);
    let mut original = settings(&cfg);
    original.recipes.insert("A4normal".into(), normal_recipe());
    save_settings(&cfg, &original);
    add_user(
        &mut cfg,
        "recipe-reader",
        Role::User,
        "recipe-reader-password",
    );
    let front = Frontend::start(cfg.clone()).await;
    let client = client();
    let page = get(&front, &client, "/admin/printers/demo/recipes").await;
    assert_eq!(page.status(), 200);
    let html = page.text().await.unwrap();
    assert!(html.contains("name=\"media_variant\"") && html.contains("name=\"resolution\""));
    let csrf = hidden(&html, "csrf");
    let mut edit = recipe_form("A3borderlessHigh", &photo_recipe());
    edit.extend(fields(&[
        ("csrf", &csrf),
        ("revision", "0"),
        ("operation", "upsert"),
        ("intent", "validate"),
    ]));
    let response = post(&front, &client, "/admin/printers/demo/recipes", &edit).await;
    assert_eq!(response.status(), 200);
    let preview = response.text().await.unwrap();
    assert!(
        preview.contains("A3borderlessHigh") && preview.contains("TA3") && preview.contains("Fine")
    );
    assert_eq!(front.app.printer("demo").unwrap().read().await.revision, 0);
    assert_eq!(
        ProfileSettings::load(&cfg.profile_path("demo"))
            .unwrap()
            .recipes,
        original.recipes
    );
    let mut invalid = edit.clone();
    invalid.insert("source".into(), "tray-1".into());
    assert_eq!(
        post(&front, &client, "/admin/printers/demo/recipes", &invalid)
            .await
            .status(),
        400
    );
    edit.insert("intent".into(), "save".into());
    assert_eq!(
        post(&front, &client, "/admin/printers/demo/recipes", &edit)
            .await
            .status(),
        303
    );
    let saved = ProfileSettings::load(&cfg.profile_path("demo")).unwrap();
    assert_eq!(saved.recipes.len(), 2);
    assert_eq!(saved.recipes["A4normal"], original.recipes["A4normal"]);
    assert_eq!(
        serde_json::to_value(&saved.defaults).unwrap(),
        serde_json::to_value(&original.defaults).unwrap()
    );
    assert_eq!(front.app.printer("demo").unwrap().read().await.revision, 1);
    assert_eq!(
        post(&front, &client, "/admin/printers/demo/recipes", &edit)
            .await
            .status(),
        409
    );
    let export_path = "/api/printers/demo/recipes/A3borderlessHigh";
    let export = client
        .get(endpoint(&front, export_path))
        .basic_auth("recipe-reader", Some("recipe-reader-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(export.status(), 200);
    let export: serde_json::Value = serde_json::from_slice(&export.bytes().await.unwrap()).unwrap();
    assert_eq!(export["resolved"]["options"]["PageSize"], "TA3");
    assert_eq!(export["resolved"]["copies"], 3);
    let account = front
        .app
        .auth
        .accounts()
        .into_iter()
        .find(|account| account.username == "recipe-reader")
        .unwrap();
    let actor = principal(&front.app.engine, FIXTURE_ADMIN, FIXTURE_PASSWORD).await;
    front
        .app
        .auth
        .update_account(
            &actor,
            "recipe-reader",
            ferriprint::auth::AccountUpdate {
                revision: account.revision,
                role: Role::User,
                enabled: true,
                printers: Default::default(),
                hash: None,
            },
        )
        .unwrap();
    assert_eq!(
        client
            .get(endpoint(&front, export_path))
            .basic_auth("recipe-reader", Some("recipe-reader-password"))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    let removal = fields(&[
        ("csrf", &csrf),
        ("revision", "1"),
        ("operation", "delete"),
        ("intent", "save"),
        ("key", "A3borderlessHigh"),
    ]);
    assert_eq!(
        post(&front, &client, "/admin/printers/demo/recipes", &removal)
            .await
            .status(),
        303
    );
    assert_eq!(get(&front, &client, export_path).await.status(), 404);
    assert_eq!(
        ProfileSettings::load(&cfg.profile_path("demo"))
            .unwrap()
            .recipes,
        original.recipes
    );
    let mut queued = request(&front, "demo", Operation::PrintJob);
    add(
        &mut queued,
        OP,
        "document-format",
        mime("application/pdf").unwrap(),
    )
    .unwrap();
    let queued = ipp(&front, "demo", queued, PDF).await;
    assert_eq!(queued.header().status_code(), StatusCode::SuccessfulOk);
    edit.insert("revision".into(), "2".into());
    assert_eq!(
        post(&front, &client, "/admin/printers/demo/recipes", &edit)
            .await
            .status(),
        409
    );
    assert_eq!(front.app.printer("demo").unwrap().read().await.revision, 2);
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 0);
}

#[tokio::test]
async fn private_onboarding_drafts_are_bounded_and_discard_releases_capacity() {
    let directory = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(empty_configuration(directory.path(), &receiver)).await;
    let client = client();
    let mut drafts = Vec::new();
    for _ in 0..16 {
        drafts.push(Wizard::start(&front, &client).await);
    }
    assert_eq!(front.app.onboarding.lock().len(), 16);
    let response = post(
        &front,
        &client,
        "/admin/onboarding/start",
        &fields(&[
            ("csrf", &hidden(&drafts[0].html, "csrf")),
            ("source", "local"),
            ("queue", "receiver"),
        ]),
    )
    .await;
    assert_eq!(response.status(), 429);
    assert_eq!(
        drafts[0]
            .submit(&front, &client, "discard", Fields::new())
            .await,
        303
    );
    assert_eq!(get(&front, &client, &drafts[0].path).await.status(), 404);
    let _replacement = Wizard::start(&front, &client).await;
    assert_eq!(front.app.onboarding.lock().len(), 16);
    assert_empty(&front, &receiver).await;
}
