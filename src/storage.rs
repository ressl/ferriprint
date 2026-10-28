// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use anyhow::{Context, Result};
use std::{
    io::Write,
    os::unix::fs::OpenOptionsExt,
    path::{Path, PathBuf},
};

pub struct StagedFile {
    temporary: PathBuf,
    destination: PathBuf,
}
impl StagedFile {
    pub fn new(destination: &Path, contents: &[u8]) -> Result<Self> {
        let parent = destination
            .parent()
            .context("managed file needs a parent directory")?;
        let temporary = parent.join(format!(".ferriprint-{}.tmp", uuid::Uuid::new_v4()));
        let staged = Self {
            temporary,
            destination: destination.to_owned(),
        };
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&staged.temporary)
            .context("create managed temporary file")?;
        anyhow::ensure!(
            crate::auth::private_file_descriptor(&file)?,
            "managed temporary file has an unsafe inherited ACL"
        );
        file.write_all(contents)?;
        file.sync_all().context("sync managed temporary file")?;
        Ok(staged)
    }
    pub fn modified_at(&self) -> Result<std::time::SystemTime> {
        std::fs::metadata(&self.temporary)?
            .modified()
            .context("read staged profile modification time")
    }
    /// Once rename succeeds the new file is authoritative. The caller MUST apply
    /// the validated in-memory snapshot before reporting a directory-sync error.
    pub fn install(self) -> Result<std::io::Result<()>> {
        std::fs::rename(&self.temporary, &self.destination)
            .context("atomically replace managed file")?;
        Ok(sync_parent(&self.destination))
    }
}
impl Drop for StagedFile {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.temporary);
    }
}
pub fn sync_parent(path: &Path) -> std::io::Result<()> {
    std::fs::File::open(path.parent().unwrap_or_else(|| Path::new(".")))?.sync_all()
}
