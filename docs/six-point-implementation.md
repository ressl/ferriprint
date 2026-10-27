# Six-point implementation and acceptance plan

This document records the requested scope **before implementation**. All six points
are separate acceptance obligations. An unchecked acceptance item is a release gate,
not an implemented product feature. Evidence must name the exact revision, toolchain,
artifacts and exercised environment; automated tests alone do not prove hardware,
macOS UI, distribution installation or publication behavior.

## Invariants and trust boundaries

- Ferriprint remains an IPP frontend to an existing, working native CUPS queue. It
  does not install vendor drivers, change native queues/defaults, rasterize files or
  claim printer-program certification.
- Preserve TLS verification, the certificate-bearing canonical hostname and the
  exact DNS-SD resolver URI (including a terminal DNS root dot). Do not use a
  `.local` alias with an unrelated certificate or silently accept a new certificate.
- Preserve the current operator-approved listener and client allow-list. Discovery
  visibility is not authorization. Additional test users and clients use an isolated
  non-printing receiver unless an existing operator record authorizes real access.
- Anonymous LAN printing deliberately means one shared `anonymous` principal; a
  client-supplied `requesting-user-name` is never an authenticated identity. The
  existing anonymous printing policy must survive an independent authenticated
  administration bootstrap. Never imply per-person isolation for anonymous jobs.
- Private TLS keys, credentials, runtime configuration, vendor PPDs/drivers and real
  operator/network records do not enter the public source, source archive, packages
  or release logs. Examples use `example.test` and documentation-only addresses.
- No physical Print-Job/Create-Job/Send-Document is part of this rollout. Metadata
  queries and Validate-Job are permitted. Exercise job submission and cancellation
  against a non-printing receiver. User-reported physical printing already worked;
  it is not repeated merely for reassurance.
- Never restart or reconfigure native CUPS, global Avahi, Docker or containerd for a
  frontend release. Preserve the durable job database, sequence and spool. No reset,
  replay, fabricated success, or new job ID is a migration technique.

## 1. Portable Rust Bonjour with the real TLS FQDN

### Flow and design

A Rust-owned discovery backend publishes the actual configured secure IPP endpoint,
its certificate-bearing FQDN, port, canonical resource path and current IPP-derived
TXT metadata. Use Linux Avahi's native service-registration API for the real
unicast FQDN; this implementation targets portable Linux servers. Native macOS
Bonjour/DNS-SD is exercised as a **client**, not a promised server backend.
Scope registration to the explicitly configured interface and address family;
do not inherit every host interface. Publish secure
`_ipps._tcp` and its universal subtype only for a TLS endpoint. Fail on collisions
rather than silently renaming the printer. Registration belongs to the frontend
lifecycle and is withdrawn on stop, failure or registration-connection loss.

The existing external Python publisher is migrated in one clean cutover after the
Rust path is proven: there must never be two active publishers for the same
service. Native service discovery must not become an alternate route around TCP
ACLs or TLS. Preserve the existing service name, stable printer UUID and raw native
resolver URI behavior.

### Security and acceptance

- [x] Linux Avahi publishes the real FQDN on only the approved interface/family;
      TXT agrees with live Get-Printer-Attributes and no plaintext alias appears.
- [x] Native macOS DNS-SD receives Add/Remove, resolves the unchanged target/port/
      resource path, and exact raw resolver URI Get/Validate succeeds with TLS trust.
- [x] Actual macOS Add Printer → Default UI displays the printer and automatic Use
      choice; close without Add/Print or changing the user's default.
- [x] Stop/failure withdraws the record, restart republishes it, collisions fail
      closed, and no global discovery service or native CUPS restart is required.
- [x] The supported platform/backend matrix describes what was built and exercised;
      unsupported or untested combinations are explicit, not certification claims.

Evidence: real Linux Avahi/private D-Bus collision, daemon-loss/re-registration and
goodbye withdrawal; native macOS resolver Add/Remove, trusted raw-root-dot
Get/Validate and Default/Use observed on isolated pre-release endpoints. Final
production ACL/native-queue hashes remain a separate deployment gate below.

## 2. Usable profile and setup assistant

### Flow and design

An operator starts from an existing queue and private PPD. The assistant reads the
real PPD choices and known upstream metadata, presents media/tray/color/duplex and
quality mappings, and writes a complete reviewed configuration/profile. Derive only
what is unambiguous; ask for explicit mappings when PPD semantics do not identify
quality presets. Validate the result through the same authoritative parser and
mapping checks used by the service. Do not modify the PPD, CUPS queue or defaults,
submit a test print, or disguise guessed choices as detected facts.

The assistant must show the resulting listener, URL, TLS/ACL/authentication and
state location before output is committed. Sensitive values come from named
credentials, not pasted into generated public examples. Existing valid profiles
must migrate without changing the resolved native options.

### Security and acceptance

- [x] An operator-style assistant run creates a complete usable profile for an
      actual installed PPD; the offline `check` succeeds without contacting hardware.
- [x] Ambiguous or missing mappings have actionable feedback, and invalid choices
      cannot produce a falsely successful result.
- [x] The generated profile runs against an isolated receiver; representative media,
      source, quality, color and sides resolve to the intended native options.
- [x] No source configuration is overwritten silently and no native queue, vendor
      file, physical job or TLS permission is changed by the assistant.
- [x] The documented command/UI flow, examples and migration instructions match the
      implemented configuration schema exactly.

Evidence: actual guided terminal flow and native installed-PPD inspect/reviewed
install/check with34 numeric variants; the real setup CLI's generated photo
profile was run through a frontend into the receiver and Photo/Fine options were
independently inspected. Invalid choices/no-overwrite and other media/tray/color/
sides mappings are covered by exercised behavioral tests and browser receipts.

## 3. Multiple Macs and real user ownership

### Flow and design

Separate network admission from authentication and ownership. Explicitly allowed
clients may connect; authenticated users have separate stable principals and may
see or operate only their own jobs. Never trust the IPP requesting-user-name as a
credential. Preserve the existing anonymous LAN mode and document its deliberately
shared visibility/cancellation semantics. Multiple independent credentials must not
collapse into the first configured username. Root HTML, IPP queries and web APIs
apply the same ownership rules.

Add a real Mac/client only under documented operator authority. A reviewed trusted
user segment may be admitted after proving mandatory credentials for new sources;
never infer trust from RFC1918 addressing or widen guest/pentest/DMZ networks for
a demonstration. Exercise extra principals on the isolated receiver when there is
no approved second physical Mac, and state that hardware boundary explicitly.

### Security and acceptance

- [x] Two authenticated actors can submit to the isolated receiver and retain their
      own identities across restart; spoofed requesting-user-name changes nothing.
- [x] Cross-owner Get-Jobs/Get-Job-Attributes, cancellation and root/web listing are
      denied or filtered consistently; unauthenticated requests do not leak jobs.
- [x] Each actor can inspect/cancel its own real upstream receiver job; terminal
      state remains durable and restart does not duplicate submissions.
- [x] Existing approved Mac metadata/Validate access and anonymous printing policy
      remain unchanged; any additional real Mac access has explicit prior authority.
- [x] Documentation distinguishes demonstrated multi-user/client behavior from the
      number of real hardware Macs actually available and exercised.

Evidence: two-actor HTTP/IPP/browser submissions, owner-filtered root/API/IPP,
cross-owner denials, receiver-confirmed cancellation and durable restart without
duplicates. Native strict/mixed setup and credential rotation/rollback completed;
temporary queues were removed and defaults/preferences restored. One physical Mac
only; explicit anonymous-source behavior was preserved in the mixed-policy control.
The existing physical endpoint/queue was not reconfigured. Post-cutover production
regression is an additional integration gate, not inferred from the fixture.

## 4. Authenticated profile/job web administration and actual cancellation

### Flow and design

Provide a usable browser interface for existing managed-profile inspection/editing
and job administration. Initial native-queue/PPD setup is a CLI assistant flow;
the browser cannot upload a PPD or alter queue identity, paths or upstream backend.
Administration always requires a distinct authenticated operator
role even where anonymous LAN IPP remains enabled. Store a generated operator
credential using the approved secret store/bootstrap workflow; never expose it in
URLs, source, logs, shell history or rendered HTML. Non-admin users see only their
own jobs. State-changing browser actions require CSRF protection and an explicit
same-origin flow; GET must not mutate state.

Profile changes use authoritative validation and atomic persistence. Report whether
a service reload/restart is needed; do not advertise a profile as active before it
is actually used. A Cancel action must issue the real upstream Cancel-Job for the
recorded upstream ID and reconcile the durable result. Unknown upstream outcomes
must remain honest rather than being relabeled cancelled.

### Security and acceptance

- [x] Actual browser UI authenticates the operator; missing/wrong credentials fail,
      and anonymous IPP policy does not implicitly grant web administration.
- [x] Browser flow validates, saves and activates an existing managed profile's
      mappings/defaults without exposing credentials or modifying CUPS/vendor inputs.
- [x] Real browser job list is owner-filtered; an authorized cancellation reaches
      the isolated receiver's actual upstream job and updates after reconciliation.
- [x] Unauthorized/cross-owner/CSRF cancellation and invalid profile mutation fail
      without state change; repeated terminal cancellation is reported honestly.
- [x] Bootstrap, credential rotation, permissions and rollback are documented and
      exercised without putting plaintext secrets in chat or repository files.

Evidence: actual Chromium native Validate/Save forms, durable active defaults,
owner-filtered user/admin jobs, CSRF/cross-owner403, active-job save409 and real
receiver terminal cancellation; the ordinary-form opaque-Origin defect was fixed
without relaxing Origin/CSRF. Actual systemd LoadCredential privacy and cold
old→new password rotation/rollback were exercised: revoked credentials401,
current credentials200, identical job owner/rows/sequence/time origin and one
independent receiver receipt with no replay.

## 5. Available paper and ink status with source and freshness

### Flow and design

Read device/queue metadata only. Prefer actual IPP marker, supply and tray attributes
when the known endpoint provides them using existing verified TLS trust. If an
operator-configured alternative such as read-only SNMP is needed, use only the
already approved device and credential; do not scan the network or disable TLS
validation. Distinguish physical-device readings from native queue advisories and
static profile capability/ready-media declarations.

Every displayed measurement identifies its source and observation time/age. A
failed refresh retains an explicitly stale last observation or reports unknown;
absence is not zero ink, an empty tray or a healthy printer. Keep native warnings
such as media-empty-report rather than inventing tray contents. Expose no SNMP
community or credential value to the browser.

### Security and acceptance

- [x] A read-only inventory records actual marker/supply/tray attributes from the
      trusted configured IPP source, with unqueried/untrusted direct-device paths
      explicitly unmeasured rather than inventing readings.
- [x] Live UI/API shows all available real readings with units/source/freshness and
      differentiates unknown, unsupported and stale observations.
- [x] Loss of the source produces stale/unknown behavior without fabricated values,
      credential disclosure or print-job side effects.
- [x] Profile-supported/ready media is not presented as a sensor reading, and queue
      warnings are not promoted to invented physical ink/tray percentages.
- [x] Automated parsing/error tests and a real metadata/browser observation agree;
      device limitations are in the compatibility matrix.

Evidence: verified native IPPS returned four actual ink markers, raw marker-change
time and queue-ready/warning metadata; actual browser showed source/age with
physical trays unknown. Stopping the isolated source produced stale/unavailable
state while retaining its last observation. No direct-device trust bypass or
invented SNMP source was needed; the matrix states physical-measurement limits.

## 6. Public GitHub, versioned Linux packages and compatibility matrix

### Flow and design

Keep the private canonical GitLab repository private. Discover the authenticated
GitHub account and inspect any destination before creating/publishing a public
Ferriprint repository; never overwrite unrelated content. Purposefully review the
current tree and reachable history for credentials, private runtime/vendor
artifacts, licensed material and operator-address disclosure before the first
public push. Generalize public operator examples and retain exact deployment
records only in the private managed operator repository.

Build from the exact final source revision with the pinned official Rust Linux
image. Publish a versioned Linux amd64 tar archive and Debian/Ubuntu package with a
self-contained Rust binary, generic strict systemd unit, manual/examples, matching
source archive, checksums, build manifest, dependency SBOM and accurate notices.
CUPS and a working native queue remain prerequisites. Package installation must not
create/configure queues, enable/start the service, import credentials, print or
restart unrelated services. Exercise install/remove in a disposable Linux
container/VM, never the workstation's native CUPS. Publish other architectures
only after actual build and exercise. Pinning inputs is not a byte-reproducibility
claim.

### Security and acceptance

- [ ] Public tree and reachable-history review finds no unreconciled credential,
      proprietary PPD/driver, private key, operator artifact or license blocker.
- [ ] Public GitHub repository, immutable version tag/release and downloadable assets
      are observed; canonical GitLab source and visibility are preserved.
- [ ] Final build manifest names exact commit/toolchain/image; binary embedded
      `/source` members match that commit exactly, with no extra/missing members.
- [x] amd64 tar installation and Debian/Ubuntu package install/remove are exercised
      in disposable Linux environments, with service not auto-enabled/started and
      no native CUPS or printer configuration touched.
- [ ] CI runs meaningful formatting, strict lint, tests, source/package/provenance
      checks and release publication gates; SBOM/notices cover actual locked deps.
- [x] Compatibility matrix distinguishes built/tested OS/architecture, discovery,
      authenticated clients, real printer metadata and unsupported combinations.

Evidence: pre-release amd64 builds,56-member exact embedded/source-offer proof,
actual compiler-matched Rust notices, complete281-package SPDX/schema/lock proof,
Debian12/Ubuntu26.04.1 install/reconfigure/remove/purge and documented tar
installation/service-user checks passed. Public clean-root/tag/downloaded-asset
and actual CI publication observations remain open until performed, not inferred
from private QA outputs.

## Integration, migration and rollback gates

1. Agree the Rust configuration/CLI/API contracts before docs/examples and package
   files declare them. Preserve existing valid job records and native option
   semantics; document any deliberate schema migration and rollback boundary.
2. Capture the live service account from both local passwd/group files and NSS.
   If already static, retain its exact UID/GID/ownership: no identity migration,
   recursive chown or state copy is required. A dynamically allocated name is not
   proof that a static account exists; any genuinely necessary migration requires
   separate reviewed stop/no-active-job/ownership/rollback gates. Never create an
   account over an active DynamicUser identity.
3. Before rollout capture all-record digest, maximum job-ID sequence, spool counts,
   no-active-job proof, native/Mac queue/default state, protected file hashes and
   service invocation IDs. Block rollout if real work is active; finish packaging
   and isolated validation independently.
4. The integration owner reviews the exact final commit, binary/source digest and
   all required tests/package exercises before authorizing the frontend-only
   cutover. Remove obsolete external publisher units/helper/config only when the
   native Rust replacement is ready; never run both publishers.
5. Retain the previous immutable release and private config/state metadata. Rollback
   switches only the frontend release/config/owned identity metadata, never resets
   durable jobs, replays uncertain submissions or edits native CUPS. A new durable
   schema must have a proven backward boundary before promising binary rollback.
6. Final evidence records the actual final revision/version, asset checksums, source
   proof, service/ACL/identity deltas, observed native/browser behavior and zero new
   physical jobs. Do not convert planned checks into completed claims.

## Agreed implementation contracts and final evidence

### Configuration authority

- `[[printers]]` holds immutable name/display name/location, explicit DNS-SD
  `service_name` (maximum63 UTF-8 bytes), private PPD path, upstream
  URI/authentication/CA and optional telemetry source. The browser cannot alter
  these fields or choose filesystem paths.
- One authoritative `state_dir/profiles/<name>.toml` holds each editable profile:
  sources, media types, color modes, sides, defaults, declared ready media,
  document formats and resolutions. There are no inline-map/managed-override
  precedence rules or compatibility aliases. Initial setup creates both static
  bootstrap configuration and the managed profile through the shared validator.
- Profile saves serialize validation, atomic durable replacement, active-profile
  swap and discovery metadata update under a per-printer write lock. Saving is
  rejected while any job for that printer is active; existing resolved job
  snapshots remain immutable.
- `[discovery]` names the explicit Linux interface and `ipv4`, `ipv6` or `both`
  protocol. Rust Avahi registration uses the real public URL FQDN and printer UUID,
  frees its owned registration on shutdown and follows daemon/connection lifecycle.
  macOS server discovery is unsupported/untested in this release.
- `[access]` has explicit users with `username`, `role = "user" | "admin"` and
  `password_hash_file`; only Argon2id hashes are stored in operator files.
  Password hashing reads stdin/TTY, never plaintext command-line arguments.
  Authentication work and credential lengths are bounded. Supplied invalid Basic
  credentials are rejected, never silently downgraded to anonymous.
  The hash loader rejects symlinks/nonregular files and effective access by any
  group, other user or untrusted named ACL principal. Owner/root/service UID only:
  root0600 originals can become systemd root0440 files with a private named
  service-UID read ACL and a non-accessible owning group. The ACL mask is not
  misread as permission for a group, and no credential-path exception is used.
  Offline `check` validates structure, not runtime TLS/hash availability.
- Deliberate shared-LAN printing uses `anonymous_trusted_lan`, stable
  `anonymous_owner` and explicit `anonymous_clients` CIDRs contained by the network
  allow-list. This permits the existing approved Mac to stay anonymous while
  separately approved new clients require real credentials; there are no
  hardcoded-IP exceptions. `/admin` always requires an authenticated admin.
- Within existing source-network/TLS admission, capability-only IPP
  Get-Printer-Attributes and the AGPL `/source` offer may be read without
  credentials. They create no anonymous job principal: all other IPP operations,
  including Validate-Job, and web/job APIs still require verified credentials or
  an explicitly shared anonymous source. Supplied invalid Authorization always
  returns401, including on these exceptions. Authentication attributes reflect
  the request source (Basic/username+password versus explicitly anonymous none);
  mixed discovery must not force a preserved anonymous client to authenticate.
- Telemetry uses the configured verified IPP source with native trust roots and
  optional explicit additive CA. Report source, fetch time/age and available
  upstream marker-change time separately. Profile `declared_ready` is an operator
  assertion, never a physical paper sensor value. No SNMP dependency is required
  when the existing trusted queue supplies the readings.

Final evidence is added from actual exercises. Release notes and the public
compatibility/install guides describe observed behavior rather than treating these
agreed design contracts as already verified.

### CLI and browser interfaces

- `setup inspect --queue URI --ppd PATH [--ca-certificate PATH]` enumerates actual
  native queue metadata and PPD options without queue changes or print submission.
- `setup install --bootstrap PATH --printer NAME [--choices PATH] --output PATH [--confirm]`
  guides explicit mapping/default choices or accepts an already reviewed profile,
  previews native resolutions and atomically installs protected bootstrap/profile
  output without silently overwriting existing files.
- `user password-hash --output PATH` reads a protected TTY or bounded stdin and
  creates a new owner-only Argon2id PHC file; plaintext is never an argv option.
- `GET/HEAD /source` is the corresponding-source exception inside existing
  network/TLS admission; supplied invalid Authorization still returns401.
- Only `POST /ipp/print/{printer}` can carry identity-free capability GPA
  (operation11) after bounded IPP parsing. Job-subresource paths are never guest
  paths. Every other operation, explicitly including Validate-Job(operation4),
  requires a verified or explicitly shared anonymous principal.
  Per-source `auth-info-required` is the keyword set `{username,password}` or
  `{none}`. Strict authenticated-only DNS-SD advertises `air=username,password`;
  mixed anonymous/authenticated discovery omits AIR and uses per-source IPP policy.
- `GET /` and `GET /api/jobs`, `/api/jobs/{id}` expose only the current owner's
  jobs unless an authenticated admin explicitly has full visibility.
- `GET /admin` and `GET/POST /admin/printers/{name}` require admin. The profile
  form supplies `intent=validate|save`, optimistic `revision` and CSRF token;
  changes apply immediately only after validation and durable persistence.
- `POST /api/jobs/{id}/cancel` requires authentication, authorization, CSRF and
  an exact HTTP(S) Origin derived from the public IPP(S) origin. GET never mutates.
- `GET /api/printers` exposes effective capabilities and honest source/freshness
  telemetry without secrets or invented physical paper data.


### Pre-release nonprinting exercise record (2026-10-27)

This records actual candidate exercises, not a claim that production was activated
or that a public tag/assets already exist. Exact released commit, binary/source/
SBOM checksums and toolchain are authoritative in the version's build manifest;
the private operator record separately holds deployment/state-preservation proof.

| Point | Observed end-to-end exercise | Evidence boundary |
|---|---|---|
| 1. Portable Linux Bonjour | Real Avahi 0.8/private D-Bus registration, collision fail-closed, daemon loss with HTTP continuity, re-registration and explicit withdrawal; native macOS SRV/resolve, fresh Remove/Add, Default/automatic Use and trusted raw-root-dot Get/Validate | No global daemon/router/firewall change; isolated nonprinting endpoint; no macOS-server backend or certification claim |
| 2. Setup and profiles | Actual guided terminal install and hidden-terminal credential input; native queue/private PPD inspection and reviewed install with 34 numeric media variants; shared validation/default preview; actual browser native forms Validate then durable Save | Existing managed mappings/defaults only; no arbitrary PPD upload or native queue mutation |
| 3. Principals and multiple clients | Two isolated authenticated actors retained ownership/IDs across restart and could not read/cancel each other's jobs; distinct Linux sources enforced credential policy. Native macOS strict and mixed/AIR-omitted Add produced identical username/password keyword sets, A3.Fullbleed and High; one authenticated native job reached the receiver with its verified owner and actual cancellation | One physical Mac, not two. Native Add temporarily changed default scopes/preferences; captured defaults/three destinations/preferences were restored, with exact pre-Mixed user-default bytes and no fixture Keychain entry. Final production legacy-client preservation is separately gated |
| 4. Web administration and cancel | Actual browser owner filtering, user management403, CSRF/cross-owner403, active-job save409, native form Validate/Save and actual upstream terminal cancellation. Cold credential rotation rejected old/accepted new credentials; rollback reversed that while preserving job/sequence/origin and exactly one receiver receipt | No mocked Origin/referrer behavior or fabricated cancellation; direct native-CUPS access is outside frontend isolation. All native authentication used an isolated receiver, with no saved credentials or physical print |
| 5. Status source/freshness | Verified native IPPS source returned real ink markers and queue-ready metadata; actual browser showed source/fetch age/raw marker-change information and unknown physical trays; stopped isolated source became visibly stale/unavailable while preserving the last observation | Native cached metadata is not sensor-confirmed loaded paper or a measured physical-supply timestamp; negative/unavailable readings are not zero |
| 6. Distribution and provenance | Native Linux fmt/strict all-target/all-feature clippy and 55 tests; actual pinned Rust 1.88.0 library+executable check/build; all-member embedded-source proof; complete 281-package locked inventory/notices plus official SPDX2.3 schema/checksum/purl validation; real Debian12/Ubuntu26.04.1 `.deb` install/reconfigure/remove/purge and documented tar install/service-user check | Disposable containers preserve state/account/config and CUPS sentinel; not booted systemd tests. Public clean-root publication, downloaded released-asset proof and frontend-only production cutover are separately gated, not inferred from local QA assets |

