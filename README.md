# Ferriprint

Ferriprint is a Rust driverless IPP frontend for an **existing native CUPS queue**.
It validates standard numeric media dimensions/margins, quality, paper source,
material, color and sides against an operator-provided PPD and explicit native
mappings, then forwards the document and resolved options over real IPP.

It does not rasterize documents, reimplement vendor engines, install drivers,
configure CUPS queues or bundle proprietary PPDs. Native CUPS and its independently
installed driver remain the renderer. Ferriprint is AGPL-3.0-only and is not
certified by Apple, a printer vendor or an IPP certification program.

## Six separately documented capabilities

The [six-point implementation plan](docs/six-point-implementation.md) was written
before implementation and gives each point its own flow, security boundary,
acceptance checklist and migration/rollback gates:

1. **Rust-owned secure Bonjour on Linux:** native Avahi registration of the real
   certificate-bearing FQDN, explicit interface/address-family scope, stable service
   identity and frontend-owned withdrawal. Native macOS is the client acceptance
   target, not a promised macOS server backend. No TLS-name workaround or Python
   runtime companion is the new deployment contract.
2. **Profile/setup assistant:** read actual queue/PPD metadata, choose explicit
   native mappings/defaults, preview resolved options and install one validated
   authoritative managed profile without changing CUPS or printing.
3. **Multiple clients and real user ownership:** explicit network admission,
   separate authenticated principals, consistent owner-filtered IPP/root/API views
   and optional deliberately shared anonymous admission limited to explicit source
   CIDRs. A client-supplied requesting-user-name is never authentication.
4. **Authenticated web administration:** an actual browser profile/default editor,
   validation and durable immediate activation; job inspection and real upstream
   cancellation, with role/owner/CSRF/exact-Origin enforcement. Protected queue,
   backend, PPD, TLS/authentication and filesystem paths are not web-editable.
5. **Source/freshness-aware paper and ink status:** verified IPP observations,
   source and fetch age, honest raw source-change times, unknown physical tray
   contents and a distinction between reported ready media and operator declarations.
6. **Public source and versioned Linux distribution:** clean public development,
   exact embedded AGPL source, pinned-input builds, Linux amd64 tar/Debian package,
   locked-dependency SPDX/notices and disposable installation/removal gates.

The [compatibility matrix](docs/compatibility.md) distinguishes actual observations
from pending release gates and unsupported combinations. Tests alone are not
hardware, browser, discovery or package-installation proof.

## Installation

See [versioned installation and provenance](docs/release-and-installation.md) for
release assets, checksums, tar/`.deb`, strict systemd unit, manual and examples.
Installation does not enable/start/restart Ferriprint, configure a native queue or
print. Upgrading an installed package does not activate a new executable in an
already running process: use a deliberate job-gated frontend restart.

Requirements:

- An existing working CUPS queue and its private driver/PPD, locally or at an
  explicitly configured trusted IPP upstream.
- TLS and explicit client networks for non-loopback listeners; a real
  certificate-valid FQDN for IPPS discovery. Verification is never disabled.
- Existing Avahi/system D-Bus when Linux discovery is enabled. Package installation
  does not silently install/reconfigure a global discovery daemon.
- For source builds, Rust `rust-version` from Cargo.toml; the exercised pinned
  release toolchain is1.99.1. Declared minimum-version checks run separately in CI.

For a source checkout:

```sh
cargo build --locked --release
```

Do not copy the synthetic fixture PPD into a real printer queue. Vendor files and
TLS/password credentials stay outside this repository and all release assets.

## Configuration: one authoritative profile

[`examples/ferriprint.toml`](examples/ferriprint.toml) is the protected bootstrap:
listener/base URL, state, network/auth policy, immutable printer identity/service
name, private PPD path, upstream and optional telemetry. Editable native mappings,
defaults, declared-ready media, formats and resolutions belong **only** in
`state_dir/profiles/<name>.toml`, illustrated by
[`examples/profile.toml`](examples/profile.toml). Unknown keys are rejected; there
is no inline-profile/managed-override precedence or legacy compatibility alias.

To validate the checked-in non-printing example from the source root:

```sh
install -d -m 0700 ./var/demo/profiles
install -m 0600 examples/profile.toml ./var/demo/profiles/demo.toml
ferriprint --config examples/ferriprint.toml check
```

For real setup, use the CLI assistant described in the
[deployment guide](docs/linux-pilot.md#initial-native-queueppd-setup):

```text
ferriprint setup inspect --queue URI --ppd PATH [--ca-certificate PATH]
ferriprint setup install --bootstrap PATH --printer NAME [--choices PATH] --output PATH [--confirm]
```

The interactive flow guides explicit PPD choices. Noninteractive choices include
`confirmed_native_mappings = true`, `[settings]` schema_version1 and fully nested
managed settings plus optional standard job previews; a bare profile is not that
wrapper. Existing outputs are not silently overwritten. Queue/driver/default
installation is outside the assistant's scope.

## Authentication and ownership

`[access]` contains named users with roles `user` or `admin` and protected
`password_hash_file` paths. Generate Argon2id files with:

```text
ferriprint user password-hash --output PATH
```

Plaintext comes from a hidden terminal or bounded protected stdin, never argv.
Hash files must be regular, non-symlink and effectively private to root/the service
UID. Prefer root0600 originals delivered by systemd LoadCredential; Linux may use
root0440 with a named service-UID read ACL and an owning-group entry with no access.
Validation checks effective ACL rights, not a credential-path exemption or a
misleading group-mask bit. See [the security policy](SECURITY.md).
The username/role bootstrap remains root-owned/read-only. Use separate personal
`user` credentials for printing and a distinct `admin` credential for management;
do not distribute the administration login as the everyday printer account.

Optional `anonymous_trusted_lan` requires explicit `anonymous_clients` contained by
`allowed_clients` and a stable `anonymous_owner`. This permits one existing source
to retain shared anonymous printing while other reviewed clients must authenticate
for job/web operations, **including Validate-Job**. Wrong supplied credentials
always fail401, including at an anonymous-allowed IP.
Anonymous users deliberately share visibility/cancellation; this is not per-person
isolation. Preserve the previous owner string when retaining its durable jobs.

Within the existing network/TLS boundary, capability-only IPP
Get-Printer-Attributes and the corresponding-source offer `/source` can be read
without a credential so native clients can discover the authentication policy.
This grants no job access, validation, submission, cancellation or administration.
Invalid supplied credentials still fail401 rather than entering this public
metadata path. Printer authentication attributes describe the **request source**:
credential-required sources see Basic/username+password; explicitly anonymous
sources retain none. Mixed-client discovery must not force authentication onto a
deliberately preserved anonymous client.

These ownership rules protect requests through Ferriprint, not a separately
reachable native CUPS queue, physical device or other print path. Operators who
require end-to-end privacy must separately restrict those direct paths; Ferriprint
does not change existing backend/device firewall or queue policies.

The root HTML and `/api/jobs`/`/api/jobs/{id}` apply the same ownership rules as
IPP Get-Jobs/Get-Job-Attributes. `/admin` and profile mutations always require an
authenticated admin. State-changing web requests require valid CSRF and an exact
HTTP(S) Origin derived from `public_url`; GET never mutates. Ordinary native
browser forms are part of [actual acceptance](docs/nonprinting-acceptance.md), not
merely hand-built HTTP tests.

## Runtime, jobs and web UI

```text
ferriprint --config PATH check
ferriprint --config PATH serve
ferriprint --config PATH abandon JOB_ID --acknowledge-possible-upstream-job
```

- `check` validates bootstrap, managed profiles and PPD mappings offline. It neither
  contacts hardware nor proves runtime TLS/hash/CA availability; startup checks
  those credentials and probes the actual upstream format intersection.
- Clients use `<public_url>ipp/print/<name>`, not CUPS's `/printers/...` resource.
  Numeric dimensions and explicit margins are honored; unsupported combinations
  are rejected rather than silently substituted. Exact native DNS-SD resolver
  URIs, including a terminal hostname root dot, retain canonical target identity.
- Durable jobs and spool survive restart; submission outcomes are reconciled with
  the real upstream ID, never replayed blindly. Cancellation targets that upstream
  ID. Unknown outcomes stay uncertain until investigated. `abandon` deletes a local
  spool payload/releases admission but does not guarantee an upstream job stopped.
- `/admin/printers/<name>` has structured defaults and advanced native mappings.
  Validate previews through the shared PPD resolver; Save atomically persists and
  activates the sole profile. Stale revisions or any active/held/uncertain job
  block a save. Existing jobs retain immutable resolved settings/backend identity.
- `/api/printers` and the browser identify telemetry source and freshness. A native
  queue's cached marker values or media-ready do not prove physical sampling or
  loaded paper. Missing trays/supplies are unknown, not fabricated zeros. Profile
  `declared_ready` is displayed as an unobserved operator declaration.
- `/source` serves the exact corresponding project source embedded at build time,
  including Cargo.lock, docs, tests, package/build inputs and license notices.
  Registry dependencies are not vendored; release SPDX inventories the complete
  lock graph, including build/dev/platform-only packages, without claiming all are
  linked into the executable.

## Verification and safe operations

```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked --all-targets
```

The isolated browser exercise actually validated/saved A3-borderless/High defaults,
submitted two independent receiver-only actor jobs using those native options,
rejected cross-owner/CSRF actions, canceled real upstream IDs and retained owners/
IDs/options across frontend restart without duplicate submissions. Actual native
PPD setup was exercised read-only against an existing Epson ET-16600 CUPS queue.
These observations are not a new physical print or certification claim.

Follow [non-printing acceptance](docs/nonprinting-acceptance.md),
[Linux deployment/migration/rollback](docs/linux-pilot.md),
[release installation](docs/release-and-installation.md) and
[public repository workflow](docs/repository-workflow.md). Do not restart native
CUPS/global Avahi/Docker/containerd, reset job state, widen unrelated network
policy, alter existing Mac queues/defaults or submit a physical page as a routine
frontend verification technique.

## Project and license

- Public development: <https://github.com/ressl/ferriprint> (`public-development`).
- [Changelog](CHANGELOG.md), [security policy](SECURITY.md),
  [contributing](CONTRIBUTING.md), [third-party notices](THIRD_PARTY_NOTICES.md).
- Copyright (C)2026 Robert Ressl. GNU Affero General Public License, version3 only;
  see [LICENSE](LICENSE) and [NOTICE](NOTICE).
