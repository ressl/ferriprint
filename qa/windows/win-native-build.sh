#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Disposable hosted-VM QA only. Builds the checked-out frontend revision inside
# an owned WSL1 Debian distro and serves it on literal loopback. No physical
# printer, no release artifact, no outbound printer connection.
set -Eeuo pipefail
export LC_ALL=C

work="$1"
mode="$2"
shift 2
root=/opt/ferriprint-win-native-qa
state=/var/lib/ferriprint-win-native-qa
src="$work/source"

fetch() {
  curl --fail --silent --show-error --location \
    --connect-timeout 15 --max-time 300 "$1" --output "$2"
  printf '%s  %s\n' "$3" "$2" | sha256sum --check --status
}

case "$mode" in
  build)
    test "$(id -u)" = 0
    test ! -e "$root"
    test ! -e "$state"
    rm -f /etc/apt/sources.list.d/debian.sources
    printf '%s\n' \
      'deb http://deb.debian.org/debian bookworm main' \
      > /etc/apt/sources.list
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
      ca-certificates curl gcc libc6-dev pkg-config xz-utils >/dev/null
    install -d -m 0700 "$work/dl"
    fetch https://static.rust-lang.org/dist/rust-1.99.1-x86_64-unknown-linux-gnu.tar.xz \
      "$work/dl/rust.tar.xz" 231fcae47ee950621374725b5fc3b5441d2fbda2955ac45dd7dc816d8876cfe7
    tar -xJf "$work/dl/rust.tar.xz" -C "$work/dl"
    "$work/dl/rust-1.99.1-x86_64-unknown-linux-gnu/install.sh" \
      --prefix=/opt/ferriprint-qa-rust --without=rust-docs >/dev/null
    export PATH=/opt/ferriprint-qa-rust/bin:$PATH
    rustc --version --verbose

    # Build on the distro's own filesystem; /mnt is a 9p share.
    install -d -m 0700 "$root/build"
    cp -a "$src/." "$root/build/"
    cd "$root/build"
    sha256sum Cargo.lock > "$work/proof/built-cargo-lock.sha256"
    # The staged source tree has no .git; the workflow supplies the exact commit.
    test -n "${QA_SOURCE_SHA:-}"
    printf '%s\n' "$QA_SOURCE_SHA" > "$work/proof/built-git-revision.txt"
    cargo build --locked --release --bin ferriprint --example receiver
    install -m 0550 target/release/ferriprint "$root/ferriprint"
    install -m 0550 target/release/examples/receiver "$root/receiver"
    sha256sum "$root/ferriprint" "$root/receiver" > "$work/proof/built-binaries.sha256"

    install -d -m 0700 "$state"
    printf '%s\n' \
      'listen = "127.0.0.1:18634"' \
      'public_url = "ipp://127.0.0.1:18634/"' \
      'state_dir = "/var/lib/ferriprint-win-native-qa"' \
      'allowed_clients = ["127.0.0.1/32"]' \
      '[access]' \
      'anonymous_trusted_lan = true' \
      'anonymous_owner = "anonymous"' \
      'anonymous_clients = ["127.0.0.1/32"]' \
      '[[cups_sources]]' \
      'id = "local"' \
      'server_uri = "ipp://127.0.0.1:18631/"' \
      'username = "ferriprint"' \
      > "$root/ferriprint.toml"
    chmod 0600 "$root/ferriprint.toml"
    ;;
  start-receiver)
    nohup "$root/receiver" --listen 127.0.0.1:18631 \
      > "$work/receiver.log" 2>&1 < /dev/null &
    ;;
  install-and-start)
    # The credential arrives on stdin so it never appears in a process argument.
    IFS= read -r secret
    test -n "$secret"
    printf '%s\n' "$secret" | "$root/ferriprint" --config "$root/ferriprint.toml" \
      user init --username qa-admin
    "$root/ferriprint" --config "$root/ferriprint.toml" setup install \
      --source local --queue receiver --printer win-native-caps \
      --display-name 'Ferriprint WIN NATIVE CAPS QA' \
      --service-name 'Ferriprint WIN NATIVE CAPS QA' \
      --location 'Disposable hosted VM loopback; no physical device' \
      --choices "$work/choices.toml" --confirm > "$work/proof/setup-install.json"
    nohup "$root/ferriprint" --config "$root/ferriprint.toml" serve \
      > "$work/frontend.log" 2>&1 < /dev/null &
    ;;
  *) exit 64 ;;
esac
