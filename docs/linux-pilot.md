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
mode0700. The startup configuration belongs in root-owned, service-group-readable
`/etc/ferriprint`, mode0750 with configuration files0640; the service cannot edit
its listener/TLS/network/trusted-source/notification transport. Managed accounts,
catalog, acquired private PPDs and profiles instead belong to the static service
UID under its0700 state directory, with owner-only managed files.
Original TLS, legacy PHC-import and optional webhook-secret inputs remain
effectively private to root/the service UID, including ACLs. Prefer root0600
originals exposed by LoadCredential where runtime access is required. Linux may
present root0440 plus a named service-UID ACL while group::---/other::--- deny
everyone else; group mode bits can be an ACL mask. Existing FD/ACL validation
remains mandatory, with no pathname exemption or broad permission relaxation.

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

# Accounts and printer grants are NOT bootstrap records in0.3.
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

For a new empty state directory, create the first administrator as the intended
static service UID; a hidden terminal or bounded stdin supplies the password:

```text
sudo -u ferriprint ferriprint --config /etc/ferriprint/ferriprint.toml user init --username print-operator
```

Use `/admin/users` for subsequent users, role/password changes, suspension and
explicit per-printer grants; `/account` changes one's own password after verifying
the current password. Administrators have global authority. Usernames, including
suspended/historical owners, are permanently reserved. The last enabled admin
cannot be removed through suspension/demotion. New accounts and anonymous users
do not gain a new printer merely because its network is admitted.

The0.3 `access.toml` authority is private service-writable state containing PHCs,
not root-owned bootstrap `access.users` or one hash path per live account. Do not
manually invent/replace managed account records or loosen secret permissions.
For existing0.2 accounts, use the explicit stopped-service migration below rather
than invoking first-user initialization against populated state.

This frontend ownership policy does not secure separately reachable native CUPS,
physical-printer or alternate print paths. Restrict those independently if
end-to-end user privacy is required; this deployment does not change their
existing firewall/queue policies.

## Initial trusted CUPS setup and complete web wizard

The native queue and its vendor driver must already work. Configure a trusted
server **once** in the protected startup configuration, not once per printer:

```toml
[[cups_sources]]
id = "office"
server_uri = "ipps://cups.example.test:631/"
username = "ferriprint"
# Optional operator-reviewed public CA:
# ca_certificate = "/etc/ferriprint/cups-ca.pem"
```

Remote sources require IPPS. Optional source authentication uses its protected
`auth.username`/`auth.password_env` configuration, never a credential-bearing URI.
Queue/PPD acquisition stays on that exact trusted origin and canonical
`/printers/QUEUE` resource. No network scan, arbitrary browser URL/path, driver
installation, device/backend administration, native default change or print is
part of onboarding.

After first-admin initialization, an empty catalog is valid. Review TLS/network
policy, validate offline as the service UID, then deliberately start only the
frontend. Open the exact HTTPS origin corresponding to `public_url`, authenticate
as administrator and choose `/admin/onboarding`:

1. Select the trusted server and an existing freshly enumerated queue.
2. Acquire its actual PPD; inspect model/state/formats/native choices and margins.
3. Set the unique Ferriprint identity/display/service/location fields.
4. Explicitly confirm tray/material/quality/color/duplex mappings. A native label
   or input DPI alone does not establish material or physical engine quality.
5. Set ordinary defaults and complete named recipes through validated controls.
6. Review the exact source/queue/PPD and resolved native options, then confirm.
7. Observe saved/HTTP-active separately from scoped discovery pending/published/
   failed. Grant individual user/anonymous printer access explicitly.

Drafts are bounded, private and tied to current administrator/revision/CSRF.
Changed queues/PPDs, stale review or a competing catalog commit require review
again. The active catalog references already durable private PPD/profile files;
invalid requests do not leave an active partial printer. A post-rename durability
failure is reported as applied-but-uncertain, never disguised as an uncommitted
failure safe to blindly repeat.

The offline CLI shares those validators and the same catalog commit:

```text
sudo -u ferriprint ferriprint --config /etc/ferriprint/ferriprint.toml setup inventory --source office
sudo -u ferriprint ferriprint --config /etc/ferriprint/ferriprint.toml setup inspect --source office --queue native-queue
sudo -u ferriprint ferriprint --config /etc/ferriprint/ferriprint.toml setup install --source office --queue native-queue --printer office --display-name "Office printer" --service-name "Office printer" --location "Office" --choices /etc/ferriprint/approved-choices.toml --confirm
sudo -u ferriprint ferriprint --config /etc/ferriprint/ferriprint.toml check
```

`--choices` is an explicit reviewed wrapper, not a bare profile:
`confirmed_native_mappings = true`, then `[settings]` with the example profile's
scalar fields and fully nested `[settings.sources]`, material/quality/color/sides/
defaults/recipe tables. Optional `[[previews]]` are standard job-template requests
resolved before commit. Unsupported semantic choices do not get guessed defaults.
The service lock prevents offline setup racing a running frontend. The browser
is the complete interactive flow without per-printer TOML; the offline CLI needs
its reviewed choices file. Neither path writes another startup configuration.

`state_dir/printers.toml` is the catalog authority;
`state_dir/profiles/office.toml` is the sole editable mapping/default/recipe
authority. Acquired PPDs, profiles and access/catalog files remain private and
service-owned. The protected startup file contains neither bootstrap users nor
printer bindings. Offline `check` is not a proof of runtime TLS/secret availability,
live backend reachability or discovery; exercise those separately without paper.

### Explicit0.2 migration and stopped-systemd credential import

Complete the no-active/held/uncertain-jobs gate, stop only Ferriprint and preserve
the exact static UID/GID, private state, old immutable binary/config/profile and
root-only original PHCs. Do not copy/reset/reown the live database or replay a
job. A stopped unit may no longer have `/run/credentials/ferriprint.service`.

When those runtime paths are absent, prepare a bounded owner-only mapping file
with exactly every legacy configured username and explicitly selected persistent
PHC source:

```toml
[password_hash_files]
print-operator = "/etc/ferriprint/auth/print-operator.argon2"
office-user = "/etc/ferriprint/auth/office-user.argon2"
```

Run the explicit import with sufficient existing permission to read those
protected originals; a root-run import retains the state directory's service
identity for newly created managed files:

```text
ferriprint --config /etc/ferriprint/ferriprint-0.2.toml migrate-0.2 --output /etc/ferriprint/ferriprint-0.3.toml --credential-sources /etc/ferriprint/migration-sources.toml
```

The old input is not rewritten. Existing destinations, missing/extra map keys,
symlink/unsafe-permission inputs or a held service lock fail rather than
overwriting or relaxing validation. Without the option only the configured old
paths are attempted; there is no guessed persistent-path fallback.
Imported historical owners are reserved, all accounts/bindings/defaults and job
identity/sequence/time-origin/backend/resolved state are preserved, and no recipe
or additional principal is invented. Review the generated protected startup file's
ownership/readability separately from owner-only PHCs/managed state; validate as
the service UID before selecting it for an explicitly authorized activation.
Do not apply this command to an unsupported original0.1 inline schema.

Offline `user recover-admin --username print-operator` accepts a new protected
stdin/hidden-terminal password under the service lock. It is not an online
recovery endpoint. Browser password rotation invalidates old credentials; use a
separate clean browser context when proving that boundary because Basic auth
can be cached across tabs.

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
   host firewall policy. Create its legitimate personal user through browser
   account administration and grant the intended printer; a new IP is not an
   account/admin grant. Keep anonymous sources and managed printer grants narrow.
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

`GET /admin/printers/office` edits structured defaults and advanced native
mappings; `/admin/printers/office/recipes` edits complete named tickets. Validate
first, inspect resolved options, then save with current revision/CSRF. Stale
edits and active/held/uncertain jobs block profile/recipe saves; old jobs keep
immutable resolved tickets. Explicit recipe overrides use the sole resolver.
Standard `job-presets-supported` exposes complete Job Template collections; clients
must copy those fields. There is no `job-preset-name` request selector and no
promise every native print dialog exposes site presets. The authorized export
`/api/printers/office/recipes/KEY` provides a resolved ticket for controlled clients.

`/admin/users` and `/account` manage current account/password/printer authority.
Suspension or revoked grants prevent new access and not-yet-dispatched work;
already dispatched work is not falsely described as canceled upstream.

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

## Active webhook alerts

The protected startup configuration fixes the destination and optional private
bearer-secret reference:

```toml
[notifications]
webhook_url = "https://notify.example.test/ferriprint"
bearer_secret_file = "/run/credentials/ferriprint.service/notify.token"
timeout_secs = 10
max_attempts = 5
backoff_initial_secs = 5
backoff_max_secs = 3600
```

The secret is optional; if used, expose an existing root0600 original through a
reviewed host-owned LoadCredential entry without loosening its permissions.
Omitting notifications config disables this transport. HTTPS verifies trust and
forbids redirects; literal-loopback HTTP is only for an isolated test receiver.
No browser control accepts another destination, arbitrary credential/path or URL.

At `/admin/notifications`, explicitly enable a printer's events and send a real
diagnostic. Check actual outbox status/attempts/acknowledgement, not just a button
response. Persisted transitions cover failed/stopped jobs, unreachable native or
configured telemetry sources, and reported paper shortage plus observed recovery.
Repeated polls/restart do not create a fresh transition; unknown/stale readings
cannot manufacture a paper recovery. Historical failures baseline on enable.

Delivery is at least once, with stable event IDs for receiver deduplication.
Pending/retry state survives restart; exhausted attempts remain visibly failed
and need explicit CSRF-protected retry.2xx acknowledges receipt, not downstream
email/chat delivery or a human reading it. Payloads exclude document/title/owner/
credential data; do not put arbitrary remote messages into notification logs.

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
