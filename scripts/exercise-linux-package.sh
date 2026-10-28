#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 Robert Ressl
set -Eeuo pipefail
# Run only in disposable Debian/Ubuntu containers. Never mount /run, /etc, /var,
# CUPS sockets, host devices or the Docker socket into the test container.
if [[ $# -ne 4 ]]; then
  echo "usage: $0 IMAGE@sha256:DIGEST linux/amd64|linux/arm64 RELEASE_ASSET_DIRECTORY EXACT_BUILD_DIRECTORY" >&2
  exit 64
fi
image="$1"
platform="$2"
assets="$(cd "$3" && pwd -P)"
build="$(cd "$4" && pwd -P)"
[[ "$image" =~ @sha256:[0-9a-f]{64}$ ]] || { echo 'digest-pinned test image required' >&2; exit 64; }
case "$platform" in
  linux/amd64) architecture=amd64; machine=x86_64 ;;
  linux/arm64) architecture=arm64; machine=aarch64 ;;
  *) echo 'unsupported test platform' >&2; exit 64 ;;
esac
daemon_architecture="$(docker info --format '{{.Architecture}}')"
case "$daemon_architecture" in
  amd64|x86_64) daemon_platform=linux/amd64 ;;
  arm64|aarch64) daemon_platform=linux/arm64 ;;
  *) echo 'unsupported Docker daemon architecture' >&2; exit 1 ;;
esac
[[ "$daemon_platform" == "$platform" ]] || { echo 'native package execution required; Docker architecture differs' >&2; exit 1; }
[[ -x "$build/qa/receiver" && -f "$build/build-manifest.txt" ]] || { echo 'matching exact-build QA receiver/manifest required' >&2; exit 1; }
docker run --rm --platform "$platform" --cap-drop=ALL --security-opt=no-new-privileges \
  --cap-add=CHOWN --cap-add=SETUID --cap-add=SETGID --cap-add=DAC_OVERRIDE --cap-add=FOWNER \
  --mount "type=bind,src=$assets,dst=/assets,readonly" \
  --mount "type=bind,src=$build/qa,dst=/qa,readonly" \
  --mount "type=bind,src=$build/build-manifest.txt,dst=/qa-build-manifest.txt,readonly" \
  --env "FERRIPRINT_TEST_ARCHITECTURE=$architecture" --env "FERRIPRINT_TEST_MACHINE=$machine" \
  "$image" bash -Eeuo pipefail -c '
    test "$(uname -m)" = "$FERRIPRINT_TEST_MACHINE"
    test "$(dpkg --print-architecture)" = "$FERRIPRINT_TEST_ARCHITECTURE"
    (cd /assets && sha256sum --check SHA256SUMS)
    export DEBIAN_FRONTEND=noninteractive
    # Only this disposable container receives packages. policy-rc.d prevents
    # dependency maintainer scripts starting daemons when no systemd is running.
    printf "#!/bin/sh\nexit 101\n" > /usr/sbin/policy-rc.d
    chmod 0755 /usr/sbin/policy-rc.d
    apt-get update -qq
    apt-get install -y --no-install-recommends ca-certificates cups-client adduser python3 python3-jsonschema
    mkdir /tmp/source-metadata
    source_archive=(/assets/ferriprint-*-source.tar.gz)
    test "${#source_archive[@]}" = 1
    tar -xzf "${source_archive[0]}" -C /tmp/source-metadata
    manifest=(/assets/ferriprint-*-linux-"$FERRIPRINT_TEST_ARCHITECTURE"-build-manifest.txt)
    test "${#manifest[@]}" = 1
    cmp /qa-build-manifest.txt "${manifest[0]}"
    source_commit="$(grep "^commit=" "${manifest[0]}" | cut -d= -f2)"
    python3 /tmp/source-metadata/ferriprint/scripts/verify-release-metadata.py "$source_commit" /assets/ferriprint-*.spdx.json
    test ! -S /run/cups/cups.sock
    # Client-only images need not contain /etc/cups. Seed meaningful existing
    # operator client configuration inside THIS disposable container only.
    install -d -m 0755 /etc/cups
    printf "# Disposable CUPS preservation sentinel\nServerName native.example.test\n" > /etc/cups/client.conf
    before="$(find /etc/cups -type f -exec sha256sum {} + 2>/dev/null | sort)"
    package=(/assets/ferriprint_*_"$FERRIPRINT_TEST_ARCHITECTURE".deb)
    test "${#package[@]}" = 1
    test "$(dpkg-deb -f "${package[0]}" Architecture)" = "$FERRIPRINT_TEST_ARCHITECTURE"
    dpkg-deb --extract "${package[0]}" /tmp/package-payload
    cmp /tmp/source-metadata/ferriprint/install/RUST-STDLIB-NOTICES.html /tmp/package-payload/usr/share/doc/ferriprint/RUST-STDLIB-NOTICES.html
    dpkg -i "${package[0]}"
    test -x /usr/bin/ferriprint
    cmp /tmp/package-payload/usr/bin/ferriprint /usr/bin/ferriprint
    /usr/bin/ferriprint --version
    # Respect the image/operator doc filter; its standard copyright exception
    # must still retain our complete license notices byte-for-byte.
    cmp /tmp/package-payload/usr/share/doc/ferriprint/copyright /usr/share/doc/ferriprint/copyright
    test "$(stat -c %a /var/lib/ferriprint)" = 700
    test "$(stat -c %a /etc/ferriprint)" = 750
    service_owner="$(id -u ferriprint):$(id -g ferriprint)"
    test "$(stat -c %u:%g /var/lib/ferriprint)" = "$service_owner"
    test "$(stat -c %u:%g /etc/ferriprint)" = "0:$(id -g ferriprint)"
    test "$(id -Gn ferriprint)" = ferriprint
    test "$(getent passwd ferriprint | cut -d: -f7)" = /usr/sbin/nologin
    test ! -e /etc/ferriprint/ferriprint.toml
    test ! -e /etc/systemd/system/multi-user.target.wants/ferriprint.service
    test ! -e /run/ferriprint
    after="$(find /etc/cups -type f -exec sha256sum {} + 2>/dev/null | sort)"
    test "$before" = "$after"
    runuser -u ferriprint -- python3 /tmp/source-metadata/ferriprint/scripts/exercise-installed-runtime.py \
      --binary /usr/bin/ferriprint --receiver /qa/receiver \
      --fixture /tmp/source-metadata/ferriprint/tests/fixtures/native.ppd \
      --profile /tmp/package-payload/usr/share/doc/ferriprint/examples/profile.toml \
      --source-archive "${source_archive[0]}" --installation package
    printf "preserve durable data\n" > /var/lib/ferriprint/package-removal-sentinel
    printf "preserve operator config\n" > /etc/ferriprint/package-removal-sentinel
    # Reconfiguration/upgrade must preserve state and not enable/start/restart.
    dpkg -i "${package[0]}"
    test "$(stat -c %u:%g /var/lib/ferriprint)" = "$service_owner"
    test -f /var/lib/ferriprint/package-removal-sentinel
    test ! -e /etc/systemd/system/multi-user.target.wants/ferriprint.service
    dpkg --remove ferriprint
    test ! -e /usr/bin/ferriprint
    test -f /var/lib/ferriprint/package-removal-sentinel
    test -f /etc/ferriprint/package-removal-sentinel
    dpkg --purge ferriprint
    test -f /var/lib/ferriprint/package-removal-sentinel
    test -f /etc/ferriprint/package-removal-sentinel
    test "$(stat -c %u:%g /var/lib/ferriprint)" = "$service_owner"
    getent passwd ferriprint >/dev/null
    # Exercise the documented tar INSTALL, not merely an extracted --version.
    install -d -m 0755 /opt/ferriprint/releases/tar-acceptance
    tar -xzf /assets/ferriprint-*-linux-"$FERRIPRINT_TEST_ARCHITECTURE".tar.gz --strip-components=1 -C /opt/ferriprint/releases/tar-acceptance
    tarroot=/opt/ferriprint/releases/tar-acceptance
    ln -s "$tarroot/bin/ferriprint" /usr/bin/ferriprint
    install -d -m 0755 /etc/systemd/system /usr/share/man/man1 /usr/share/doc/ferriprint
    install -m 0644 "$tarroot/lib/systemd/system/ferriprint.service" /etc/systemd/system/ferriprint.service
    install -m 0644 "$tarroot/share/man/man1/ferriprint.1" /usr/share/man/man1/ferriprint.1
    cp -a "$tarroot/share/doc/ferriprint/." /usr/share/doc/ferriprint/
    cmp /tmp/source-metadata/ferriprint/install/RUST-STDLIB-NOTICES.html /usr/share/doc/ferriprint/RUST-STDLIB-NOTICES.html
    cmp /tmp/package-payload/usr/share/doc/ferriprint/copyright /usr/share/doc/ferriprint/copyright
    cmp /tmp/package-payload/usr/bin/ferriprint "$tarroot/bin/ferriprint"
    test "$(stat -c %u:%g "$tarroot/bin/ferriprint")" = 0:0
    test "$(stat -c %u:%g /etc/systemd/system/ferriprint.service)" = 0:0
    /usr/bin/ferriprint --version
    /usr/bin/ferriprint --help
    runuser -u ferriprint -- python3 /tmp/source-metadata/ferriprint/scripts/exercise-installed-runtime.py \
      --binary /usr/bin/ferriprint --receiver /qa/receiver \
      --fixture /usr/share/doc/ferriprint/examples/native.ppd \
      --profile /usr/share/doc/ferriprint/examples/profile.toml \
      --source-archive "${source_archive[0]}" --installation tar
    test ! -e /etc/systemd/system/multi-user.target.wants/ferriprint.service
    rm /usr/bin/ferriprint /etc/systemd/system/ferriprint.service /usr/share/man/man1/ferriprint.1
    test "$before" = "$(find /etc/cups -type f -exec sha256sum {} + 2>/dev/null | sort)"
    test ! -S /run/cups/cups.sock
    printf "package_exercise=PASS tar_install_check=PASS native_runtime_receiver=PASS os=%s architecture=%s machine=%s ownership=PASS no_service_autostart=PASS cups_unchanged=PASS state_retained=PASS\n" \
      "$(. /etc/os-release; printf %s "$PRETTY_NAME")" "$(dpkg --print-architecture)" "$(uname -m)"
  '
