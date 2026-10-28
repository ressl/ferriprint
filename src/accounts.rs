// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{
    admin::{Escape, csrf_valid, error, finish, page},
    auth::{AccountView, AuthError, Principal},
    config::Role,
    server::App,
};
use axum::{
    Extension, Form, Router,
    extract::{DefaultBodyLimit, State, rejection::FormRejection},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{collections::BTreeSet, fmt::Write, sync::Arc};
use subtle::ConstantTimeEq;
use zeroize::{Zeroize, Zeroizing};

pub fn router() -> Router<Arc<App>> {
    Router::new()
        .route(
            "/account",
            get(account_page)
                .post(change_password)
                .layer(DefaultBodyLimit::max(16 * 1024)),
        )
        .route(
            "/admin/users",
            get(users_page)
                .post(save_account)
                .layer(DefaultBodyLimit::max(128 * 1024)),
        )
        .route(
            "/admin/anonymous",
            post(anonymous_grants).layer(DefaultBodyLimit::max(128 * 1024)),
        )
}

struct FormFailure(StatusCode, &'static str);
impl IntoResponse for FormFailure {
    fn into_response(self) -> Response {
        error(self.0, self.1)
    }
}
// URL-encoded checkboxes may repeat `printer`. Keep decoded passwords owned and
// erase even rejected forms; never put submitted values into error responses.
struct Fields(Vec<(String, String)>);
impl Drop for Fields {
    fn drop(&mut self) {
        for (_, value) in &mut self.0 {
            value.zeroize();
        }
    }
}
impl Fields {
    fn parse(
        form: Result<Form<Vec<(String, String)>>, FormRejection>,
        allowed: &[&str],
    ) -> Result<Self, FormFailure> {
        let form = match form {
            Ok(Form(form)) => Self(form),
            Err(rejection) => {
                return Err(FormFailure(
                    rejection.status(),
                    "Invalid or oversized account form",
                ));
            }
        };
        let mut seen = BTreeSet::new();
        for (name, _) in &form.0 {
            if !allowed.contains(&name.as_str())
                || (name != "printer" && !seen.insert(name.as_str()))
            {
                return Err(FormFailure(
                    StatusCode::BAD_REQUEST,
                    "Unknown or duplicate form field",
                ));
            }
        }
        drop(seen);
        Ok(form)
    }
    fn value(&self, name: &str) -> Option<&str> {
        self.0
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.as_str())
    }
    fn secret(&mut self, name: &str) -> Zeroizing<String> {
        Zeroizing::new(
            self.0
                .iter_mut()
                .find(|(key, _)| key == name)
                .map(|(_, value)| std::mem::take(value))
                .unwrap_or_default(),
        )
    }
    fn revision(&self) -> Result<u64, FormFailure> {
        self.value("revision")
            .and_then(|value| value.parse().ok())
            .ok_or(FormFailure(
                StatusCode::BAD_REQUEST,
                "A current account revision is required",
            ))
    }
    fn role(&self) -> Result<Role, FormFailure> {
        match self.value("role") {
            Some("user") => Ok(Role::User),
            Some("admin") => Ok(Role::Admin),
            _ => Err(FormFailure(
                StatusCode::BAD_REQUEST,
                "Role must be user or admin",
            )),
        }
    }
    fn enabled(&self) -> Result<bool, FormFailure> {
        match self.value("enabled") {
            None => Ok(false),
            Some("true") => Ok(true),
            _ => Err(FormFailure(
                StatusCode::BAD_REQUEST,
                "Invalid enabled selection",
            )),
        }
    }
    fn grants(&self, app: &App) -> Result<BTreeSet<String>, FormFailure> {
        let available = printer_names(app);
        let mut grants = BTreeSet::new();
        for (_, name) in self.0.iter().filter(|(key, _)| key == "printer") {
            if !available.contains(name) || !grants.insert(name.clone()) || grants.len() > 1024 {
                return Err(FormFailure(
                    StatusCode::BAD_REQUEST,
                    "Unknown or duplicate printer grant",
                ));
            }
        }
        Ok(grants)
    }
    fn new_password(&mut self, required: bool) -> Result<Option<Zeroizing<String>>, FormFailure> {
        let password = self.secret("password");
        let confirmation = self.secret("password_confirm");
        if !required && password.is_empty() && confirmation.is_empty() {
            return Ok(None);
        }
        if !(12..=1024).contains(&password.len()) || !(12..=1024).contains(&confirmation.len()) {
            return Err(FormFailure(
                StatusCode::BAD_REQUEST,
                "New passwords must contain 12..1024 UTF-8 bytes",
            ));
        }
        if !bool::from(password.as_bytes().ct_eq(confirmation.as_bytes())) {
            return Err(FormFailure(
                StatusCode::BAD_REQUEST,
                "New password confirmation does not match",
            ));
        }
        Ok(Some(password))
    }
}

fn printer_names(app: &App) -> BTreeSet<String> {
    app.printer_snapshot()
        .into_iter()
        .map(|(name, _)| name)
        .collect()
}
fn csrf_field(body: &mut String, token: &str) {
    let _ = write!(
        body,
        "<input type=\"hidden\" name=\"csrf\" value=\"{}\">",
        Escape(token)
    );
}
fn role_field(body: &mut String, role: Role) {
    body.push_str("<p><label>Role <select name=\"role\" required>");
    for (value, label, selected) in [
        ("user", "User — only granted printers", role == Role::User),
        ("admin", "Admin — global authority", role == Role::Admin),
    ] {
        let _ = write!(
            body,
            "<option value=\"{value}\"{}>{label}</option>",
            if selected { " selected" } else { "" }
        );
    }
    body.push_str("</select></label></p>");
}
fn password_fields(body: &mut String, required: bool) {
    let required = if required { " required" } else { "" };
    let _ = write!(
        body,
        "<p><label>New password <input type=\"password\" name=\"password\" autocomplete=\"new-password\" minlength=\"12\" maxlength=\"1024\"{required}></label></p><p><label>Confirm new password <input type=\"password\" name=\"password_confirm\" autocomplete=\"new-password\" minlength=\"12\" maxlength=\"1024\"{required}></label></p>"
    );
    body.push_str("<p>Passwords must contain 12..1024 UTF-8 bytes. Password values are never displayed again.</p>");
}
fn grant_fields(body: &mut String, names: &BTreeSet<String>, selected: &BTreeSet<String>) {
    body.push_str("<fieldset><legend>Individual printer grants</legend>");
    if names.is_empty() {
        body.push_str("<p>No Ferriprint printers are installed. An ordinary user without grants cannot access a printer.</p>");
    }
    for name in names {
        let _ = write!(
            body,
            "<p><label><input type=\"checkbox\" name=\"printer\" value=\"{}\"{}> {}</label></p>",
            Escape(name),
            if selected.contains(name) {
                " checked"
            } else {
                ""
            },
            Escape(name)
        );
    }
    body.push_str("</fieldset>");
}
fn administrator(app: &App, principal: &Principal) -> bool {
    principal.is_admin() && app.auth.current(principal)
}
fn authorized_form(
    app: &App,
    principal: &Principal,
    headers: &HeaderMap,
    fields: &Fields,
    admin: bool,
) -> bool {
    (!admin || administrator(app, principal))
        && csrf_valid(
            app,
            principal,
            headers,
            fields.value("csrf").unwrap_or_default(),
        )
}
fn forbidden() -> Response {
    error(
        StatusCode::FORBIDDEN,
        "Current account authority and a valid same-origin CSRF token are required",
    )
}

async fn account_page(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    let Some(csrf) = app.auth.csrf(&principal) else {
        return error(
            StatusCode::FORBIDDEN,
            "Sign in with an enabled personal account; anonymous password changes are disabled",
        );
    };
    let mut body = page("My Ferriprint account");
    let _ = write!(
        body,
        "<p>Signed in as <strong>{}</strong>. This permanent identity owns your existing jobs; it cannot be renamed or deleted.</p><form method=\"post\" action=\"/account\">",
        Escape(&principal.username)
    );
    csrf_field(&mut body, csrf);
    let _ = write!(
        body,
        "<input type=\"hidden\" name=\"revision\" value=\"{}\"><p><label>Current password <input type=\"password\" name=\"current_password\" autocomplete=\"current-password\" maxlength=\"1024\" required></label></p>",
        principal.revision
    );
    password_fields(&mut body, true);
    body.push_str("<p><button type=\"submit\">Change my password</button></p></form><p>A successful password change invalidates open forms and the old password immediately. The result is shown here without redirecting; use the new password when your browser next asks you to sign in.</p>");
    finish(body).into_response()
}

async fn change_password(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    form: Result<Form<Vec<(String, String)>>, FormRejection>,
) -> Response {
    let mut fields = match Fields::parse(
        form,
        &[
            "csrf",
            "revision",
            "current_password",
            "password",
            "password_confirm",
        ],
    ) {
        Ok(fields) => fields,
        Err(response) => return response.into_response(),
    };
    if !authorized_form(&app, &principal, &headers, &fields, false) {
        return forbidden();
    }
    let revision = match fields.revision() {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    if revision != principal.revision {
        return error(
            StatusCode::CONFLICT,
            "Account changed; reload before editing",
        );
    }
    let current = fields.secret("current_password");
    if current.is_empty() || current.len() > 1024 {
        return error(
            StatusCode::FORBIDDEN,
            "Current password verification failed",
        );
    }
    let password = match fields.new_password(true) {
        Ok(Some(password)) => password,
        Ok(None) => unreachable!("required password"),
        Err(response) => return response.into_response(),
    };
    let verified = {
        let mut credentials = Zeroizing::new(Vec::with_capacity(
            principal.username.len() + 1 + current.len(),
        ));
        credentials.extend_from_slice(principal.username.as_bytes());
        credentials.push(b':');
        credentials.extend_from_slice(current.as_bytes());
        let mut authorization = Zeroizing::new(String::from("Basic "));
        STANDARD.encode_string(credentials.as_slice(), &mut authorization);
        app.auth.authenticate(&authorization).await
    };
    drop(current);
    match verified {
        Ok(verified) if verified.revision == revision => {}
        Ok(_) => {
            return error(
                StatusCode::CONFLICT,
                "Account changed; reload before editing",
            );
        }
        Err(AuthError::Busy) => {
            return error(
                StatusCode::SERVICE_UNAVAILABLE,
                "Password verification capacity reached; retry later",
            );
        }
        Err(AuthError::Invalid) => {
            return error(
                StatusCode::FORBIDDEN,
                "Current password verification failed or the account changed",
            );
        }
    }
    let hash = match hash_password(&app, password).await {
        Ok(hash) => hash,
        Err(response) => return response.into_response(),
    };
    mutation_result(
        app.auth.change_password(&principal, hash),
        "Password change",
        true,
        true,
    )
}

async fn users_page(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
) -> Response {
    if !administrator(&app, &principal) {
        return forbidden();
    }
    let Some(csrf) = app.auth.csrf(&principal) else {
        return forbidden();
    };
    let names = printer_names(&app);
    let accounts = app.auth.accounts();
    let mut body = page("Ferriprint users and printer access");
    body.push_str("<p><a href=\"/account\">Change my own password</a></p><p>Administrators have global authority over every printer and every owner's jobs, regardless of the individual grants below. Regular users may access only their listed printers and their own jobs. Removing a grant or suspending an account takes effect immediately for new access; a job already dispatched upstream is not falsely claimed canceled.</p><p>Usernames are permanent identities: there is no rename, delete or reuse. Suspended accounts retain their identity and job ownership. The final enabled administrator cannot be suspended or demoted.</p><section id=\"create-account\"><h2>Create account</h2><form method=\"post\" action=\"/admin/users\">");
    csrf_field(&mut body, csrf);
    body.push_str("<input type=\"hidden\" name=\"intent\" value=\"create\"><p><label>Permanent username <input name=\"username\" autocomplete=\"username\" maxlength=\"64\" pattern=\"[A-Za-z0-9_.@\\-]{1,64}\" required></label></p><p>Use 1..64 ASCII letters, digits, -, _, . or @. New accounts start enabled.</p>");
    role_field(&mut body, Role::User);
    password_fields(&mut body, true);
    grant_fields(&mut body, &names, &BTreeSet::new());
    body.push_str("<p><button type=\"submit\">Create account</button></p></form></section><section><h2>Existing accounts</h2>");
    for account in accounts {
        account_form(&mut body, &account, &names, csrf);
    }
    body.push_str("</section>");
    if app.config.access.anonymous_trusted_lan {
        body.push_str(
            "<section id=\"anonymous-grants\"><h2>Explicit anonymous printer access</h2>",
        );
        let _ = write!(
            body,
            "<p>The startup policy enables the shared identity <strong>{}</strong> only on its configured trusted networks. These grants do not enable anonymous administration or browser mutations. Any job accepted under this identity shares its ownership.</p><form method=\"post\" action=\"/admin/anonymous\">",
            Escape(app.config.access.anonymous_owner.as_deref().unwrap_or(""))
        );
        csrf_field(&mut body, csrf);
        grant_fields(&mut body, &names, &app.auth.anonymous_grants());
        body.push_str("<p><button type=\"submit\">Save anonymous printer grants</button></p></form></section>");
    }
    finish(body).into_response()
}
fn account_form(body: &mut String, account: &AccountView, names: &BTreeSet<String>, csrf: &str) {
    let _ = write!(
        body,
        "<article id=\"account-{}\"><h3>{}</h3><p>Status: {} · Revision {}</p><form method=\"post\" action=\"/admin/users\"><input type=\"hidden\" name=\"intent\" value=\"update\"><input type=\"hidden\" name=\"username\" value=\"{}\">",
        Escape(&account.username),
        Escape(&account.username),
        if account.enabled {
            "enabled"
        } else {
            "suspended — identity and job ownership retained"
        },
        account.revision,
        Escape(&account.username)
    );
    csrf_field(body, csrf);
    let _ = write!(
        body,
        "<input type=\"hidden\" name=\"revision\" value=\"{}\"><p><label><input type=\"checkbox\" name=\"enabled\" value=\"true\"{}> Account enabled (uncheck to suspend; check to reactivate)</label></p>",
        account.revision,
        if account.enabled { " checked" } else { "" }
    );
    role_field(body, account.role);
    grant_fields(body, names, &account.printers);
    body.push_str("<details><summary>Administrator password reset (optional)</summary><p>Leave both password fields empty to retain the existing password. A reset does not require the user's current password.</p>");
    password_fields(body, false);
    body.push_str(
        "</details><p><button type=\"submit\">Apply account changes</button></p></form></article>",
    );
}

async fn save_account(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    form: Result<Form<Vec<(String, String)>>, FormRejection>,
) -> Response {
    let mut fields = match Fields::parse(
        form,
        &[
            "csrf",
            "intent",
            "username",
            "revision",
            "role",
            "enabled",
            "password",
            "password_confirm",
            "printer",
        ],
    ) {
        Ok(fields) => fields,
        Err(response) => return response.into_response(),
    };
    if !authorized_form(&app, &principal, &headers, &fields, true) {
        return forbidden();
    }
    let create = match fields.value("intent") {
        Some("create") => true,
        Some("update") => false,
        _ => {
            return error(
                StatusCode::BAD_REQUEST,
                "Account intent must be create or update",
            );
        }
    };
    let username = fields.value("username").unwrap_or_default().to_owned();
    if crate::config::validate_principal(&username).is_err() {
        return error(
            StatusCode::BAD_REQUEST,
            "Username must contain 1..64 ASCII letters, digits, -, _, . or @",
        );
    }
    let revision = if create {
        if fields.value("revision").is_some() || fields.value("enabled").is_some() {
            return error(
                StatusCode::BAD_REQUEST,
                "New accounts start enabled and do not have an existing revision",
            );
        }
        None
    } else {
        let revision = match fields.revision() {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        };
        let Some(account) = app
            .auth
            .accounts()
            .into_iter()
            .find(|account| account.username == username)
        else {
            return error(StatusCode::NOT_FOUND, "Account not found");
        };
        if account.revision != revision {
            return error(
                StatusCode::CONFLICT,
                "Account changed; reload before editing",
            );
        }
        Some(revision)
    };
    let role = match fields.role() {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let enabled = if create {
        true
    } else {
        match fields.enabled() {
            Ok(value) => value,
            Err(response) => return response.into_response(),
        }
    };
    let grants = match fields.grants(&app) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let password = match fields.new_password(create) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    let password_changed = password.is_some();
    let hash = match password {
        Some(password) => match hash_password(&app, password).await {
            Ok(hash) => Some(hash),
            Err(response) => return response.into_response(),
        },
        None => None,
    };
    match revision {
        Some(revision) => mutation_result(
            app.auth.update_account(
                &principal,
                &username,
                crate::auth::AccountUpdate {
                    revision,
                    role,
                    enabled,
                    printers: grants,
                    hash,
                },
            ),
            "Account update",
            principal.username == username,
            password_changed,
        ),
        None => mutation_result(
            app.auth.create_account(
                &principal,
                &username,
                role,
                hash.expect("creation requires a password"),
                grants,
            ),
            "Account creation",
            false,
            false,
        ),
    }
}

async fn anonymous_grants(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    headers: HeaderMap,
    form: Result<Form<Vec<(String, String)>>, FormRejection>,
) -> Response {
    let fields = match Fields::parse(form, &["csrf", "printer"]) {
        Ok(fields) => fields,
        Err(response) => return response.into_response(),
    };
    if !app.config.access.anonymous_trusted_lan
        || !authorized_form(&app, &principal, &headers, &fields, true)
    {
        return forbidden();
    }
    let grants = match fields.grants(&app) {
        Ok(value) => value,
        Err(response) => return response.into_response(),
    };
    mutation_result(
        app.auth.set_anonymous_grants(&principal, grants),
        "Anonymous printer grant update",
        false,
        false,
    )
}

async fn hash_password(app: &App, password: Zeroizing<String>) -> Result<String, FormFailure> {
    app.auth.hash_password(password).await.map_err(|failure| {
        if failure
            .to_string()
            .contains("password hashing capacity reached")
        {
            FormFailure(
                StatusCode::SERVICE_UNAVAILABLE,
                "Password hashing capacity reached; retry later",
            )
        } else {
            FormFailure(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Password hashing failed; no change was applied",
            )
        }
    })
}
fn mutation_result(
    result: anyhow::Result<()>,
    action: &str,
    self_changed: bool,
    password_changed: bool,
) -> Response {
    let durability_warning = match result {
        Ok(()) => false,
        Err(failure) => {
            let message = failure.to_string();
            if message.contains("APPLIED") {
                true
            } else if message.contains("account changed; reload") {
                return error(
                    StatusCode::CONFLICT,
                    "Account changed; reload before editing",
                );
            } else if message.contains("at least one enabled administrator") {
                return error(
                    StatusCode::CONFLICT,
                    "The final enabled administrator cannot be suspended or demoted; no change was applied",
                );
            } else if message.contains("username exists or is permanently reserved") {
                return error(
                    StatusCode::CONFLICT,
                    "Username exists or is permanently reserved; no change was applied",
                );
            } else if message.contains("account not found") {
                return error(StatusCode::NOT_FOUND, "Account not found");
            } else if message.contains("current administrator required")
                || message.contains("current account required")
            {
                return forbidden();
            } else if message.contains("too many managed identities")
                || message.contains("too many printer grants")
            {
                return error(
                    StatusCode::BAD_REQUEST,
                    "Managed account or grant limit reached; no change was applied",
                );
            } else {
                return error(
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Access state could not be committed; no change was applied",
                );
            }
        }
    };
    let mut body = page("Ferriprint account change applied");
    let _ = write!(body, "<p><strong>{} APPLIED.</strong></p>", Escape(action));
    if durability_warning {
        body.push_str("<p class=\"warning\">The change is active, but directory synchronization failed; crash durability is unconfirmed. Do not blindly retry the operation.</p>");
    }
    if self_changed {
        body.push_str("<p>Your account revision and CSRF token have changed. Previously opened forms are stale. Your browser may require reauthentication, and your current role, enabled state and grants apply immediately.</p>");
        if password_changed {
            body.push_str("<p>The old password no longer authenticates. Use the new password when your browser asks you to sign in; a browser that keeps sending the old Basic credentials may need a fresh authentication session.</p>");
        }
        body.push_str("<p>This is the applied result, not a redirect to a page that may reject your former credentials.</p>");
    }
    body.push_str("<p><a href=\"/account\">My account (current credentials required)</a> · <a href=\"/admin/users\">Users and printer access (current administrator required)</a></p>");
    finish(body).into_response()
}
