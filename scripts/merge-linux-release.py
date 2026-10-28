#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 Robert Ressl
"""Merge both native release gates, requiring byte-identical common source/SPDX."""
import argparse
import filecmp
import hashlib
from pathlib import Path
import re
import shutil


def checksum(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def inputs(directory, architecture):
    if not directory.is_dir() or any(not p.is_file() or p.is_symlink() for p in directory.iterdir()):
        raise SystemExit(f"release inputs must contain only regular files: {directory}")
    sources = list(directory.glob("ferriprint-*-source.tar.gz"))
    if len(sources) != 1:
        raise SystemExit(f"one exact source archive required: {directory}")
    version = sources[0].name.removeprefix("ferriprint-").removesuffix("-source.tar.gz")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", version):
        raise SystemExit("unsupported release version")
    debversion = version.replace("-", "~", 1) + "-1"
    names = {
        f"ferriprint-{version}-source.tar.gz",
        f"ferriprint-{version}.spdx.json",
        f"ferriprint-{version}-linux-{architecture}.tar.gz",
        f"ferriprint-{version}-linux-{architecture}-build-manifest.txt",
        f"ferriprint_{debversion}_{architecture}.deb",
    }
    if {p.name for p in directory.iterdir()} != names | {"SHA256SUMS"}:
        raise SystemExit(f"missing or extra {architecture} release assets")
    sums = {}
    for line in (directory / "SHA256SUMS").read_text().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64})  ([^/\\]+)", line)
        if not match or match[2] in sums:
            raise SystemExit(f"invalid/duplicate checksum entry: {directory}")
        sums[match[2]] = match[1]
    if set(sums) != names or any(checksum(directory / name) != sums[name] for name in names):
        raise SystemExit(f"{architecture} release checksum mismatch")
    manifest = (directory / f"ferriprint-{version}-linux-{architecture}-build-manifest.txt").read_text()
    for expected in [f"architecture={architecture}\n", f"platform=linux/{architecture}\n", "native_execution=PASS\n"]:
        if expected not in manifest:
            raise SystemExit(f"missing native {architecture} build proof")
    identities = []
    for key in ["commit", "tree"]:
        matches = re.findall(rf"^{key}=([0-9a-f]{{40}})$", manifest, re.MULTILINE)
        if len(matches) != 1:
            raise SystemExit(f"one exact {key} required in {architecture} manifest")
        identities.append(matches[0])
    for name in [f"ferriprint-{version}-source.tar.gz", f"ferriprint-{version}.spdx.json"]:
        if sums[name] not in manifest:
            raise SystemExit(f"common asset not identified by {architecture} manifest: {name}")
    return version, identities, names


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("amd64_assets", type=Path)
    parser.add_argument("arm64_assets", type=Path)
    parser.add_argument("output_directory", type=Path)
    args = parser.parse_args()
    amd64 = inputs(args.amd64_assets, "amd64")
    arm64 = inputs(args.arm64_assets, "arm64")
    if amd64[:2] != arm64[:2]:
        raise SystemExit("native release versions/commits/trees differ")
    common = amd64[2] & arm64[2]
    for name in common:
        if not filecmp.cmp(args.amd64_assets / name, args.arm64_assets / name, shallow=False):
            raise SystemExit(f"common release bytes differ; refusing publication: {name}")
    out = args.output_directory
    out.mkdir(parents=True, exist_ok=True)
    if any(out.iterdir()):
        raise SystemExit("release output directory must be empty")
    for source, description in [(args.amd64_assets, amd64), (args.arm64_assets, arm64)]:
        for name in sorted(description[2]):
            if not (out / name).exists():
                shutil.copy2(source / name, out / name)
    (out / "SHA256SUMS").write_text("".join(f"{checksum(path)}  {path.name}\n" for path in sorted(out.iterdir())))
    print(f"release_merge=PASS version={amd64[0]} commit={amd64[1][0]} native_architectures=amd64,arm64 common_source_spdx=byte-identical checksum_set=single")


if __name__ == "__main__":
    main()
