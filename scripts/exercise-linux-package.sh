#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 Robert Ressl
set -Eeuo pipefail
# Run only in disposable Debian/Ubuntu containers. Never mount /run, /etc, /var,
# CUPS sockets, host devices or the Docker socket into the test container.
if [[ $# -ne 2 ]]; then
  echo "usage: $0 IMAGE@sha256:DIGEST RELEASE_ASSET_DIRECTORY" >&2
  exit 64
fi
image="$1"
assets="$(cd "$2" && pwd -P)"
[[ "$image" =~ @sha256:[0-9a-f]{64}$ ]] || { echo 'digest-pinned test image required' >&2; exit 64; }
docker run --rm --platform linux/amd64 --cap-drop=ALL --security-opt=no-new-privileges \
  --cap-add=CHOWN --cap-add=SETUID --cap-add=SETGID --cap-add=DAC_OVERRIDE --cap-add=FOWNER \
  --mount "type=bind,src=$assets,dst=/assets,readonly" \
  "$image" bash -Eeuo pipefail -c '
    (cd /assets && sha256sum --check SHA256SUMS)
    export DEBIAN_FRONTEND=noninteractive
    # Only this disposable container receives packages. policy-rc.d prevents
    # dependency maintainer scripts starting daemons when no systemd is running.
    printf "#!/bin/sh\nexit 101\n" > /usr/sbin/policy-rc.d
    chmod 0755 /usr/sbin/policy-rc.d
    apt-get update -qq
    apt-get install -y --no-install-recommends ca-certificates cups-client adduser python3 python3-jsonschema
    mkdir /tmp/source-metadata
    tar -xzf /assets/ferriprint-*-source.tar.gz -C /tmp/source-metadata
    source_commit="$(grep -h "^commit=" /assets/ferriprint-*-build-manifest.txt | cut -d= -f2)"
    python3 /tmp/source-metadata/ferriprint/scripts/verify-release-metadata.py "$source_commit" /assets/ferriprint-*.spdx.json
    test ! -S /run/cups/cups.sock
    # Client-only images need not contain /etc/cups. Seed meaningful existing
    # operator client configuration inside THIS disposable container only.
    install -d -m 0755 /etc/cups
    printf "# Disposable CUPS preservation sentinel\nServerName native.example.test\n" > /etc/cups/client.conf
    before="$(find /etc/cups -type f -exec sha256sum {} + 2>/dev/null | sort)"
    dpkg-deb --extract /assets/ferriprint_*_amd64.deb /tmp/package-payload
    cmp /tmp/source-metadata/ferriprint/install/RUST-STDLIB-NOTICES.html /tmp/package-payload/usr/share/doc/ferriprint/RUST-STDLIB-NOTICES.html
    dpkg -i /assets/ferriprint_*_amd64.deb
    test -x /usr/bin/ferriprint
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
    printf "preserve durable data\n" > /var/lib/ferriprint/package-removal-sentinel
    printf "preserve operator config\n" > /etc/ferriprint/package-removal-sentinel
    # Reconfiguration/upgrade must preserve state and not enable/start/restart.
    dpkg -i /assets/ferriprint_*_amd64.deb
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
    tar -xzf /assets/ferriprint-*-linux-amd64.tar.gz --strip-components=1 -C /opt/ferriprint/releases/tar-acceptance
    tarroot=/opt/ferriprint/releases/tar-acceptance
    ln -s "$tarroot/bin/ferriprint" /usr/bin/ferriprint
    install -d -m 0755 /etc/systemd/system /usr/share/man/man1 /usr/share/doc/ferriprint
    install -m 0644 "$tarroot/lib/systemd/system/ferriprint.service" /etc/systemd/system/ferriprint.service
    install -m 0644 "$tarroot/share/man/man1/ferriprint.1" /usr/share/man/man1/ferriprint.1
    cp -a "$tarroot/share/doc/ferriprint/." /usr/share/doc/ferriprint/
    cmp /tmp/source-metadata/ferriprint/install/RUST-STDLIB-NOTICES.html /usr/share/doc/ferriprint/RUST-STDLIB-NOTICES.html
    cmp /tmp/package-payload/usr/share/doc/ferriprint/copyright /usr/share/doc/ferriprint/copyright
    test "$(stat -c %u:%g "$tarroot/bin/ferriprint")" = 0:0
    test "$(stat -c %u:%g /etc/systemd/system/ferriprint.service)" = 0:0
    /usr/bin/ferriprint --version
    /usr/bin/ferriprint --help
    install -d -m 0700 -o ferriprint -g ferriprint /tmp/tar-check /tmp/tar-check/profiles
    install -m 0600 -o ferriprint -g ferriprint /usr/share/doc/ferriprint/examples/profile.toml /tmp/tar-check/profiles/demo.toml
    python3 -c "from pathlib import Path; p=Path(\"/usr/share/doc/ferriprint/examples/ferriprint.toml\"); s=p.read_text().replace(\"../var/demo\", \"/tmp/tar-check\").replace(\"../tests/fixtures/native.ppd\", \"/usr/share/doc/ferriprint/examples/native.ppd\"); Path(\"/tmp/tar-check/ferriprint.toml\").write_text(s)"
    chown ferriprint:ferriprint /tmp/tar-check/ferriprint.toml
    chmod 0600 /tmp/tar-check/ferriprint.toml
    runuser -u ferriprint -- /usr/bin/ferriprint --config /tmp/tar-check/ferriprint.toml check
    test ! -e /etc/systemd/system/multi-user.target.wants/ferriprint.service
    rm /usr/bin/ferriprint /etc/systemd/system/ferriprint.service /usr/share/man/man1/ferriprint.1
    test "$before" = "$(find /etc/cups -type f -exec sha256sum {} + 2>/dev/null | sort)"
    test ! -S /run/cups/cups.sock
    printf "package_exercise=PASS tar_install_check=PASS os=%s architecture=%s ownership=PASS no_service_autostart=PASS cups_unchanged=PASS state_retained=PASS\n" \
      "$(. /etc/os-release; printf %s "$PRETTY_NAME")" "$(dpkg --print-architecture)"
  '
