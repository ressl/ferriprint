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
