#!/usr/bin/env bash
# SPDX-License-Identifier: AGPL-3.0-only
# Proposed disposable hosted-VM setup only. Never executed before Parent gate.
set -Eeuo pipefail
export LC_ALL=C
work="$1"
mode="$2"
root=/opt/ferriprint-windows-local-qa
state=/var/lib/ferriprint-windows-local-qa
frontend_sha=473767d296f1cc2f86d0f84312da31f4aa031e195437ef7f5d348ad3f5e0bee6
verify() { printf '%s  %s\n' "$1" "$2" | sha256sum --check --status; }
fetch() { curl --fail --silent --show-error --location --connect-timeout 15 --max-time 180 "$1" --output "$2"; verify "$3" "$2"; }
owned_pid() {
  local expected="$1" link candidate='' matches=0
  for link in /proc/[0-9]*/exe; do
    test "$(readlink "$link" 2>/dev/null || true)" = "$expected" || continue
    test "$(stat -c '%u' "$link" 2>/dev/null || true)" = 994 || continue
    candidate="${link#/proc/}"
    candidate="${candidate%/exe}"
    matches=$((matches + 1))
  done
  test "$matches" = 1
  printf '%s\n' "$candidate"
}
case "$mode" in
  build)
    test "$(id -u)" = 0
    test ! -e "$root"
    test ! -e "$state"
    if ! test -f /sbin/runuser || ! test -x /sbin/runuser; then
      printf '%s\n' 'Required existing Debian /sbin/runuser is missing or not executable; no install or fallback is authorized' >&2
      exit 1
    fi
    # These package operations are confined to the newly imported disposable distro.
    # The signed timestamped Debian snapshot keeps inputs bounded; validity/signatures stay enabled.
    rm -f /etc/apt/sources.list.d/debian.sources
    printf '%s\n' \
      'deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] http://snapshot.debian.org/archive/debian/20261028T000000Z bookworm main' \
      'deb [signed-by=/usr/share/keyrings/debian-archive-keyring.gpg] http://snapshot.debian.org/archive/debian-security/20261028T000000Z bookworm-security main' \
      > /etc/apt/sources.list
    apt-get update -qq
    DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends ca-certificates curl gcc libc6-dev pkg-config xz-utils strace patch
    test -f /usr/bin/strace && test -x /usr/bin/strace
    install -d -m 0700 "$work/build" "$work/proof"
    fetch https://static.rust-lang.org/dist/rust-1.99.1-x86_64-unknown-linux-gnu.tar.xz "$work/build/rust.tar.xz" 231fcae47ee950621374725b5fc3b5441d2fbda2955ac45dd7dc816d8876cfe7
    tar -xJf "$work/build/rust.tar.xz" -C "$work/build"
    "$work/build/rust-1.99.1-x86_64-unknown-linux-gnu/install.sh" --prefix=/opt/ferriprint-qa-rust --without=rust-docs
    export PATH=/opt/ferriprint-qa-rust/bin:$PATH
    rustc --version --verbose > "$work/proof/rust-toolchain.txt"
    fetch https://github.com/ressl/ferriprint/releases/download/v0.3.0/SHA256SUMS "$work/build/SHA256SUMS" eb03ce37a00595035738ae3741498d2b502d56090af2c0fc578cf79e6cc021b6
    fetch https://github.com/ressl/ferriprint/releases/download/v0.3.0/ferriprint-0.3.0-linux-amd64.tar.gz "$work/build/frontend.tar.gz" ecc0ed1610e3697a982403c2aba3e31d021347d754bd04122a110610b9e81b32
    fetch https://github.com/ressl/ferriprint/releases/download/v0.3.0/ferriprint-0.3.0-source.tar.gz "$work/build/source.tar.gz" 135ea616c34a10ed823461712f43d926a6e6dce12fdded96c59bf8830b2c4b1b
    grep -Fxq 'ecc0ed1610e3697a982403c2aba3e31d021347d754bd04122a110610b9e81b32  ferriprint-0.3.0-linux-amd64.tar.gz' "$work/build/SHA256SUMS"
    grep -Fxq '135ea616c34a10ed823461712f43d926a6e6dce12fdded96c59bf8830b2c4b1b  ferriprint-0.3.0-source.tar.gz' "$work/build/SHA256SUMS"
    tar -xzf "$work/build/frontend.tar.gz" -C "$work/build"
    tar -xzf "$work/build/source.tar.gz" -C "$work/build"
    verify a121364e8159cbb1ad2e5782f76ebe9f9981776abac42c986c5556cec0928240 "$work/build/ferriprint/tests/fixtures/native.ppd"
    verify 2a9b628feaaf696f1a147c9bc6c93dd55f19a793e2fb8754c919352ddae02c4d "$work/build/ferriprint/examples/profile.toml"
    verify 406e7f5e9ec8f7752a407bb7f2abfef6948f3f41287fd586759119d7bdb2c125 "$work/windows-metadata-only-receiver.rs"
    verify 6306e3814a2e9dc6ba7f51325e76fa8d6d11da77f5089ba8dfca4bec8399856e "$work/windows-public-choices.toml"
    # Fingerprint every original archive file before the separate derivative QA build.
    source_core_manifest="$work/proof/published-source-core.sha256"
    (cd "$work/build/ferriprint" && tar -tzf "$work/build/source.tar.gz" | while IFS= read -r member; do
      case "$member" in
        */) continue ;;
        ferriprint/*) relative="${member#ferriprint/}" ;;
        *) printf '%s\n' 'Unexpected published source archive member' >&2; exit 1 ;;
      esac
      sha256sum -- "$relative"
    done) > "$source_core_manifest"
    # Ordinary Cargo QA root; original core files and lock stay byte-exact.
    verify 862e18a17153381c26e1d8773c7b947487d948efdd4c5b629231be4fd050893c "$work/reader-build/Cargo.toml"
    verify f2c921156198575070a9bf48776cc4e3d7058cd3e6bd3de800589a73fa74c44a "$work/reader-build/Cargo.lock"
    verify 5103121e96c0ce7bd490b0c401e9e29823368cff7d6397711a6537fe849865e9 "$work/sqlite-wal-retry-counters.patch"
    fetch https://static.crates.io/crates/libsqlite3-sys/libsqlite3-sys-0.35.0.crate "$work/build/libsqlite3-sys-0.35.0.crate" 133c182a6a2c87864fe97778797e46c7e999672690dc9fa3ee8e241aa4a9c13f
    tar -xzf "$work/build/libsqlite3-sys-0.35.0.crate" -C "$work/build"
    sqlite_base="$work/build/libsqlite3-sys-0.35.0"
    sqlite_derivative="$work/build/patched-libsqlite3-sys-0.35.0"
    test ! -e "$sqlite_derivative"
    cp -a "$sqlite_base" "$sqlite_derivative"
    verify c9a0b6829b81d5f1b78392181f09744c818117a725667411d517b98149fcd3be "$sqlite_base/sqlite3/sqlite3.c"
    verify c9a0b6829b81d5f1b78392181f09744c818117a725667411d517b98149fcd3be "$sqlite_derivative/sqlite3/sqlite3.c"
    (cd "$sqlite_derivative" && patch --batch --forward --fuzz=0 --dry-run -p1 -i "$work/sqlite-wal-retry-counters.patch")
    (cd "$sqlite_derivative" && patch --batch --forward --fuzz=0 -p1 -i "$work/sqlite-wal-retry-counters.patch")
    verify 10d037540968e9a221986e7f742a1792c5c33ec6445051dd32c463a6f8bf0fbd "$sqlite_derivative/sqlite3/sqlite3.c"
    sqlite_other_manifest="$work/build/sqlite-unmodified-other-files.sha256"
    (cd "$sqlite_base" && tar -tzf "$work/build/libsqlite3-sys-0.35.0.crate" | while IFS= read -r member; do
      case "$member" in
        */) continue ;;
        libsqlite3-sys-0.35.0/*) relative="${member#libsqlite3-sys-0.35.0/}" ;;
        *) printf '%s\n' 'Unexpected SQLite crate member' >&2; exit 1 ;;
      esac
      if test "$relative" != sqlite3/sqlite3.c; then sha256sum -- "$relative"; fi
    done) > "$sqlite_other_manifest"
    (cd "$sqlite_derivative" && sha256sum --check --status "$sqlite_other_manifest")
    cargo build --locked --release --manifest-path "$work/reader-build/Cargo.toml" --target-dir "$work/build/receiver-target"
    (cd "$sqlite_derivative" && sha256sum --check --status "$sqlite_other_manifest")
    verify 10d037540968e9a221986e7f742a1792c5c33ec6445051dd32c463a6f8bf0fbd "$sqlite_derivative/sqlite3/sqlite3.c"
    (cd "$work/build/ferriprint" && sha256sum --check --status "$source_core_manifest")
    verify 135ea616c34a10ed823461712f43d926a6e6dce12fdded96c59bf8830b2c4b1b "$work/build/source.tar.gz"
    printf '%s\n' 'QA helper is an explicit derivative: separate QA manifest/lock and a pinned owned SQLite3.50.2 C copy with fixed WAL/DMS counts and first lock-return metadata. Original core72/Cargo.lock and shared registry/cache are not patched; published frontend ELF/source remain original. QA binary/source identity is NOT an official artifact or original-dependency identity. The recorded F_GETLK output PID is labelled UNCHANGED_INPUT_SENTINEL_NOT_A_HOLDER when it still equals the caller-supplied -2 sentinel, and DIFFERS_FROM_INPUT_SENTINEL_UNVERIFIED otherwise; request type/range/whence remain unchanged and a value equal to the input is never treated as a holder. The logical-ledger read is executed only after the owned frontend has exited, so it involves no competing product holder.' > "$work/proof/qa-receiver-source-scope.txt"
    printf '%s\n' '{"kind":"QA_DERIVATIVE_SQLITE_WAL_DMS_COUNTERS_V2","official_frontend_unchanged":true,"core72_and_core_lock_unchanged":true,"shared_registry_cache_patched":false,"base_crate_sha256":"133c182a6a2c87864fe97778797e46c7e999672690dc9fa3ee8e241aa4a9c13f","base_sqlite_version":"3.50.2","base_C_sha256":"c9a0b6829b81d5f1b78392181f09744c818117a725667411d517b98149fcd3be","patched_C_sha256":"10d037540968e9a221986e7f742a1792c5c33ec6445051dd32c463a6f8bf0fbd","explicit_patch_sha256":"5103121e96c0ce7bd490b0c401e9e29823368cff7d6397711a6537fe849865e9","QA_manifest_sha256":"862e18a17153381c26e1d8773c7b947487d948efdd4c5b629231be4fd050893c","QA_derivative_lock_sha256":"f2c921156198575070a9bf48776cc4e3d7058cd3e6bd3de800589a73fa74c44a","fixed_reason_count":15,"first_lock_return_metadata":3,"getlk_output_pid_labels":["UNCHANGED_INPUT_SENTINEL_NOT_A_HOLDER","DIFFERS_FROM_INPUT_SENTINEL_UNVERIFIED","NO_GUARD_BRANCH_OBSERVED"],"F_GETLK_input_l_pid_sentinel":-2,"returned_or_unchanged_PID_not_holder_or_writer_layer_proof":true,"retry_lock_sleep_logic_unchanged":true,"instrumentation_may_perturb_scheduling":true}' > "$work/proof/sqlite-reader-build-provenance.json"
    groupadd --gid 979 ferriprintqa
    useradd --uid 994 --gid 979 --no-create-home --shell /usr/sbin/nologin ferriprintqa
    install -d -o root -g 979 -m 0750 "$root"
    install -d -o 994 -g 979 -m 0700 "$state"
    install -o root -g 979 -m 0550 "$work/build/ferriprint-0.3.0-linux-amd64/bin/ferriprint" "$root/ferriprint"
    verify "$frontend_sha" "$root/ferriprint"
    install -o root -g 979 -m 0550 "$work/build/receiver-target/release/windows-metadata-only-receiver" "$root/receiver"
    install -o root -g 979 -m 0440 "$work/build/ferriprint/tests/fixtures/native.ppd" "$root/public-native.ppd"
    install -o root -g 979 -m 0640 "$work/windows-public-choices.toml" "$root/choices.toml"
    printf '%s\n' \
      'listen = "127.0.0.1:18634"' \
      'public_url = "ipp://127.0.0.1:18634/"' \
      'state_dir = "/var/lib/ferriprint-windows-local-qa"' \
      'allowed_clients = ["127.0.0.1/32"]' \
      '[access]' \
      'anonymous_trusted_lan = true' \
      'anonymous_owner = "anonymous"' \
      'anonymous_clients = ["127.0.0.1/32"]' \
      '[[cups_sources]]' \
      'id = "isolated"' \
      'server_uri = "ipp://127.0.0.1:18631/"' \
      'username = "ferriprint"' > "$root/ferriprint.toml"
    chown root:979 "$root/ferriprint.toml"
    chmod 0640 "$root/ferriprint.toml"
    # No discovery, telemetry, notifications, vendor-driver installation, CUPS or physical backend.
    cp "$work/build/ferriprint/LICENSE" "$work/build/ferriprint/NOTICE" "$work/build/ferriprint/THIRD_PARTY_NOTICES.md" "$work/proof/"
    cp "$work/build/ferriprint/install/RUST-STDLIB-NOTICES.html" "$work/proof/"
    cp "$work/build/SHA256SUMS" "$work/proof/published-SHA256SUMS"
    sha256sum "$root/ferriprint" "$root/receiver" "$root/public-native.ppd" "$root/choices.toml" "$work/windows-metadata-only-receiver.rs" "$work/reader-build/Cargo.toml" "$work/reader-build/Cargo.lock" "$work/sqlite-wal-retry-counters.patch" "$sqlite_base/sqlite3/sqlite3.c" "$sqlite_derivative/sqlite3/sqlite3.c" "$work/build/libsqlite3-sys-0.35.0.crate" "$work/build/ferriprint/Cargo.lock" "$work/build/source.tar.gz" "$work/build/frontend.tar.gz" "$work/proof/"* > "$work/proof/source-and-binary-sha256.txt"
    dpkg-query -W ca-certificates curl gcc libc6-dev pkg-config xz-utils strace patch > "$work/proof/disposable-distro-package-versions.txt"
    ;;
  start-receiver)
    nohup /sbin/runuser -u ferriprintqa -- "$root/receiver" "$root/public-native.ppd" > "$work/receiver.log" 2>&1 < /dev/null &
    ;;
  install-and-start)
    /sbin/runuser -u ferriprintqa -- "$root/ferriprint" --config "$root/ferriprint.toml" setup install --source isolated --queue receiver --printer windows-local-qa --display-name 'Ferriprint Windows LOCAL NONPRINTING QA' --service-name 'Ferriprint Windows LOCAL NONPRINTING QA' --location 'Disposable hosted VM loopback; no physical device' --choices "$root/choices.toml" --confirm
    sha256sum "$state/profiles/windows-local-qa.toml" > "$work/proof/actual-managed-profile-sha256.txt"
    nohup /sbin/runuser -u ferriprintqa -- "$root/ferriprint" --config "$root/ferriprint.toml" serve > "$work/frontend.log" 2>&1 < /dev/null &
    ;;
  runtime-proof)
    receiver_pid=$(owned_pid "$root/receiver")
    frontend_pid=$(owned_pid "$root/ferriprint")
    test "$(sha256sum "/proc/$frontend_pid/exe" | cut -d' ' -f1)" = "$frontend_sha"
    test "$(sha256sum "/proc/$receiver_pid/exe" | cut -d' ' -f1)" = "$(sha256sum "$root/receiver" | cut -d' ' -f1)"
    sha256sum "/proc/$frontend_pid/exe" "/proc/$receiver_pid/exe"
    stat -c '%u:%g %a %n' "$state" "$state/access.toml" "$state/printers.toml" "$state/profiles/windows-local-qa.toml" "$root/ferriprint.toml"
    test "$(stat -c '%u:%g:%a' "$state")" = '994:979:700'
    for authority in "$state/access.toml" "$state/printers.toml" "$state/profiles/windows-local-qa.toml"; do
      test "$(stat -c '%u:%g:%a' "$authority")" = '994:979:600'
    done
    test "$(stat -c '%u:%g:%a' "$root/ferriprint.toml")" = '0:979:640'
    for executable in "$root/ferriprint" "$root/receiver"; do
      test "$(stat -c '%u:%g:%a' "$executable")" = '0:979:550'
    done
    ;;
  observer-quiesce)
    test "$#" -eq 3
    case "$3" in ''|*[!0-9]*) exit 64 ;; esac
    "$root/receiver" --observe-quiesce "$work" "$3"
    ;;
  observer-arm)
    test ! -e "$work/proof/native-observer.json"
    test ! -e "$work/native-observer-command"
    test -f "$work/proof/native-observer-startup.json"
    test -f /usr/bin/setsid && test -x /usr/bin/setsid
    /usr/bin/setsid nohup "$root/receiver" --observe "$work" > /dev/null 2>&1 < /dev/null &
    "$root/receiver" --observe-control "$work" armed
    ;;
  observer-native|observer-stop)
    "$root/receiver" --observe-control "$work" "${mode#observer-}"
    ;;
  *) exit 64 ;;
esac
