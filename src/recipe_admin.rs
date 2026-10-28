// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{
    admin::{self, Escape},
    auth::Principal,
    domain::{Capabilities, JobTemplate, Margins, Media, Resolution},
    recipes::Recipe,
    server::App,
};
use anyhow::{Context, Result, bail, ensure};
use axum::{
    Extension, Form, Json, Router,
    extract::{DefaultBodyLimit, Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
};
use serde::Deserialize;
use std::{
    collections::{BTreeSet, HashMap},
    fmt::Write,
    sync::Arc,
};

pub fn router() -> Router<Arc<App>> {
    Router::new()
        .route(
            "/admin/printers/{printer}/recipes",
            get(editor)
                .post(update)
                .layer(DefaultBodyLimit::max(1024 * 1024)),
        )
        .route("/api/printers/{printer}/recipes/{recipe}", get(export))
}

pub(crate) const RECIPE_FIELDS: &[&str] = &[
    "key",
    "display_name",
    "media_variant",
    "source",
    "media_type",
    "quality",
    "color_mode",
    "sides",
    "copies",
    "orientation",
    "resolution",
    "print_scaling",
];

pub(crate) fn field<'a>(fields: &'a HashMap<String, String>, key: &str) -> Result<&'a str> {
    fields
        .get(key)
        .map(String::as_str)
        .with_context(|| format!("Missing form field: {key}"))
}

pub(crate) fn media_value(media: &str, borderless: bool) -> String {
    format!(
        "{media}|{}",
        if borderless { "borderless" } else { "bordered" }
    )
}

pub(crate) fn parse_media(value: &str) -> Result<(String, bool)> {
    let (media, variant) = value
        .split_once('|')
        .context("Choose a paper size and margin variant")?;
    ensure!(!media.is_empty(), "Choose a paper size");
    let borderless = match variant {
        "bordered" => false,
        "borderless" => true,
        _ => bail!("Invalid media margin variant"),
    };
    Ok((media.into(), borderless))
}

pub(crate) fn read_recipe(fields: &HashMap<String, String>) -> Result<(String, Recipe)> {
    let key = field(fields, "key")?.to_owned();
    crate::recipes::validate_key(&key)?;
    let (media, borderless) = parse_media(field(fields, "media_variant")?)?;
    let (x, y) = field(fields, "resolution")?
        .split_once('x')
        .context("Choose an input resolution")?;
    let recipe = Recipe {
        display_name: field(fields, "display_name")?.into(),
        media,
        borderless,
        source: field(fields, "source")?.into(),
        media_type: field(fields, "media_type")?.into(),
        quality: field(fields, "quality")?
            .parse()
            .context("Invalid quality")?,
        color_mode: field(fields, "color_mode")?.into(),
        sides: field(fields, "sides")?.into(),
        copies: field(fields, "copies")?
            .parse()
            .context("Invalid copy count")?,
        orientation: field(fields, "orientation")?
            .parse()
            .context("Invalid orientation")?,
        resolution: Resolution {
            x: x.parse().context("Invalid input resolution")?,
            y: y.parse().context("Invalid input resolution")?,
        },
        print_scaling: field(fields, "print_scaling")?.into(),
    };
    recipe.validate()?;
    Ok((key, recipe))
}

pub(crate) fn input(body: &mut String, label: &str, name: &str, value: &str, maximum: usize) {
    let _ = write!(
        body,
        "<p><label>{} <input name=\"{}\" value=\"{}\" maxlength=\"{maximum}\" required></label></p>",
        Escape(label),
        Escape(name),
        Escape(value)
    );
}

pub(crate) fn media_selector<'a>(
    body: &mut String,
    media: impl IntoIterator<Item = &'a Media>,
    name: &str,
    selected_media: &str,
    borderless: bool,
) {
    let _ = write!(
        body,
        "<p><label>Paper size and exact margin variant <select name=\"{}\">",
        Escape(name)
    );
    let selected = media_value(selected_media, borderless);
    let mut seen = BTreeSet::new();
    for paper in media {
        let borderless = paper.margins == Margins::default();
        if !seen.insert((paper.name.as_str(), borderless)) {
            continue;
        }
        let value = media_value(&paper.name, borderless);
        let m = paper.margins;
        let _ = write!(
            body,
            "<option value=\"{}\"{}>{} · {} · {} × {}; margins L/B/R/T {}/{}/{}/{} (0.01 mm)</option>",
            Escape(&value),
            if value == selected { " selected" } else { "" },
            Escape(&paper.name),
            if borderless { "borderless" } else { "bordered" },
            paper.width,
            paper.height,
            m.left,
            m.bottom,
            m.right,
            m.top
        );
    }
    body.push_str("</select></label></p>");
}

pub(crate) fn default_recipe(cap: &Capabilities) -> Recipe {
    let d = &cap.defaults;
    Recipe {
        display_name: String::new(),
        media: d.media.name.clone(),
        borderless: d.media.margins == Margins::default(),
        source: d.source.clone(),
        media_type: d.media_type.clone(),
        quality: d.quality,
        color_mode: d.color_mode.clone(),
        sides: d.sides.clone(),
        copies: d.copies,
        orientation: d.orientation,
        resolution: d.resolution,
        print_scaling: d.print_scaling.clone(),
    }
}

/// Every field is explicit; saving never inherits an unrendered printer default.
pub(crate) fn recipe_fields(body: &mut String, cap: &Capabilities, key: &str, recipe: &Recipe) {
    input(
        body,
        "Stable recipe key (ASCII letters, digits, '-' and '_')",
        "key",
        key,
        127,
    );
    input(
        body,
        "Display name",
        "display_name",
        &recipe.display_name,
        255,
    );
    media_selector(
        body,
        &cap.media,
        "media_variant",
        &recipe.media,
        recipe.borderless,
    );
    admin::selector(
        body,
        "Source",
        "source",
        cap.sources.iter().map(String::as_str),
        &recipe.source,
    );
    admin::selector(
        body,
        "Material",
        "media_type",
        cap.media_types.iter().map(String::as_str),
        &recipe.media_type,
    );
    let qualities: Vec<_> = cap.qualities.iter().map(i32::to_string).collect();
    admin::selector(
        body,
        "Quality (3 draft, 4 normal, 5 high; explicit native mappings only)",
        "quality",
        qualities.iter().map(String::as_str),
        &recipe.quality.to_string(),
    );
    admin::selector(
        body,
        "Color",
        "color_mode",
        cap.color_modes.iter().map(String::as_str),
        &recipe.color_mode,
    );
    admin::selector(
        body,
        "Sides",
        "sides",
        cap.sides.iter().map(String::as_str),
        &recipe.sides,
    );
    let _ = write!(
        body,
        "<p><label>Copies <input type=\"number\" name=\"copies\" min=\"1\" max=\"999\" value=\"{}\" required></label></p>",
        recipe.copies
    );
    admin::selector(
        body,
        "Orientation (3 portrait, 4 landscape, 5 reverse landscape, 6 reverse portrait)",
        "orientation",
        ["3", "4", "5", "6"],
        &recipe.orientation.to_string(),
    );
    let resolutions: Vec<_> = cap
        .resolutions
        .iter()
        .map(|r| format!("{}x{}", r.x, r.y))
        .collect();
    admin::selector(
        body,
        "Input raster resolution (not native engine quality)",
        "resolution",
        resolutions.iter().map(String::as_str),
        &format!("{}x{}", recipe.resolution.x, recipe.resolution.y),
    );
    admin::selector(
        body,
        "Scaling",
        "print_scaling",
        ["none", "fit", "fill", "auto", "auto-fit"],
        &recipe.print_scaling,
    );
}

#[derive(Default, Deserialize)]
#[serde(deny_unknown_fields)]
struct EditorQuery {
    recipe: Option<String>,
}

async fn editor(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Path(name): Path<String>,
    Query(query): Query<EditorQuery>,
) -> Response {
    if !principal.is_admin() || !app.auth.current(&principal) {
        return admin::error(StatusCode::FORBIDDEN, "Current administrator required");
    }
    let Some(entry) = app.printer(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let printer = entry.read().await;
    let Some(csrf) = app.auth.csrf(&principal) else {
        return admin::error(StatusCode::FORBIDDEN, "Account changed; authenticate again");
    };
    let cap = printer.profile.capabilities();
    let mut body = admin::page(&format!(
        "Complete print recipes: {}",
        printer.config.display_name
    ));
    let _ = write!(
        body,
        "<p>Profile revision {}. Validate previews the expanded ticket without saving. Save uses the shared live-decoder validator, active-job gate and atomic profile cutover. Historical jobs and other defaults/recipes are unchanged.</p><p><a href=\"/admin/printers/{}\">Printer profile</a></p>",
        printer.revision,
        Escape(&name)
    );
    body.push_str("<p>Standard job-presets-supported exposes complete tickets. Native clients must copy the ticket attributes; a preset-aware GUI or a job-preset-name request selector is not claimed. Exported CUPS options target this Ferriprint endpoint, never its native backend.</p><h2>Saved recipes</h2>");
    if printer.profile.recipes().is_empty() {
        body.push_str("<p>No recipes saved.</p>");
    }
    for (key, resolved) in printer.profile.recipes() {
        let _ = write!(
            body,
            "<article><h3>{} — {}</h3><p><a href=\"?recipe={}\">Edit</a> · <a href=\"/api/printers/{}/recipes/{}\">Export complete JSON/CUPS options</a></p><pre>{}</pre><form method=\"post\"><input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"revision\" value=\"{}\"><input type=\"hidden\" name=\"key\" value=\"{}\"><input type=\"hidden\" name=\"operation\" value=\"delete\"><button name=\"intent\" value=\"validate\">Preview removal only</button> <button name=\"intent\" value=\"save\">Delete this recipe</button></form></article>",
            Escape(key),
            Escape(&resolved.display_name),
            Escape(key),
            Escape(&name),
            Escape(key),
            Escape(
                &serde_json::to_string_pretty(&resolved.resolved).expect("resolved job serializes")
            ),
            Escape(csrf),
            printer.revision,
            Escape(key)
        );
    }
    let (key, recipe) = match query.recipe.as_deref() {
        Some(key) => match printer.profile.settings().recipes.get(key) {
            Some(recipe) => (key, recipe.clone()),
            None => return admin::error(StatusCode::NOT_FOUND, "Recipe not found"),
        },
        None => ("", default_recipe(cap)),
    };
    body.push_str("<h2>Add or replace one complete recipe</h2><form method=\"post\">");
    let _ = write!(
        body,
        "<input type=\"hidden\" name=\"csrf\" value=\"{}\"><input type=\"hidden\" name=\"revision\" value=\"{}\"><input type=\"hidden\" name=\"operation\" value=\"upsert\">",
        Escape(csrf),
        printer.revision
    );
    recipe_fields(&mut body, cap, key, &recipe);
    body.push_str("<p>Not every combination of the choices is compatible. The same PPD resolver checks material/quality, margins, trays, color, duplex and constraints.</p><button name=\"intent\" value=\"validate\">Validate and fully preview only</button> <button name=\"intent\" value=\"save\">Save and activate this recipe</button></form>");
    admin::finish(body).into_response()
}

async fn update(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Path(name): Path<String>,
    headers: HeaderMap,
    Form(fields): Form<HashMap<String, String>>,
) -> Response {
    if !principal.is_admin()
        || !admin::csrf_valid(
            &app,
            &principal,
            &headers,
            fields.get("csrf").map_or("", String::as_str),
        )
    {
        return admin::error(
            StatusCode::FORBIDDEN,
            "Current administrator and valid same-origin CSRF token required",
        );
    }
    let request = (|| -> Result<_> {
        let intent = field(&fields, "intent")?;
        ensure!(
            matches!(intent, "validate" | "save"),
            "Invalid recipe intent"
        );
        let operation = field(&fields, "operation")?;
        ensure!(
            matches!(operation, "upsert" | "delete"),
            "Invalid recipe operation"
        );
        let basic = ["csrf", "revision", "intent", "operation", "key"];
        ensure!(
            fields.keys().all(|key| basic.contains(&key.as_str())
                || (operation == "upsert" && RECIPE_FIELDS.contains(&key.as_str()))),
            "Unexpected recipe field"
        );
        let revision = field(&fields, "revision")?
            .parse::<u64>()
            .context("Invalid profile revision")?;
        let key = field(&fields, "key")?.to_owned();
        crate::recipes::validate_key(&key)?;
        let recipe = if operation == "upsert" {
            Some(read_recipe(&fields)?.1)
        } else {
            None
        };
        Ok((revision, key, recipe, intent == "save"))
    })();
    let (revision, key, recipe, save) = match request {
        Ok(value) => value,
        Err(e) => return admin::error(StatusCode::BAD_REQUEST, e),
    };
    let Some(entry) = app.printer(&name) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let mut settings = {
        let printer = entry.read().await;
        if printer.revision != revision {
            return admin::error(
                StatusCode::CONFLICT,
                "Profile changed; reload before editing",
            );
        }
        printer.profile.settings().clone()
    };
    if let Some(recipe) = recipe {
        settings.recipes.insert(key, recipe);
    } else if settings.recipes.remove(&key).is_none() {
        return admin::error(StatusCode::NOT_FOUND, "Recipe not found");
    }
    admin::apply_settings(&app, &principal, &name, revision, settings, save).await
}

async fn export(
    State(app): State<Arc<App>>,
    Extension(principal): Extension<Principal>,
    Path((printer, key)): Path<(String, String)>,
) -> Response {
    if !principal.authenticated || !app.auth.can_print(&principal, &printer) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(entry) = app.printer(&printer) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let printer = entry.read().await;
    if !app.auth.can_print(&principal, &printer.config.name) {
        return StatusCode::FORBIDDEN.into_response();
    }
    let Some(recipe) = printer.profile.recipes().get(&key) else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let resolved = match printer.profile.resolve_recipe(
        &key,
        &JobTemplate::default(),
        &printer.profile.capabilities().defaults.document_format,
    ) {
        Ok(value) => value,
        Err(e) => return admin::error(StatusCode::BAD_REQUEST, e),
    };
    let cups_options = resolved.cups_options();
    Json(serde_json::json!({"key": key, "display_name": recipe.display_name, "resolved": resolved, "cups_options": cups_options})).into_response()
}
