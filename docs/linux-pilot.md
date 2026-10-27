# Isolated Linux IPPS deployment

This is a side-by-side frontend deployment, not a native CUPS migration. Never
change an existing native queue's device URI, driver/PPD, defaults, sharing,
enabled state or client default to test Ferriprint. A successful IPP response is
not proof of physical page dimensions, loaded paper or print quality. Do not
submit a physical test page without separate operator approval.

Examples below use reserved `example.test` and TEST-NET addresses. They are not
working production endpoints or a network allow-list to copy blindly. Keep exact
host inventory, credentials, artifact digests and operator intent in private
managed records, not this public guide.

## Release and service identity

Use the [versioned installation guide](release-and-installation.md). Build/install
an exact public committed revision and verify the binary, manifest, matching
embedded source and architecture before execution. Pin the official Rust image
by digest; do not install a compiler globally on the print server. Preserve the
previous immutable release and the private durable state for rollback.

`install/ferriprint.service` runs static `ferriprint:ferriprint`, no login shell,
no supplementary privileged groups, no capabilities and strict system/home/device
protections. Its only persistent writable directory is `/var/lib/ferriprint`,
mode0700. Bootstrap configuration and PPD belong in root-owned,
service-group-readable `/etc/ferriprint`, mode0750 with files0640. Password hashes
are different: regular non-symlink files must be **effectively private to root and
the service UID**, including ACLs. Prefer root0600 originals exposed by
LoadCredential. Linux systemd can present root0440 plus a named service-UID read
ACL while group::--- and other::--- deny everyone else; group mode bits can be
an ACL mask, not group access. Startup validates effective rights, never a special
credential pathname or a blanket permission relaxation. The service cannot alter
its bootstrap, backend paths, TLS originals, PPD or installed executable.

Inspect both `getent passwd/group` and local passwd/group files before changing
an account. DynamicUser NSS names are not static users. If the existing service is
already static and ownership is correct, retain its exact UID/GID and do not
reown/copy the state. Any necessary identity change is a separate stopped-service,
no-active-jobs migration with exact ownership backup and rollback; never create a
static name over an active dynamically allocated identity.

## Canonical endpoint, TLS and network admission

Use a free dedicated high port on the intended service address, for example:

```toml
listen = "192.0.2.10:8631"
public_url = "ipps://print.example.test:8631/"
state_dir = "/var/lib/ferriprint"
allowed_clients = ["198.51.100.0/24"]

[tls]
certificate = "/run/credentials/ferriprint.service/server.crt"
private_key = "/run/credentials/ferriprint.service/server.key"
```

The FQDN must resolve normally from clients and have a matching trusted
certificate. `example.test` needs an operator-controlled private CA; it cannot
receive a public WebPKI certificate. Real deployments use their actual
certificate-bearing name. Never disable chain/hostname validation, blindly trust
a newly presented device certificate, add a misleading `.local` TLS alias or
broaden ACME private-key permissions.

The generic service uses systemd LoadCredential from root-managed TLS paths.
A host-owned drop-in may reset `LoadCredential=` and point to an existing ACME
lineage. Configure an independent renewal hook that validates its specific
lineage/hostname/lifetime/key match and `try-restart`s **only** Ferriprint. It must
not start an intentionally stopped frontend or touch native CUPS renewal hooks.

Mirror the reviewed application allow-list in an independent narrow port-specific
host firewall guard. Never flush unrelated nftables tables, add guest/pentest/DMZ
or whole management ranges, or infer trust merely from RFC1918 address space.
A documented trusted user-office network may admit authenticated clients while
retaining exactly one prior anonymous client:

```toml
[access]
anonymous_trusted_lan = true
anonymous_owner = "anonymous"
anonymous_clients = ["198.51.100.20/32"]

[[access.users]]
username = "print-operator"
role = "admin"
password_hash_file = "/run/credentials/ferriprint.service/print-operator.hash"
```

Every other admitted source requires a configured credential for job and web/API
operations, **including Validate-Job**. Capability-only Get-Printer-Attributes and
the AGPL `/source` offer are readable without credentials within the existing
network/TLS boundary; they grant no job access or management rights. Wrong
supplied credentials fail401 even on those read-only exceptions and at an
anonymous-allowed address; they never fall back to anonymous.

The printer's authentication attributes report Basic/username+password for the
credential-required request source and none for an explicitly anonymous source.
Do not force a legacy anonymous client through a global discovery auth hint.
`requesting-user-name` is not authentication. Anonymous clients share one owner
and can see/cancel that shared owner's jobs. Preserve the previous anonymous owner
string when retaining its durable job history. Never claim this mode isolates
people. A newly permitted client does not receive a made-up family account or
automatic admin role.

Generate a unique operator password in the approved secret store. Use
`ferriprint user password-hash --output PATH` with protected stdin or a terminal;
never put plaintext in an argument, URL, committed TOML or shared transcript.
Only the Argon2id hash is installed. Hash-file paths in `[access.users]` should
name the credential mount, for example
`/run/credentials/ferriprint.service/print-operator.hash`. Add a host-owned service
drop-in such as `LoadCredential=print-operator.hash:/etc/ferriprint/auth/print-operator.argon2`
and keep that original root0600 under a root0700 directory. Do not use the0640
configuration-file convention for hashes; it is deliberately rejected. Back up
hash metadata safely, rotate through a fresh file and intentional frontend
reload/restart, and never change native Mac queue credentials/defaults merely for
an administration bootstrap.

This frontend ownership policy does not secure separately reachable native CUPS,
physical-printer or alternate print paths. Restrict those independently if
end-to-end user privacy is required; this deployment does not change their
existing firewall/queue policies.

## Initial native queue/PPD setup

The native queue must already work. Keep its PPD private and use the assistant:

```sh
ferriprint setup inspect \
  --queue 'ipps://print.example.test:631/printers/native-queue' \
  --ppd /etc/ferriprint/native-queue.ppd
```

If that upstream uses a private CA, pass its reviewed public CA with
`--ca-certificate PATH`. Inspection fetches metadata only, enumerates PPD media
sizes/margins/options and never modifies the queue or prints.

Create an operator-reviewed static bootstrap candidate naming the listener, TLS,
access, state and immutable `[[printers]]` identity/PPD/backend. Then install one
profile, interactively or from explicit reviewed choices:

```sh
ferriprint setup install \
  --bootstrap /private/bootstrap-candidate.toml --printer office \
  --output /private/reviewed-ferriprint.toml
```

Without `--choices`, the assistant guides mapping/default choices using actual
PPD options. Ambiguous quality semantics require operator choice; do not infer
that the third option is necessarily High. Review its resolved option previews
for each material/quality. Installation refuses overwriting existing output and
creates a complete managed profile through the same validator as the service.
It never installs a driver, uploads a PPD to the web UI or configures CUPS.

For noninteractive review, pass `--choices /private/approved-choices.toml --confirm`.
That document is not a bare profile: it has `confirmed_native_mappings = true`,
then `[settings]` with `schema_version = 1` and the profile's scalar settings;
mapping/default tables are nested as `[settings.sources]`,
`[settings.media_types.stationery]`, `[settings.color_modes]`, `[settings.sides]`
and `[settings.defaults]`. Optional `[[previews]]` contain standard job-template
requests that are resolved through the same profile constraints and printed before
installation. Without explicit native-mapping confirmation, installation fails.

There is one authoritative editable profile per printer:
`state_dir/profiles/office.toml`. It contains source/media-type/quality/color/sides
mappings, defaults, `declared_ready`, formats and resolutions. The static bootstrap
contains no inline maps or override layer. For the checked-in non-printing example,
copy the separate profile before `check`/`serve`:

```sh
install -d -m 0700 ./var/demo/profiles
install -m 0600 examples/profile.toml ./var/demo/profiles/demo.toml
ferriprint --config examples/ferriprint.toml check
```

For a real service use absolute paths in the reviewed bootstrap candidate. After
the no-active-jobs gate, generate as the service UID so the sole managed profile
is born with the right owner, then promote only the static bootstrap as root.
Do **not** recursively chown existing state/spool/database:

```sh
set -euo pipefail
sudo install -d -m 0700 -o ferriprint -g ferriprint /var/lib/ferriprint/setup-bootstrap
sudo -u ferriprint ferriprint setup install \
  --bootstrap /etc/ferriprint/bootstrap.candidate.toml --printer office \
  --output /var/lib/ferriprint/setup-bootstrap/ferriprint.toml
sudo -u ferriprint ferriprint --config /var/lib/ferriprint/setup-bootstrap/ferriprint.toml check
bootstrap_tmp="$(sudo mktemp /etc/ferriprint/bootstrap.XXXXXXXX)"
trap 'sudo rm -f "$bootstrap_tmp"' EXIT
# Read the service-owned output as the service UID, not as root following a
# potentially substituted source symlink. The destination is root-owned/new.
sudo -u ferriprint cat /var/lib/ferriprint/setup-bootstrap/ferriprint.toml | sudo tee "$bootstrap_tmp" >/dev/null
sudo chown root:ferriprint "$bootstrap_tmp"
sudo chmod 0640 "$bootstrap_tmp"
sudo -u ferriprint ferriprint --config "$bootstrap_tmp" check
sudo mv -T "$bootstrap_tmp" /etc/ferriprint/ferriprint.toml
trap - EXIT
sudo -u ferriprint rm /var/lib/ferriprint/setup-bootstrap/ferriprint.toml
sudo -u ferriprint rmdir /var/lib/ferriprint/setup-bootstrap
```

The managed profile stays service-owned0600 under its private state directory;
the active bootstrap is root:service-group0640 and read-only to the service.
Offline `check` contacts no printer and does not open TLS/hash secrets: actual
runtime credential/ACL loading and startup remain a separate acceptance gate.

### Migration from the original inline profile schema

Stop only Ferriprint after the job/spool gate. Copy the existing mapping/default/
format/resolution fields byte-for-byte into the named managed profile, rename only
`ready` to `declared_ready`, and remove those inline fields from the bootstrap.
Keep immutable queue identity, PPD path, default native choices and durable owner
strings unchanged. Replace the old singleton `[auth]` with explicit `[access]`
users only through a reviewed credential migration. There are no old/new aliases.
Validate offline with the new binary before switching the selected release.
Do not reset SQLite, lose job sequence numbers or replay an uncertain submission.

## Rust-owned secure Bonjour

Configure the existing system Avahi service explicitly:

```toml
[discovery]
interface = "print0"
protocol = "ipv4"
```

Choose the actual approved interface, not every host interface. Ferriprint's Rust
Avahi D-Bus registration publishes secure `_ipps._tcp` plus the universal subtype
with the real canonical FQDN/port, stable printer UUID, resource path
`ipp/print/office` and current effective IPP-derived TXT formats/capabilities.
Do not advertise a plaintext `_ipp`, LPD or alternate TLS hostname. It is compatible
discovery metadata, not an Apple AirPrint certification claim.

Registration is frontend-owned: stop/failure/connection loss withdraws it; valid
restart republishes it, and collision fails closed without inventing a renamed
printer. Existing Avahi may also serve lower-trust interfaces; scoping this entry
is not permission to reconfigure Avahi, add reflectors or change gateway policy.
The main static service user must authenticate to the existing system bus without
relaxing global D-Bus policy.

Migrate an external publisher cleanly. At the approved frontend stop, stop/disable
and remove only the obsolete owned companion unit/helper/config, then start the
Rust-owned publisher with the frontend. Never run both. Retain exact previous
files privately for rollback, not a dormant compatibility path in the product.
There is no reverse dependency that starts a deliberately stopped frontend.
Linux Avahi is the supported server backend; native macOS is a tested **client**,
not a promised macOS server-discovery backend.

For an **existing configured Mac** acceptance check, use live native DNS-SD to
observe Add/Remove and resolve exact FQDN/port/TXT, and inspect System Settings →
Printers & Scanners → Add Printer → **Default**. Select the clearly named frontend
and inspect automatic **Use**, then cancel without Add or Print. Do not duplicate
the current queue or intentionally change its defaults/credentials.
Get-Printer-Attributes and Validate-Job must work with the **unmodified** native
backend resolver URI, including a terminal DNS root dot; testing a normalized
replacement URI is not equivalent proof.

The canonical manual endpoint is
`ipps://print.example.test:8631/ipp/print/office`, not `/printers/...`. In Add
Printer's IP form, Address is only the hostname, protocol is secure IPP, Queue is
the resource path and port is explicit. Prefer the native Default discovery flow
or an independently verified complete DeviceURI; do not select Generic PostScript
just to bypass a connection failure. Creating a duplicate queue is not an
acceptance requirement.

### Onboarding an additional authenticated Mac

1. Admit only an already reviewed trusted client/network in both app and narrow
   host firewall policy. Configure a legitimate personal `user` identity through
   protected bootstrap/LoadCredential first; a new IP is not an account or an
   admin grant. Keep deliberately retained anonymous sources explicit and narrow.
2. Record existing destinations/defaults privately before adding anything. In
   Default discovery select the exact frontend name/target/UUID and its automatic
   driverless Use, not a similarly named physical printer or Generic PostScript.
   Add a new client queue only when intended, never a duplicate of the pilot.
3. **macOS may automatically make a newly added queue the default.** If that was
   not intended, immediately restore the previous effective default and its
   system/per-user scope before any job; verify existing preferences/destinations.
   A test must record an observed temporary change and restoration, not claim
   the OS left everything unchanged.
4. Capability GPA is deliberately readable inside network/TLS admission, so native
   setup can discover Basic authentication before a protected operation. For a
   credential-required source the generated queue must retain
   `auth-info-required=username,password`; a source explicitly allowed anonymous
   remains none. Missing/wrong credentials must still fail Validate/job operations.
5. Supply the **personal printing** credential through the native authentication
   flow, not the web administrator's password, a DeviceURI, shell argv or a shared
   log. If an existing local job is held for authentication, use Print Center's
   Authenticate/Resume for **that same job** rather than submitting another copy.
   Save to an OS credential store only when the operator/user intends it; isolated
   acceptance does not save credentials or change the existing queue's Keychain.
6. Physical-backed deployment checks use Get/Validate/status only. A native local
   submission requires separate explicit approval and a verified **isolated
   nonprinting receiver** target. Remove only a temporary owned queue afterward,
   verify the original destinations/defaults/preferences, and prove owner-filtered
   queries, actual upstream cancellation and no duplicate receiver submission.

## Authenticated browser workflow

Open the HTTPS origin corresponding to the public IPPS URL. Browser Basic auth
uses the configured user; `/admin` always requires an authenticated admin,
even when that Mac can print anonymously. The root and job APIs show only the
principal's jobs unless the principal is an explicit admin.

`GET /admin/printers/office` presents structured default selectors and advanced
managed-profile TOML. Validate first, review the resolved choices, then save. The
form carries the current revision and CSRF token. Saving rejects concurrent stale
edits and any active/held/uncertain printer job, validates all PPD mappings,
atomically persists the sole profile and activates it with refreshed discovery
metadata. It does not require a frontend restart. Queue identity, PPD/backend,
TLS/authentication and filesystem paths are deliberately not web-editable.

The job view shows durable state, owner, resolved options and upstream result.
Authorized Cancel issues the actual upstream Cancel-Job by its recorded ID;
a successful local button response is not sufficient evidence of cancellation.
Observe terminal reconciliation from an isolated non-printing receiver. Wrong
owner, wrong role, absent/foreign Origin or invalid CSRF cannot mutate state.
Unknown upstream outcomes and already-terminal cancellations remain honest.

## Paper/ink status and freshness

`GET /api/printers` and the browser show verified IPP telemetry from the explicitly
configured source. Prefer a known trusted native queue when it already supplies
markers; never disable direct-device TLS merely to obtain a newer reading. Ink
levels, colors, names/types, ready-media and queue warnings are displayed only
when actually returned. Include source and fetch time/age, plus upstream
marker-change time when available: fetching a native queue now does not prove its
cached ink reading was physically sampled now.

Native queue `media-ready` is source-reported, not proof of current physical tray
contents. `declared_ready` in the profile is an operator assertion, not a sensor.
If no physical tray/count attributes exist, say unknown/unsupported. Failed
refreshes become stale/unknown without fabricated zeros or a false healthy state;
preserve queue warnings such as `media-empty-report`. No status request prints,
changes a queue or exposes a community/credential.

## Frontend-only cutover and rollback

Before any approved cutover capture privately:

1. Complete durable-record digest/counts, maximum job ID and sequence, all phases,
   spool file/count/bytes; require no real active/held/uncertain work.
2. Native and Mac queue counts/defaults, protected CUPS/PPD/config hashes, existing
   main/companion/Avahi/CUPS/Docker/containerd invocation IDs and listener/ACL state.
3. Exact reviewed source commit/tree, binary/source/SBOM/package hashes, source
   member proof and predecessor release/config/profile/owned-unit backup.

Block cutover on active work. Never reset/replay jobs or submit physical jobs to
make a rollout appear complete. Only after the integration owner accepts exact
artifacts and UI/API/discovery gates, stop Ferriprint gracefully, perform its
owned configuration/publisher cutover, validate offline and select/start the new
immutable frontend release. Do not restart native CUPS, global Avahi, Docker or
containerd. Recheck no-job deltas and original defaults/permissions afterward.

Rollback selects the compatible previous frontend plus its matching private
bootstrap/profile/publisher wiring after the same gate. Never run two publishers,
reset a database, delete an uncertain record or reassign ownership. A durable
schema change must have a proven backward boundary before promising binary-only
rollback. To disable the side-by-side deployment, stop/disable only its frontend
and owned port guard; preserve state and native printing unchanged.

A rollback to an older singleton-anonymous release must restore the previous
**narrow app and firewall allow-lists before starting it**; never leave a newly
admitted authenticated network in front of old shared-anonymous behavior. After
real multi-user use, do not drop ownership/privacy by exposing an old unfiltered
job view. Use an accepted ownership-preserving release or keep the frontend
stopped pending compatibility review, retaining the same database and IDs.
