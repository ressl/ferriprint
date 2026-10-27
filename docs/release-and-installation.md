# Versioned Linux installation and release provenance

Ferriprint packages contain a self-contained Rust executable, not a vendor print
engine. A working native CUPS queue, its separately installed driver/PPD, trusted
network/TLS policy and operator-owned configuration remain prerequisites. The
native queue may be on the same machine or an explicitly configured upstream.
Ferriprint never configures that queue or installs a vendor driver.

See [compatibility](compatibility.md) for **observed** platforms and limitations,
[the six-point acceptance plan](six-point-implementation.md) for release gates and
[the isolated deployment guide](linux-pilot.md) for operational safeguards.

## Assets and verification

Each public version release at
<https://github.com/ressl/ferriprint/releases> provides:

- `ferriprint-VERSION-linux-amd64.tar.gz`: executable, strict systemd unit, manual,
  examples, guides, project/locked-crate notices and verbatim Rust library notices,
  build manifest and locked-dependency SPDX.
- `ferriprint_VERSION-1_amd64.deb`: Debian/Ubuntu package for the same executable.
- `ferriprint-VERSION-source.tar.gz`: the exact project source embedded in that
  executable and served as `/source`, including build/package scripts and notices.
- `ferriprint-VERSION.spdx.json`: SPDX2.3 inventory of the complete locked Cargo
  registry graph, including build/dev/platform-only packages; it is not a false
  assertion that every locked package is linked into the runtime or an inventory
  of compiler/standard-library/OS packages.
- `ferriprint-VERSION-build-manifest.txt` and `SHA256SUMS`: exact commit/tree,
  digest-pinned build image/toolchain, lockfile/binary/source/SBOM/Rust-library
  notice checksums and dynamic system-library requirements.

Download from the intended version page, then verify every asset before execution:

```sh
sha256sum --check SHA256SUMS
```

Checksums detect corruption but do not independently authenticate the publisher.
Use the intended repository/release over trusted HTTPS and inspect its source/tag.
Inputs are pinned; this is **not** a measured byte-reproducibility claim. Project
source is complete for Ferriprint; registry dependency sources are fetched from
Cargo.lock and are not vendored. Proprietary PPDs, credentials and TLS keys are
never part of the assets.

The binary also contains the Rust standard library. Its actual release compiler's
`COPYRIGHT-library.html` is supplied verbatim as `RUST-STDLIB-NOTICES.html` in
installed documentation and `install/RUST-STDLIB-NOTICES.html` in matching source.
This upstream library notice can cover platform-specific components not linked
into this binary. Release metadata generation compares its bytes with the pinned
compiler; when changing that compiler, update the notice from that exact image,
not an unrelated current web version. No proprietary driver/PPD is substituted
for these open-source runtime notices.

## Debian/Ubuntu package

```sh
sudo apt install ./ferriprint_VERSION-1_amd64.deb
```

This resolves only required CA certificates, glibc/libgcc and account-management
dependencies. The Rust IPP client does not need `cups-client` or a local CUPS
daemon package; an existing working local **or remote** CUPS queue/driver is an
operator prerequisite, not something this package creates.
The package installs `/usr/bin/ferriprint`,
`/usr/lib/systemd/system/ferriprint.service`, the manual and documentation. It
creates only a dedicated no-login service account and private configuration/state
directories when absent. Existing state is never recursively reowned or reset.

Minimal distribution images can intentionally filter `/usr/share/doc/*` and man
pages during dpkg installation. Ferriprint honors that operator policy rather
than silently changing it. The standard `/usr/share/doc/ferriprint/copyright`
contains complete project/locked-crate license texts and all Rust-library notice
text; standard image filters retain this pathname. Original notices, manual and
examples remain in the `.deb` payload and matching tar/source archives even when
filtered from that particular installation. The raw upstream HTML stays
verbatim; the combined copyright file renders its text for readable offline
license access.

**Installation never enables or starts Ferriprint**, starts a print job, imports a
certificate/credential, changes a CUPS queue, or restarts CUPS/Avahi. The absence of
an active sample configuration is deliberate: using the example blindly could
expose a service or point at the wrong backend.

Upgrades likewise do not enable, start or restart the frontend. An already running
process continues using the previous executable until an explicitly controlled,
job-gated restart. Do not interpret a successful package upgrade as activation of
the new version; inspect the running release identity and follow the frontend-only
cutover procedure.

After configuring and offline validation, explicitly review and start the service:

```sh
sudo -u ferriprint ferriprint --config /etc/ferriprint/ferriprint.toml check
sudo systemctl start ferriprint.service
# Enable only after the intended endpoint/authentication has been accepted:
sudo systemctl enable ferriprint.service
```

Before package removal, check all durable/upstream jobs and gracefully stop only
Ferriprint. Removal refuses to stop an active frontend automatically. It retains
operator configuration, credentials, the service account and durable state,
including after purge; delete them only as a separate reviewed operator action.
No package script modifies a native queue or stops an unrelated service.

## Tar archive

Extract into a new root-owned immutable release directory rather than overwriting
a running binary. Create the dedicated no-login `ferriprint` account first if it
does not already exist, retain any existing static UID/GID/state ownership, then:

```sh
sudo install -d -m 0755 /opt/ferriprint/releases/VERSION
sudo tar -xzf ferriprint-VERSION-linux-amd64.tar.gz --strip-components=1 -C /opt/ferriprint/releases/VERSION
sudo ln -s /opt/ferriprint/releases/VERSION/bin/ferriprint /usr/bin/ferriprint
sudo install -m 0644 /opt/ferriprint/releases/VERSION/lib/systemd/system/ferriprint.service /etc/systemd/system/ferriprint.service
sudo install -m 0644 /opt/ferriprint/releases/VERSION/share/man/man1/ferriprint.1 /usr/share/man/man1/ferriprint.1
```

Create destination directories if absent and copy the supplied documentation/
examples to `/usr/share/doc/ferriprint`. Do not mix package and tar installations
or overwrite an existing binary/symlink without the frontend-only upgrade gate.
The public synthetic PPD included with the examples is only for offline acceptance,
never a driver for a physical queue. Keep the real private PPD and live credentials
outside the public immutable release tree. Nothing above enables/starts a service.

The generic unit runs as static `ferriprint:ferriprint` with no capabilities,
strict read-only system/home protection, private devices/tmp, UMask0077 and private
`StateDirectory=ferriprint`. Its only persistent writable location is
`/var/lib/ferriprint`. Network restrictions still require application ACLs and a
narrow host firewall policy; the unit does not install either automatically.

The unit obtains `server.crt` and `server.key` through systemd `LoadCredential`
from `/etc/ferriprint/tls/fullchain.pem` and `privkey.pem`. Prefer root-managed
symlinks or a host-owned LoadCredential drop-in to an existing trusted lineage;
never loosen the lineage's permissions or copy a key to the workstation. The
configuration TLS paths refer to the private runtime credential copies:

```toml
[tls]
certificate = "/run/credentials/ferriprint.service/server.crt"
private_key = "/run/credentials/ferriprint.service/server.key"
```

A host using `/opt/ferriprint/current/ferriprint` instead of the package path must
explicitly reset `ExecStart=` in a reviewed drop-in. This keeps the generic unit
portable and makes immutable-release selection a host policy, not a second hidden
binary installation convention.

## Building and packaging the exact revision

Use the digest-pinned official Rust image recorded in the version's manifest:

```sh
scripts/build-linux-release.sh FULL_COMMIT rust:VERSION-bookworm@sha256:DIGEST linux/amd64 /private/empty-build
python3 scripts/package-linux-release.py FULL_COMMIT /private/empty-build /private/empty-assets
```

The build script uses `git archive`, a disposable capability-dropped container,
the invoking UID/GID, locked dependencies and no writable source mount. It checks
notices against the fetched packages, emits SPDX and extracts the actual embedded
source without executing the binary. `verify-source.py` then compares **every**
archive member and its bytes with the exact public git tree, rejecting symlinks,
extra/missing members and mismatches. The packaging step repeats source proof and
refuses overwriting versioned assets.

`exercise-linux-package.sh` verifies the complete asset checksums and
installs/reconfigures/removes/purges the Debian package, then performs the
documented immutable-prefix tar **installation**, symlink/unit/man/docs installation
and service-user offline example `check`, only in disposable digest-pinned
Debian/Ubuntu containers. Tar installation reuses the account intentionally
retained after package removal; this is not a booted systemd test. Actual
pre-release exercises passed on Debian12 and Ubuntu26.04.1 amd64.

It validates SPDX2.3 against the pinned official schema and exact Cargo.lock graph,
mounts only the asset directory read-only, blocks dependency daemon auto-start,
and checks a nonempty **container-only** CUPS client-configuration sentinel remains
byte-identical. No daemon or real queue is needed for that preservation invariant.
It verifies actual UID/GID/mode plus durable/operator sentinel retention. Narrow
CHOWN/SETUID/SETGID/DAC_OVERRIDE/FOWNER capabilities exist only inside that disposable
package-test container; release builds remain capability-dropped. No host CUPS
path/device, Docker socket or live systemd runtime is mounted. `python3-jsonschema`
is a QA dependency there, never a Ferriprint runtime dependency. CI publication
depends on these exercises and normal Rust quality gates.
It additionally compares packaged raw Rust notices with matching source and the
mandatory installed copyright file with its actual package bytes, respecting
documentation filters. The tar installation directly installs and compares the
full notices, man and examples rather than assuming dpkg installed filtered docs.

## Public history and canonical source

Public source and contributions use
<https://github.com/ressl/ferriprint>; the existing GitLab project remains private
and retains its historical branches. The first public release uses a reviewed
sanitized **new root commit**, not a rewritten private history. The
`public-development` branch and release tags identify the same clean commit on
both remotes; future public work continues from this branch without merging old
private parents.

The export tree must be byte-identical to the reviewed final private tree. Its
**public** commit is the build/source/tag input; the private predecessor and shared
tree proof are recorded only in private operator records. Never publish those
historical operator records or include Git metadata in the source archive.
See [the repository workflow](repository-workflow.md) for branch/remote rules.
