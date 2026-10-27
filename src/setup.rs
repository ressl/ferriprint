// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{
    config::{Config, Defaults, MediaTypeConfig, PrinterConfig, ProfileSettings},
    domain::{DriverOptions, JobTemplate, MediaRequest, Resolution},
    profile::{PpdInventory, PrinterProfile},
    upstream::Upstream,
};
use anyhow::{Context, Result, ensure};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::{
    collections::BTreeMap,
    io::{IsTerminal, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use url::Url;

#[derive(Subcommand)]
pub enum SetupCommand {
    /// Read an existing queue and provisioned PPD; does not submit or change jobs.
    Inspect {
        #[arg(long)]
        queue: Url,
        #[arg(long)]
        ppd: PathBuf,
        #[arg(long)]
        ca_certificate: Option<PathBuf>,
    },
    /// Guide explicit mappings, preview effective options, then install both real files.
    Install {
        #[arg(long)]
        bootstrap: PathBuf,
        #[arg(long)]
        printer: String,
        #[arg(long)]
        choices: Option<PathBuf>,
        #[arg(long)]
        output: PathBuf,
        /// Required for noninteractive installation; mappings must also be confirmed.
        #[arg(long)]
        confirm: bool,
    },
}
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct SetupChoices {
    pub confirmed_native_mappings: bool,
    pub settings: ProfileSettings,
    #[serde(default)]
    pub previews: Vec<JobTemplate>,
}
pub async fn run(command: SetupCommand) -> Result<()> {
    match command {
        SetupCommand::Inspect {
            queue,
            ppd,
            ca_certificate,
        } => {
            let printer = PrinterConfig {
                name: "inspection".into(),
                display_name: "Existing queue inspection".into(),
                service_name: "Ferriprint queue inspection".into(),
                location: String::new(),
                ppd,
                upstream_uri: queue,
                upstream_username: "ferriprint".into(),
                upstream_auth: None,
                upstream_ca_certificate: ca_certificate,
                telemetry: None,
            };
            let inventory = PpdInventory::load(&printer.ppd)?;
            let status = Upstream::new(&printer, Duration::from_secs(30))?
                .inspect()
                .await?;
            println!(
                "{}",
                serde_json::to_string_pretty(
                    &serde_json::json!({"native_ppd":inventory,"upstream":{"state":status.state,"accepting":status.accepting,"document_formats":status.document_formats,"urf_supported":status.urf_supported,"pwg_types":status.pwg_types},"notice":"Read-only inspection; native option names and DPI do not prove quality/material semantics."})
                )?
            );
        }
        SetupCommand::Install {
            bootstrap,
            printer,
            choices,
            output,
            confirm,
        } => {
            install(&bootstrap, &printer, choices.as_deref(), &output, confirm).await?;
        }
    }
    Ok(())
}
async fn install(
    bootstrap: &Path,
    printer_name: &str,
    choices_path: Option<&Path>,
    output: &Path,
    confirm: bool,
) -> Result<()> {
    let config = Config::load(bootstrap)?;
    let printer = config
        .printers
        .iter()
        .find(|p| p.name == printer_name)
        .context("--printer is not in bootstrap configuration")?;
    ensure!(
        !output.exists(),
        "output configuration already exists; refusing overwrite"
    );
    let destination = config.profile_path(printer_name);
    ensure!(
        !destination.exists(),
        "managed profile already exists; edit it using authenticated administration, not setup overwrite"
    );
    let inventory = PpdInventory::load(&printer.ppd)?;
    println!(
        "Native media, margins, options and constraints:\n{}",
        serde_json::to_string_pretty(&inventory)?
    );
    let choices = if let Some(path) = choices_path {
        let mut contents = String::new();
        std::fs::File::open(path)
            .context("read explicit setup choices")?
            .take(256 * 1024 + 1)
            .read_to_string(&mut contents)?;
        ensure!(contents.len() <= 256 * 1024, "setup choices exceed 256 KiB");
        toml::from_str::<SetupChoices>(&contents).context("parse explicit setup choices")?
    } else {
        ensure!(
            std::io::stdin().is_terminal(),
            "noninteractive setup requires --choices and --confirm; no mappings are guessed"
        );
        guided(&inventory)?
    };
    ensure!(
        choices.confirmed_native_mappings,
        "explicit confirmation of native quality/material/tray/color/sides mappings is required"
    );
    let mut profile = PrinterProfile::load(printer, &choices.settings)?;
    let status = Upstream::new(printer, Duration::from_secs(config.backend_timeout_secs))?
        .inspect()
        .await
        .context("inspect existing native CUPS queue (no jobs submitted)")?;
    profile.restrict_formats(&status.document_formats)?;
    let _urf = profile.urf_supported(&status.urf_supported)?;
    for other in config.printers.iter().filter(|p| p.name != printer_name) {
        let settings = ProfileSettings::load(&config.profile_path(&other.name))?;
        PrinterProfile::load(other, &settings)?;
    }
    println!(
        "Effective resolved defaults:\n{}",
        serde_json::to_string_pretty(&profile.capabilities().defaults)?
    );
    for (i, preview) in choices.previews.iter().enumerate() {
        let resolved = profile
            .resolve(preview, &profile.capabilities().defaults.document_format)
            .with_context(|| {
                format!(
                    "explicit preview {} violates PPD mappings/constraints",
                    i + 1
                )
            })?;
        println!(
            "Effective explicit preview {}:\n{}",
            i + 1,
            serde_json::to_string_pretty(&resolved)?
        );
    }
    println!(
        "Installation review:\n{}",
        serde_json::to_string_pretty(
            &serde_json::json!({"listen":config.listen,"public_url":config.public_url,"state_dir":config.state_dir,"tls_certificate":config.tls.as_ref().map(|t|&t.certificate),"tls_private_key_path":config.tls.as_ref().map(|t|&t.private_key),"allowed_clients":config.allowed_clients,"access":config.access,"existing_native_queue":printer.upstream_uri,"profile_destination":destination,"bootstrap_destination":output,"document_formats_actually_usable":profile.capabilities().document_formats,"warning":"Input DPI is not maximum engine DPI. CUPS queue/PPD/defaults are never modified."})
        )?
    );
    if !confirm {
        ensure!(
            std::io::stdin().is_terminal()
                && prompt("Type INSTALL to install this reviewed configuration and profile")?
                    == "INSTALL",
            "installation not confirmed; no files written"
        );
    }
    let profiles = destination
        .parent()
        .context("invalid profile destination")?;
    std::fs::create_dir_all(profiles)?;
    std::fs::set_permissions(&config.state_dir, std::fs::Permissions::from_mode(0o700))?;
    std::fs::set_permissions(profiles, std::fs::Permissions::from_mode(0o700))?;
    let settings = toml::to_string_pretty(&choices.settings)?;
    let bootstrap_contents = toml::to_string_pretty(&config)?;
    write_new(&destination, settings.as_bytes())?;
    if let Err(error) = write_new(output, bootstrap_contents.as_bytes()) {
        let _ = std::fs::remove_file(&destination);
        return Err(error);
    }
    crate::storage::sync_parent(&destination)?;
    crate::storage::sync_parent(output)?;
    println!(
        "Installed usable bootstrap {} and authoritative profile {}. Run ferriprint --config {} check, then serve. No native queue changed and no print submitted.",
        output.display(),
        destination.display(),
        output.display()
    );
    Ok(())
}
fn write_new(path: &Path, content: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .with_context(|| format!("create {} without overwrite", path.display()))?;
    file.write_all(content)?;
    file.sync_all()?;
    Ok(())
}
fn prompt(label: &str) -> Result<String> {
    print!("{label}: ");
    std::io::stdout().flush()?;
    let mut line = String::new();
    ensure!(
        std::io::stdin().read_line(&mut line)? > 0,
        "setup input ended before confirmation"
    );
    ensure!(line.len() <= 4096, "setup answer too long");
    Ok(line.trim().to_owned())
}
fn mapping(label: &str) -> Result<DriverOptions> {
    let input = prompt(&format!(
        "{label} — exact Option=Choice pairs separated by commas, or 'none' for no native override"
    ))?;
    if input == "none" {
        return Ok(BTreeMap::new());
    }
    let mut result = BTreeMap::new();
    for assignment in input.split(',') {
        let (key, value) = assignment
            .trim()
            .split_once('=')
            .context("expected Option=Choice; see enumerated native options")?;
        ensure!(
            !key.is_empty()
                && !value.is_empty()
                && result.insert(key.to_owned(), value.to_owned()).is_none(),
            "empty or duplicate native option assignment"
        );
    }
    Ok(result)
}
fn guided(inventory: &PpdInventory) -> Result<SetupChoices> {
    println!(
        "Quality/material semantics are NOT inferred from HIGH, labels or DPI. Confirm choices using the native driver documentation/UI. Enter standard keywords and exact native choices shown above."
    );
    let mut sources = BTreeMap::new();
    if let Some(slots) = inventory.choices.get("InputSlot") {
        for native in slots {
            let keyword = prompt(&format!(
                "Standard source keyword for native InputSlot={native} (or 'skip')"
            ))?;
            if keyword != "skip" {
                sources.insert(keyword, native.clone());
            }
        }
    } else {
        ensure!(
            prompt(
                "PPD has no InputSlot; type CONFIRM for auto using the unchanged native default"
            )? == "CONFIRM",
            "source not confirmed"
        );
        sources.insert("auto".into(), String::new());
    }
    let mut media_types = BTreeMap::new();
    loop {
        let keyword =
            prompt("Material standard keyword (e.g. stationery, photographic-glossy), or 'done'")?;
        if keyword == "done" {
            break;
        }
        let normal = mapping(&format!("Explicit normal-quality mapping for {keyword}"))?;
        let draft = if prompt("Map draft for this material? yes/no")? == "yes" {
            Some(mapping("Explicit draft mapping")?)
        } else {
            None
        };
        let high = if prompt("Map high for this material? yes/no")? == "yes" {
            Some(mapping("Explicit high mapping")?)
        } else {
            None
        };
        ensure!(
            media_types
                .insert(
                    keyword,
                    MediaTypeConfig {
                        draft,
                        normal,
                        high
                    }
                )
                .is_none(),
            "duplicate material keyword"
        );
    }
    let mut color_modes = BTreeMap::new();
    for mode in ["monochrome", "color"] {
        if prompt(&format!("Expose {mode}? yes/no"))? == "yes" {
            color_modes.insert(mode.into(), mapping(&format!("Explicit {mode} mapping"))?);
        }
    }
    let mut sides = BTreeMap::new();
    for mode in ["one-sided", "two-sided-long-edge", "two-sided-short-edge"] {
        if prompt(&format!("Expose {mode}? yes/no"))? == "yes" {
            sides.insert(mode.into(), mapping(&format!("Explicit {mode} mapping"))?);
        }
    }
    for (i, page) in inventory.pages.iter().enumerate() {
        println!(
            "{}: {} -> {} {}x{} (1/100mm), margins {:?}",
            i + 1,
            page.native,
            page.media.name,
            page.media.width,
            page.media.height,
            page.media.margins
        );
    }
    let index: usize = prompt("Default native media variant number")?
        .parse()
        .context("media variant must be a number")?;
    let page = inventory
        .pages
        .get(
            index
                .checked_sub(1)
                .context("media numbering starts at 1")?,
        )
        .context("unknown native media variant")?;
    let default_source = prompt("Default standard source keyword")?;
    let default_type = prompt("Default standard material keyword")?;
    let default_quality =
        prompt("Default quality: 3=draft,4=normal,5=high (confirmed mapping, not DPI)")?.parse()?;
    let color_mode = prompt("Default color mode keyword")?;
    let default_sides = prompt("Default sides keyword")?;
    let resolution = Resolution {
        x: prompt("Confirmed upstream input X DPI")?.parse()?,
        y: prompt("Confirmed upstream input Y DPI")?.parse()?,
    };
    let document_formats = prompt(
        "Confirmed allowed MIME formats, comma separated (intersection with real CUPS is enforced)",
    )?
    .split(',')
    .map(|s| s.trim().to_owned())
    .collect();
    let defaults = Defaults {
        media: page.media.name.clone(),
        borderless: page.media.margins == Default::default(),
        source: default_source,
        media_type: default_type,
        quality: default_quality,
        color_mode,
        sides: default_sides,
    };
    let mut previews = Vec::new();
    for (kind, map) in &media_types {
        for quality in [3, 4, 5] {
            if (quality == 3 && map.draft.is_none()) || (quality == 5 && map.high.is_none()) {
                continue;
            }
            previews.push(JobTemplate {
                media_col: Some(MediaRequest {
                    media_type: Some(kind.clone()),
                    ..Default::default()
                }),
                quality: Some(quality),
                ..Default::default()
            });
        }
    }
    let settings = ProfileSettings {
        schema_version: 1,
        sources,
        media_types,
        color_modes,
        sides,
        defaults,
        declared_ready: Vec::new(),
        document_formats,
        resolutions: vec![resolution],
    };
    let confirmed_native_mappings = prompt(
        "Type CONFIRM after reviewing ALL tray/material/quality/color/sides mappings (including photo/special media if mapped)",
    )? == "CONFIRM";
    Ok(SetupChoices {
        confirmed_native_mappings,
        settings,
        previews,
    })
}
