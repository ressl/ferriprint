#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 Robert Ressl
set -Eeuo pipefail

# Build a clean, exact revision in a disposable Linux container. No host toolchain
# installation, operator files, SSH credentials, or writable source mount is needed.
if [[ $# -ne 4 ]]; then
  echo "usage: $0 COMMIT RUST_IMAGE@sha256:DIGEST linux/amd64|linux/arm64 OUTPUT_DIRECTORY" >&2
  exit 64
fi
commit="$1"
image="$2"
platform="$3"
out="$4"
[[ "$commit" =~ ^[0-9a-f]{40}$ ]] || { echo "full commit required" >&2; exit 64; }
[[ "$image" =~ @sha256:[0-9a-f]{64}$ ]] || { echo "digest-pinned Rust image required" >&2; exit 64; }
case "$platform" in linux/amd64|linux/arm64) ;; *) echo "unsupported platform" >&2; exit 64 ;; esac
root="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
[[ "$(git -C "$root" rev-parse "$commit^{commit}")" == "$commit" ]]
mkdir -p "$out"
out="$(cd "$out" && pwd -P)"
[[ -z "$(find "$out" -mindepth 1 -maxdepth 1 -print -quit)" ]] || {
  echo "output directory must be empty" >&2; exit 1;
}
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
git -C "$root" archive --format=tar "$commit" -o "$work/source.tar"
epoch="$(git -C "$root" show -s --format=%ct "$commit")"
tree="$(git -C "$root" rev-parse "$commit^{tree}")"
docker run --rm --platform "$platform" --cap-drop=ALL --security-opt=no-new-privileges \
  --user "$(id -u):$(id -g)" --env CARGO_HOME=/tmp/cargo \
  --mount "type=bind,src=$work/source.tar,dst=/source.tar,readonly" \
  --mount "type=bind,src=$out,dst=/out" \
  --env "FERRIPRINT_BUILD_COMMIT=$commit" --env "FERRIPRINT_BUILD_IMAGE=$image" \
  --env "SOURCE_DATE_EPOCH=$epoch" --env "FERRIPRINT_BUILD_TREE=$tree" \
  "$image" bash -Eeuo pipefail -c '
    mkdir /tmp/build
    tar -xf /source.tar -C /tmp/build
    cd /tmp/build
    cargo metadata --locked --format-version 1 > /tmp/cargo-metadata.json
    python3 scripts/release-metadata.py --metadata /tmp/cargo-metadata.json \
      --commit "$FERRIPRINT_BUILD_COMMIT" --source-date-epoch "$SOURCE_DATE_EPOCH" \
      --notices THIRD_PARTY_NOTICES.md --check-notices --sbom /out/ferriprint.spdx.json
    cargo build --locked --release --bin ferriprint
    install -m 0755 target/release/ferriprint /out/ferriprint
    python3 scripts/extract-source.py /out/ferriprint /out/ferriprint-source.tar.gz
    {
      printf "commit=%s\ntree=%s\nimage=%s\nplatform=%s\nsource_date_epoch=%s\n" \
        "$FERRIPRINT_BUILD_COMMIT" "$FERRIPRINT_BUILD_TREE" "$FERRIPRINT_BUILD_IMAGE" \
        "$(uname -m)" "$SOURCE_DATE_EPOCH"
      rustc --version --verbose
      cargo --version --verbose
      sha256sum Cargo.lock install/RUST-STDLIB-NOTICES.html /out/ferriprint /out/ferriprint-source.tar.gz /out/ferriprint.spdx.json
      ldd /out/ferriprint
    } > /out/build-manifest.txt
  '
(cd "$root" && python3 scripts/verify-source.py "$commit" "$out/ferriprint-source.tar.gz")
printf 'release artifacts: %s\n' "$out"
