// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

pub type DriverOptions = BTreeMap<String, String>;

/// IPP dimensions and margins are hundredths of a millimetre.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Margins {
    pub left: i32,
    pub bottom: i32,
    pub right: i32,
    pub top: i32,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Media {
    pub name: String,
    pub width: i32,
    pub height: i32,
    pub margins: Margins,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ReadyMedia {
    pub media: Media,
    pub source: String,
    pub media_type: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct Resolution {
    pub x: i32,
    pub y: i32,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct MediaRequest {
    pub name: Option<String>,
    pub width: Option<i32>,
    pub height: Option<i32>,
    pub left: Option<i32>,
    pub bottom: Option<i32>,
    pub right: Option<i32>,
    pub top: Option<i32>,
    pub source: Option<String>,
    pub media_type: Option<String>,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct JobTemplate {
    pub media: Option<String>,
    pub media_col: Option<MediaRequest>,
    pub copies: Option<i32>,
    pub quality: Option<i32>,
    pub color_mode: Option<String>,
    pub sides: Option<String>,
    pub orientation: Option<i32>,
    pub resolution: Option<Resolution>,
    pub print_scaling: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedJob {
    pub options: DriverOptions,
    pub media: Media,
    pub source: String,
    pub media_type: String,
    pub copies: i32,
    pub quality: i32,
    pub color_mode: String,
    pub sides: String,
    pub orientation: i32,
    pub resolution: Resolution,
    pub print_scaling: String,
    pub document_format: String,
}

impl ResolvedJob {
    /// Complete CUPS options for a native client targeting the Ferriprint queue.
    /// Each pair is an option name/value, suitable for its own `-o name=value`
    /// argument. Callers must pass arguments separately or shell-quote them;
    /// these are not a shell command or options for bypassing Ferriprint and
    /// addressing its native backend. No preset-aware client GUI is implied.
    pub fn cups_options(&self) -> Vec<(String, String)> {
        vec![
            (
                "media-col".into(),
                format!(
                    "{{media-size={{x-dimension={} y-dimension={}}} media-size-name={} media-left-margin={} media-bottom-margin={} media-right-margin={} media-top-margin={} media-source={} media-type={}}}",
                    self.media.width,
                    self.media.height,
                    self.media.name,
                    self.media.margins.left,
                    self.media.margins.bottom,
                    self.media.margins.right,
                    self.media.margins.top,
                    self.source,
                    self.media_type,
                ),
            ),
            ("print-quality".into(), self.quality.to_string()),
            ("print-color-mode".into(), self.color_mode.clone()),
            ("sides".into(), self.sides.clone()),
            ("copies".into(), self.copies.to_string()),
            ("orientation-requested".into(), self.orientation.to_string()),
            (
                "printer-resolution".into(),
                format!("{}x{}dpi", self.resolution.x, self.resolution.y),
            ),
            ("print-scaling".into(), self.print_scaling.clone()),
        ]
    }
}

/// Resolved recipe metadata for browser/JSON consumers, keyed by the stable
/// recipe identifier in `Capabilities::recipes`.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ResolvedRecipe {
    pub display_name: String,
    pub resolved: ResolvedJob,
}

#[derive(Clone, Debug)]
pub struct Capabilities {
    pub make_and_model: String,
    pub media: Vec<Media>,
    pub ready: Vec<ReadyMedia>,
    pub sources: Vec<String>,
    pub media_types: Vec<String>,
    pub color_modes: Vec<String>,
    pub sides: Vec<String>,
    pub qualities: Vec<i32>,
    pub resolutions: Vec<Resolution>,
    pub document_formats: Vec<String>,
    pub defaults: ResolvedJob,
    pub recipes: BTreeMap<String, ResolvedRecipe>,
}

#[derive(Debug, thiserror::Error)]
#[error("{attribute}: {message}")]
pub struct AttributeError {
    pub attribute: &'static str,
    pub message: String,
}

impl AttributeError {
    pub fn new(attribute: &'static str, message: impl Into<String>) -> Self {
        Self {
            attribute,
            message: message.into(),
        }
    }
}
