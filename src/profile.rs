// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{
    config::{PrinterConfig, ProfileSettings},
    domain::*,
};
use anyhow::{Context, Result, ensure};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::Read,
    path::Path,
    time::{SystemTime, UNIX_EPOCH},
};

/// IPP seconds since the durable printer time origin, derived only from the
/// authoritative files. Queries must not manufacture a new configuration time.
pub(crate) fn configuration_change_time(
    profile_modified: SystemTime,
    ppd: &Path,
    origin: i64,
) -> Result<i32> {
    let ppd_modified = std::fs::metadata(ppd)
        .context("read provisioned PPD metadata")?
        .modified()
        .context("read provisioned PPD modification time")?;
    let seconds = profile_modified
        .max(ppd_modified)
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| i128::from(duration.as_secs()));
    Ok((seconds - i128::from(origin)).clamp(0, i128::from(i32::MAX)) as i32)
}

#[derive(Clone, Debug)]
struct Page {
    media: Media,
    native: String,
}
#[derive(Debug)]
struct Ppd {
    model: String,
    choices: BTreeMap<String, BTreeSet<String>>,
    defaults: DriverOptions,
    constraints: Vec<Vec<(String, Option<String>)>>,
    pages: Vec<Page>,
}

impl Ppd {
    fn parse(text: &str) -> Result<Self> {
        let mut ppd = Self {
            model: "Native CUPS printer".into(),
            choices: BTreeMap::new(),
            defaults: BTreeMap::new(),
            constraints: Vec::new(),
            pages: Vec::new(),
        };
        let mut dimensions = BTreeMap::<String, Vec<f64>>::new();
        let mut areas = BTreeMap::<String, Vec<f64>>::new();
        let mut records = Vec::new();
        let mut record = String::new();
        for line in text.lines() {
            if record.is_empty() && (!line.starts_with('*') || line.starts_with("*%")) {
                continue;
            }
            record.push_str(line);
            // PPD multiline PostScript strings may contain escaped quotes.
            let quotes = record
                .as_bytes()
                .iter()
                .enumerate()
                .filter(|(i, b)| **b == b'"' && (*i == 0 || record.as_bytes()[i - 1] != b'\\'))
                .count();
            if quotes % 2 == 0 {
                records.push(std::mem::take(&mut record));
            } else {
                record.push('\n');
            }
        }
        ensure!(record.is_empty(), "unterminated PPD string");
        for line in records {
            let Some((key, value)) = line[1..].split_once(':') else {
                continue;
            };
            let value = value.trim().trim_matches('"');
            let mut tokens = key.split_whitespace();
            let Some(option) = tokens.next() else {
                continue;
            };
            let choice = tokens.next().map(|s| s.split('/').next().unwrap_or(s));
            match option {
                "NickName" | "ModelName" if value.len() <= 255 => ppd.model = value.to_owned(),
                "PaperDimension" | "ImageableArea" => {
                    let name = choice.context("PPD size record has no choice")?;
                    let values: Vec<f64> = value
                        .split_whitespace()
                        .map(str::parse)
                        .collect::<std::result::Result<_, _>>()?;
                    ensure!(
                        values.iter().all(|n| n.is_finite()),
                        "nonfinite PPD dimension"
                    );
                    if option == "PaperDimension" {
                        ensure!(values.len() == 2, "invalid PaperDimension");
                        dimensions.insert(name.into(), values);
                    } else {
                        ensure!(values.len() == 4, "invalid ImageableArea");
                        areas.insert(name.into(), values);
                    }
                }
                "UIConstraints" | "NonUIConstraints" | "cupsUIConstraints" => {
                    let fields: Vec<_> = value.split_whitespace().collect();
                    let mut terms = Vec::new();
                    let mut i = 0;
                    while i < fields.len() {
                        ensure!(fields[i].starts_with('*'), "malformed PPD constraint");
                        let option = fields[i][1..].to_owned();
                        i += 1;
                        let choice = if i < fields.len() && !fields[i].starts_with('*') {
                            let c = Some(fields[i].to_owned());
                            i += 1;
                            c
                        } else {
                            None
                        };
                        terms.push((option, choice));
                    }
                    ensure!(terms.len() >= 2, "invalid PPD constraint");
                    ppd.constraints.push(terms);
                }
                _ if option.starts_with("Default") && choice.is_none() => {
                    ppd.defaults.insert(option[7..].into(), value.into());
                }
                _ if choice.is_some()
                    && !matches!(
                        option,
                        "OpenUI"
                            | "CloseUI"
                            | "OrderDependency"
                            | "JCLOpenUI"
                            | "JCLCloseUI"
                            | "CustomPageSize"
                            | "ParamCustomPageSize"
                            | "cupsUIResolver"
                    ) =>
                {
                    ppd.choices
                        .entry(option.into())
                        .or_default()
                        .insert(choice.unwrap().into());
                }
                _ => {}
            }
        }
        ppd.defaults.retain(|k, _| ppd.choices.contains_key(k));
        for (name, dims) in dimensions {
            let Some(area) = areas.get(&name) else {
                continue;
            };
            if !ppd
                .choices
                .get("PageSize")
                .is_some_and(|c| c.contains(&name))
            {
                continue;
            }
            let unit = |n: f64| (n * 2540.0 / 72.0).round() as i32;
            let width = unit(dims[0]);
            let height = unit(dims[1]);
            ensure!(
                width > 0 && height > 0 && width <= 1_000_000 && height <= 1_000_000,
                "invalid PPD paper size"
            );
            let margins = Margins {
                left: unit(area[0]),
                bottom: unit(area[1]),
                right: unit(dims[0] - area[2]),
                top: unit(dims[1] - area[3]),
            };
            ensure!(
                [margins.left, margins.bottom, margins.right, margins.top]
                    .iter()
                    .all(|m| *m >= -10),
                "invalid negative PPD margins"
            );
            let margins = Margins {
                left: margins.left.max(0),
                bottom: margins.bottom.max(0),
                right: margins.right.max(0),
                top: margins.top.max(0),
            };
            let (media_name, width, height) = media_name(width, height);
            let media = Media {
                name: media_name,
                width,
                height,
                margins,
            };
            if !ppd.pages.iter().any(|p| p.media == media) {
                ppd.pages.push(Page {
                    media,
                    native: name,
                });
            }
        }
        ensure!(
            !ppd.pages.is_empty(),
            "PPD has no PageSize with numeric PaperDimension and ImageableArea"
        );
        for (k, v) in &ppd.defaults {
            ppd.validate_option(k, v)?;
        }
        Ok(ppd)
    }
    fn validate_option(&self, key: &str, value: &str) -> Result<()> {
        ensure!(
            self.choices.get(key).is_some_and(|c| c.contains(value)),
            "PPD does not define {key}={value}"
        );
        Ok(())
    }
    fn validate_mapping(&self, map: &DriverOptions) -> Result<()> {
        for (k, v) in map {
            ensure!(
                !matches!(k.as_str(), "PageSize" | "PageRegion" | "InputSlot"),
                "{k} is controlled by media/source, not another mapping"
            );
            self.validate_option(k, v)?;
        }
        Ok(())
    }
    fn check_constraints(&self, options: &DriverOptions) -> bool {
        !self.constraints.iter().any(|terms| {
            terms.iter().all(|(key, choice)| {
                options
                    .get(key)
                    .or_else(|| self.defaults.get(key))
                    .is_some_and(|actual| {
                        choice.as_ref().map_or_else(
                            || {
                                !matches!(
                                    actual.to_ascii_lowercase().as_str(),
                                    "none" | "off" | "false"
                                )
                            },
                            |c| c == actual,
                        )
                    })
            })
        })
    }
}

fn media_name(width: i32, height: i32) -> (String, i32, i32) {
    for (name, w, h) in [
        ("iso_a3_297x420mm", 29700, 42000),
        ("iso_a4_210x297mm", 21000, 29700),
        ("iso_a5_148x210mm", 14800, 21000),
        ("iso_a6_105x148mm", 10500, 14800),
        ("iso_b5_176x250mm", 17600, 25000),
        ("jis_b5_182x257mm", 18200, 25700),
        ("na_letter_8.5x11in", 21590, 27940),
        ("na_legal_8.5x14in", 21590, 35560),
        ("na_index-4x6_4x6in", 10160, 15240),
        ("na_5x7_5x7in", 12700, 17780),
    ] {
        if (width - w).abs() <= 10 && (height - h).abs() <= 10 {
            return (name.into(), w, h);
        }
    }
    (
        format!(
            "custom_{}x{}_{:.2}x{:.2}mm",
            width,
            height,
            width as f64 / 100.0,
            height as f64 / 100.0
        ),
        width,
        height,
    )
}

#[derive(serde::Serialize)]
pub struct NativePage {
    pub native: String,
    pub media: Media,
}
#[derive(serde::Serialize)]
pub struct PpdInventory {
    pub model: String,
    pub pages: Vec<NativePage>,
    pub choices: BTreeMap<String, BTreeSet<String>>,
    pub defaults: DriverOptions,
    pub constraints: Vec<Vec<(String, Option<String>)>>,
}
impl PpdInventory {
    pub fn load(path: &Path) -> Result<Self> {
        let ppd = read_ppd(path)?;
        Ok(Self {
            model: ppd.model,
            pages: ppd
                .pages
                .into_iter()
                .map(|p| NativePage {
                    native: p.native,
                    media: p.media,
                })
                .collect(),
            choices: ppd.choices,
            defaults: ppd.defaults,
            constraints: ppd.constraints,
        })
    }
}
fn read_ppd(path: &Path) -> Result<Ppd> {
    let mut input =
        std::fs::File::open(path).with_context(|| format!("read PPD {}", path.display()))?;
    let mut bytes = Vec::new();
    input
        .by_ref()
        .take(16 * 1024 * 1024 + 1)
        .read_to_end(&mut bytes)?;
    ensure!(bytes.len() <= 16 * 1024 * 1024, "PPD exceeds 16 MiB limit");
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut decoded = Vec::new();
        flate2::read::GzDecoder::new(bytes.as_slice())
            .take(16 * 1024 * 1024 + 1)
            .read_to_end(&mut decoded)?;
        ensure!(
            decoded.len() <= 16 * 1024 * 1024,
            "decompressed PPD exceeds 16 MiB limit"
        );
        bytes = decoded;
    }
    let text = match String::from_utf8(bytes) {
        Ok(text) => text,
        Err(error) => {
            let bytes = error.into_bytes();
            ensure!(
                bytes
                    .split(|b| *b == b'\n')
                    .any(|line| line.starts_with(b"*LanguageEncoding:")
                        && line.windows(9).any(|w| w == b"ISOLatin1")),
                "PPD must be UTF-8 or declare ISOLatin1"
            );
            bytes.into_iter().map(char::from).collect()
        }
    };
    Ppd::parse(&text)
}

pub struct PrinterProfile {
    config: ProfileSettings,
    ppd: Ppd,
    capabilities: Capabilities,
}
impl PrinterProfile {
    pub fn load(config: &PrinterConfig, settings: &ProfileSettings) -> Result<Self> {
        settings.validate()?;
        let ppd = read_ppd(&config.ppd)?;
        Self::from_ppd(ppd, settings)
    }
    fn from_ppd(ppd: Ppd, settings: &ProfileSettings) -> Result<Self> {
        if ppd.choices.contains_key("InputSlot") {
            for source in settings.sources.values() {
                ppd.validate_option("InputSlot", source)?;
            }
        } else {
            ensure!(
                settings.sources.len() == 1
                    && settings.sources.get("auto").is_some_and(String::is_empty),
                "PPD without InputSlot requires the explicitly confirmed auto='' native-default source mapping"
            );
        }
        ensure!(
            !settings.sources.is_empty(),
            "explicit sources mapping is required"
        );
        for mapping in settings.media_types.values() {
            for m in [
                mapping.draft.as_ref(),
                Some(&mapping.normal),
                mapping.high.as_ref(),
            ]
            .into_iter()
            .flatten()
            {
                ppd.validate_mapping(m)?;
            }
        }
        for (mode, map) in &settings.color_modes {
            ensure!(
                matches!(mode.as_str(), "color" | "monochrome"),
                "unsupported color mode {mode}"
            );
            ppd.validate_mapping(map)?;
        }
        for (sides, map) in &settings.sides {
            ensure!(
                matches!(
                    sides.as_str(),
                    "one-sided" | "two-sided-long-edge" | "two-sided-short-edge"
                ),
                "unsupported sides {sides}"
            );
            ppd.validate_mapping(map)?;
        }
        for key in settings.sources.keys().chain(settings.media_types.keys()) {
            ensure!(
                !key.is_empty()
                    && key.len() <= 127
                    && key
                        .bytes()
                        .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-'),
                "invalid standard keyword {key}"
            );
        }
        ensure!(
            settings.document_formats.iter().all(|s| matches!(
                s.as_str(),
                "application/pdf" | "image/jpeg" | "image/png" | "image/pwg-raster" | "image/urf"
            )),
            "only PDF/JPEG/PNG/PWG/URF formats are implemented"
        );
        let mut qualities = vec![4];
        if settings.media_types.values().any(|m| m.draft.is_some()) {
            qualities.push(3);
        }
        if settings.media_types.values().any(|m| m.high.is_some()) {
            qualities.push(5);
        }
        qualities.sort();
        let dummy = ResolvedJob {
            options: BTreeMap::new(),
            media: ppd.pages[0].media.clone(),
            source: String::new(),
            media_type: String::new(),
            copies: 1,
            quality: 4,
            color_mode: String::new(),
            sides: String::new(),
            orientation: 3,
            resolution: settings.resolutions[0],
            print_scaling: "none".into(),
            document_format: settings.document_formats[0].clone(),
        };
        let mut profile = Self {
            capabilities: Capabilities {
                make_and_model: ppd.model.clone(),
                media: ppd.pages.iter().map(|p| p.media.clone()).collect(),
                ready: Vec::new(),
                sources: settings.sources.keys().cloned().collect(),
                media_types: settings.media_types.keys().cloned().collect(),
                color_modes: settings.color_modes.keys().cloned().collect(),
                sides: settings.sides.keys().cloned().collect(),
                qualities,
                resolutions: settings.resolutions.clone(),
                document_formats: settings.document_formats.clone(),
                defaults: dummy,
            },
            ppd,
            config: settings.clone(),
        };
        let defaults = profile
            .resolve(&JobTemplate::default(), &settings.document_formats[0])
            .context("invalid configured defaults")?;
        profile.capabilities.defaults = defaults;
        for ready in &settings.declared_ready {
            let media = profile
                .named_page(&ready.media, ready.borderless)
                .context("invalid ready media")?
                .media
                .clone();
            let request = JobTemplate {
                media_col: Some(MediaRequest {
                    name: Some(media.name.clone()),
                    left: Some(media.margins.left),
                    bottom: Some(media.margins.bottom),
                    right: Some(media.margins.right),
                    top: Some(media.margins.top),
                    source: Some(ready.source.clone()),
                    media_type: Some(ready.media_type.clone()),
                    ..Default::default()
                }),
                ..Default::default()
            };
            profile
                .resolve(&request, &settings.document_formats[0])
                .context("invalid ready media combination")?;
            profile.capabilities.ready.push(ReadyMedia {
                media,
                source: ready.source.clone(),
                media_type: ready.media_type.clone(),
            });
        }
        Ok(profile)
    }
    pub fn settings(&self) -> &ProfileSettings {
        &self.config
    }
    pub fn capabilities(&self) -> &Capabilities {
        &self.capabilities
    }
    pub fn restrict_formats(&mut self, upstream: &[String]) -> Result<()> {
        self.capabilities
            .document_formats
            .retain(|f| upstream.contains(f));
        ensure!(
            !self.capabilities.document_formats.is_empty(),
            "configured formats have no intersection with upstream document-format-supported"
        );
        self.capabilities.defaults.document_format = self.capabilities.document_formats[0].clone();
        Ok(())
    }
    /// Decoder tokens come from CUPS; quality presets belong to this frontend.
    /// A 300 dpi raster input can select a high-quality native engine preset.
    pub fn urf_supported(&self, upstream: &[String]) -> Result<Vec<String>> {
        if !self
            .capabilities
            .document_formats
            .iter()
            .any(|f| f == "image/urf")
        {
            return Ok(Vec::new());
        }
        let upstream_resolutions: Vec<i32> = upstream
            .iter()
            .filter_map(|v| v.strip_prefix("RS"))
            .flat_map(|v| v.split('-'))
            .filter_map(|v| v.parse().ok())
            .collect();
        let mut resolutions: Vec<_> = self
            .capabilities
            .resolutions
            .iter()
            .filter(|r| r.x == r.y && upstream_resolutions.contains(&r.x))
            .map(|r| r.x)
            .collect();
        resolutions.sort();
        resolutions.dedup();
        ensure!(
            !resolutions.is_empty(),
            "configured URF input resolutions do not intersect upstream RS decoder capabilities"
        );
        let mut tokens: Vec<_> = upstream
            .iter()
            .filter(|v| !v.starts_with("PQ") && !v.starts_with("RS"))
            .cloned()
            .collect();
        tokens.push(format!(
            "PQ{}",
            self.capabilities
                .qualities
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join("-")
        ));
        tokens.push(format!(
            "RS{}",
            resolutions
                .iter()
                .map(i32::to_string)
                .collect::<Vec<_>>()
                .join("-")
        ));
        Ok(tokens)
    }
    fn named_page(&self, name: &str, borderless: bool) -> Option<&Page> {
        self.ppd
            .pages
            .iter()
            .find(|p| p.media.name == name && (p.media.margins == Margins::default()) == borderless)
    }
    pub fn resolve(
        &self,
        request: &JobTemplate,
        format: &str,
    ) -> std::result::Result<ResolvedJob, AttributeError> {
        let err = AttributeError::new;
        if !self
            .capabilities
            .document_formats
            .iter()
            .any(|f| f == format)
        {
            return Err(err(
                "document-format",
                "not accepted by configured upstream",
            ));
        }
        let d = &self.config.defaults;
        let page = if let Some(col) = &request.media_col {
            if col.width.is_some() != col.height.is_some() {
                return Err(err("media-col", "media-size requires both dimensions"));
            }
            let name = col.name.as_deref();
            if let (Some(w), Some(h)) = (col.width, col.height)
                && (w <= 0 || h <= 0)
            {
                return Err(err("media-col", "dimensions must be positive"));
            }
            let mut candidates: Vec<_> = self
                .ppd
                .pages
                .iter()
                .filter(|p| {
                    name.is_none_or(|n| p.media.name == n)
                        && col.width.is_none_or(|w| (p.media.width - w).abs() <= 10)
                        && col.height.is_none_or(|h| (p.media.height - h).abs() <= 10)
                        && col.left.is_none_or(|m| p.media.margins.left == m)
                        && col.bottom.is_none_or(|m| p.media.margins.bottom == m)
                        && col.right.is_none_or(|m| p.media.margins.right == m)
                        && col.top.is_none_or(|m| p.media.margins.top == m)
                })
                .collect();
            if name.is_none() && col.width.is_none() {
                candidates.retain(|p| p.media.name == d.media);
            }
            // Only unspecified margins may use the configured bordered/borderless preference.
            candidates.sort_by_key(|p| (p.media.margins == Margins::default()) != d.borderless);
            candidates.first().copied().ok_or_else(|| {
                err(
                    "media-col",
                    "no PPD PageSize matches requested dimensions and margins",
                )
            })?
        } else {
            self.named_page(request.media.as_deref().unwrap_or(&d.media), d.borderless)
                .ok_or_else(|| {
                    err(
                        "media",
                        "unknown media or unavailable default margin variant",
                    )
                })?
        };
        let col = request.media_col.as_ref();
        let source = col.and_then(|c| c.source.as_deref()).unwrap_or(&d.source);
        let media_type = col
            .and_then(|c| c.media_type.as_deref())
            .unwrap_or(&d.media_type);
        let quality = request.quality.unwrap_or(d.quality);
        let mode = request.color_mode.as_deref().unwrap_or(&d.color_mode);
        let sides = request.sides.as_deref().unwrap_or(&d.sides);
        let source_native = self
            .config
            .sources
            .get(source)
            .ok_or_else(|| err("media-col", "unsupported media-source"))?;
        let media_map = self
            .config
            .media_types
            .get(media_type)
            .ok_or_else(|| err("media-col", "unsupported media-type"))?;
        let mapping = match quality {
            3 => media_map.draft.as_ref(),
            4 => Some(&media_map.normal),
            5 => media_map.high.as_ref(),
            _ => None,
        }
        .ok_or_else(|| err("print-quality", "quality unavailable for this media-type"))?;
        let color_map = self
            .config
            .color_modes
            .get(mode)
            .ok_or_else(|| err("print-color-mode", "unsupported color mode"))?;
        let side_map = self
            .config
            .sides
            .get(sides)
            .ok_or_else(|| err("sides", "unsupported sides"))?;
        let copies = request.copies.unwrap_or(1);
        if !(1..=999).contains(&copies) {
            return Err(err("copies", "supported range is 1..999"));
        }
        let orientation = request.orientation.unwrap_or(3);
        if !(3..=6).contains(&orientation) {
            return Err(err("orientation-requested", "unsupported orientation"));
        }
        let resolution = request.resolution.unwrap_or(self.config.resolutions[0]);
        if !self.config.resolutions.contains(&resolution) {
            return Err(err("printer-resolution", "unsupported input resolution"));
        }
        let scaling = request.print_scaling.as_deref().unwrap_or("none");
        if !matches!(scaling, "none" | "fit" | "fill" | "auto" | "auto-fit") {
            return Err(err("print-scaling", "unsupported scaling"));
        }
        // Explicit mappings must agree when they address the same driver option.
        let mut options = DriverOptions::new();
        for map in [mapping, color_map, side_map] {
            for (k, v) in map {
                if options
                    .insert(k.clone(), v.clone())
                    .is_some_and(|old| old != *v)
                {
                    return Err(err("media-col", "quality/color/sides mappings conflict"));
                }
            }
        }
        options.insert("PageSize".into(), page.native.clone());
        if self.ppd.choices.contains_key("PageRegion") {
            if !self.ppd.choices["PageRegion"].contains(&page.native) {
                return Err(err(
                    "media-col",
                    "PageRegion does not support the selected PageSize",
                ));
            }
            options.insert("PageRegion".into(), page.native.clone());
        }
        if !source_native.is_empty() {
            options.insert("InputSlot".into(), source_native.clone());
        }
        if !self.ppd.check_constraints(&options) {
            return Err(err("media-col", "combination violates PPD constraints"));
        }
        Ok(ResolvedJob {
            options,
            media: page.media.clone(),
            source: source.into(),
            media_type: media_type.into(),
            copies,
            quality,
            color_mode: mode.into(),
            sides: sides.into(),
            orientation,
            resolution,
            print_scaling: scaling.into(),
            document_format: format.into(),
        })
    }
}
