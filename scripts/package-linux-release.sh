#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 Robert Ressl
set -Eeuo pipefail
# Keep dpkg/Python inside the same native pinned builder, including on macOS.
if [[ $# -ne 5 ]]; then
  echo "usage: $0 COMMIT RUST_IMAGE@sha256:DIGEST linux/amd64|linux/arm64 BUILD_DIRECTORY ASSET_DIRECTORY" >&2
  exit 64
fi
commit="$1"
image="$2"
platform="$3"
[[ "$commit" =~ ^[0-9a-f]{40}$ && "$image" =~ @sha256:[0-9a-f]{64}$ ]] || {
  echo 'full commit and digest-pinned image required' >&2; exit 64;
}
case "$platform" in linux/amd64|linux/arm64) ;; *) echo 'unsupported package platform' >&2; exit 64 ;; esac
case "$(docker info --format '{{.Architecture}}')" in
  amd64|x86_64) daemon_platform=linux/amd64 ;;
  arm64|aarch64) daemon_platform=linux/arm64 ;;
  *) echo 'unsupported Docker daemon architecture' >&2; exit 1 ;;
esac
[[ "$daemon_platform" == "$platform" ]] || { echo 'native packaging required; Docker architecture differs' >&2; exit 1; }
root="$(git -C "$(dirname "$0")" rev-parse --show-toplevel)"
build="$(cd "$4" && pwd -P)"
grep -Fxq "platform=$platform" "$build/build-manifest.txt" || { echo 'build platform differs from package platform' >&2; exit 1; }
grep -Fxq "image=$image" "$build/build-manifest.txt" || { echo 'package image differs from the exact build image' >&2; exit 1; }
mkdir -p "$5"
assets="$(cd "$5" && pwd -P)"
docker run --rm --platform "$platform" --network=none --cap-drop=ALL --security-opt=no-new-privileges \
  --user "$(id -u):$(id -g)" \
  --mount "type=bind,src=$root,dst=/repo,readonly" \
  --mount "type=bind,src=$build,dst=/build,readonly" \
  --mount "type=bind,src=$assets,dst=/assets" \
  --env GIT_CONFIG_COUNT=1 --env GIT_CONFIG_KEY_0=safe.directory --env GIT_CONFIG_VALUE_0=/repo \
  --workdir /repo "$image" python3 scripts/package-linux-release.py "$commit" /build /assets
