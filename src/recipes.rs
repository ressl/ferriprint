// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::domain::{AttributeError, JobTemplate, Media, MediaRequest, Resolution};
use serde::{Deserialize, Serialize};

/// A complete, administrator-defined job ticket. None of its job fields inherit
/// mutable printer defaults. PPD support and cross-option constraints are checked
/// by `PrinterProfile`, using the same resolver as ordinary job requests.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Recipe {
    pub display_name: String,
    pub media: String,
    pub borderless: bool,
    pub source: String,
    pub media_type: String,
    pub quality: i32,
    pub color_mode: String,
    pub sides: String,
    pub copies: i32,
    pub orientation: i32,
    pub resolution: Resolution,
    pub print_scaling: String,
}

/// Recipe identifiers are stable, case-sensitive ASCII names, not localized
/// labels. Uppercase is intentional: `A4normal` and `A3borderlessHigh` are valid.
pub fn validate_key(key: &str) -> Result<(), AttributeError> {
    if key.is_empty()
        || key.len() > 127
        || !key.as_bytes()[0].is_ascii_alphanumeric()
        || !key
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_'))
    {
        return Err(AttributeError::new(
            "recipe",
            "key must be 1..127 ASCII letters, digits, '-' or '_', starting with a letter or digit",
        ));
    }
    Ok(())
}

impl Recipe {
    /// Validate display metadata only. Job attributes deliberately have no second
    /// validator: the existing PPD resolver is their sole authority.
    pub fn validate(&self) -> Result<(), AttributeError> {
        if self.display_name.trim().is_empty()
            || self.display_name.len() > 255
            || self.display_name.chars().any(char::is_control)
        {
            return Err(AttributeError::new(
                "recipe",
                "display_name must be nonempty, at most 255 UTF-8 bytes and contain no control characters",
            ));
        }
        Ok(())
    }

    pub(crate) fn job_template(&self, media: &Media) -> JobTemplate {
        JobTemplate {
            media: None,
            media_col: Some(MediaRequest {
                name: Some(media.name.clone()),
                width: Some(media.width),
                height: Some(media.height),
                left: Some(media.margins.left),
                bottom: Some(media.margins.bottom),
                right: Some(media.margins.right),
                top: Some(media.margins.top),
                source: Some(self.source.clone()),
                media_type: Some(self.media_type.clone()),
            }),
            copies: Some(self.copies),
            quality: Some(self.quality),
            color_mode: Some(self.color_mode.clone()),
            sides: Some(self.sides.clone()),
            orientation: Some(self.orientation),
            resolution: Some(self.resolution),
            print_scaling: Some(self.print_scaling.clone()),
        }
    }
}
