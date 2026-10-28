// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{
    admin::{Escape, csrf_valid, error, finish, page as html_page},
    auth::Principal,
    server::App,
};
use axum::{
    Extension, Form, Router,
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Redirect, Response},
    routing::{get, post},
};
use serde::Deserialize;
use std::{fmt::Write, sync::Arc};

pub fn router() -> Router<Arc<App>> {
    Router::new()
        .route("/admin/notifications", get(page))
        .route("/admin/notifications/{printer}", post(update))
        .route("/admin/notifications/deliveries/{id}/retry", post(retry))
        .layer(axum::extract::DefaultBodyLimit::max(4096))
}
pub async fn page(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    if !principal.is_admin() || !app.auth.current(&principal) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let mut body = html_page("Active operational webhook notifications");
    body.push_str("<p>The destination and optional bearer credential are protected startup configuration, never a browser URI/credential editor. Events contain no document, title, owner or password. Delivery is at least once; receivers deduplicate the stable event ID. Unknown/stale physical readings never mean empty paper or a recovery.</p>");
    if !app.notifications.configured() {
        body.push_str("<p class=\"warning\">Webhook transport is not configured or is disabled. No diagnostic or delivery success is claimed. Configure the protected HTTPS destination before enabling alerts.</p>");
    }
    let token = app.auth.csrf(&principal).expect("current administrator");
    for (name, entry) in app.printer_snapshot() {
        let printer = entry.read().await;
        let enabled = match app.notifications.is_enabled(&name) {
            Ok(value) => value,
            Err(_) => {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Notification state unavailable",
                );
            }
        };
        let _ = write!(
            body,
            "<section><h2>{}</h2><p>Automatic transition alerts: {}. Alerts cover failed/stopped jobs, unavailable native/configured telemetry sources, and source-reported paper shortage plus observed recovery.</p><form method=\"post\" action=\"/admin/notifications/{}\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"enabled\" value=\"{}\"><button name=\"intent\" value=\"toggle\">{}</button> <button name=\"intent\" value=\"diagnostic\">Send real diagnostic webhook</button></form></section>",
            Escape(&printer.config.display_name),
            if enabled { "enabled" } else { "disabled" },
            Escape(&name),
            Escape(token),
            if enabled { "false" } else { "true" },
            if enabled {
                "Disable automatic events"
            } else {
                "Enable automatic events"
            }
        );
    }
    let deliveries = match app.notifications.deliveries() {
        Ok(value) => value,
        Err(_) => {
            return error(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Notification delivery evidence unavailable",
            );
        }
    };
    body.push_str("<h2>Durable outbox and actual delivery outcomes</h2><p>Queued is not delivered. HTTP 2xx is the receiver acknowledgement, not proof a downstream human read the notification. Exhausted attempts remain visible and need an explicit retry.</p><table><thead><tr><th>Event</th><th>Printer / source / condition</th><th>Outcome</th><th>Attempts / times</th><th>Action</th></tr></thead><tbody>");
    for delivery in deliveries {
        let _ = write!(
            body,
            "<tr><td><code>{}</code><br>{}</td><td>{}<br>{}<br>{}</td><td>{}<br>{}<br>HTTP {:?}</td><td>{} in this cycle; {} total<br>created {}; delivered {:?}; next {:?}</td><td>",
            Escape(&delivery.id),
            Escape(&delivery.event_type),
            Escape(&delivery.printer),
            Escape(&delivery.source),
            Escape(&delivery.condition),
            Escape(&delivery.status),
            Escape(
                delivery
                    .failure_class
                    .as_deref()
                    .unwrap_or("no recorded transport failure")
            ),
            delivery.http_status,
            delivery.attempts,
            delivery.total_attempts,
            delivery.timestamp,
            delivery.delivered_at,
            delivery.next_attempt_at
        );
        if delivery.status == "exhausted" {
            let _ = write!(
                body,
                "<form method=\"post\" action=\"/admin/notifications/deliveries/{}/retry\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><button>Retry this stable event ID</button></form>",
                Escape(&delivery.id),
                Escape(token)
            );
        }
        body.push_str("</td></tr>");
    }
    body.push_str("</tbody></table>");
    if !app.auth.current(&principal) {
        return StatusCode::FORBIDDEN.into_response();
    }
    finish(body).into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NotificationForm {
    csrf: String,
    intent: String,
    enabled: Option<String>,
}
pub async fn update(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Path(printer): Path<String>,
    headers: HeaderMap,
    Form(form): Form<NotificationForm>,
) -> Response {
    if !principal.is_admin() || !csrf_valid(&app, &principal, &headers, &form.csrf) {
        return error(
            StatusCode::FORBIDDEN,
            "Current administrator and same-origin CSRF token required",
        );
    }
    if app.printer(&printer).is_none() {
        return StatusCode::NOT_FOUND.into_response();
    }
    let result = match form.intent.as_str() {
        "toggle" => match form.enabled.as_deref() {
            Some("true") | Some("false") => {
                app.auth.with_access(&principal, Some(&printer), true, || {
                    app.notifications
                        .set_enabled(&printer, form.enabled.as_deref() == Some("true"))
                })
            }
            _ => {
                return error(
                    StatusCode::BAD_REQUEST,
                    "Explicit notification enabled state required",
                );
            }
        },
        "diagnostic" => app.auth.with_access(&principal, Some(&printer), true, || {
            app.notifications.diagnostic(&printer).map(|_| ())
        }),
        _ => return error(StatusCode::BAD_REQUEST, "Unknown notification action"),
    };
    if let Err(failure) = result {
        return error(StatusCode::BAD_REQUEST, failure);
    }
    if form.intent == "diagnostic"
        && let Err(_) = app.notifications.tick().await
    {
        return error(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Diagnostic queued durably, but notification processing failed; inspect the durable outbox before retrying",
        );
    }
    Redirect::to("/admin/notifications").into_response()
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RetryForm {
    csrf: String,
}
pub async fn retry(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Path(id): Path<String>,
    headers: HeaderMap,
    Form(form): Form<RetryForm>,
) -> Response {
    if !principal.is_admin() || !csrf_valid(&app, &principal, &headers, &form.csrf) {
        return error(
            StatusCode::FORBIDDEN,
            "Current administrator and same-origin CSRF token required",
        );
    }
    match app
        .auth
        .with_access(&principal, None, true, || app.notifications.retry(&id))
    {
        Ok(()) => Redirect::to("/admin/notifications").into_response(),
        Err(failure) => error(StatusCode::BAD_REQUEST, failure),
    }
}
