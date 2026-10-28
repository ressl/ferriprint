// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

mod support;

use ferriprint::{config::Config, wire::*};
use futures_util::{StreamExt, TryStreamExt};
use ipp::prelude::*;
use std::{
    sync::{Arc, atomic::Ordering},
    time::Duration,
};
use support::*;

const PASSWORD: &str = "isolated-browser-user-password";
const NEW_PASSWORD: &str = "isolated-browser-new-password";
const RESET_PASSWORD: &str = "isolated-browser-reset-password";
const OP: DelimiterTag = DelimiterTag::OperationAttributes;
const JOB: DelimiterTag = DelimiterTag::JobAttributes;
type Fields = Vec<(String, String)>;

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}
fn protected_config(directory: &std::path::Path, receiver: &Receiver) -> Config {
    let mut cfg = config(directory, &receiver.uri(), 0);
    cfg.access.anonymous_trusted_lan = false;
    cfg.access.anonymous_owner = None;
    cfg.access.anonymous_clients.clear();
    cfg
}
fn origin(front: &Frontend) -> String {
    http_url(&front.app.config.public_url)
        .unwrap()
        .origin()
        .ascii_serialization()
}
async fn get(front: &Frontend, actor: &str, password: &str, path: &str) -> reqwest::Response {
    client()
        .get(format!("http://{}{path}", front.address))
        .basic_auth(actor, Some(password))
        .send()
        .await
        .unwrap()
}
async fn json(response: reqwest::Response) -> serde_json::Value {
    assert_eq!(response.status(), 200);
    serde_json::from_slice(&response.bytes().await.unwrap()).unwrap()
}
async fn html(front: &Frontend, actor: &str, password: &str, path: &str) -> String {
    let response = get(front, actor, password, path).await;
    assert_eq!(response.status(), 200);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert!(
        response.headers()["content-security-policy"]
            .to_str()
            .unwrap()
            .contains("form-action 'self'")
    );
    let body = response.text().await.unwrap();
    no_credentials(
        &body,
        &[PASSWORD, NEW_PASSWORD, RESET_PASSWORD, FIXTURE_PASSWORD],
    );
    body
}
fn no_credentials(body: &str, passwords: &[&str]) {
    assert!(!body.contains("$argon2"));
    assert!(!body.contains("password_hash"));
    for password in passwords {
        assert!(!body.contains(password));
    }
}
fn hidden(body: &str, name: &str) -> String {
    body.split(&format!("name=\"{name}\" value=\""))
        .nth(1)
        .unwrap()
        .split('"')
        .next()
        .unwrap()
        .to_owned()
}
fn account_article<'a>(body: &'a str, username: &str) -> &'a str {
    body.split(&format!("<article id=\"account-{username}\">"))
        .nth(1)
        .unwrap()
        .split("</article>")
        .next()
        .unwrap()
}
fn create_fields(
    page: &str,
    username: &str,
    password: &str,
    role: &str,
    printers: &[&str],
) -> Fields {
    let mut fields = vec![
        ("csrf".into(), hidden(page, "csrf")),
        ("intent".into(), "create".into()),
        ("username".into(), username.into()),
        ("role".into(), role.into()),
        ("password".into(), password.into()),
        ("password_confirm".into(), password.into()),
    ];
    fields.extend(
        printers
            .iter()
            .map(|name| ("printer".into(), (*name).into())),
    );
    fields
}
fn update_fields(
    page: &str,
    username: &str,
    role: &str,
    enabled: bool,
    printers: &[&str],
) -> Fields {
    let article = account_article(page, username);
    let mut fields = vec![
        ("csrf".into(), hidden(article, "csrf")),
        ("intent".into(), "update".into()),
        ("username".into(), hidden(article, "username")),
        ("revision".into(), hidden(article, "revision")),
        ("role".into(), role.into()),
    ];
    if enabled {
        fields.push(("enabled".into(), "true".into()));
    }
    fields.extend(
        printers
            .iter()
            .map(|name| ("printer".into(), (*name).into())),
    );
    fields
}
fn replace(fields: &mut Fields, name: &str, value: &str) {
    fields.iter_mut().find(|(key, _)| key == name).unwrap().1 = value.to_owned();
}
async fn post(
    front: &Frontend,
    actor: &str,
    password: &str,
    path: &str,
    fields: &Fields,
) -> reqwest::Response {
    client()
        .post(format!("http://{}{path}", front.address))
        .basic_auth(actor, Some(password))
        .header("origin", origin(front))
        .header("sec-fetch-site", "same-origin")
        .form(fields)
        .send()
        .await
        .unwrap()
}
async fn applied(response: reqwest::Response) -> String {
    assert_eq!(response.status(), 200);
    assert!(response.headers().get("location").is_none());
    let body = response.text().await.unwrap();
    assert!(body.contains("APPLIED"));
    no_credentials(
        &body,
        &[PASSWORD, NEW_PASSWORD, RESET_PASSWORD, FIXTURE_PASSWORD],
    );
    body
}
async fn create(front: &Frontend, username: &str, role: &str, printers: &[&str]) {
    let page = html(front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
    let fields = create_fields(&page, username, PASSWORD, role, printers);
    applied(
        post(
            front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &fields,
        )
        .await,
    )
    .await;
}
fn request(
    front: &Frontend,
    printer: &str,
    operation: Operation,
    id: Option<i32>,
) -> IppRequestResponse {
    let mut request = IppRequestResponse::new(
        IppVersion::v2_0(),
        operation,
        Some(front.app.config.printer_uri(printer).parse().unwrap()),
    )
    .unwrap();
    if let Some(id) = id {
        add(&mut request, OP, "job-id", IppValue::Integer(id)).unwrap();
    }
    if matches!(
        operation,
        Operation::PrintJob
            | Operation::ValidateJob
            | Operation::CreateJob
            | Operation::SendDocument
    ) {
        add(
            &mut request,
            OP,
            "document-format",
            mime("application/pdf").unwrap(),
        )
        .unwrap();
    }
    if operation == Operation::SendDocument {
        add(&mut request, OP, "last-document", IppValue::Boolean(true)).unwrap();
    }
    request
}
async fn decode(response: reqwest::Response) -> IppRequestResponse {
    assert_eq!(response.status(), 200);
    let mut reader =
        tokio_util::io::StreamReader::new(response.bytes_stream().map_err(std::io::Error::other));
    read_metadata(&mut reader, 1024 * 1024).await.unwrap()
}
async fn ipp(
    front: &Frontend,
    actor: &str,
    password: &str,
    printer: &str,
    request: IppRequestResponse,
    document: &[u8],
) -> IppRequestResponse {
    let mut bytes = request.to_bytes().to_vec();
    bytes.extend_from_slice(document);
    decode(
        client()
            .post(format!("http://{}/ipp/print/{printer}", front.address))
            .basic_auth(actor, Some(password))
            .header("content-type", "application/ipp")
            .body(bytes)
            .send()
            .await
            .unwrap(),
    )
    .await
}
fn id(reply: &IppRequestResponse) -> i32 {
    assert!(reply.header().status_code().is_success());
    attr(reply, JOB, "job-id").and_then(integer).unwrap()
}

#[tokio::test]
async fn browser_creation_grants_exactly_one_printer_and_preserves_job_ownership() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = protected_config(dir.path(), &receiver);
    add_printer(&mut cfg, "other");
    let front = Frontend::start(cfg).await;
    let page = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
    assert!(page.contains("type=\"checkbox\" name=\"printer\" value=\"demo\""));
    assert!(page.contains("type=\"checkbox\" name=\"printer\" value=\"other\""));
    assert!(page.contains("Administrators have global authority"));
    create(&front, "browser-user", "user", &["demo"]).await;
    assert_eq!(
        get(&front, "browser-user", PASSWORD, "/account")
            .await
            .status(),
        200
    );
    assert_eq!(
        get(&front, "browser-user", PASSWORD, "/admin/users")
            .await
            .status(),
        403
    );
    let printers = json(get(&front, "browser-user", PASSWORD, "/api/printers").await).await;
    let names: Vec<_> = printers
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| entry["name"].as_str().unwrap())
        .collect();
    assert_eq!(names, ["demo"]);

    let mut print = request(&front, "demo", Operation::PrintJob, None);
    add(&mut print, OP, "requesting-user-name", name("bob").unwrap()).unwrap();
    add(
        &mut print,
        OP,
        "job-name",
        name("browser-private-document").unwrap(),
    )
    .unwrap();
    let own = id(&ipp(
        &front,
        "browser-user",
        PASSWORD,
        "demo",
        print,
        b"%PDF-1.4\n%%EOF",
    )
    .await);
    let own_json = json(
        get(
            &front,
            "browser-user",
            PASSWORD,
            &format!("/api/jobs/{own}"),
        )
        .await,
    )
    .await;
    assert_eq!(own_json["owner"], "browser-user");
    front.app.engine.tick().await.unwrap();
    assert_eq!(receiver.device.jobs.lock().len(), 1);
    assert_eq!(
        receiver.device.jobs.lock()[0].bytes,
        b"%PDF-1.4\n%%EOF".len() as u64
    );

    let mut private = request(&front, "other", Operation::CreateJob, None);
    add(
        &mut private,
        OP,
        "job-name",
        name("other-printer-private-document").unwrap(),
    )
    .unwrap();
    let other = id(&ipp(
        &front,
        FIXTURE_ADMIN,
        FIXTURE_PASSWORD,
        "other",
        private,
        &[],
    )
    .await);
    for operation in [
        Operation::GetPrinterAttributes,
        Operation::PrintJob,
        Operation::ValidateJob,
        Operation::CreateJob,
        Operation::GetJobs,
    ] {
        let reply = ipp(
            &front,
            "browser-user",
            PASSWORD,
            "other",
            request(&front, "other", operation, None),
            &[],
        )
        .await;
        assert_eq!(
            reply.header().status_code(),
            StatusCode::ClientErrorNotAuthorized
        );
        assert!(
            !String::from_utf8_lossy(&reply.to_bytes()).contains("other-printer-private-document")
        );
    }
    assert_eq!(
        get(
            &front,
            "browser-user",
            PASSWORD,
            &format!("/api/jobs/{other}")
        )
        .await
        .status(),
        403
    );
    for operation in [
        Operation::GetJobAttributes,
        Operation::SendDocument,
        Operation::CancelJob,
    ] {
        let reply = ipp(
            &front,
            "bob",
            FIXTURE_PASSWORD,
            "demo",
            request(&front, "demo", operation, Some(own)),
            &[],
        )
        .await;
        assert_eq!(
            reply.header().status_code(),
            StatusCode::ClientErrorNotAuthorized
        );
        assert!(reply.attributes().groups_of(JOB).next().is_none());
        assert!(!String::from_utf8_lossy(&reply.to_bytes()).contains("browser-private-document"));
    }
    assert_eq!(
        get(&front, "bob", FIXTURE_PASSWORD, &format!("/api/jobs/{own}"))
            .await
            .status(),
        403
    );
    let bob_page = html(&front, "bob", FIXTURE_PASSWORD, "/account").await;
    assert_eq!(
        post(
            &front,
            "bob",
            FIXTURE_PASSWORD,
            &format!("/api/jobs/{own}/cancel"),
            &vec![("csrf".into(), hidden(&bob_page, "csrf"))]
        )
        .await
        .status(),
        403
    );
    for (actor, password, forbidden_title) in [
        ("bob", FIXTURE_PASSWORD, "browser-private-document"),
        ("browser-user", PASSWORD, "other-printer-private-document"),
    ] {
        for path in ["/", "/api/jobs"] {
            let body = get(&front, actor, password, path)
                .await
                .text()
                .await
                .unwrap();
            assert!(!body.contains(forbidden_title));
        }
    }
    create(&front, "global-admin", "admin", &[]).await;
    let printers = json(get(&front, "global-admin", PASSWORD, "/api/printers").await).await;
    assert_eq!(printers.as_array().unwrap().len(), 2);
    assert_eq!(
        get(
            &front,
            "global-admin",
            PASSWORD,
            &format!("/api/jobs/{other}")
        )
        .await
        .status(),
        200
    );
}

#[tokio::test]
async fn own_password_admin_reset_suspension_reactivation_and_stale_forms_are_real_changes() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let front = Frontend::start(protected_config(dir.path(), &receiver)).await;
    create(&front, "password-user", "user", &["demo"]).await;
    let page = html(&front, "password-user", PASSWORD, "/account").await;
    let old_csrf = hidden(&page, "csrf");
    let old_revision = hidden(&page, "revision");
    let mut fields = vec![
        ("csrf".into(), old_csrf.clone()),
        ("revision".into(), old_revision.clone()),
        (
            "current_password".into(),
            "incorrect-current-password".into(),
        ),
        ("password".into(), NEW_PASSWORD.into()),
        ("password_confirm".into(), NEW_PASSWORD.into()),
    ];
    assert_eq!(
        post(&front, "password-user", PASSWORD, "/account", &fields)
            .await
            .status(),
        403
    );
    assert_eq!(
        get(&front, "password-user", PASSWORD, "/account")
            .await
            .status(),
        200
    );
    replace(&mut fields, "current_password", PASSWORD);
    replace(&mut fields, "password_confirm", RESET_PASSWORD);
    assert_eq!(
        post(&front, "password-user", PASSWORD, "/account", &fields)
            .await
            .status(),
        400
    );
    replace(&mut fields, "password_confirm", NEW_PASSWORD);
    let result = applied(post(&front, "password-user", PASSWORD, "/account", &fields).await).await;
    assert!(result.contains("old password no longer authenticates"));
    assert_eq!(
        get(&front, "password-user", PASSWORD, "/account")
            .await
            .status(),
        401
    );
    let page = html(&front, "password-user", NEW_PASSWORD, "/account").await;
    assert_ne!(hidden(&page, "csrf"), old_csrf);
    assert_ne!(hidden(&page, "revision"), old_revision);
    replace(&mut fields, "current_password", NEW_PASSWORD);
    assert_eq!(
        post(&front, "password-user", NEW_PASSWORD, "/account", &fields)
            .await
            .status(),
        403
    );
    replace(&mut fields, "csrf", &hidden(&page, "csrf"));
    assert_eq!(
        post(&front, "password-user", NEW_PASSWORD, "/account", &fields)
            .await
            .status(),
        409
    );

    let admin_page = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
    let mut reset = update_fields(&admin_page, "password-user", "user", true, &["demo"]);
    reset.extend([
        ("password".into(), RESET_PASSWORD.into()),
        ("password_confirm".into(), RESET_PASSWORD.into()),
    ]);
    applied(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &reset,
        )
        .await,
    )
    .await;
    assert_eq!(
        get(&front, "password-user", NEW_PASSWORD, "/account")
            .await
            .status(),
        401
    );
    assert_eq!(
        get(&front, "password-user", RESET_PASSWORD, "/account")
            .await
            .status(),
        200
    );
    assert_eq!(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &reset
        )
        .await
        .status(),
        409
    );

    let admin_page = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
    let suspend = update_fields(&admin_page, "password-user", "user", false, &["demo"]);
    applied(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &suspend,
        )
        .await,
    )
    .await;
    assert_eq!(
        get(&front, "password-user", RESET_PASSWORD, "/account")
            .await
            .status(),
        401
    );
    let admin_page = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
    let duplicate = create_fields(&admin_page, "password-user", PASSWORD, "user", &["demo"]);
    assert_eq!(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &duplicate
        )
        .await
        .status(),
        409
    );
    let reactivate = update_fields(&admin_page, "password-user", "user", true, &["demo"]);
    applied(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &reactivate,
        )
        .await,
    )
    .await;
    assert_eq!(
        get(&front, "password-user", RESET_PASSWORD, "/account")
            .await
            .status(),
        200
    );
    assert_eq!(
        get(&front, "password-user", PASSWORD, "/account")
            .await
            .status(),
        401
    );

    let admin_page = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
    let mut own_reset = update_fields(&admin_page, FIXTURE_ADMIN, "admin", true, &[]);
    own_reset.extend([
        ("password".into(), NEW_PASSWORD.into()),
        ("password_confirm".into(), NEW_PASSWORD.into()),
    ]);
    let result = applied(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &own_reset,
        )
        .await,
    )
    .await;
    assert!(result.contains("CSRF token have changed"));
    assert_eq!(
        get(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users")
            .await
            .status(),
        401
    );
    assert_eq!(
        get(&front, FIXTURE_ADMIN, NEW_PASSWORD, "/admin/users")
            .await
            .status(),
        200
    );
    let persisted = std::fs::read_to_string(dir.path().join("access.toml")).unwrap();
    for password in [PASSWORD, NEW_PASSWORD, RESET_PASSWORD] {
        assert!(!persisted.contains(password));
    }
}

#[tokio::test]
async fn csrf_origin_fetch_metadata_bounds_roles_and_unknown_grants_reject_without_changes() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let cfg = protected_config(dir.path(), &receiver);
    let access_path = dir.path().join("access.toml");
    let mut access: ferriprint::auth::AccessState =
        toml::from_str(&std::fs::read_to_string(&access_path).unwrap()).unwrap();
    access.reserved.insert("historical-owner".into());
    std::fs::write(&access_path, toml::to_string_pretty(&access).unwrap()).unwrap();
    let front = Frontend::start(cfg).await;
    let page = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
    assert!(!page.contains("action=\"/admin/anonymous\""));
    let fields = create_fields(&page, "rejected-user", PASSWORD, "user", &["demo"]);
    let before = std::fs::read(&access_path).unwrap();
    let expected_origin = origin(&front);
    for (request_origin, fetch_site) in [
        (None, None),
        (Some("null"), None),
        (Some("https://untrusted.example"), None),
        (Some(expected_origin.as_str()), Some("cross-site")),
    ] {
        let mut builder = client()
            .post(format!("http://{}/admin/users", front.address))
            .basic_auth(FIXTURE_ADMIN, Some(FIXTURE_PASSWORD))
            .form(&fields);
        if let Some(value) = request_origin {
            builder = builder.header("origin", value);
        }
        if let Some(value) = fetch_site {
            builder = builder.header("sec-fetch-site", value);
        }
        let response = builder.send().await.unwrap();
        assert_eq!(response.status(), 403);
        no_credentials(&response.text().await.unwrap(), &[PASSWORD]);
    }
    let mut bad_csrf = fields.clone();
    replace(&mut bad_csrf, "csrf", "not-the-current-token");
    assert_eq!(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &bad_csrf
        )
        .await
        .status(),
        403
    );
    assert_eq!(
        post(&front, "bob", FIXTURE_PASSWORD, "/admin/users", &fields)
            .await
            .status(),
        403
    );
    let bob = html(&front, "bob", FIXTURE_PASSWORD, "/account").await;
    let mut foreign_token = fields.clone();
    replace(&mut foreign_token, "csrf", &hidden(&bob, "csrf"));
    assert_eq!(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &foreign_token
        )
        .await
        .status(),
        403
    );
    let oversized_password = "x".repeat(1025);
    for (field, value) in [
        ("role", "Admin"),
        ("role", "operator"),
        ("printer", "unknown-printer"),
        ("password", "too-short"),
        ("password_confirm", NEW_PASSWORD),
        ("password", oversized_password.as_str()),
        ("intent", "delete"),
    ] {
        let mut invalid = fields.clone();
        replace(&mut invalid, field, value);
        let response = post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &invalid,
        )
        .await;
        assert_eq!(response.status(), 400);
        let body = response.text().await.unwrap();
        no_credentials(&body, &[PASSWORD, NEW_PASSWORD]);
        if field == "password" {
            assert!(!body.contains(value));
        }
    }
    let mut duplicate = fields.clone();
    duplicate.push(("role".into(), "user".into()));
    assert_eq!(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &duplicate
        )
        .await
        .status(),
        400
    );
    let mut rename = fields.clone();
    rename.push(("rename".into(), "replacement-owner".into()));
    assert_eq!(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &rename
        )
        .await
        .status(),
        400
    );
    let response = client()
        .post(format!("http://{}/admin/users", front.address))
        .basic_auth(FIXTURE_ADMIN, Some(FIXTURE_PASSWORD))
        .header("origin", origin(&front))
        .header("content-type", "application/x-www-form-urlencoded")
        .body(format!("password={}", "x".repeat(128 * 1024)))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 413);
    assert_eq!(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/anonymous",
            &vec![("csrf".into(), hidden(&page, "csrf"))]
        )
        .await
        .status(),
        403
    );
    let reserved = create_fields(&page, "historical-owner", PASSWORD, "user", &["demo"]);
    assert_eq!(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &reserved
        )
        .await
        .status(),
        409
    );
    assert_eq!(std::fs::read(&access_path).unwrap(), before);
    assert_eq!(
        get(&front, "rejected-user", PASSWORD, "/account")
            .await
            .status(),
        401
    );

    // These valid legacy identities must not be lost to browser dot-path normalization.
    for username in [".", ".."] {
        create(&front, username, "user", &["demo"]).await;
        let page = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
        let fields = update_fields(&page, username, "user", false, &[]);
        applied(
            post(
                &front,
                FIXTURE_ADMIN,
                FIXTURE_PASSWORD,
                "/admin/users",
                &fields,
            )
            .await,
        )
        .await;
        assert_eq!(
            get(&front, username, PASSWORD, "/account").await.status(),
            401
        );
    }
}

#[tokio::test]
async fn anonymous_grants_are_explicit_and_never_an_authenticated_permission_fallback() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = config(dir.path(), &receiver.uri(), 0);
    add_printer(&mut cfg, "other");
    let front = Frontend::start(cfg).await;
    let page = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
    assert!(page.contains("action=\"/admin/anonymous\""));
    let fields = vec![
        ("csrf".into(), hidden(&page, "csrf")),
        ("printer".into(), "other".into()),
    ];
    applied(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/anonymous",
            &fields,
        )
        .await,
    )
    .await;
    for (printer, status) in [
        ("demo", StatusCode::ClientErrorNotAuthorized),
        ("other", StatusCode::SuccessfulOk),
    ] {
        let response = client()
            .post(format!("http://{}/ipp/print/{printer}", front.address))
            .header("content-type", "application/ipp")
            .body(request(&front, printer, Operation::GetPrinterAttributes, None).to_bytes())
            .send()
            .await
            .unwrap();
        assert_eq!(decode(response).await.header().status_code(), status);
    }
    assert_eq!(
        client()
            .get(format!("http://{}/account", front.address))
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    assert_eq!(
        client()
            .post(format!("http://{}/account", front.address))
            .header("origin", origin(&front))
            .form(&[("csrf", "anonymous-cannot-have-a-token")])
            .send()
            .await
            .unwrap()
            .status(),
        403
    );
    create(&front, "no-grants", "user", &[]).await;
    let reply = ipp(
        &front,
        "no-grants",
        PASSWORD,
        "other",
        request(&front, "other", Operation::GetPrinterAttributes, None),
        &[],
    )
    .await;
    assert_eq!(
        reply.header().status_code(),
        StatusCode::ClientErrorNotAuthorized
    );
    let no_grants = json(get(&front, "no-grants", PASSWORD, "/api/printers").await).await;
    assert!(no_grants.as_array().unwrap().is_empty());
    assert_eq!(
        post(&front, "no-grants", PASSWORD, "/admin/anonymous", &fields)
            .await
            .status(),
        403
    );
    let mut bad = fields.clone();
    replace(&mut bad, "printer", "unknown-printer");
    assert_eq!(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/anonymous",
            &bad
        )
        .await
        .status(),
        400
    );
}

async fn wait_for_upload(front: &Frontend, job: i32) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let response = get(
                front,
                FIXTURE_ADMIN,
                FIXTURE_PASSWORD,
                &format!("/api/jobs/{job}"),
            )
            .await;
            assert_eq!(response.status(), 200);
            let body = json(response).await;
            if body["phase"] == "uploading" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("stream must reach the real uploading phase before revocation");
}

#[tokio::test]
async fn browser_revocation_and_suspension_stop_slow_uploads_and_queued_submission() {
    for suspend in [false, true] {
        let dir = tempfile::tempdir().unwrap();
        let receiver = Receiver::start().await;
        let front = Frontend::start(protected_config(dir.path(), &receiver)).await;
        create(&front, "upload-user", "user", &["demo"]).await;
        let queued = id(&ipp(
            &front,
            "upload-user",
            PASSWORD,
            "demo",
            request(&front, "demo", Operation::PrintJob, None),
            b"%PDF-1.4\nqueued\n%%EOF",
        )
        .await);
        let held = id(&ipp(
            &front,
            "upload-user",
            PASSWORD,
            "demo",
            request(&front, "demo", Operation::CreateJob, None),
            &[],
        )
        .await);
        let mut first = request(&front, "demo", Operation::SendDocument, Some(held))
            .to_bytes()
            .to_vec();
        first.extend_from_slice(b"%PDF-1.4\npartial-document");
        let release = Arc::new(tokio::sync::Notify::new());
        let gate = release.clone();
        let stream = futures_util::stream::iter([Ok::<_, std::io::Error>(bytes::Bytes::from(
            first,
        ))])
        .chain(futures_util::stream::once(async move {
            gate.notified().await;
            Ok(bytes::Bytes::from_static(b"\n%%EOF"))
        }));
        let endpoint = format!("http://{}/ipp/print/demo", front.address);
        let upload = tokio::spawn(async move {
            decode(
                client()
                    .post(endpoint)
                    .basic_auth("upload-user", Some(PASSWORD))
                    .header("content-type", "application/ipp")
                    .body(reqwest::Body::wrap_stream(stream))
                    .send()
                    .await
                    .unwrap(),
            )
            .await
        });
        wait_for_upload(&front, held).await;
        let page = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
        let fields = update_fields(
            &page,
            "upload-user",
            "user",
            !suspend,
            if suspend { &["demo"] } else { &[] },
        );
        applied(
            post(
                &front,
                FIXTURE_ADMIN,
                FIXTURE_PASSWORD,
                "/admin/users",
                &fields,
            )
            .await,
        )
        .await;
        release.notify_one();
        let reply = tokio::time::timeout(Duration::from_secs(5), upload)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            reply.header().status_code(),
            StatusCode::ClientErrorNotAuthorized
        );
        front.app.engine.tick().await.unwrap();
        assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 0);
        assert!(receiver.device.jobs.lock().is_empty());
        for job in [held, queued] {
            let response = get(
                &front,
                FIXTURE_ADMIN,
                FIXTURE_PASSWORD,
                &format!("/api/jobs/{job}"),
            )
            .await;
            assert_eq!(response.status(), 200);
            let body = json(response).await;
            assert_eq!(body["state"], 8);
            assert_ne!(body["phase"], "queued");
            assert_ne!(body["phase"], "submitted");
            assert_eq!(
                get(&front, "upload-user", PASSWORD, &format!("/api/jobs/{job}"))
                    .await
                    .status(),
                if suspend { 401 } else { 403 }
            );
        }
        if !suspend {
            let reply = ipp(
                &front,
                "upload-user",
                PASSWORD,
                "demo",
                request(&front, "demo", Operation::GetPrinterAttributes, None),
                &[],
            )
            .await;
            assert_eq!(
                reply.header().status_code(),
                StatusCode::ClientErrorNotAuthorized
            );
        }
    }
}

#[tokio::test]
async fn concurrent_last_admin_protection_and_managed_account_restart_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let cfg = protected_config(dir.path(), &receiver);
    let restart = cfg.clone();
    let front = Frontend::start(cfg).await;
    let page = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
    let original_revision = hidden(account_article(&page, FIXTURE_ADMIN), "revision");
    for (role, enabled) in [("user", true), ("admin", false)] {
        let fields = update_fields(&page, FIXTURE_ADMIN, role, enabled, &["demo"]);
        let response = post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &fields,
        )
        .await;
        assert_eq!(response.status(), 409);
        assert!(
            response
                .text()
                .await
                .unwrap()
                .contains("final enabled administrator")
        );
    }
    let unchanged = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
    assert_eq!(
        hidden(account_article(&unchanged, FIXTURE_ADMIN), "revision"),
        original_revision
    );
    create(&front, "second-admin", "admin", &[]).await;
    let first_page = html(&front, FIXTURE_ADMIN, FIXTURE_PASSWORD, "/admin/users").await;
    let second_page = html(&front, "second-admin", PASSWORD, "/admin/users").await;
    let first = update_fields(&first_page, FIXTURE_ADMIN, "user", true, &["demo"]);
    let second = update_fields(&second_page, "second-admin", "user", true, &["demo"]);
    let (first, second) = tokio::join!(
        post(
            &front,
            FIXTURE_ADMIN,
            FIXTURE_PASSWORD,
            "/admin/users",
            &first
        ),
        post(&front, "second-admin", PASSWORD, "/admin/users", &second),
    );
    let statuses = [first.status().as_u16(), second.status().as_u16()];
    drop(first);
    drop(second);
    assert!(statuses == [200, 409] || statuses == [409, 200]);
    let (admin, admin_password, demoted, demoted_password) = if statuses[0] == 409 {
        (FIXTURE_ADMIN, FIXTURE_PASSWORD, "second-admin", PASSWORD)
    } else {
        ("second-admin", PASSWORD, FIXTURE_ADMIN, FIXTURE_PASSWORD)
    };
    assert_eq!(
        get(&front, demoted, demoted_password, "/admin/users")
            .await
            .status(),
        403
    );
    let page = html(&front, admin, admin_password, "/admin/users").await;
    let create = create_fields(&page, "persisted-user", PASSWORD, "user", &["demo"]);
    applied(post(&front, admin, admin_password, "/admin/users", &create).await).await;
    let mut held = request(&front, "demo", Operation::CreateJob, None);
    add(
        &mut held,
        OP,
        "job-name",
        name("permanent-owned-history").unwrap(),
    )
    .unwrap();
    let job = id(&ipp(&front, "persisted-user", PASSWORD, "demo", held, &[]).await);
    let page = html(&front, admin, admin_password, "/admin/users").await;
    let mut suspend = update_fields(&page, "persisted-user", "user", false, &["demo"]);
    suspend.extend([
        ("password".into(), RESET_PASSWORD.into()),
        ("password_confirm".into(), RESET_PASSWORD.into()),
    ]);
    applied(post(&front, admin, admin_password, "/admin/users", &suspend).await).await;
    let persisted_revision = hidden(
        account_article(
            &html(&front, admin, admin_password, "/admin/users").await,
            "persisted-user",
        ),
        "revision",
    );
    drop(front);
    tokio::time::sleep(Duration::from_millis(20)).await;
    let front = Frontend::start(restart).await;
    assert_eq!(
        get(&front, demoted, demoted_password, "/admin/users")
            .await
            .status(),
        403
    );
    assert_eq!(
        get(&front, "persisted-user", RESET_PASSWORD, "/account")
            .await
            .status(),
        401
    );
    let page = html(&front, admin, admin_password, "/admin/users").await;
    assert_eq!(
        hidden(account_article(&page, "persisted-user"), "revision"),
        persisted_revision
    );
    let duplicate = create_fields(&page, "persisted-user", PASSWORD, "user", &["demo"]);
    assert_eq!(
        post(&front, admin, admin_password, "/admin/users", &duplicate)
            .await
            .status(),
        409
    );
    let stored = json(get(&front, admin, admin_password, &format!("/api/jobs/{job}")).await).await;
    assert_eq!(stored["id"], job);
    assert_eq!(stored["owner"], "persisted-user");
    assert_eq!(stored["title"], "permanent-owned-history");
    let reactivate = update_fields(&page, "persisted-user", "user", true, &["demo"]);
    applied(post(&front, admin, admin_password, "/admin/users", &reactivate).await).await;
    assert_eq!(
        get(&front, "persisted-user", PASSWORD, "/account")
            .await
            .status(),
        401
    );
    assert_eq!(
        get(
            &front,
            "persisted-user",
            RESET_PASSWORD,
            &format!("/api/jobs/{job}")
        )
        .await
        .status(),
        200
    );
    assert_eq!(receiver.device.attempts.load(Ordering::SeqCst), 0);
}
