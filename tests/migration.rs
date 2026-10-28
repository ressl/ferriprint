// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl
mod support;

use ferriprint::{
    auth::{Authenticator, password_hash},
    config::{Config, Role},
    domain::JobTemplate,
    jobs::JobEngine,
    profile::PrinterProfile,
};
use std::{
    collections::BTreeMap,
    io::Cursor,
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Path, PathBuf},
    process::Command,
};
use support::*;

fn private(path: &Path, contents: impl AsRef<[u8]>) {
    std::fs::write(path, contents).unwrap();
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600)).unwrap();
}
fn migrate(old: &Path, new: &Path, mapping: Option<&Path>) -> std::process::Output {
    let mut command = Command::new(env!("CARGO_BIN_EXE_ferriprint"));
    command
        .arg("--config")
        .arg(old)
        .arg("migrate-0.2")
        .arg("--output")
        .arg(new);
    if let Some(mapping) = mapping {
        command.arg("--credential-sources").arg(mapping);
    }
    command.output().unwrap()
}
fn ledger(path: &Path) -> (Vec<(i32, String)>, i64, i64) {
    let db =
        rusqlite::Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY)
            .unwrap();
    let mut statement = db
        .prepare("SELECT id,record FROM jobs ORDER BY id")
        .unwrap();
    let rows = statement
        .query_map([], |r| Ok((r.get(0)?, r.get(1)?)))
        .unwrap()
        .collect::<rusqlite::Result<Vec<_>>>()
        .unwrap();
    let sequence = db
        .query_row(
            "SELECT seq FROM sqlite_sequence WHERE name='jobs'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let origin = db
        .query_row(
            "SELECT value FROM service_meta WHERE key='time-origin'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    (rows, sequence, origin)
}
fn sources(path: &Path, entries: &BTreeMap<&str, PathBuf>) {
    let mut table = toml::Table::new();
    table.insert(
        "password_hash_files".into(),
        toml::Value::try_from(entries).unwrap(),
    );
    private(path, toml::to_string_pretty(&table).unwrap());
}
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_cli_imports_explicit_private_sources_after_runtime_mounts_disappear_without_replaying_jobs()
 {
    let dir = tempfile::tempdir().unwrap();
    let receiver = Receiver::start().await;
    let mut cfg = config(dir.path(), &receiver.uri(), 0);
    add_user(&mut cfg, "old-user", Role::User, FIXTURE_PASSWORD);
    let engine = JobEngine::open(&cfg).await.unwrap();
    let resolved = PrinterProfile::load(&cfg.printers[0], &settings(&cfg))
        .unwrap()
        .resolve(&JobTemplate::default(), "application/pdf")
        .unwrap();
    let anonymous = engine.auth.anonymous().unwrap();
    let queued = engine
        .create_job(
            "demo",
            &anonymous,
            "retained queued document",
            "application/pdf",
            resolved.clone(),
        )
        .await
        .unwrap();
    engine
        .ingest_document(
            queued.id,
            &anonymous,
            Cursor::new(b"%PDF-1.4\nretained migration document\n%%EOF"),
        )
        .await
        .unwrap();
    let historical_owner = principal(&engine, "old-user", FIXTURE_PASSWORD).await;
    let terminal = engine
        .create_job(
            "demo",
            &historical_owner,
            "historical ownership",
            "application/pdf",
            resolved,
        )
        .await
        .unwrap();
    engine
        .cancel_job(terminal.id, &historical_owner)
        .await
        .unwrap();
    let old = dir.path().join("legacy.toml");
    let new = dir.path().join("managed.toml");
    let map = dir.path().join("migration-sources.toml");
    let admin_source = dir.path().join("persistent-admin.argon2");
    let user_source = dir.path().join("persistent-user.argon2");
    let admin_password = "isolated-migration-admin-password";
    let user_password = "isolated-migration-user-password";
    private(
        &admin_source,
        password_hash(admin_password.as_bytes()).unwrap(),
    );
    private(
        &user_source,
        password_hash(user_password.as_bytes()).unwrap(),
    );
    let mut table = toml::Table::try_from(&cfg).unwrap();
    table.insert(
        "printers".into(),
        toml::Value::try_from(&cfg.printers).unwrap(),
    );
    let users = [
        (
            "operator",
            Role::Admin,
            "/run/credentials/ferriprint-isolated-missing.service/admin.hash",
        ),
        (
            "alice",
            Role::User,
            "/run/credentials/ferriprint-isolated-missing.service/user.hash",
        ),
    ]
    .into_iter()
    .map(|(username, role, path)| {
        let mut account = toml::Table::new();
        account.insert("username".into(), username.into());
        account.insert("role".into(), toml::Value::try_from(role).unwrap());
        account.insert("password_hash_file".into(), path.into());
        toml::Value::Table(account)
    })
    .collect();
    table
        .get_mut("access")
        .unwrap()
        .as_table_mut()
        .unwrap()
        .insert("users".into(), toml::Value::Array(users));
    private(&old, toml::to_string_pretty(&table).unwrap());
    let entries = BTreeMap::from([
        ("operator", admin_source.clone()),
        ("alice", user_source.clone()),
    ]);
    sources(&map, &entries);
    let locked = migrate(&old, &new, Some(&map));
    assert!(!locked.status.success());
    assert!(!new.exists());
    drop(engine);
    std::fs::remove_file(dir.path().join("access.toml")).unwrap();
    std::fs::remove_file(dir.path().join("printers.toml")).unwrap();
    let db = dir.path().join("jobs.sqlite3");
    let before = ledger(&db);
    let metadata = std::fs::metadata(&db).unwrap();
    let payload = dir
        .path()
        .join("spool")
        .join(format!("{}.document", queued.id));
    let document = std::fs::read(&payload).unwrap();
    let profile_before = std::fs::read(cfg.profile_path("demo")).unwrap();
    assert!(
        Config::load(&old).is_err(),
        "legacy startup authority must not be silently mixed with managed state"
    );
    let missing = migrate(&old, &new, None);
    assert!(!missing.status.success());
    assert!(!new.exists() && !dir.path().join("access.toml").exists());
    assert_eq!(ledger(&db), before);
    sources(&map, &BTreeMap::from([("operator", admin_source.clone())]));
    assert!(!migrate(&old, &new, Some(&map)).status.success());
    let mut extra = entries.clone();
    extra.insert("unknown", admin_source.clone());
    sources(&map, &extra);
    assert!(!migrate(&old, &new, Some(&map)).status.success());
    sources(&map, &entries);
    std::fs::set_permissions(&admin_source, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(!migrate(&old, &new, Some(&map)).status.success());
    std::fs::set_permissions(&admin_source, std::fs::Permissions::from_mode(0o600)).unwrap();
    let link = dir.path().join("symlink.argon2");
    std::os::unix::fs::symlink(&admin_source, &link).unwrap();
    let mut linked = entries.clone();
    linked.insert("operator", link);
    sources(&map, &linked);
    assert!(!migrate(&old, &new, Some(&map)).status.success());
    sources(&map, &entries);
    let imported = migrate(&old, &new, Some(&map));
    assert!(
        imported.status.success(),
        "{}",
        String::from_utf8_lossy(&imported.stderr)
    );
    let stdout = String::from_utf8(imported.stdout).unwrap();
    assert!(
        !stdout.contains(admin_password)
            && !stdout.contains(user_password)
            && !stdout.contains("$argon2")
    );
    assert_eq!(ledger(&db), before);
    assert_eq!(std::fs::read(&payload).unwrap(), document);
    assert_eq!(
        std::fs::read(cfg.profile_path("demo")).unwrap(),
        profile_before
    );
    assert_eq!(
        (
            std::fs::metadata(&db).unwrap().uid(),
            std::fs::metadata(&db).unwrap().gid(),
            std::fs::metadata(&db).unwrap().ino()
        ),
        (metadata.uid(), metadata.gid(), metadata.ino())
    );
    for name in ["access.toml", "printers.toml"] {
        assert_eq!(
            std::fs::metadata(dir.path().join(name))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
    }
    let managed = Config::load(&new).unwrap();
    assert_eq!(
        managed.printers[0].backend_identity(),
        cfg.printers[0].backend_identity()
    );
    let auth = Authenticator::open(&managed).unwrap();
    use base64::Engine;
    let header = |name: &str, password: &str| {
        format!(
            "Basic {}",
            base64::engine::general_purpose::STANDARD.encode(format!("{name}:{password}"))
        )
    };
    let operator = auth
        .authenticate(&header("operator", admin_password))
        .await
        .unwrap();
    assert!(operator.is_admin());
    assert_eq!(
        auth.authenticate(&header("alice", user_password))
            .await
            .unwrap()
            .username,
        "alice"
    );
    assert!(
        auth.create_account(
            &operator,
            "old-user",
            Role::User,
            password_hash(b"isolated-unrelated-new-password").unwrap(),
            Default::default()
        )
        .is_err(),
        "historical owner identity remains permanently reserved"
    );
    let reopened = JobEngine::open(&managed).await.unwrap();
    assert_eq!(
        reopened.get_job(queued.id).await.unwrap().unwrap().phase,
        "queued"
    );
    assert_eq!(
        reopened.get_job(terminal.id).await.unwrap().unwrap().owner,
        "old-user"
    );
    assert_eq!(
        receiver
            .device
            .attempts
            .load(std::sync::atomic::Ordering::SeqCst),
        0,
        "offline migration/reopen never dispatches a document"
    );
    drop(reopened);
    assert!(!migrate(&old, &new, Some(&map)).status.success());
    assert_eq!(ledger(&db), before);
}

#[test]
fn malformed_inline_credentials_and_import_maps_never_escape_in_error_chains() {
    let directory = tempfile::tempdir().unwrap();
    let config = config(directory.path(), "ipp://127.0.0.1:9/printers/receiver", 0);
    let hash = password_hash(b"isolated-secret-redaction-password").unwrap();
    let access = config.state_dir.join("access.toml");
    private(
        &access,
        format!(
            "schema_version = 1\nrevision = 1\n[accounts.operator]\npassword_hash = \"{hash}\" invalid trailing syntax\n"
        ),
    );
    let failure = match Authenticator::open(&config) {
        Ok(_) => panic!("malformed access was accepted"),
        Err(failure) => format!("{failure:#}"),
    };
    assert!(!failure.contains(&hash));
    let recovery = ferriprint::managed::recover_administrator(
        &config,
        "operator",
        password_hash(b"isolated-recovery-password").unwrap(),
    )
    .unwrap_err();
    assert!(!format!("{recovery:#}").contains(&hash));
    let startup = directory.path().join("protected.toml");
    private(&startup, toml::to_string_pretty(&config).unwrap());
    let serving = Command::new(env!("CARGO_BIN_EXE_ferriprint"))
        .arg("--config")
        .arg(&startup)
        .arg("serve")
        .output()
        .unwrap();
    assert!(!serving.status.success());
    assert!(!String::from_utf8_lossy(&serving.stdout).contains(&hash));
    assert!(!String::from_utf8_lossy(&serving.stderr).contains(&hash));
    let original = directory.path().join("original.argon2");
    private(&original, &hash);
    let mut legacy = toml::Table::try_from(&config).unwrap();
    legacy.insert(
        "printers".into(),
        toml::Value::try_from(&config.printers).unwrap(),
    );
    legacy
        .get_mut("access")
        .unwrap()
        .as_table_mut()
        .unwrap()
        .insert(
            "users".into(),
            toml::Value::Array(vec![toml::Value::Table(toml::Table::from_iter([
                ("username".into(), "operator".into()),
                ("role".into(), "admin".into()),
                (
                    "password_hash_file".into(),
                    original.display().to_string().into(),
                ),
            ]))]),
        );
    let old = directory.path().join("legacy.toml");
    private(&old, toml::to_string_pretty(&legacy).unwrap());
    let map = directory.path().join("invalid-sources.toml");
    private(
        &map,
        format!("[password_hash_files]\noperator = \"{hash}\" invalid trailing syntax\n"),
    );
    let importing = migrate(&old, &directory.path().join("new.toml"), Some(&map));
    assert!(!importing.status.success());
    assert!(!String::from_utf8_lossy(&importing.stdout).contains(&hash));
    assert!(!String::from_utf8_lossy(&importing.stderr).contains(&hash));
}

#[cfg(target_os = "linux")]
#[test]
#[ignore = "requires root; native Linux release gates exercise this explicitly"]
fn root_migration_keeps_bootstrap_root_owned_service_readable_and_not_service_writable() {
    use std::os::unix::process::CommandExt;
    assert_eq!(
        unsafe { libc::geteuid() },
        0,
        "run this permission boundary acceptance as root"
    );
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o711)).unwrap();
    let state = directory.path().join("state");
    std::fs::create_dir(&state).unwrap();
    let config = config(&state, "ipp://127.0.0.1:9/printers/receiver", 0);
    std::fs::remove_file(state.join("access.toml")).unwrap();
    std::fs::remove_file(state.join("printers.toml")).unwrap();
    let chown = |path: &Path, uid, gid| {
        let path = std::ffi::CString::new(path.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::chown(path.as_ptr(), uid, gid) }, 0);
    };
    let owned_paths = [
        state.clone(),
        state.join("service.lock"),
        state.join("profiles"),
        config.profile_path("demo"),
    ];
    for path in &owned_paths {
        chown(path, 994, 979);
    }
    let original = directory.path().join("original.argon2");
    private(
        &original,
        password_hash(b"isolated-root-migration-password").unwrap(),
    );
    let mut table = toml::Table::try_from(&config).unwrap();
    table.insert(
        "printers".into(),
        toml::Value::try_from(&config.printers).unwrap(),
    );
    table
        .get_mut("access")
        .unwrap()
        .as_table_mut()
        .unwrap()
        .insert(
            "users".into(),
            toml::Value::Array(vec![toml::Value::Table(toml::Table::from_iter([
                ("username".into(), "operator".into()),
                ("role".into(), "admin".into()),
                (
                    "password_hash_file".into(),
                    original.display().to_string().into(),
                ),
            ]))]),
        );
    let old = directory.path().join("legacy.toml");
    private(&old, toml::to_string_pretty(&table).unwrap());
    chown(&old, 0, 979);
    std::fs::set_permissions(&old, std::fs::Permissions::from_mode(0o640)).unwrap();
    let new = directory.path().join("protected.toml");
    let result = migrate(&old, &new, None);
    assert!(result.status.success(), "root migration failed");
    let metadata = std::fs::metadata(&new).unwrap();
    assert_eq!(
        (
            metadata.uid(),
            metadata.gid(),
            metadata.permissions().mode() & 0o777
        ),
        (0, 979, 0o640)
    );
    for name in ["access.toml", "printers.toml"] {
        let metadata = std::fs::metadata(state.join(name)).unwrap();
        assert_eq!(
            (
                metadata.uid(),
                metadata.gid(),
                metadata.permissions().mode() & 0o777
            ),
            (994, 979, 0o600)
        );
    }
    assert!(
        Command::new("/usr/bin/test")
            .arg("-r")
            .arg(&new)
            .uid(994)
            .gid(979)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        !Command::new("/usr/bin/test")
            .arg("-w")
            .arg(&new)
            .uid(994)
            .gid(979)
            .status()
            .unwrap()
            .success()
    );
}

#[tokio::test]
async fn missing_catalog_is_not_an_empty_server_and_explicit_initialization_creates_a_usable_empty_catalog()
 {
    use std::io::Write;
    let directory = tempfile::tempdir().unwrap();
    let configured = config(directory.path(), "ipp://127.0.0.1:9/printers/receiver", 0);
    let protected = directory.path().join("protected.toml");
    private(&protected, toml::to_string_pretty(&configured).unwrap());
    std::fs::remove_file(directory.path().join("printers.toml")).unwrap();
    assert!(Config::load(&protected).is_err());
    assert!(
        ferriprint::server::App::open(configured.clone())
            .await
            .is_err()
    );
    let fresh = tempfile::tempdir().unwrap();
    let mut bootstrap = configured;
    bootstrap.state_dir = fresh.path().join("state");
    bootstrap.printers.clear();
    let protected = fresh.path().join("protected.toml");
    private(&protected, toml::to_string_pretty(&bootstrap).unwrap());
    let mut initialize = Command::new(env!("CARGO_BIN_EXE_ferriprint"))
        .arg("--config")
        .arg(&protected)
        .args(["user", "init", "--username", "first-admin"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .unwrap();
    initialize
        .stdin
        .take()
        .unwrap()
        .write_all(b"isolated-first-admin-password\n")
        .unwrap();
    let result = initialize.wait_with_output().unwrap();
    assert!(result.status.success());
    let config = Config::load(&protected).unwrap();
    assert!(config.printers.is_empty());
    let front = Frontend::start(config).await;
    let response = reqwest::Client::new()
        .get(format!("http://{}/admin/onboarding", front.address))
        .basic_auth("first-admin", Some("isolated-first-admin-password"))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 200);
}
