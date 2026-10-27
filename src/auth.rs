// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use crate::config::{AccessConfig, Role};
use anyhow::{Context, Result, ensure};
use argon2::{
    Algorithm, Argon2, Params, PasswordHash, PasswordHasher, PasswordVerifier, Version,
    password_hash::SaltString,
};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{
    collections::HashMap,
    io::{IsTerminal, Read, Write},
    os::unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    path::Path,
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
}
impl Principal {
    pub fn is_admin(&self) -> bool {
        self.authenticated && self.role == Role::Admin
    }
    pub fn can_access(&self, owner: &str) -> bool {
        self.is_admin() || self.username == owner
    }
}
struct User {
    hash: String,
    role: Role,
    csrf: String,
}
pub struct Authenticator {
    users: HashMap<String, Arc<User>>,
    dummy: Arc<User>,
    work: Arc<Semaphore>,
}
#[derive(Debug)]
pub enum AuthError {
    Invalid,
    Busy,
}
fn private_hash_file(file: &std::fs::File) -> Result<bool> {
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

impl Authenticator {
    pub fn open(config: &AccessConfig) -> Result<Self> {
        let mut users = HashMap::new();
        for user in &config.users {
            let mut options = std::fs::OpenOptions::new();
            options.read(true).custom_flags(libc::O_NOFOLLOW);
            let file = options
                .open(&user.password_hash_file)
                .with_context(|| format!("open password hash for {}", user.username))?;
            ensure!(
                private_hash_file(&file)?,
                "password hash for {} must be a regular file accessible only to root and the service account",
                user.username
            );
            let mut hash = String::new();
            file.take(513).read_to_string(&mut hash)?;
            ensure!(hash.len() <= 512, "password hash file too large");
            let hash = hash.trim().to_owned();
            validate_hash(&hash)?;
            users.insert(
                user.username.clone(),
                Arc::new(User {
                    hash,
                    role: user.role,
                    csrf: uuid::Uuid::new_v4().to_string(),
                }),
            );
        }
        let dummy = Arc::new(User {
            hash: password_hash(uuid::Uuid::new_v4().as_bytes())?,
            role: Role::User,
            csrf: String::new(),
        });
        Ok(Self {
            users,
            dummy,
            work: Arc::new(Semaphore::new(4)),
        })
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
        let (known, user) = match self.users.get(username) {
            Some(user) => (true, Arc::clone(user)),
            None => (false, Arc::clone(&self.dummy)),
        };
        let username = username.to_owned();
        let permit = self
            .work
            .clone()
            .try_acquire_owned()
            .map_err(|_| AuthError::Busy)?;
        let role = user.role;
        let valid = tokio::task::spawn_blocking(move || {
            let _permit = permit;
            PasswordHash::new(&user.hash).is_ok_and(|hash| {
                Argon2::default()
                    .verify_password(&decoded[split + 1..], &hash)
                    .is_ok()
            })
        })
        .await
        .map_err(|_| AuthError::Invalid)?;
        if !valid || !known {
            return Err(AuthError::Invalid);
        }
        Ok(Principal {
            username,
            role,
            authenticated: true,
        })
    }
    pub fn csrf(&self, principal: &Principal) -> Option<&str> {
        if !principal.authenticated {
            return None;
        }
        self.users.get(&principal.username).map(|u| u.csrf.as_str())
    }
    pub fn verify_csrf(&self, principal: &Principal, token: &str) -> bool {
        self.csrf(principal)
            .is_some_and(|expected| bool::from(expected.as_bytes().ct_eq(token.as_bytes())))
    }
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
pub fn write_password_hash(output: &Path) -> Result<()> {
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
    let hash = password_hash(password.as_bytes())?;
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(output)
        .context("create password hash (refuses overwrite)")?;
    writeln!(file, "{hash}")?;
    file.sync_all()?;
    println!(
        "Created owner-only Argon2id password hash; configure its protected path in access.users."
    );
    Ok(())
}
