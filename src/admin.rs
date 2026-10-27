// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{
    auth::Principal,
    config::ProfileSettings,
    jobs::JobRecord,
    profile::{PpdInventory, PrinterProfile},
    server::App,
    storage::StagedFile,
};
use axum::{
    Extension, Form, Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{Html, IntoResponse, Redirect, Response},
};
use serde::Deserialize;
use std::{
    collections::HashMap,
    fmt::{self, Write},
    sync::Arc,
};

/// Escape once while writing; never copy/replace the complete input repeatedly.
struct Escape<'a>(&'a str);
impl fmt::Display for Escape<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        for c in self.0.chars() {
            match c {
                '&' => f.write_str("&amp;")?,
                '<' => f.write_str("&lt;")?,
                '>' => f.write_str("&gt;")?,
                '"' => f.write_str("&quot;")?,
                '\'' => f.write_str("&#39;")?,
                _ => f.write_char(c)?,
            }
        }
        Ok(())
    }
}
fn page(title: &str) -> String {
    format!(
        "<!doctype html><html lang=\"en\"><meta charset=\"utf-8\"><meta name=\"viewport\" content=\"width=device-width\"><title>{}</title><style>body{{font:16px system-ui;max-width:1100px;margin:2rem auto;padding:0 1rem}}textarea{{width:100%;min-height:30rem;font:14px monospace}}pre{{white-space:pre-wrap;overflow-wrap:anywhere}}td,th{{padding:.4rem;text-align:left;border-bottom:1px solid #ddd}}button{{padding:.5rem}}details{{margin:1rem 0}}.warning{{color:#8b2000}}</style><nav><a href=\"/\">My jobs</a> · <a href=\"/admin\">Administration</a> · <a href=\"/source\">Corresponding source</a></nav><h1>{}</h1>",
        Escape(title),
        Escape(title)
    )
}
fn finish(mut body: String) -> Html<String> {
    body.push_str("<footer><p>Copyright © 2026 Robert Ressl. GNU AGPL version 3 only. <a href=\"/source\">Corresponding Source matching this build</a>. Native CUPS remains the renderer; no printer-program certification is claimed.</p></footer></html>");
    Html(body)
}
fn error(status: StatusCode, message: impl fmt::Display) -> Response {
    let mut body = page("Ferriprint request not applied");
    let _ = write!(
        body,
        "<p class=\"warning\">{}</p><p><a href=\"/admin\">Back to administration</a></p>",
        Escape(&message.to_string())
    );
    (status, finish(body)).into_response()
}
fn quantity(body: &mut String, value: Option<&crate::telemetry::Quantity>) {
    use crate::telemetry::MeasurementUnit;
    match value {
        Some(value) => {
            if let Some(known) = value.value {
                let _ = write!(body, "{known}");
                match &value.unit {
                    MeasurementUnit::Percent => body.push('%'),
                    MeasurementUnit::Sheets => body.push_str(" sheets"),
                    MeasurementUnit::HundredthsMillimeter => body.push_str(" × 0.01 mm"),
                    MeasurementUnit::SourceReported(unit) => {
                        let _ = write!(body, " {}", Escape(unit));
                    }
                    MeasurementUnit::Unknown => body.push_str(" (unit unknown)"),
                }
            } else {
                let _ = write!(body, "unknown (source value {})", value.raw);
            }
        }
        None => body.push_str("unknown"),
    }
}
fn telemetry_html(body: &mut String, snapshot: &crate::telemetry::TelemetrySnapshot) {
    use crate::telemetry::Freshness;
    let freshness = match snapshot.freshness {
        Freshness::Fresh => "fresh metadata fetch",
        Freshness::Stale => "STALE metadata fetch",
        Freshness::Unknown => "unknown / not observed",
    };
    body.push_str("<section><h3>Paper, ink and supplies</h3>");
    if let Some(source) = &snapshot.source {
        let _ = write!(
            body,
            "<p>Source: {:?}, <code>{}</code></p>",
            source.kind,
            Escape(&source.uri)
        );
    } else {
        body.push_str("<p>Source not configured; physical supplies are unknown.</p>");
    }
    let _ = write!(body, "<p>Freshness: {freshness}. ");
    if let Some(observed) = snapshot.observed_at {
        let age = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs()
            .saturating_sub(observed);
        let _ = write!(
            body,
            "Retrieved at UNIX second {observed} ({age} seconds ago). "
        );
    }
    body.push_str("Physical measurement freshness: unknown. Retrieval time is not a sensor measurement timestamp.</p>");
    if let Some(error) = &snapshot.last_error {
        let _ = write!(
            body,
            "<p class=\"warning\">Refresh failed: {}</p>",
            Escape(error)
        );
    }
    let values = &snapshot.measurements;
    if values.markers.is_empty() {
        body.push_str(
            "<p>Ink/toner marker readings: unknown or unsupported (no reported values).</p>",
        );
    } else {
        body.push_str("<table><caption>Source-reported ink/toner values</caption><thead><tr><th>Name</th><th>Type</th><th>Color</th><th>Level</th><th>Low threshold</th><th>High threshold</th></tr></thead><tbody>");
        for marker in &values.markers {
            let _ = write!(
                body,
                "<tr><td>{}</td><td>{}</td><td>{}</td><td>",
                Escape(marker.name.as_deref().unwrap_or("unnamed marker")),
                Escape(marker.marker_type.as_deref().unwrap_or("unknown")),
                Escape(marker.color.as_deref().unwrap_or("unknown"))
            );
            quantity(body, marker.level.as_ref());
            body.push_str("</td><td>");
            quantity(body, marker.low_level.as_ref());
            body.push_str("</td><td>");
            quantity(body, marker.high_level.as_ref());
            body.push_str("</td></tr>");
        }
        body.push_str("</tbody></table>");
    }
    if let Some(change) = values.marker_change_time {
        let _ = write!(
            body,
            "<p>Raw marker-change-time: {change} (source-defined seconds; not asserted to be UNIX time or the last measurement time).</p>"
        );
    }
    if values.input_trays.is_empty() {
        body.push_str(
            "<p>Physical input-tray contents: unknown (no tray measurements reported).</p>",
        );
    } else {
        body.push_str("<ul>");
        for tray in &values.input_trays {
            let _ = write!(
                body,
                "<li>Input tray {}: ",
                Escape(tray.name.as_deref().unwrap_or("unnamed"))
            );
            quantity(body, tray.level.as_ref());
            body.push_str("</li>");
        }
        body.push_str("</ul>");
    }
    if !values.media_ready.is_empty() {
        let _ = write!(
            body,
            "<p>Source-reported ready media: {}. This is not proof of physically loaded paper.</p>",
            Escape(&values.media_ready.join(", "))
        );
    }
    if let Ok(value) = serde_json::to_string_pretty(snapshot) {
        let _ = write!(
            body,
            "<details><summary>All source reports, supplies, output trays and availability details</summary><pre>{}</pre></details>",
            Escape(&value)
        );
    }
    body.push_str("</section>");
}
fn selector<'a>(
    body: &mut String,
    label: &str,
    name: &str,
    values: impl IntoIterator<Item = &'a str>,
    selected: &str,
) {
    let _ = write!(
        body,
        "<p><label for=\"{}\">{} </label><select id=\"{}\" name=\"{}\">",
        Escape(name),
        Escape(label),
        Escape(name),
        Escape(name)
    );
    for value in values {
        let _ = write!(
            body,
            "<option value=\"{}\"{}>{}</option>",
            Escape(value),
            if value == selected { " selected" } else { "" },
            Escape(value)
        );
    }
    body.push_str("</select></p>");
}
async fn visible_jobs(app: &App, principal: &Principal) -> anyhow::Result<Vec<JobRecord>> {
    let mut jobs = Vec::new();
    for name in app.printers.keys() {
        jobs.extend(
            app.engine
                .query_jobs(
                    name,
                    (!principal.is_admin()).then_some(principal.username.as_str()),
                    "all",
                    1000,
                )
                .await?,
        );
    }
    jobs.sort_unstable_by_key(|job| std::cmp::Reverse(job.id));
    Ok(jobs)
}
fn job_state_name(state: i32) -> &'static str {
    match state {
        3 => "Pending",
        4 => "Held",
        5 => "Processing",
        6 => "Stopped",
        7 => "Canceled",
        8 => "Aborted",
        9 => "Completed",
        _ => "Unknown",
    }
}
fn job_html(body: &mut String, job: &JobRecord, csrf: Option<&str>) {
    let _ = write!(
        body,
        "<article id=\"job-{}\"><h3>Job {}: {}</h3><p>Printer {} · Owner {} · State {} ({}) · Phase {} · Upstream ID {}</p><p>{}</p><p>Reasons: {}</p>",
        job.id,
        job.id,
        Escape(&job.title),
        Escape(&job.printer),
        Escape(&job.owner),
        job_state_name(job.state),
        job.state,
        Escape(&job.phase),
        job.upstream_id
            .map(|id| id.to_string())
            .unwrap_or_else(|| "not assigned".into()),
        Escape(&job.message),
        Escape(&job.reasons.join(", "))
    );
    if let Ok(resolved) = serde_json::to_string_pretty(&job.resolved) {
        let _ = write!(
            body,
            "<details><summary>Immutable effective native settings</summary><pre>{}</pre></details>",
            Escape(&resolved)
        );
    }
    if job.state < 7
        && let Some(token) = csrf
    {
        let _ = write!(
            body,
            "<form method=\"post\" action=\"/api/jobs/{}/cancel\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button>Cancel job {}</button></form>",
            job.id,
            Escape(token),
            job.id
        );
    }
    body.push_str("</article>");
}
pub async fn status_page(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    let mut body = page("Ferriprint jobs and printer status");
    let _ = write!(
        body,
        "<p>Principal: {} ({}). Job identity comes from verified credentials, never requesting-user-name.</p>",
        Escape(&principal.username),
        if principal.is_admin() {
            "administrator: all owners visible"
        } else if principal.authenticated {
            "authenticated user: own jobs only"
        } else {
            "deliberately shared trusted-LAN owner"
        }
    );
    for (name, entry) in &app.printers {
        let printer = entry.read().await;
        let _ = write!(
            body,
            "<section><h2>{}</h2><p><code>{}</code></p>",
            Escape(&printer.config.display_name),
            Escape(&app.config.printer_uri(name))
        );
        match printer.upstream.inspect().await {
            Ok(status) => {
                let _ = write!(
                    body,
                    "<p>Native queue state {} · Accepting jobs {} · Reasons {}</p>",
                    status.state,
                    status.accepting,
                    Escape(&status.reasons.join(", "))
                );
            }
            Err(_) => body.push_str(
                "<p class=\"warning\">Native queue state unavailable (unknown, not healthy).</p>",
            ),
        }
        let telemetry = printer.telemetry.snapshot();
        telemetry_html(&mut body, &telemetry);
        if !printer.profile.capabilities().ready.is_empty()
            && let Ok(ready) = serde_json::to_string_pretty(&printer.profile.capabilities().ready)
        {
            let _ = write!(
                body,
                "<details><summary>Operator-declared media (unobserved)</summary><pre>{}</pre></details>",
                Escape(&ready)
            );
        }
        body.push_str("</section>");
    }
    match visible_jobs(&app, &principal).await {
        Ok(jobs) => {
            for job in &jobs {
                job_html(&mut body, job, app.auth.csrf(&principal));
            }
        }
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "Job storage unavailable"),
    }
    finish(body).into_response()
}
pub async fn admin_page(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    if !principal.is_admin() {
        return error(StatusCode::FORBIDDEN, "Administrator role required");
    }
    let mut body = page("Ferriprint administration");
    body.push_str("<p>Edit, validate, save and immediately activate existing printer mapping/default settings below. Queue identity, PPD paths, TLS, network admission and credentials are protected bootstrap configuration; new queues are installed with the setup CLI, never a browser path/URI editor.</p><ul>");
    for (name, entry) in &app.printers {
        let printer = entry.read().await;
        let _ = write!(
            body,
            "<li><a href=\"/admin/printers/{}\">{}</a> — active revision {}</li>",
            Escape(name),
            Escape(&printer.config.display_name),
            printer.revision
        );
    }
    body.push_str("</ul><h2>Jobs (explicit administrator access)</h2>");
    match visible_jobs(&app, &principal).await {
        Ok(jobs) => {
            for job in &jobs {
                job_html(&mut body, job, app.auth.csrf(&principal));
            }
        }
        Err(_) => return error(StatusCode::INTERNAL_SERVER_ERROR, "Job storage unavailable"),
    }
    finish(body).into_response()
}
pub async fn profile_page(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Path(name): Path<String>,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    if !principal.is_admin() {
        return error(StatusCode::FORBIDDEN, "Administrator role required");
    }
    let Some(entry) = app.printers.get(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let printer = entry.read().await;
    let mut body = page(&format!("Profile: {}", printer.config.display_name));
    if query.get("applied").is_some_and(|v| v == "1") {
        body.push_str("<p>Profile durably saved and activated. New jobs use these settings.</p>");
    }
    let _ = write!(
        body,
        "<p>Active revision {}. Save requires no active/held/uncertain jobs on this printer. Existing job settings and protected native queue identity can never be changed by this editor.</p>",
        printer.revision
    );
    let mut editable = match toml::Table::try_from(printer.profile.settings()) {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Cannot serialize active profile",
            );
        }
    };
    editable.remove("defaults");
    let settings = match toml::to_string_pretty(&editable) {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Cannot serialize mappings",
            );
        }
    };
    let _ = write!(
        body,
        "<form method=\"post\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"revision\" value=\"{}\"><fieldset><legend>Default settings</legend>",
        Escape(app.auth.csrf(&principal).expect("authenticated admin")),
        printer.revision
    );
    let cap = printer.profile.capabilities();
    let defaults = &printer.profile.settings().defaults;
    let media: std::collections::BTreeSet<_> = cap.media.iter().map(|m| m.name.as_str()).collect();
    selector(
        &mut body,
        "Paper size",
        "default_media",
        media,
        &defaults.media,
    );
    let _ = write!(
        body,
        "<p><label><input type=\"checkbox\" name=\"default_borderless\" value=\"true\"{}> Borderless native media variant (must be supported)</label></p>",
        if defaults.borderless { " checked" } else { "" }
    );
    selector(
        &mut body,
        "Paper source",
        "default_source",
        cap.sources.iter().map(String::as_str),
        &defaults.source,
    );
    selector(
        &mut body,
        "Material",
        "default_media_type",
        cap.media_types.iter().map(String::as_str),
        &defaults.media_type,
    );
    body.push_str("<p><label for=\"default_quality\">Quality </label><select id=\"default_quality\" name=\"default_quality\">");
    for quality in &cap.qualities {
        let _ = write!(
            body,
            "<option value=\"{}\"{}>{}: {}</option>",
            quality,
            if *quality == defaults.quality {
                " selected"
            } else {
                ""
            },
            quality,
            match quality {
                3 => "Draft",
                5 => "High",
                _ => "Normal",
            }
        );
    }
    body.push_str("</select></p>");
    selector(
        &mut body,
        "Color",
        "default_color_mode",
        cap.color_modes.iter().map(String::as_str),
        &defaults.color_mode,
    );
    selector(
        &mut body,
        "Sides",
        "default_sides",
        cap.sides.iter().map(String::as_str),
        &defaults.sides,
    );
    let _ = write!(
        body,
        "</fieldset><details><summary>Advanced native mappings and capabilities</summary><p>Versioned TOML below excludes defaults, which are edited by the selectors above. Exact native choices appear in the provisioned PPD inventory. No semantic quality guesses or filesystem paths are accepted.</p><label for=\"settings\">Mapping and capability settings</label><textarea id=\"settings\" name=\"settings\" required maxlength=\"262144\">{}</textarea></details><p><button name=\"intent\" value=\"validate\">Validate and preview only</button> <button name=\"intent\" value=\"save\">Save and activate</button></p></form>",
        Escape(&settings)
    );
    if let Ok(defaults) = serde_json::to_string_pretty(&printer.profile.capabilities().defaults) {
        let _ = write!(
            body,
            "<details open><summary>Effective resolved defaults</summary><pre>{}</pre></details>",
            Escape(&defaults)
        );
    }
    match PpdInventory::load(&printer.config.ppd)
        .and_then(|inventory| Ok(serde_json::to_string_pretty(&inventory)?))
    {
        Ok(inventory) => {
            let _ = write!(
                body,
                "<details><summary>Provisioned native PPD media, margins, choices and constraints</summary><pre>{}</pre></details>",
                Escape(&inventory)
            );
        }
        Err(_) => body
            .push_str("<p>Provisioned PPD inventory unavailable; saves will fail validation.</p>"),
    }
    finish(body).into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ProfileForm {
    csrf: String,
    revision: u64,
    settings: String,
    intent: String,
    default_media: String,
    default_borderless: Option<String>,
    default_source: String,
    default_media_type: String,
    default_quality: i32,
    default_color_mode: String,
    default_sides: String,
}
fn csrf_valid(app: &App, principal: &Principal, headers: &HeaderMap, token: &str) -> bool {
    if !principal.authenticated || !app.auth.verify_csrf(principal, token) {
        return false;
    }
    let Ok(public) = crate::wire::http_url(&app.config.public_url) else {
        return false;
    };
    if headers.get("origin").and_then(|v| v.to_str().ok())
        != Some(public.origin().ascii_serialization().as_str())
    {
        return false;
    }
    !headers
        .get("sec-fetch-site")
        .is_some_and(|value| value != "same-origin" && value != "none")
}
pub async fn save_profile(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Form(form): Form<ProfileForm>,
) -> Response {
    if !principal.is_admin() || !csrf_valid(&app, &principal, &headers, &form.csrf) {
        return error(
            StatusCode::FORBIDDEN,
            "Administrator role and valid same-origin CSRF token required",
        );
    }
    if form.settings.len() > 256 * 1024 || !matches!(form.intent.as_str(), "validate" | "save") {
        return error(StatusCode::BAD_REQUEST, "Invalid profile request");
    }
    let mut table: toml::Table = match toml::from_str(&form.settings) {
        Ok(value) => value,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    if table.contains_key("defaults") {
        return error(
            StatusCode::BAD_REQUEST,
            "Use the structured default selectors; defaults are not accepted in advanced mappings",
        );
    }
    if form
        .default_borderless
        .as_deref()
        .is_some_and(|v| v != "true")
    {
        return error(StatusCode::BAD_REQUEST, "Invalid borderless selection");
    }
    let defaults = crate::config::Defaults {
        media: form.default_media,
        borderless: form.default_borderless.is_some(),
        source: form.default_source,
        media_type: form.default_media_type,
        quality: form.default_quality,
        color_mode: form.default_color_mode,
        sides: form.default_sides,
    };
    let value = match toml::Value::try_from(&defaults) {
        Ok(value) => value,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    table.insert("defaults".into(), value);
    let settings: ProfileSettings = match table.try_into() {
        Ok(value) => value,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let Some(entry) = app.printers.get(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut printer = entry.write().await;
    if printer.revision != form.revision {
        return error(
            StatusCode::CONFLICT,
            "Profile changed; reload before editing",
        );
    }
    let mut profile = match PrinterProfile::load(&printer.config, &settings) {
        Ok(value) => value,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let status = match printer.upstream.inspect().await {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::BAD_GATEWAY,
                "Native queue unavailable; profile not applied",
            );
        }
    };
    if let Err(e) = profile.restrict_formats(&status.document_formats) {
        return error(StatusCode::BAD_REQUEST, e);
    }
    let urf = match profile.urf_supported(&status.urf_supported) {
        Ok(value) => value,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    if form.intent == "validate" {
        let mut body = page("Profile validated; not saved");
        let _ = write!(
            body,
            "<p>Shared PPD parser, constraints and live native decoder intersection accepted this profile. Nothing changed.</p><h2>Effective resolved defaults</h2><pre>{}</pre><p><a href=\"/admin/printers/{}\">Return to active profile</a></p>",
            Escape(
                &serde_json::to_string_pretty(&profile.capabilities().defaults).unwrap_or_default()
            ),
            Escape(&name)
        );
        return finish(body).into_response();
    }
    match app.engine.active_count(&name).await {
        Ok(0) => {}
        Ok(_) => {
            return error(
                StatusCode::CONFLICT,
                "Printer has active/held/uncertain jobs; profile not saved or applied",
            );
        }
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Cannot verify active-job gate; profile not applied",
            );
        }
    }
    let contents = match toml::to_string_pretty(&settings) {
        Ok(value) => value,
        Err(e) => return error(StatusCode::BAD_REQUEST, e),
    };
    let staged = match StagedFile::new(&app.config.profile_path(&name), contents.as_bytes()) {
        Ok(value) => value,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let config_change_time = match staged.modified_at().and_then(|modified| {
        crate::profile::configuration_change_time(
            modified,
            &printer.config.ppd,
            app.engine.time_origin(),
        )
    }) {
        Ok(value) => value,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    let durability = match staged.install() {
        Ok(value) => value,
        Err(e) => return error(StatusCode::INTERNAL_SERVER_ERROR, e),
    };
    // The rename is the authoritative cutover. These assignments cannot fail.
    printer.profile = Arc::new(profile);
    printer.urf_supported = urf;
    printer.pwg_types = status.pwg_types;
    printer.revision += 1;
    printer.config_change_time = config_change_time;
    app.profile_changes
        .send_modify(|revision| *revision = revision.wrapping_add(1));
    if durability.is_err() {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Profile APPLIED, but directory synchronization failed; crash durability is unconfirmed. Do not assume the old profile is active.",
        );
    }
    Redirect::to(&format!("/admin/printers/{name}?applied=1")).into_response()
}
pub async fn jobs_api(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    match visible_jobs(&app, &principal).await {
        Ok(jobs) => Json(jobs).into_response(),
        Err(_) => error(StatusCode::INTERNAL_SERVER_ERROR, "Job storage unavailable"),
    }
}
pub async fn job_api(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<i32>,
) -> Response {
    match app.engine.get_job(id).await {
        Ok(Some(job)) if principal.can_access(&job.owner) => Json(job).into_response(),
        Ok(Some(_)) => StatusCode::FORBIDDEN.into_response(),
        Ok(None) => StatusCode::NOT_FOUND.into_response(),
        Err(_) => StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    }
}
#[derive(Deserialize)]
pub struct CancelForm {
    csrf: String,
}
pub async fn cancel_job(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<i32>,
    headers: HeaderMap,
    Form(form): Form<CancelForm>,
) -> Response {
    if !csrf_valid(&app, &principal, &headers, &form.csrf) {
        return error(
            StatusCode::FORBIDDEN,
            "Authenticated same-origin CSRF token required; anonymous web mutation is disabled",
        );
    }
    let job = match app.engine.get_job(id).await {
        Ok(Some(job)) => job,
        Ok(None) => return StatusCode::NOT_FOUND.into_response(),
        Err(_) => return StatusCode::INTERNAL_SERVER_ERROR.into_response(),
    };
    if !principal.can_access(&job.owner) {
        return StatusCode::FORBIDDEN.into_response();
    }
    match app.engine.cancel_job(id, &job.owner).await {
        Ok(_) => Redirect::to("/").into_response(),
        Err(e) => error(StatusCode::CONFLICT, e),
    }
}
pub async fn printers_api(
    State(app): State<Arc<App>>,
    Extension(_principal): Extension<Principal>,
) -> Response {
    let mut printers = Vec::with_capacity(app.printers.len());
    for (name, entry) in &app.printers {
        let printer = entry.read().await;
        let status = printer.upstream.inspect().await.ok();
        printers.push(serde_json::json!({"name":name,"display_name":printer.config.display_name,"revision":printer.revision,"effective_defaults":printer.profile.capabilities().defaults,"upstream_state":status.as_ref().map(|s|s.state),"upstream_accepting":status.as_ref().map(|s|s.accepting),"upstream_reasons":status.as_ref().map(|s|&s.reasons),"telemetry":printer.telemetry.snapshot()}));
    }
    Json(printers).into_response()
}
