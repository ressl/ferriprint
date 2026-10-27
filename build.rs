// SPDX-License-Identifier: AGPL-3.0-only
// Copyright (C) 2026 Robert Ressl

use flate2::{Compression, GzBuilder};
use std::{
    env, fs, io,
    path::{Path, PathBuf},
};

fn collect(
    root: &Path,
    dir: &Path,
    extensions: &[&str],
    files: &mut Vec<PathBuf>,
) -> io::Result<()> {
    println!("cargo:rerun-if-changed={}", dir.display());
    if !dir.exists() {
        return Ok(());
    }
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let kind = entry.file_type()?;
        let path = entry.path();
        if kind.is_symlink() {
            return Err(io::Error::other(format!(
                "source allowlist cannot include symlinks: {}",
                path.display()
            )));
        }
        if kind.is_dir() {
            collect(root, &path, extensions, files)?;
        } else if path
            .extension()
            .and_then(|e| e.to_str())
            .is_some_and(|e| extensions.contains(&e))
        {
            files.push(
                path.strip_prefix(root)
                    .map_err(io::Error::other)?
                    .to_owned(),
            );
        }
    }
    Ok(())
}
fn main() -> io::Result<()> {
    let root = PathBuf::from(
        env::var_os("CARGO_MANIFEST_DIR").ok_or_else(|| io::Error::other("missing source root"))?,
    );
    let mut files = Vec::new();
    // Exact public root paths only. Operator configuration, PPDs and state are never traversed.
    for name in [
        "Cargo.toml",
        "Cargo.lock",
        "build.rs",
        "README.md",
        "LICENSE",
        "COPYING",
        "NOTICE",
        "THIRD_PARTY_NOTICES.md",
        "DEPENDENCIES.md",
        "CONTRIBUTING.md",
        "SECURITY.md",
        "CHANGELOG.md",
        ".gitignore",
        ".gitlab-ci.yml",
    ] {
        let path = root.join(name);
        println!("cargo:rerun-if-changed={name}");
        if path.is_file() {
            if path.symlink_metadata()?.file_type().is_symlink() {
                return Err(io::Error::other("public root source file is a symlink"));
            }
            files.push(PathBuf::from(name));
        }
    }
    for (dir, extensions) in [
        ("src", &["rs"][..]),
        ("tests", &["rs", "ppd", "test", "pdf", "pem"][..]),
        ("examples", &["rs", "toml", "ppd"][..]),
        ("docs", &["md"][..]),
        (".github/workflows", &["yml", "yaml"][..]),
        ("scripts", &["sh", "rs", "py"][..]),
        ("install", &["service", "plist", "sh", "1", "html"][..]),
    ] {
        collect(&root, &root.join(dir), extensions, &mut files)?;
    }
    files.sort();
    files.dedup();
    let out = PathBuf::from(
        env::var_os("OUT_DIR").ok_or_else(|| io::Error::other("missing output directory"))?,
    )
    .join("ferriprint-source.tar.gz");
    let gzip = GzBuilder::new()
        .mtime(0)
        .write(fs::File::create(out)?, Compression::default());
    let mut archive = tar::Builder::new(gzip);
    for file in files {
        println!("cargo:rerun-if-changed={}", file.display());
        let bytes = fs::read(root.join(&file))?;
        let mut header = tar::Header::new_gnu();
        header.set_size(bytes.len() as u64);
        header.set_mode(if file.extension().is_some_and(|e| e == "sh") {
            0o755
        } else {
            0o644
        });
        header.set_uid(0);
        header.set_gid(0);
        header.set_mtime(0);
        header.set_cksum();
        archive.append_data(
            &mut header,
            Path::new("ferriprint").join(file),
            bytes.as_slice(),
        )?;
    }
    archive.into_inner()?.finish()?;
    Ok(())
}
