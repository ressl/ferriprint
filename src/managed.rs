// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::{
    auth::{AccessState, Account},
    config::{Config, PrinterConfig, Role},
    jobs::JobRecord,
};
use anyhow::{Context, Result, ensure};
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{Read, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Path, PathBuf},
};

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PrinterCatalog {
    pub schema_version: u32,
    pub revision: u64,
    #[serde(default)]
    pub printers: Vec<PrinterConfig>,
}
impl Default for PrinterCatalog {
    fn default() -> Self {
        Self {
            schema_version: 1,
            revision: 0,
            printers: Vec::new(),
        }
    }
}
pub fn load_catalog(config: &Config) -> Result<PrinterCatalog> {
    let path = config.state_dir.join("printers.toml");
    let bytes = crate::auth::read_private_file(&path, 1024 * 1024)
        .context("managed printer catalog unavailable; explicitly initialize/migrate, or restore the authoritative private catalog before serving")?;
    let catalog: PrinterCatalog =
        toml::from_str(std::str::from_utf8(&bytes)?).context("parse managed printer catalog")?;
    ensure!(
        catalog.schema_version == 1 && catalog.printers.len() <= 1024,
        "unsupported or oversized printer catalog"
    );
    for printer in &catalog.printers {
        ensure!(
            printer.ppd.is_absolute(),
            "managed PPD path must be absolute"
        );
    }
    Ok(catalog)
}
/// The same nonblocking service lock protects serve, initialization, recovery and migration.
pub fn lock_state(config: &Config) -> Result<std::fs::File> {
    if !config.state_dir.try_exists()? {
        std::fs::create_dir_all(&config.state_dir)?;
        std::fs::set_permissions(&config.state_dir, std::fs::Permissions::from_mode(0o700))?;
    }
    let directory = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_DIRECTORY)
        .open(&config.state_dir)?;
    ensure!(
        directory.metadata()?.permissions().mode() & 0o077 == 0,
        "state directory must be owner-only"
    );
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(config.state_dir.join("service.lock"))?;
    fs2::FileExt::try_lock_exclusive(&lock)
        .context("state directory is already in use; stop the service before offline changes")?;
    Ok(lock)
}
fn write_new(config: &Config, path: &Path, contents: &[u8]) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .context("create new managed file without overwrite")?;
    let result = (|| {
        ensure!(
            crate::auth::private_file_descriptor(&file)?,
            "new managed file has an unsafe inherited ACL; no credentials written"
        );
        let owner = std::fs::metadata(&config.state_dir)?;
        if unsafe { libc::geteuid() } == 0 {
            ensure!(
                unsafe { libc::fchown(file.as_raw_fd(), owner.uid(), owner.gid()) } == 0,
                "assign new managed file to existing state owner"
            );
        } else {
            ensure!(
                file.metadata()?.uid() == owner.uid(),
                "run offline initialization as the state owner (or root)"
            );
        }
        file.write_all(contents)?;
        file.sync_all()?;
        crate::storage::sync_parent(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}
fn write_protected_configuration(
    path: &Path,
    contents: &[u8],
    protection: &std::fs::Metadata,
) -> Result<()> {
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .context("create protected configuration without overwrite")?;
    let result = (|| {
        ensure!(
            crate::auth::private_file_descriptor(&file)?,
            "new protected configuration has an unsafe inherited ACL"
        );
        if unsafe { libc::geteuid() } == 0 {
            ensure!(
                unsafe { libc::fchown(file.as_raw_fd(), protection.uid(), protection.gid()) } == 0,
                "preserve protected configuration owner/group"
            );
        } else {
            let current = file.metadata()?;
            ensure!(
                current.uid() == protection.uid() && current.gid() == protection.gid(),
                "run migration as the protected configuration owner/group"
            );
        }
        file.set_permissions(std::fs::Permissions::from_mode(protection.mode() & 0o777))?;
        file.write_all(contents)?;
        file.sync_all()?;
        crate::storage::sync_parent(path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = std::fs::remove_file(path);
    }
    result
}
fn install_initial(
    config: &Config,
    access: &AccessState,
    catalog: &PrinterCatalog,
    output: Option<(&Path, &[u8], &std::fs::Metadata)>,
) -> Result<()> {
    let access_path = config.state_dir.join("access.toml");
    let catalog_path = config.state_dir.join("printers.toml");
    ensure!(
        !access_path.try_exists()? && !catalog_path.try_exists()?,
        "managed state already exists; refusing overwrite"
    );
    if let Some((path, _, _)) = output {
        ensure!(
            !path.try_exists()?,
            "output configuration already exists; refusing overwrite"
        );
    }
    access.validate(config.access.anonymous_owner.as_deref())?;
    let mut created = Vec::new();
    let result = (|| {
        for (path, bytes) in [
            (&access_path, toml::to_string_pretty(access)?.into_bytes()),
            (&catalog_path, toml::to_string_pretty(catalog)?.into_bytes()),
        ] {
            write_new(config, path, &bytes)?;
            created.push(path.to_owned());
        }
        if let Some((path, bytes, protection)) = output {
            write_protected_configuration(path, bytes, protection)?;
            created.push(path.to_owned());
        }
        Ok(())
    })();
    if result.is_err() {
        for path in created {
            let _ = std::fs::remove_file(path);
        }
    }
    result
}
pub fn initialize(config: &Config, username: &str, hash: String) -> Result<()> {
    crate::config::validate_principal(username)?;
    let _lock = lock_state(config)?;
    let mut reserved = BTreeSet::new();
    if let Some(owner) = &config.access.anonymous_owner {
        reserved.insert(owner.clone());
    }
    let access = AccessState {
        schema_version: 1,
        revision: 1,
        accounts: BTreeMap::from([(
            username.into(),
            Account {
                role: Role::Admin,
                enabled: true,
                password_hash: hash.into(),
                revision: 1,
                printers: BTreeSet::new(),
            },
        )]),
        reserved,
        anonymous_printers: BTreeSet::new(),
    };
    install_initial(config, &access, &PrinterCatalog::default(), None)
}
pub fn recover_administrator(config: &Config, username: &str, hash: String) -> Result<()> {
    let _lock = lock_state(config)?;
    crate::config::validate_principal(username)?;
    let path = config.state_dir.join("access.toml");
    let bytes = crate::auth::read_private_file(&path, 1024 * 1024)?;
    let mut access: AccessState = toml::from_str(std::str::from_utf8(&bytes)?)
        .map_err(|_| anyhow::anyhow!("invalid managed access syntax or account fields"))?;
    ensure!(
        !access.reserved.contains(username)
            && config.access.anonymous_owner.as_deref() != Some(username),
        "username is permanently reserved"
    );
    let revision = access.accounts.get(username).map_or(Ok(1), |a| {
        a.revision
            .checked_add(1)
            .context("account revision exhausted")
    })?;
    let printers = access
        .accounts
        .get(username)
        .map(|a| a.printers.clone())
        .unwrap_or_default();
    access.accounts.insert(
        username.into(),
        Account {
            role: Role::Admin,
            enabled: true,
            password_hash: hash.into(),
            revision,
            printers,
        },
    );
    access.revision = access
        .revision
        .checked_add(1)
        .context("access revision exhausted")?;
    access.validate(config.access.anonymous_owner.as_deref())?;
    let durability =
        crate::storage::StagedFile::new(&path, toml::to_string_pretty(&access)?.as_bytes())?
            .install()?;
    durability.context(
        "Access APPLIED but directory synchronization failed; crash durability unconfirmed",
    )
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct LegacyUser {
    username: String,
    role: Role,
    password_hash_file: PathBuf,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct CredentialSources {
    password_hash_files: BTreeMap<String, PathBuf>,
}
fn absolute(base: &Path, path: &Path) -> PathBuf {
    if path.is_absolute() {
        path.into()
    } else {
        base.join(path)
    }
}
fn read_bounded(path: &Path) -> Result<(String, std::fs::Metadata)> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)?;
    let protection = file.metadata()?;
    ensure!(
        protection.is_file()
            && protection.uid() == unsafe { libc::geteuid() }
            && protection.mode() & 0o022 == 0,
        "legacy protected configuration must be a regular file owned by the migration operator without group/other write access"
    );
    let mut text = String::new();
    file.take(1024 * 1024 + 1).read_to_string(&mut text)?;
    ensure!(
        text.len() <= 1024 * 1024,
        "configuration exceeds size limit"
    );
    Ok((text, protection))
}
/// An explicit offline cutover; legacy paths never become a runtime fallback.
pub fn migrate_02(input: &Path, output: &Path, sources: Option<&Path>) -> Result<()> {
    ensure!(
        input != output,
        "migration output must differ from legacy input"
    );
    let (legacy_text, protection) = read_bounded(input)?;
    let mut table: toml::Table = toml::from_str(&legacy_text)
        .map_err(|_| anyhow::anyhow!("invalid legacy configuration syntax"))?;
    let mut printers: Vec<PrinterConfig> = table
        .remove("printers")
        .context("legacy configuration has no printers")?
        .try_into()?;
    let users: Vec<LegacyUser> = table
        .get_mut("access")
        .and_then(toml::Value::as_table_mut)
        .context("legacy access configuration missing")?
        .remove("users")
        .map(|v| {
            v.try_into()
                .map_err(|_| anyhow::anyhow!("invalid legacy account entries"))
        })
        .transpose()?
        .unwrap_or_default();
    let mut config: Config = table.try_into()?;
    let parent = input.parent().unwrap_or_else(|| Path::new("."));
    let directory = if parent.is_absolute() {
        parent.to_owned()
    } else {
        std::env::current_dir()?.join(parent)
    };
    let base = directory.as_path();
    config.state_dir = absolute(base, &config.state_dir);
    if let Some(tls) = &mut config.tls {
        tls.certificate = absolute(base, &tls.certificate);
        tls.private_key = absolute(base, &tls.private_key);
    }
    for printer in &mut printers {
        printer.ppd = absolute(base, &printer.ppd);
        if let Some(ca) = &mut printer.upstream_ca_certificate {
            *ca = absolute(base, ca);
        }
        if let Some(telemetry) = &mut printer.telemetry {
            telemetry.resolve_paths(base);
        }
    }
    config.printers = printers.clone();
    config.validate()?;
    let _lock = lock_state(&config)?;
    let mapping = if let Some(path) = sources {
        let bytes = crate::auth::read_private_file(path, 64 * 1024)?;
        let map: CredentialSources =
            toml::from_str(std::str::from_utf8(&bytes)?).map_err(|_| {
                anyhow::anyhow!("invalid migration credential-source mapping syntax or fields")
            })?;
        let expected: BTreeSet<_> = users.iter().map(|u| u.username.as_str()).collect();
        ensure!(
            map.password_hash_files
                .keys()
                .map(String::as_str)
                .collect::<BTreeSet<_>>()
                == expected,
            "credential sources must name exactly every legacy account"
        );
        Some((
            map,
            path.parent().unwrap_or_else(|| Path::new(".")).to_owned(),
        ))
    } else {
        None
    };
    let all_printers: BTreeSet<_> = printers.iter().map(|p| p.name.clone()).collect();
    let mut accounts = BTreeMap::new();
    for user in users {
        let path = match &mapping {
            Some((map, base)) => absolute(base, &map.password_hash_files[&user.username]),
            None => absolute(base, &user.password_hash_file),
        };
        let hash=crate::auth::read_password_hash(&path).context("legacy credential unavailable/unsafe; stopped systemd credential mounts require explicit --credential-sources mapping to protected persistent PHCs")?;
        ensure!(
            !accounts.contains_key(&user.username),
            "duplicate legacy username"
        );
        accounts.insert(
            user.username,
            Account {
                role: user.role,
                enabled: true,
                password_hash: hash.into(),
                revision: 1,
                printers: all_printers.clone(),
            },
        );
    }
    let mut reserved = BTreeSet::new();
    if let Some(owner) = &config.access.anonymous_owner {
        reserved.insert(owner.clone());
    }
    let db_path = config.state_dir.join("jobs.sqlite3");
    if db_path.try_exists()? {
        let db = rusqlite::Connection::open_with_flags(
            &db_path,
            rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY | rusqlite::OpenFlags::SQLITE_OPEN_NO_MUTEX,
        )?;
        let mut statement = db.prepare("SELECT record FROM jobs ORDER BY id")?;
        for record in statement.query_map([], |row| row.get::<_, String>(0))? {
            let job: JobRecord = serde_json::from_str(&record?)?;
            if !accounts.contains_key(&job.owner) {
                reserved.insert(job.owner.clone());
            }
            if job.state < 7 {
                ensure!(
                    printers.iter().any(|p| p.name == job.printer
                        && job.backend_identity.as_ref() == Some(&p.backend_identity())),
                    "active job backend differs; resolve with original configuration before migration"
                );
            }
        }
    }
    for printer in &printers {
        let settings = crate::config::ProfileSettings::load(&config.profile_path(&printer.name))?;
        crate::profile::PrinterProfile::load(printer, &settings)?;
    }
    // Existing protected queues establish a trusted server list, not a browser URI editor.
    if config.cups_sources.is_empty() {
        for (index, printer) in printers.iter().enumerate() {
            let mut server = printer.upstream_uri.clone();
            server.set_path("/");
            config.cups_sources.push(crate::setup::CupsSource {
                id: format!("imported-{}", index + 1),
                server_uri: server,
                username: printer.upstream_username.clone(),
                auth: printer.upstream_auth.clone(),
                ca_certificate: printer.upstream_ca_certificate.clone(),
            });
        }
    }
    let access = AccessState {
        schema_version: 1,
        revision: 1,
        accounts,
        reserved,
        anonymous_printers: if config.access.anonymous_trusted_lan {
            all_printers
        } else {
            BTreeSet::new()
        },
    };
    let catalog = PrinterCatalog {
        schema_version: 1,
        revision: 1,
        printers,
    };
    config.validate()?;
    let text = toml::to_string_pretty(&config)?;
    install_initial(
        &config,
        &access,
        &catalog,
        Some((output, text.as_bytes(), &protection)),
    )?;
    println!(
        "Imported managed accounts and printer catalog; existing profiles/jobs/spool were not rewritten. Use the NEW configuration. Old-binary rollback requires backed-up old profiles/configuration/PHCs."
    );
    Ok(())
}
