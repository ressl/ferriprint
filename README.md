# Ferriprint

Ferriprint is a Rust driverless IPP frontend for an **existing native CUPS queue**.
It validates standard numeric media dimensions/margins, quality, paper source,
material, color and sides against an operator-provided PPD and explicit native
mappings, then forwards the document and resolved options over real IPP.

It does not rasterize documents, reimplement vendor engines, install drivers,
configure CUPS queues or bundle proprietary PPDs. Native CUPS and its independently
installed driver remain the renderer. Ferriprint is AGPL-3.0-only and is not
certified by Apple, a printer vendor or an IPP certification program.

**Current release: Ferriprint 0.3.0**, published on 2026-10-28 for native Linux
amd64 and arm64 as tar archives and Debian packages. Its immutable source is
`503a7b90bc38a5d8b6917788ff7e237b2e684216`; the
[five-point implementation contract](docs/five-point-implementation.md) describes
its managed accounts/catalogs and other additions. This is not hardware
certification. Existing0.2 state requires the explicit stopped migration before
running0.3; the immutable0.2 documentation remains historical.

The five additions are complete named recipes, browser account/password/printer
rights, a persisted operational webhook outbox, browser onboarding of existing
trusted CUPS queues, and native Linux amd64/arm64 distribution gates. Actual
acceptance and missing physical-client/printer prerequisites belong in the
[compatibility matrix](docs/compatibility.md), not an inferred support list.

## The immutable0.2 baseline

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

The [compatibility matrix](docs/compatibility.md) records actual observations and
unsupported combinations. Tests alone are not hardware, browser, discovery or
package-installation proof.

## Installation

**Released:** [Ferriprint 0.3.0](https://github.com/ressl/ferriprint/releases/tag/v0.3.0).
The immutable release commit is `503a7b90bc38a5d8b6917788ff7e237b2e684216`.
The actual publisher passed both native architectures and eight installed
package/tar runtimes across Debian12 and Ubuntu26.04.1. All nine downloaded
assets, both ELF identities and the common embedded source were verified.
See the [exact release evidence](docs/release-and-installation.md#observed-release-030)
and [dated native-client observations](docs/compatibility.md#post-release-native-client-observations-2026-10-28).
Later documentation-only descendants do not retag the release, replace its assets
or relabel its embedded source.

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

## Configuration: one authority per concern

[`examples/ferriprint.toml`](examples/ferriprint.toml) is the protected startup
configuration: listener/base URL, TLS/network/anonymous policy, private state,
trusted CUPS servers and optional fixed notification transport. It contains
**no** `access.users` or `[[printers]]` records in0.3.

- `state_dir/access.toml`: private managed accounts, PHCs, enabled/role/printer
  grants, anonymous grants and permanently reserved usernames.
- `state_dir/printers.toml`: authoritative installed Ferriprint printer bindings.
- `state_dir/profiles/<name>.toml`: sole mapping/default/complete-recipe settings,
  illustrated by [`examples/profile.toml`](examples/profile.toml).
- Private acquired PPDs remain under the managed state directory; native CUPS and
  its separately installed vendor driver remain the renderer.
- `state_dir/jobs.sqlite3`: durable jobs and persisted notification transitions/
  outbox. No reset, state-copy shortcut or job replay is part of migration.

Initialize a **new empty** state directory as its intended owner. Passwords come
from a hidden terminal or bounded stdin, never an argument:

```text
ferriprint --config PATH user init --username operator
ferriprint --config PATH check
ferriprint --config PATH serve
```

An empty catalog is legitimate. As the administrator, open the HTTP(S) origin
corresponding to `public_url`, then `/admin/onboarding`: choose a protected trusted
CUPS source and an existing queue, inspect the acquired PPD, confirm native
mappings, set defaults/recipes, review and commit. No per-printer TOML edit, native
queue creation, driver installation or print submission is required.

The offline CLI uses the same source inventory, validators and catalog commit:

```text
ferriprint --config PATH setup inventory --source office
ferriprint --config PATH setup inspect --source office --queue native-queue
ferriprint --config PATH setup install --source office --queue native-queue --printer office --display-name "Office printer" --service-name "Office printer" --location "Office" --choices APPROVED_CHOICES --confirm
```

Choices require `confirmed_native_mappings = true`, `[settings]` and its fully
nested managed settings plus optional `[[previews]]`; a bare profile is not this
wrapper. Unsupported choices/changed queues or PPDs fail rather than inventing a
driverless profile. See [setup and migration](docs/linux-pilot.md).

## Authentication and ownership

Administrators manage accounts at `/admin/users`: create, suspend/reactivate,
change role/password and grant/revoke individual printers. Ordinary users change
their own password at `/account` with current-password verification. Administrators
have explicit global authority; ordinary users require a current enabled account
and the printer's grant for status, recipes and every job operation.

Usernames are permanent: there is no delete/rename/reuse operation, including for
suspended users and imported historical owners. The final enabled administrator
cannot be suspended/demoted. Revisions/CSRF and current-authority rechecks protect
concurrent mutations, uploads and not-yet-dispatched jobs. Already dispatched
upstream work is not falsely claimed canceled by suspending its owner.

The managed access document is owner-only, service-writable private state, not a
root-owned bootstrap user list or plaintext password file. The0.2 PHC descriptor/
effective-ACL checks remain mandatory for explicit credential import and optional
private secret inputs. There is no credential-path exemption or permission
broadening. See [security](SECURITY.md).

For0.2 state, stop only Ferriprint after the active-job gate, preserve its identity
and state, then use the explicit import. A stopped systemd unit may no longer have
its `/run/credentials` mount; provide a protected username-to-persistent-PHC path
map through `--credential-sources` rather than guessing replacement paths:

```text
ferriprint --config OLD_CONFIG migrate-0.2 --output NEW_CONFIG --credential-sources PROTECTED_MAP
```

Migration refuses existing destinations and preserves historical owners, jobs,
backend identity and defaults. Runtime rejects legacy bootstrap users/printers;
it does not retain a second precedence layer. Offline administrator recovery is
`ferriprint --config PATH user recover-admin --username operator`, under the same
service-lock/private-state gate, not a browser backdoor.

Optional `anonymous_trusted_lan` still requires explicit contained
`anonymous_clients` and a stable `anonymous_owner`;0.3 additionally requires an
explicit managed anonymous grant for the printer. This is one shared principal,
not per-person isolation. Network admission is never an account/admin grant.

Inside the configured network/TLS boundary, capability-only
Get-Printer-Attributes and `/source` remain credential-free exceptions, without
job data. All job operations including Validate-Job require current authority;
known authenticated users without a printer grant are denied, not downgraded to
anonymous. Invalid supplied credentials always fail401, even on capability/source
requests or an anonymous-eligible address.

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
- `/admin/printers/<name>/recipes` edits complete named tickets, including numeric
  media/margins, source/material/quality/color/sides, copies/orientation, input
  resolution and scaling. Validate before save; no recipe field silently inherits
  later defaults. Standard `job-presets-supported` exposes complete job templates.
  Clients must copy the selected preset's attributes: there is no invented
  `job-preset-name` request selector or universal native-dialog visibility claim.
  Authorized users can export a resolved recipe from
  `/api/printers/<name>/recipes/<key>`; explicit supported overrides win.
- `/admin/onboarding` acquires queues/PPDs only from configured trusted CUPS
  origins. Saved/HTTP-active is distinct from discovery pending/published/failed;
  no success page claims advertising before actual scoped registration.
- `/admin/notifications` enables per-printer transition alerts, sends a real
  diagnostic to the fixed protected destination, and shows durable delivery/
  retry evidence. Failed/stopped jobs, source outages and reported paper shortage
  produce privacy-minimal events plus observed recoveries. Unknown/stale readings
  do not prove empty paper or recovery. HTTP2xx is acknowledgement, not proof a
  person read a message; receivers must deduplicate stable event IDs.
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
