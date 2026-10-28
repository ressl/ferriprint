// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::config::{Config, Role};
use anyhow::{Context, Result, ensure};
use argon2::{
    Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version,
    password_hash::SaltString,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use std::{
    collections::{BTreeMap, BTreeSet},
    io::{IsTerminal, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::Arc,
};
use subtle::ConstantTimeEq;
use tokio::sync::Semaphore;
use zeroize::Zeroizing;

#[derive(Clone, Debug)]
pub struct Principal {
    pub username: String,
    pub role: Role,
    pub authenticated: bool,
    pub revision: u64,
    csrf: String,
}
impl Principal {
    pub fn is_admin(&self) -> bool {
        self.authenticated && self.role == Role::Admin
    }
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Account {
    pub role: Role,
    pub enabled: bool,
    pub password_hash: Arc<String>,
    pub revision: u64,
    #[serde(default)]
    pub printers: BTreeSet<String>,
}
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct AccessState {
    pub schema_version: u32,
    pub revision: u64,
    pub accounts: BTreeMap<String, Account>,
    #[serde(default)]
    pub reserved: BTreeSet<String>,
    #[serde(default)]
    pub anonymous_printers: BTreeSet<String>,
}
impl AccessState {
    pub fn validate(&self, anonymous_owner: Option<&str>) -> Result<()> {
        ensure!(
            self.schema_version == 1,
            "unsupported managed access schema"
        );
        ensure!(
            self.accounts.len() <= 64 && self.reserved.len() <= 65536,
            "too many managed identities"
        );
        ensure!(
            self.accounts
                .values()
                .any(|a| a.enabled && a.role == Role::Admin),
            "at least one enabled administrator is required"
        );
        for (username, account) in &self.accounts {
            crate::config::validate_principal(username)?;
            ensure!(
                Some(username.as_str()) != anonymous_owner && !self.reserved.contains(username),
                "username is permanently reserved"
            );
            validate_hash(&account.password_hash)?;
            validate_grants(&account.printers)?;
        }
        for username in &self.reserved {
            crate::config::validate_principal(username)?;
        }
        validate_grants(&self.anonymous_printers)?;
        Ok(())
    }
}
fn validate_grants(grants: &BTreeSet<String>) -> Result<()> {
    ensure!(grants.len() <= 1024, "too many printer grants");
    for name in grants {
        crate::config::validate_printer_name(name)?;
    }
    Ok(())
}
struct AuthState {
    document: AccessState,
    csrf: BTreeMap<String, String>,
}
pub struct Authenticator {
    path: PathBuf,
    anonymous_owner: Option<String>,
    state: RwLock<AuthState>,
    dummy_hash: Arc<String>,
    work: Arc<Semaphore>,
}
#[derive(Debug)]
pub enum AuthError {
    Invalid,
    Busy,
}
pub(crate) fn private_file_descriptor(file: &std::fs::File) -> Result<bool> {
    let metadata = file.metadata()?;
    // The file owner must not be able to replace credentials on behalf of a
    // different local account. Root and the service account are trusted writers.
    let service_uid = unsafe { libc::geteuid() };
    if !metadata.is_file() || (metadata.uid() != 0 && metadata.uid() != service_uid) {
        return Ok(false);
    }
    let owner_only_mode = metadata.permissions().mode() & 0o077 == 0;
    #[cfg(target_os = "linux")]
    {
        // POSIX ACL non-owner entries cannot exceed the group-class mask.
        Ok(owner_only_mode || private_linux_acl(file, service_uid))
    }
    #[cfg(target_os = "macos")]
    {
        // Darwin extended ACEs are independent of these mode bits. Their
        // semantics are not supported for hash files, so require native absence.
        Ok(owner_only_mode && no_macos_extended_acl(file))
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = owner_only_mode;
        Ok(false)
    }
}

#[cfg(target_os = "macos")]
fn no_macos_extended_acl(file: &std::fs::File) -> bool {
    use std::os::fd::AsRawFd;
    unsafe extern "C" {
        fn acl_get_fd_np(fd: libc::c_int, acl_type: libc::c_int) -> *mut libc::c_void;
        fn acl_free(acl: *mut libc::c_void) -> libc::c_int;
    }
    let acl = unsafe { acl_get_fd_np(file.as_raw_fd(), 0x100) }; // ACL_TYPE_EXTENDED
    if acl.is_null() {
        // On an already-open descriptor, these mean there is no extended ACL.
        // Unsupported filesystems and permission/metadata errors fail closed.
        return matches!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ENOENT | libc::ENOATTR)
        );
    }
    unsafe {
        acl_free(acl);
    }
    false
}

#[cfg(target_os = "linux")]
fn private_linux_acl(file: &std::fs::File, service_uid: u32) -> bool {
    use std::os::fd::AsRawFd;
    // Linux POSIX ACL xattr ABI: version 2, then little-endian tag/perm/id
    // entries. Mode group bits are the ACL MASK, not group:: permissions.
    const USER_OBJ: u16 = 0x01;
    const USER: u16 = 0x02;
    const GROUP_OBJ: u16 = 0x04;
    const GROUP: u16 = 0x08;
    const MASK: u16 = 0x10;
    const OTHER: u16 = 0x20;
    let mut bytes = [0u8; 4 + 32 * 8];
    let size = unsafe {
        libc::fgetxattr(
            file.as_raw_fd(),
            c"system.posix_acl_access".as_ptr(),
            bytes.as_mut_ptr().cast(),
            bytes.len(),
        )
    };
    if size < 4 || !(size as usize - 4).is_multiple_of(8) || size as usize > bytes.len() {
        return false;
    }
    if u32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) != 2 {
        return false;
    }
    let entries = &bytes[4..size as usize];
    let mut mask = None;
    for entry in entries.as_chunks::<8>().0 {
        if u16::from_le_bytes([entry[0], entry[1]]) == MASK
            && mask
                .replace(u16::from_le_bytes([entry[2], entry[3]]))
                .is_some()
        {
            return false;
        }
    }
    let Some(mask) = mask else {
        return false;
    };
    let mut base = 0u8;
    for entry in entries.as_chunks::<8>().0 {
        let tag = u16::from_le_bytes([entry[0], entry[1]]);
        let permission = u16::from_le_bytes([entry[2], entry[3]]);
        let id = u32::from_le_bytes([entry[4], entry[5], entry[6], entry[7]]);
        if permission & !7 != 0 {
            return false;
        }
        let bit = match tag {
            USER_OBJ => 1,
            GROUP_OBJ if permission & mask == 0 => 2,
            MASK => 4,
            OTHER if permission == 0 => 8,
            USER if id == 0 || id == service_uid || permission & mask == 0 => 0,
            GROUP if permission & mask == 0 => 0,
            _ => return false,
        };
        if base & bit != 0 {
            return false;
        }
        base |= bit;
    }
    base == 15
}

/// Open the descriptor first, then verify effective privacy. No path exemption exists.
pub fn read_private_file(path: &Path, limit: usize) -> Result<Vec<u8>> {
    let file = std::fs::OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW)
        .open(path)
        .context("open private credential/state file")?;
    ensure!(
        private_file_descriptor(&file)?,
        "credential/state file must be regular and accessible only to root and the service account"
    );
    let mut bytes = Vec::new();
    file.take(limit as u64 + 1).read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() <= limit,
        "private credential/state file exceeds size limit"
    );
    Ok(bytes)
}
pub fn read_password_hash(path: &Path) -> Result<String> {
    let bytes = read_private_file(path, 512)?;
    let hash = std::str::from_utf8(&bytes)
        .context("password hash must be UTF-8")?
        .trim()
        .to_owned();
    validate_hash(&hash)?;
    Ok(hash)
}
impl Authenticator {
    pub fn open(config: &Config) -> Result<Arc<Self>> {
        let path = config.state_dir.join("access.toml");
        let bytes = read_private_file(&path, 1024 * 1024)
            .context("open managed access; initialize or explicitly migrate-0.2 before serving")?;
        let document: AccessState = toml::from_str(std::str::from_utf8(&bytes)?)
            .map_err(|_| anyhow::anyhow!("invalid managed access syntax or account fields"))?;
        document.validate(config.access.anonymous_owner.as_deref())?;
        let csrf = document
            .accounts
            .keys()
            .map(|name| (name.clone(), uuid::Uuid::new_v4().to_string()))
            .collect();
        Ok(Arc::new(Self {
            path,
            anonymous_owner: config.access.anonymous_owner.clone(),
            state: RwLock::new(AuthState { document, csrf }),
            dummy_hash: Arc::new(password_hash(uuid::Uuid::new_v4().as_bytes())?),
            work: Arc::new(Semaphore::new(4)),
        }))
    }
    pub async fn authenticate(
        &self,
        authorization: &str,
    ) -> std::result::Result<Principal, AuthError> {
        if authorization.len() > 2048 {
            return Err(AuthError::Invalid);
        }
        let (scheme, encoded) = authorization.split_once(' ').ok_or(AuthError::Invalid)?;
        if !scheme.eq_ignore_ascii_case("basic") {
            return Err(AuthError::Invalid);
        }
        let decoded = Zeroizing::new(
            STANDARD
                .decode(encoded.trim_start_matches(' '))
                .map_err(|_| AuthError::Invalid)?,
        );
        let split = decoded
            .iter()
            .position(|b| *b == b':')
            .ok_or(AuthError::Invalid)?;
        let username = std::str::from_utf8(&decoded[..split]).map_err(|_| AuthError::Invalid)?;
        if crate::config::validate_principal(username).is_err() || decoded.len() - split - 1 > 1024
        {
            return Err(AuthError::Invalid);
        }
        let (known, hash, role, revision, csrf) = {
            let state = self.state.read();
            match state.document.accounts.get(username) {
                Some(user) if user.enabled => (
                    true,
                    Arc::clone(&user.password_hash),
                    user.role,
                    user.revision,
                    state.csrf[username].clone(),
                ),
                _ => (false, self.dummy_hash.clone(), Role::User, 0, String::new()),
            }
        };
        let username = username.to_owned();
        let permit = self
            .work
            .clone()
            .try_acquire_owned()
            .map_err(|_| AuthError::Busy)?;
        let valid = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            PasswordHash::new(&hash).is_ok_and(|hash| {
                Argon2::default()
                    .verify_password(&decoded[split + 1..], &hash)
                    .is_ok()
            })
        })
        .await
        .map_err(|_| AuthError::Invalid)?;
        let principal = Principal {
            username,
            role,
            authenticated: true,
            revision,
            csrf,
        };
        if !valid || !known || !self.current(&principal) {
            return Err(AuthError::Invalid);
        }
        Ok(principal)
    }
    pub fn anonymous(&self) -> Option<Principal> {
        Some(Principal {
            username: self.anonymous_owner.clone()?,
            role: Role::User,
            authenticated: false,
            revision: self.state.read().document.revision,
            csrf: String::new(),
        })
    }
    fn current_in(&self, state: &AuthState, principal: &Principal) -> bool {
        if principal.authenticated {
            state
                .document
                .accounts
                .get(&principal.username)
                .is_some_and(|a| {
                    a.enabled && a.revision == principal.revision && a.role == principal.role
                })
        } else {
            self.anonymous_owner.as_deref() == Some(&principal.username)
                && principal.revision == state.document.revision
        }
    }
    fn allowed_in(&self, state: &AuthState, principal: &Principal, printer: &str) -> bool {
        self.current_in(state, principal)
            && if principal.authenticated {
                principal.is_admin()
                    || state.document.accounts[&principal.username]
                        .printers
                        .contains(printer)
            } else {
                state.document.anonymous_printers.contains(printer)
            }
    }
    pub fn current(&self, principal: &Principal) -> bool {
        self.current_in(&self.state.read(), principal)
    }
    pub fn can_print(&self, principal: &Principal, printer: &str) -> bool {
        self.allowed_in(&self.state.read(), principal, printer)
    }
    pub fn can_access(&self, principal: &Principal, printer: &str, owner: &str) -> bool {
        self.can_print(principal, printer) && (principal.is_admin() || principal.username == owner)
    }
    pub fn owner_allowed(&self, owner: &str, printer: &str) -> bool {
        let state = self.state.read();
        if self.anonymous_owner.as_deref() == Some(owner) {
            return state.document.anonymous_printers.contains(printer);
        }
        state
            .document
            .accounts
            .get(owner)
            .is_some_and(|a| a.enabled && (a.role == Role::Admin || a.printers.contains(printer)))
    }
    /// Serialize a bounded local commit against account/grant revocation.
    pub fn with_access<T>(
        &self,
        principal: &Principal,
        printer: Option<&str>,
        admin: bool,
        commit: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let state = self.state.read();
        ensure!(
            self.current_in(&state, principal)
                && (!admin || principal.is_admin())
                && printer.is_none_or(|p| self.allowed_in(&state, principal, p)),
            "not authorized: credentials or printer access changed"
        );
        commit()
    }
    pub fn with_owner_access<T>(
        &self,
        owner: &str,
        printer: &str,
        commit: impl FnOnce() -> Result<T>,
    ) -> Result<T> {
        let state = self.state.read();
        let allowed = if self.anonymous_owner.as_deref() == Some(owner) {
            state.document.anonymous_printers.contains(printer)
        } else {
            state.document.accounts.get(owner).is_some_and(|a| {
                a.enabled && (a.role == Role::Admin || a.printers.contains(printer))
            })
        };
        ensure!(
            allowed,
            "not authorized: account disabled or printer access revoked"
        );
        commit()
    }
    pub fn csrf<'a>(&self, principal: &'a Principal) -> Option<&'a str> {
        (principal.authenticated && self.current(principal)).then_some(principal.csrf.as_str())
    }
    pub fn verify_csrf(&self, principal: &Principal, token: &str) -> bool {
        self.csrf(principal)
            .is_some_and(|expected| bool::from(expected.as_bytes().ct_eq(token.as_bytes())))
    }
    pub fn accounts(&self) -> Vec<AccountView> {
        self.state
            .read()
            .document
            .accounts
            .iter()
            .map(|(username, a)| AccountView {
                username: username.clone(),
                role: a.role,
                enabled: a.enabled,
                revision: a.revision,
                printers: a.printers.clone(),
            })
            .collect()
    }
    pub fn anonymous_grants(&self) -> BTreeSet<String> {
        self.state.read().document.anonymous_printers.clone()
    }
    fn install(
        &self,
        state: &mut AuthState,
        mut next: AccessState,
        changed: Option<&str>,
    ) -> Result<()> {
        next.revision = next
            .revision
            .checked_add(1)
            .context("access revision exhausted")?;
        next.validate(self.anonymous_owner.as_deref())?;
        let contents = toml::to_string_pretty(&next)?;
        ensure!(
            contents.len() <= 1024 * 1024,
            "managed access exceeds size limit"
        );
        let durability =
            crate::storage::StagedFile::new(&self.path, contents.as_bytes())?.install()?;
        state.document = next;
        if let Some(name) = changed {
            state
                .csrf
                .insert(name.to_owned(), uuid::Uuid::new_v4().to_string());
        }
        durability.context(
            "Access APPLIED but directory synchronization failed; crash durability unconfirmed",
        )
    }
    pub fn create_account(
        &self,
        actor: &Principal,
        username: &str,
        role: Role,
        hash: String,
        printers: BTreeSet<String>,
    ) -> Result<()> {
        crate::config::validate_principal(username)?;
        validate_hash(&hash)?;
        let mut state = self.state.write();
        ensure!(
            self.current_in(&state, actor) && actor.is_admin(),
            "current administrator required"
        );
        ensure!(
            !state.document.accounts.contains_key(username)
                && !state.document.reserved.contains(username)
                && self.anonymous_owner.as_deref() != Some(username),
            "username exists or is permanently reserved"
        );
        let mut next = state.document.clone();
        next.accounts.insert(
            username.into(),
            Account {
                role,
                enabled: true,
                password_hash: hash.into(),
                revision: 1,
                printers,
            },
        );
        self.install(&mut state, next, Some(username))
    }
    pub fn update_account(
        &self,
        actor: &Principal,
        username: &str,
        update: AccountUpdate,
    ) -> Result<()> {
        let AccountUpdate {
            revision,
            role,
            enabled,
            printers,
            hash,
        } = update;
        let mut state = self.state.write();
        ensure!(
            self.current_in(&state, actor) && actor.is_admin(),
            "current administrator required"
        );
        let mut next = state.document.clone();
        let account = next
            .accounts
            .get_mut(username)
            .context("account not found")?;
        ensure!(
            account.revision == revision,
            "account changed; reload before editing"
        );
        account.role = role;
        account.enabled = enabled;
        account.printers = printers;
        account.revision = account
            .revision
            .checked_add(1)
            .context("account revision exhausted")?;
        if let Some(hash) = hash {
            validate_hash(&hash)?;
            account.password_hash = hash.into();
        }
        self.install(&mut state, next, Some(username))
    }
    pub fn change_password(&self, actor: &Principal, hash: String) -> Result<()> {
        validate_hash(&hash)?;
        let mut state = self.state.write();
        ensure!(
            actor.authenticated && self.current_in(&state, actor),
            "current account required"
        );
        let mut next = state.document.clone();
        let account = next
            .accounts
            .get_mut(&actor.username)
            .context("account not found")?;
        account.password_hash = hash.into();
        account.revision = account
            .revision
            .checked_add(1)
            .context("account revision exhausted")?;
        self.install(&mut state, next, Some(&actor.username))
    }
    pub fn set_anonymous_grants(
        &self,
        actor: &Principal,
        printers: BTreeSet<String>,
    ) -> Result<()> {
        let mut state = self.state.write();
        ensure!(
            self.current_in(&state, actor) && actor.is_admin(),
            "current administrator required"
        );
        let mut next = state.document.clone();
        next.anonymous_printers = printers;
        self.install(&mut state, next, None)
    }
    pub async fn hash_password(&self, password: Zeroizing<String>) -> Result<String> {
        let permit = self
            .work
            .clone()
            .try_acquire_owned()
            .context("password hashing capacity reached")?;
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            password_hash(password.as_bytes())
        })
        .await?
    }
}
pub struct AccountUpdate {
    pub revision: u64,
    pub role: Role,
    pub enabled: bool,
    pub printers: BTreeSet<String>,
    pub hash: Option<String>,
}
#[derive(Serialize)]
pub struct AccountView {
    pub username: String,
    pub role: Role,
    pub enabled: bool,
    pub revision: u64,
    pub printers: BTreeSet<String>,
}
fn validate_hash(value: &str) -> Result<()> {
    let hash =
        PasswordHash::new(value).map_err(|_| anyhow::anyhow!("invalid Argon2id password hash"))?;
    ensure!(
        hash.algorithm.as_str() == "argon2id" && hash.version == Some(19),
        "password hash must use Argon2id version 19"
    );
    let get = |name| {
        hash.params
            .get_decimal(name)
            .context("Argon2id hash missing cost parameter")
    };
    ensure!(
        (19456..=32768).contains(&get("m")?) && (2..=4).contains(&get("t")?) && get("p")? == 1,
        "password hash costs must be m=19456..32768, t=2..4, p=1"
    );
    ensure!(
        hash.salt.is_some_and(|s| s.len() >= 16) && hash.hash.is_some_and(|h| h.len() == 32),
        "password hash requires a random salt and 32-byte output"
    );
    Ok(())
}
pub fn password_hash(password: &[u8]) -> Result<String> {
    ensure!(
        (12..=1024).contains(&password.len()),
        "password must contain 12..1024 bytes"
    );
    let salt =
        SaltString::encode_b64(uuid::Uuid::new_v4().as_bytes()).expect("random 16-byte salt");
    let argon = Argon2::new(
        Algorithm::Argon2id,
        Version::V0x13,
        Params::new(19456, 2, 1, Some(32)).expect("fixed Argon2 parameters"),
    );
    Ok(argon
        .hash_password(password, &salt)
        .map_err(|_| anyhow::anyhow!("password hashing failed"))?
        .to_string())
}
/// No password argument exists. Terminal input is hidden; redirected input is bounded.
pub fn read_password() -> Result<Zeroizing<String>> {
    let password = if std::io::stdin().is_terminal() {
        let password = Zeroizing::new(rpassword::prompt_password("New Ferriprint password: ")?);
        let confirm = Zeroizing::new(rpassword::prompt_password("Repeat password: ")?);
        ensure!(
            bool::from(password.as_bytes().ct_eq(confirm.as_bytes())),
            "passwords differ"
        );
        password
    } else {
        let mut bytes = Zeroizing::new(Vec::new());
        std::io::stdin().take(1026).read_to_end(&mut bytes)?;
        ensure!(bytes.len() <= 1025, "password input too long");
        if bytes.last() == Some(&b'\n') {
            bytes.pop();
            if bytes.last() == Some(&b'\r') {
                bytes.pop();
            }
        }
        std::str::from_utf8(&bytes).context("password input must be UTF-8")?;
        Zeroizing::new(String::from_utf8(std::mem::take(&mut *bytes)).expect("validated UTF-8"))
    };
    Ok(password)
}
pub fn write_password_hash(output: &Path) -> Result<()> {
    let password = read_password()?;
    let hash = password_hash(password.as_bytes())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output)
        .context("create password hash (refuses overwrite)")?;
    ensure!(
        private_file_descriptor(&file)?,
        "new password hash file has an unsafe inherited ACL; no PHC written"
    );
    writeln!(file, "{hash}")?;
    file.sync_all()?;
    println!("Created owner-only Argon2id password hash for explicit offline credential import.");
    Ok(())
}
