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
case "$platform" in
  linux/amd64) architecture=amd64; machine=x86_64; rust_host=x86_64-unknown-linux-gnu; elf_machine=62 ;;
  linux/arm64) architecture=arm64; machine=aarch64; rust_host=aarch64-unknown-linux-gnu; elf_machine=183 ;;
  *) echo "unsupported platform" >&2; exit 64 ;;
esac
daemon_architecture="$(docker info --format '{{.Architecture}}')"
case "$daemon_architecture" in
  amd64|x86_64) daemon_platform=linux/amd64 ;;
  arm64|aarch64) daemon_platform=linux/arm64 ;;
  *) echo "unsupported Docker daemon architecture: $daemon_architecture" >&2; exit 1 ;;
esac
[[ "$daemon_platform" == "$platform" ]] || {
  echo "native build required: Docker daemon $daemon_platform does not match $platform" >&2; exit 1;
}
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
  --env "FERRIPRINT_BUILD_PLATFORM=$platform" --env "FERRIPRINT_BUILD_ARCHITECTURE=$architecture" \
  --env "FERRIPRINT_BUILD_MACHINE=$machine" --env "FERRIPRINT_BUILD_RUST_HOST=$rust_host" \
  --env "FERRIPRINT_BUILD_ELF_MACHINE=$elf_machine" --env "FERRIPRINT_DAEMON_ARCHITECTURE=$daemon_architecture" \
  "$image" bash -Eeuo pipefail -c '
    test "$(uname -m)" = "$FERRIPRINT_BUILD_MACHINE"
    test "$(dpkg --print-architecture)" = "$FERRIPRINT_BUILD_ARCHITECTURE"
    test "$(rustc --version --verbose | grep "^host: " | cut -d" " -f2)" = "$FERRIPRINT_BUILD_RUST_HOST"
    mkdir /tmp/build
    tar -xf /source.tar -C /tmp/build
    cd /tmp/build
    cargo metadata --locked --format-version 1 > /tmp/cargo-metadata.json
    python3 scripts/release-metadata.py --metadata /tmp/cargo-metadata.json \
      --commit "$FERRIPRINT_BUILD_COMMIT" --source-date-epoch "$SOURCE_DATE_EPOCH" \
      --notices THIRD_PARTY_NOTICES.md --check-notices --sbom /out/ferriprint.spdx.json
    cargo build --locked --release --bin ferriprint --example receiver
    install -m 0755 target/release/ferriprint /out/ferriprint
    install -d -m 0755 /out/qa
    install -m 0755 target/release/examples/receiver /out/qa/receiver
    python3 -c "import os
with open(\"/out/ferriprint\", \"rb\") as executable:
    header = executable.read(20)
assert header[:6] == b\"\x7fELF\x02\x01\" and int.from_bytes(header[18:20], \"little\") == int(os.environ[\"FERRIPRINT_BUILD_ELF_MACHINE\"]), \"ELF architecture mismatch\""
    /out/ferriprint --version
    python3 scripts/extract-source.py /out/ferriprint /out/ferriprint-source.tar.gz
    {
      printf "commit=%s\ntree=%s\nimage=%s\nplatform=%s\narchitecture=%s\nnative_daemon_architecture=%s\nmachine=%s\nelf_machine=%s\nnative_execution=PASS\nsource_date_epoch=%s\n" \
        "$FERRIPRINT_BUILD_COMMIT" "$FERRIPRINT_BUILD_TREE" "$FERRIPRINT_BUILD_IMAGE" \
        "$FERRIPRINT_BUILD_PLATFORM" "$FERRIPRINT_BUILD_ARCHITECTURE" \
        "$FERRIPRINT_DAEMON_ARCHITECTURE" "$(uname -m)" "$FERRIPRINT_BUILD_ELF_MACHINE" "$SOURCE_DATE_EPOCH"
      rustc --version --verbose
      cargo --version --verbose
      sha256sum Cargo.lock install/RUST-STDLIB-NOTICES.html /out/ferriprint /out/ferriprint-source.tar.gz /out/ferriprint.spdx.json
      ldd /out/ferriprint
      readelf --version-info /out/ferriprint
      dpkg-query -W libc6 libgcc-s1
    } > /out/build-manifest.txt
  '
(cd "$root" && python3 scripts/verify-source.py "$commit" "$out/ferriprint-source.tar.gz")
printf 'release artifacts: %s\n' "$out"
