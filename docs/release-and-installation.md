# Versioned Linux installation and release provenance

Ferriprint packages contain a self-contained Rust executable, not a vendor print
engine. A working native CUPS queue, its separately installed driver/PPD, trusted
network/TLS policy and operator-owned configuration remain prerequisites. The
native queue may be on the same machine or an explicitly configured upstream.
Ferriprint never configures that queue or installs a vendor driver.

See [compatibility](compatibility.md) for **observed** platforms and limitations,
[the0.3 contract](five-point-implementation.md) for current release gates and
[the isolated deployment guide](linux-pilot.md) for operational safeguards.
The latest published version remains0.2.0; the0.3 architecture-aware asset
contract below is not a statement that an ARM64 release has already been published.

## Assets and verification

The sole GitHub version-tag publisher at
<https://github.com/ressl/ferriprint/releases> targets these0.3 assets only after
both native architecture gates:

- `ferriprint-VERSION-linux-amd64.tar.gz` and
  `ferriprint-VERSION-linux-arm64.tar.gz`: architecture-matched executable,
  strict systemd unit/manual/examples/guides, project/locked-crate notices,
  verbatim Rust library notices, build manifest and locked-dependency SPDX.
- `ferriprint_VERSION-1_amd64.deb` and `ferriprint_VERSION-1_arm64.deb`:
  Debian/Ubuntu packages for those same actual ELF executables.
- `ferriprint-VERSION-source.tar.gz`: one byte-identical common source archive,
  embedded in **both** executables and served as `/source`, with every tracked
  reviewed source file and no extra operator material.
- `ferriprint-VERSION.spdx.json`: one byte-identical common SPDX2.3 inventory of
  the complete locked Cargo registry graph, including build/dev/platform-only
  packages; not a false claim of runtime linkage or compiler/OS inventory.
- `ferriprint-VERSION-linux-ARCH-build-manifest.txt` for each architecture:
  exact commit/tree, immutable platform-specific compiler image, native
  daemon/kernel/rustc/ELF proof, toolchain, lockfile/binary/source/SPDX/Rust-notice
  checksums and actual dynamic system-library requirements.
- One merged `SHA256SUMS` covers the eight other files, not itself. Common source
  or SPDX differences block merging/publication rather than silently selecting
  one architecture's bytes.

The QA-only same-source native `receiver` example remains outside public release
assets/packages and their checksum list. It is not a product runtime companion.

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

## Observed release 0.2.0

- [Immutable release and downloads](https://github.com/ressl/ferriprint/releases/tag/v0.2.0).
- [Actual successful gated publisher](https://github.com/ressl/ferriprint/actions/runs/40594946394).
- Source/tag commit `24ccd15668502e24854cdaea87b4500c8e767554`, tree
  `f5d4d648e1d35d58e5e4817812b1528fc08278e9`.
- Actual downloaded tar ELF SHA256:
  `0d49ea440e1734f2e1a27fc6435c9495fae07ee7a2f2c20f5b6308648cd7c279`.
- Actual released source was extracted from that ELF: all56 tracked members
  matched the tagged commit byte-for-byte, and the archive equaled the separately
  downloaded source asset. The deployed `/source` matched the same archive.

| Actual downloaded asset | SHA256 |
| --- | --- |
| `ferriprint-0.2.0-build-manifest.txt` | `2b15755cfb69387b9f0c097d47c9576ab8809b83af7a182f0fe553bc406bf3d9` |
| `ferriprint-0.2.0-linux-amd64.tar.gz` | `74068492c040765708f64fce61cb8b5889f1a2820bb8a218d099600d73e6d0ab` |
| `ferriprint-0.2.0-source.tar.gz` | `f0fd4336a3860d168f7b2e2f89fb5a2d625deecca83dc8bd611aa3518c9f6779` |
| `ferriprint-0.2.0.spdx.json` | `645174cbadd381425368fa18d1717f79801a237cfcd39add45b6f95c49cfa3ab` |
| `ferriprint_0.2.0-1_amd64.deb` | `33ffe6e167860e6c343a5e859e700f6d329a81d6e34ef476190e75d2301827d4` |
| `SHA256SUMS` | `b57fb8f7c739c8b311381a1fd7ee2261292a0698526417f7f309da8eb2d0af2f` |

Every downloaded asset also matched GitHub's uploaded digest. `SHA256SUMS`
contains the five other files, not itself. The manifest records actual
Rust1.99.1/compiler-matched library notices and complete281-package locked SPDX;
Debian12 and Ubuntu26.04.1 installation/removal plus the documented tar path
passed in the publishing workflow. Package/manifest bytes differed from local
pre-release builds; no whole-build byte-reproducibility assertion is made.
Later documentation-only commits may add observations, but never retag0.2.0,
replace its assets or relabel the matching running source as a newer commit.


## Debian/Ubuntu package

```sh
ARCH="$(dpkg --print-architecture)" # amd64 or arm64; use the matching published asset
sudo apt install "./ferriprint_VERSION-1_${ARCH}.deb"
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
# New empty state only; existing0.2 state requires the explicit stopped migration:
sudo -u ferriprint ferriprint --config /etc/ferriprint/ferriprint.toml user init --username operator
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

Extract the matching architecture into a new root-owned immutable release
directory rather than overwriting a running binary. Create the dedicated
no-login `ferriprint` account first if absent, preserving an existing static
UID/GID/state owner, then:

```sh
ARCH=arm64 # choose amd64 for x86_64; the ELF, host and asset must agree
sudo install -d -m 0755 /opt/ferriprint/releases/VERSION
sudo tar -xzf "ferriprint-VERSION-linux-${ARCH}.tar.gz" --strip-components=1 -C /opt/ferriprint/releases/VERSION
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

Use the immutable **platform-specific** official Rust1.99.1 manifests:

| Native target | Compiler image digest |
|---|---|
| Linux amd64 / x86_64 | `sha256:8bf7188ae46ad73b801c8d405eedc13ce63a7852b562885743a56890723549a8` |
| Linux arm64 / AArch64 | `sha256:32f7944ed00949c44d3944a0a852b3e5ea862a610a38b43ac92a33aa87256b75` |

The former amd64 pin is a single architecture manifest, not an ARM64 image.
Registry inspection of the Rust1.99.1 multi-platform index
`sha256:48b7f435b8fc6378ce43b44f15e3a47fd8919b2825f835c39bd4bd3da50af5b8`
identified both actual manifests. Release builds use the immutable per-platform
values, not a mutable tag or an assumed platform interpretation.

Example for an actual native AArch64 Docker daemon:

```sh
ARCH=arm64
RUST_IMAGE=rust:1.99.1-bookworm@sha256:32f7944ed00949c44d3944a0a852b3e5ea862a610a38b43ac92a33aa87256b75
scripts/build-linux-release.sh FULL_COMMIT "$RUST_IMAGE" "linux/$ARCH" /private/build-arm64
scripts/package-linux-release.sh FULL_COMMIT "$RUST_IMAGE" "linux/$ARCH" /private/build-arm64 /private/assets-arm64
scripts/exercise-linux-package.sh debian:bookworm-slim@sha256:7ff6ba31527b9e336162fa061746c366c4b2032f4d2cff2078bea43f78efe3f8 "linux/$ARCH" /private/assets-arm64 /private/build-arm64
scripts/exercise-linux-package.sh ubuntu:26.04@sha256:19fcd1bf268f02b60aca3bd30bd8a6d9a11759a430e32921833e65f16d87348a "linux/$ARCH" /private/assets-arm64 /private/build-arm64
```

Run the corresponding amd64 commands on a separate native x86_64 Docker daemon,
using its pinned compiler and distinct empty build/asset directories. Every
script rejects a mismatched daemon platform; build/installed-runtime gates also
check native uname/dpkg/rustc host and actual ELF machine62 (x86_64) or183
(AArch64), then execute the real binary. QEMU/cross-compilation alone is not
called native acceptance.

The builder uses `git archive`, a disposable capability-dropped container,
invoking UID/GID, locked dependencies and no writable source mount. It checks
locked notices against actual fetched packages, emits SPDX and extracts the
embedded source. `verify-source.py` compares **every** archive member/byte with
the exact public git tree, rejecting links, duplicates, extras/missing files and
mismatches. Packaging repeats the full source proof and derives package names/
Architecture from ELF rather than relabeling an amd64 binary.

The explicit packaging wrapper runs the existing Python/dpkg packager inside the
**same** native pinned compiler image, including when invoked on macOS without
host `dpkg-deb`. Source/build mounts are read-only, output is the only writable
host mount, networking/capabilities are disabled and the caller UID is retained.
Git trusts only that exact mounted checkout through process-scoped config, not
global host configuration. No host compiler/package installation or Python-made
Debian-container substitute is required.

`exercise-linux-package.sh IMAGE PLATFORM ASSETS EXACT_BUILD` checks checksums,
the matching manifest/QA receiver and SPDX2.3 against the official pinned schema
and exact Cargo.lock graph. In each disposable Debian/Ubuntu container it
installs/reconfigures/removes/purges the package, preserving static identity,
private durable/operator sentinels and a meaningful container-only CUPS client
configuration. Dependency auto-start is blocked. No host CUPS/device/systemd/
Docker socket is mounted; narrow account/ownership capabilities stay inside the
disposable test container.

Both the **actually installed package executable** and documented immutable tar
installation run as the dedicated nonroot service UID in separate private state:
real0.3 password-stdin initialization and trusted-source catalog installation,
offline `check`, actual receiver-backed GPA/Validate/PDF Print, known upstream
ID Get-Job-Attributes, actual Cancel/reconciliation, durable restart and exact
one-job graceful-SIGINT receiver ledger. Served `/source` must equal the release
source asset. The receiver has no physical backend. This is not a booted-systemd
test, proof of a client GUI, or a replacement for feature/browser acceptance.

After both native asset directories are available:

```sh
python3 scripts/merge-linux-release.py /private/assets-amd64 /private/assets-arm64 /private/final-assets
```

The merger requires exact expected asset membership/checksums, identical source/
SPDX bytes and matching commit/tree/version, then creates the one final checksum
set. The GitHub workflow uses native `ubuntu-24.04` and `ubuntu-24.04-arm` runners;
its sole publisher depends on both complete distro/runtime gates. Do not race it
with manual release creation or publish a tag before the coordinating review.
The package exerciser additionally compares packaged raw Rust notices with source and the
mandatory installed copyright file with its actual package bytes, respecting
documentation filters. The tar installation directly installs and compares the
full notices, man and examples rather than assuming dpkg installed filtered docs.
The0.2 observations above remain historical;0.3 gates must use the integrated0.3
interfaces and final exact source, not relabel old executable/package results.

## Public history and canonical source

Public source and contributions use
<https://github.com/ressl/ferriprint>; the existing GitLab project remains private
and retains its historical branches. The public lineage starts at a reviewed
sanitized **new root commit**, not a rewritten private history. The first release
uses an ordinary clean public descendant that corrected the observed container
checkout-ownership CI boundary. `public-development` and immutable version tags
are mirrored on both remotes; future work never merges old private parents.

The export tree must be byte-identical to the reviewed final private tree. Its
**public** commit is the build/source/tag input; the private predecessor and shared
tree proof are recorded only in private operator records. Never publish those
historical operator records or include Git metadata in the source archive.
See [the repository workflow](repository-workflow.md) for branch/remote rules.
