# Ferriprint 0.3: five-point implementation and acceptance plan

Status: design awaiting the coordinating operator's approval; no feature source changes precede approval. This document records all five requested outcomes, not an optional roadmap. The existing native CUPS queue and its installed driver remain the renderer. Nothing in this plan authorizes a production deployment, a new native queue on an operator's computer, or a physical print.

## Baseline and implementation boundaries

The reviewed 0.2 implementation already has bounded IPP parsing and uploads, explicit PPD mappings and constraint resolution, immutable resolved job records, strict frontend/backend identity separation, private credential-descriptor/ACL validation, same-origin CSRF checks, durable SQLite jobs, atomic managed profile replacement, source delivery, scoped native discovery, and read-only supply observations. These mechanisms are reused rather than duplicated. Current managed profiles live at `state_dir/profiles/NAME.toml`; `jobs.sqlite3` remains the job authority. Input raster DPI is not native engine quality.

The core workstream owns Rust, Cargo, examples, behavior tests, and this plan. The release workstream owns other documentation, CI, packaging/install tooling and notices. Public development stays on the clean public branch; private historical ancestry and operator data are not imported. Test identities, queue endpoints, keys, captures and PPDs remain disposable/private outside public source except for the existing synthetic fixture.

## Authoritative storage, cutover, and recovery

There will be one authority per concern:

| Concern | 0.3 authority | Change boundary |
| --- | --- | --- |
| Listener, TLS, network admission, anonymous-network policy, trusted CUPS servers, notification destination/secret reference | protected startup configuration | operator-controlled, never browser-editable |
| Frontend accounts, enabled state, roles, per-printer grants, permanent reserved usernames | private versioned `state_dir/access.toml` | validated atomic replacement under the authenticator write lock |
| Active Ferriprint printer bindings | private versioned `state_dir/printers.toml` | validated atomic replacement under the catalog write lock |
| Mapping/default/complete recipe semantics | existing private `profiles/NAME.toml` | existing exclusive printer lock, active-job gate, atomic replacement |
| Native capabilities and rendering | existing CUPS queue plus privately acquired/provisioned PPD | read-only metadata retrieval; no queue/driver/default modification |
| Jobs and notification transition/outbox records | existing private `jobs.sqlite3` | SQLite transactions and persisted delivery state |

Runtime configuration does not retain bootstrap users/printers that silently shadow or override managed records. An explicit offline `migrate-0.2` command reads the old configuration and validated PHC descriptors, imports every account and printer/profile binding, reserves all historical job owners (including the anonymous owner), writes owner-only managed files without overwrite, and emits a separate 0.3 protected configuration. The old input is not rewritten or removed. A runtime attempt to use legacy `access.users`/`printers` fails with the explicit migration instruction rather than choosing a precedence rule. Migration preserves job IDs, UUIDs, immutable resolved settings, backend identity, sequence/time origin and spool. It holds the existing service lock and refuses a running instance. Missing/invalid credentials, mismatched existing output, unresolved active backend identities or write failure stop migration; no production migration occurs without separate operator approval.

### Offline credential import when systemd credentials disappear

A stopped systemd service no longer has its `/run/credentials/UNIT/…` mount. Migration must not assume the old `password_hash_file` paths still exist. The migration-only `--credential-sources PATH` option accepts a bounded, effective-private TOML mapping from every legacy username to an explicitly chosen persistent PHC file. Example mapping (synthetic account names):

```toml
[password_hash_files]
admin = "/etc/ferriprint/auth/admin.argon2"
office-user = "/etc/ferriprint/auth/office-user.argon2"
```

With the service stopped and the protected original PHCs available to the migration process, the actual command shape is:

```sh
ferriprint --config /etc/ferriprint/ferriprint-0.2.toml migrate-0.2 \
  --output /etc/ferriprint/ferriprint-0.3.toml \
  --credential-sources /etc/ferriprint/migration-sources.toml
```

The mapping contains paths, not passwords/PHC bytes. Every supplied source is opened with `O_NOFOLLOW` and checked through the existing effective-private regular-file/FD/ACL validator before bounded PHC parsing; the mapping file itself is protected too. If the option is present it must name exactly all legacy configured accounts, with no missing or unknown username. If absent, only the exact legacy configured hash paths are attempted; missing runtime paths fail with the explicit import instruction. There is no guessed sibling path, default fallback, ACL exemption, live-service database copy, or credential bytes in argv/output/logs. A root-run migration preserves the existing service-owned private state directory and job/spool ownership; only newly created managed files are assigned to that inspected state-directory owner/group so the service can read them. It never reowns or resets the database/spool. The old configuration/original PHCs remain available for an explicitly authorized rollback; the temporary source map is never read by the running 0.3 service. After cutover, `access.toml` is the sole credential/ACL authority.

Protected startup configuration has a different writer and ownership boundary from managed accounts/catalog state. Migration preserves the legacy configuration's operator owner, group and read mode, rejects group/other write access, and never transfers that output to the service-owned state UID. A privileged installation therefore keeps root-controlled bootstrap configuration (for example root:service-group `0640`), readable but not writable by the service. A nonroot same-user loopback fixture preserves its operator ownership and does not claim Unix privilege separation. Only new `access.toml`/`printers.toml` receive the existing state owner/group; no database/spool ownership changes occur. Managed inline-PHC syntax/serde failures, offline recovery parsing and migration credential-map parsing return sanitized error classes without the original TOML source/error chain, so malformed input cannot disclose a hash line in CLI logs.

The migration acceptance fixture has deliberately nonexistent legacy `/run/credentials/...` paths, protected persistent source files and preexisting job history. Without the explicit map migration fails without changing jobs; with the map it imports the exact accounts, authenticates their original passwords, preserves IDs/owners/backend identities/sequence/time origin/resolved settings and produces no spool replay. Invalid source permissions, symlinks, missing/extra mapping keys and a held service lock reject before committing output.

A missing, unreadable or corrupt managed printer catalog is a runtime error, never an implicit empty catalog. Only the explicit `user init` command parses protected bootstrap settings before any managed files exist; it creates both an account authority and a versioned empty catalog. A valid empty catalog supports the entire onboarding UI. Lost catalog state requires explicit restoration/recovery before serving and cannot silently reuse an old printer name with retained grants or history.

For a new installation an offline account-initialization command creates the first managed administrator using hidden terminal or bounded stdin password input; the browser is never an unauthenticated administrator bootstrap. A listener with no managed printers is legitimate and exposes the authenticated onboarding wizard. A catalog commit references already durable private PPD/profile files and fully prepared runtime profile/backend objects. Failure before catalog rename leaves no active printer, and request-owned staging files are removed. Rename is the authority cutover: the prepared in-memory HTTP-route/backend snapshots are then swapped without fallible I/O. A subsequent directory-fsync failure is explicitly reported as APPLIED with crash durability unconfirmed, following existing profile behavior. Avahi/network registration is a separate fallible supervised operation, never part of that infallibility claim. Restart loads only the catalog, not orphan staging files. Recovery uses an explicitly verified private backup with the service stopped; never reset/copy/reown/replay a production job database.

## 1. Complete named print recipes

### User and administrator flow

The profile page gains a structured recipe editor: stable ASCII recipe key, display label, media size, exact bordered/borderless variant, source, material, quality, color, sides, copies, orientation, input resolution and scaling. Examples include `A4normal`, `A3borderlessHigh`, and `duplex`; they are not installed unless the actual PPD/mappings support them. A recipe is complete: no hidden inherited field may change its meaning when printer defaults change. Validate shows the full resolved native options and margins before save. Recipes and ordinary default changes obey the existing revision conflict and active-job gate. The 0.3 profile contract keeps `schema_version=1` with an optional `recipes` map; absent/empty recipes preserve all existing defaults and create no guessed recipe. This is forward migration, not backward compatibility: the 0.2 parser denies unknown fields and cannot read a nonempty recipe map. Any authorized old-binary rollback therefore needs the backed-up old profiles as well as the old startup configuration and PHCs; migration itself does not rewrite existing profiles.

The sole resolver expands a selected recipe into a complete job template and then applies explicitly supplied request fields in its browser/CLI preview/export surface. Precedence is: explicit job attributes > selected recipe > printer defaults (defaults only when no recipe selected). An explicit `media` replaces the recipe's whole size/margin selection; an explicit `media-col` merges only specified members over the recipe media collection unless it explicitly changes size, in which case stale recipe dimensions/margins are discarded and the requested size is validated. Existing native compatibility remains unchanged: when a client sends both `media` and `media-col`, the collection wins, including a stale fallback `media` keyword. [PWG 5100.3-2001 section 3.13, printed page 39](https://ftp.pwg.org/pub/pwg/candidates/cs-ippprodprint10-20010212-5100.3.pdf) says clients must not send both but permits a server to reject or choose either; retaining Ferriprint's established collection-wins choice is compliant and avoids an unrelated client regression. Unknown recipes, incomplete recipes, unsupported explicit overrides, internal contradictions within the media collection, and incompatible source/material/duplex combinations reject before spool/submission; there is no silent substitution. Final resolved options are immutable job evidence; native clients copying a standard preset do not provide reliable recipe-name lineage.

### Standard exposure and actual client check

Advertise standard `job-presets-supported` collections containing `preset-name`, `preset-category=site` and complete standard job attributes. [PWG 5100.13-2023 sections 6.5.8, 6.5.8.1, 6.5.8.2 and 12.1.2](https://ftp.pwg.org/pub/pwg/candidates/cs-ippnodriver20-20230301-5100.13.pdf) define these members and require a client to copy the selected preset's Job Template members into its ticket. The [primary IANA IPP Attributes registry](https://www.iana.org/assignments/ipp-registrations/ipp-registrations.xml) registers `job-presets-supported` (`1setOf collection`), `preset-category` (`type2 keyword`) and `preset-name` (`keyword | name(MAX)`) against that specification; the inspected registry has no `job-preset-name` attribute. There is no cited standard request selector, so Ferriprint will not implement or falsely advertise `job-preset-name`. The browser and printer JSON show the same recipes and expanded options, with an exact native CUPS option export for clients lacking automatic preset UI. Protocol exposure is not proof that a native client's dialog implements preset dropdowns. Actual native CUPS/macOS inspection and application are separate acceptance evidence: query the exposed preset collection, select/apply using the real native client interface available (copying/exporting the complete member attributes when necessary), and inspect the nonprinting receiver's effective options and immutable job record. If a client's GUI does not display standard presets, record that observed limitation; the operational native application path must still be exercised, and no claim of a GUI dropdown is made. A custom-attribute echo is not native selection/application evidence.

### Acceptance

An isolated receiver observes all fields of A4 bordered normal, explicitly selected A3 borderless high and duplex recipes. Omitted fields do not drift after default edits; explicit allowed overrides win; an invalid combination/unknown key produces no downstream job. Recipe display survives restart, IPP requested-attribute filtering works, and recipe edits do not mutate historical jobs. At least one real native-client application and the real browser editor are exercised without sending paper.

## 2. Browser account administration and printer rights

### User flow and authority

An administrator can list accounts, create a user/admin with a confirmed password, suspend/reactivate an account, change role/password, and grant/revoke individual Ferriprint printer access. Ordinary users can change their own password with current-password verification. Passwords travel only in bounded same-origin POST bodies over the existing TLS/loopback admission policy and are never rendered back, logged, put in URLs/argv or returned by an API. Argon2id costs, bounded authentication concurrency, zeroizing inputs and private hash file/descriptor checks are retained. Managed storage contains PHCs, never plaintext. APIs return redacted account metadata, not hashes or CSRF secrets.

Usernames are permanent identities: no rename/delete/reuse operation exists. Disabled users remain reserved, including imported historical job owners, preventing a new account from inheriting another person's jobs. Administrators have explicit global administration rights; ordinary users need an enabled account and an individual printer grant for printer status, recipes, jobs, print, validate, create, attach and cancel. Anonymous access remains an explicitly configured shared identity restricted to its network list and gains an explicit managed printer grant set; it cannot administer or mutate browser state. Discovery/capability probes disclose no job data; a known authenticated but unauthorized account is denied rather than falling back to anonymous.

### Security and transitions

Every mutation requires current administrator/owner authority, valid CSRF token, exact Origin and acceptable Fetch Metadata. Role/grant/enabled changes rotate the account revision and CSRF token. Authentication snapshots are rechecked against current state at privileged commit boundaries, after a long upload, and immediately before a queued job enters submission. Suspension, password rotation or revoked printer access prevents an in-flight old credential snapshot from committing a new document; previously accepted but not yet submitted unauthorized jobs become a clear terminal authorization failure. Submission already dispatched before the revocation boundary is not falsely claimed canceled upstream. Browser/API/IPP job access all recheck current account and printer access, not just cached Basic authentication at the middleware edge.

The final enabled administrator cannot be suspended or demoted. Concurrent administrator mutations serialize under the same write lock, so two administrators cannot each remove the other and both succeed. Invalid requests leave old state active. Offline administrator recovery uses the same validators, private storage and service-lock gate, not a runtime backdoor.

### Acceptance

Using the real browser, create an account, grant only printer A, authenticate it, print to A through an isolated receiver, and deny B plus B's jobs/status. Change password and prove the old password fails. Revoke A/suspend during a deliberately slow upload and before queued submission; neither may leak a job downstream. Check cross-owner Get/Send/Cancel/web/API access, forged requesting-user-name, CSRF/Origin rejection, no PHC disclosure, reserved-name rejection, concurrent last-admin protection, restart durability and explicit 0.2 import with unchanged job ownership.

## 3. Active operational notifications

### Real transport and configuration

Implement an actual JSON webhook channel using the existing HTTPS client stack. The protected startup configuration supplies a fixed HTTPS destination (literal loopback HTTP only for an explicit isolated test), an optional private bearer-secret file, request timeout and bounded retry policy. Browser administration can enable/disable events per printer and send a real diagnostic delivery to this already trusted destination; it cannot enter arbitrary URLs, paths or credentials. TLS verification remains enabled, redirects are forbidden, response bodies are bounded, and secrets are read through the existing effective-private descriptor validator. A useful receiver such as an operator's existing notification bridge can route these JSON events into email/chat; a status badge alone does not satisfy this work item.

Payloads contain a stable event UUID/idempotency key, event type, Ferriprint printer identity, optional local numeric job ID, source kind, observation time and conservative state/reason code. They exclude document data, filenames/job titles, account passwords/hashes/tokens, backend secrets, raw arbitrary remote messages and user-controlled URLs. The admin UI shows delivery time/status and sanitized failure classes, not secret response content.

### Persisted transition model

Observe failed/aborted/stopped-uncertain jobs, native queue or configured telemetry source unreachable, and actual reported paper shortage (`media-empty`/equivalent reported reasons), never infer paper absence from missing measurements. Persist each monitored condition and event/outbox insertion transactionally: healthy/unknown -> problem creates one alert; repeated same condition polls/restart do not; problem -> observed healthy creates one recovery; a later new problem creates a new alert. Unknown/stale observations cannot falsely resolve a paper shortage. Existing historical failed jobs are baseline on first enable, not a burst of retroactive alerts. Job failures are terminal/transition identified; one stopped/uncertain transition is not resent on every poll.

The outbox sends outside database/state locks. A successful 2xx marks delivered; transport failure/non-2xx persists attempts and the next bounded-backoff time. Pending events survive restart. Delivery is at least once: a receiver must deduplicate the stable event ID if acknowledgement is lost. Exhausted events remain visibly failed and can be explicitly retried through CSRF-protected administration; no infinite tight loop, no silent discarded failure and no false delivery claim. Disabling stops new events while preserving evidence; diagnostic deliveries also record their actual outcome.

### Acceptance

A real isolated HTTP receiver receives and validates a diagnostic event, one failed-job alert, one queue/source outage alert, one paper-shortage alert and observed recoveries. Duplicate polls and restart do not generate extra transition events. Simulate non-2xx/timeout, restart with a pending outbox, retry to recovery and prove stable event IDs/no payload disclosure. Unknown physical measurements remain unknown, and no physical queue is printed for notification tests.

## 4. Complete browser onboarding for existing CUPS queues

### Protected inventory boundary

Startup configuration contains an explicit list of trusted CUPS source IDs, fixed server base URI, optional CA/auth references and upstream username. These are operator-controlled once per server, not once per printer. The wizard accepts only an enumerated source ID and queue name returned by fresh CUPS-Get-Printers metadata from that source. A queue's returned URI is checked against the configured origin and canonical `/printers/QUEUE` resource; redirects, credentials, queries/fragments, path traversal, foreign origins, malformed names and class/device/backend administration are rejected. Native discovery is an inventory operation, not network scanning. No browser endpoint accepts an arbitrary URI, filesystem path, shell command, device URI, network/TLS change or uploaded PPD.

The selected queue's PPD is retrieved privately from the same fixed CUPS server and exact derived queue resource with bounded transfer/decompression and no redirects. It is parsed as data using the existing parser and never executed. CUPS denial/missing PPD/unavailable source is an actionable failure; it does not fall back to a fabricated driverless profile or arbitrary local file read.

### Wizard steps

1. Choose a configured trusted CUPS server; refresh existing queue inventory with accepting/state/model provenance.
2. Choose an existing queue. Fetch current metadata and PPD; show native pages/margins/options/constraints and actual accepted decoder formats.
3. Set a unique Ferriprint name/display/service label/location, using structured form fields.
4. Map native tray choices, materials at each supported quality, color and duplex options through structured native-choice controls. Every semantic mapping is explicitly confirmed; no inference that a native label `HIGH` or DPI proves photographic quality. Unsupported quality/modes can be omitted rather than fabricated.
5. Choose ordinary defaults and complete named recipes using only validated profile values; preview the full native resolved settings. Optional declared-ready media remains explicitly unobserved, not sensed tray contents.
6. Review the trusted source/queue, endpoint, ACL defaults (new users/anonymous get no access unless explicitly selected), recipe/default previews and nonprinting validation result; confirm save once.
7. Commit durable PPD/profile/catalog state and activate the already prepared HTTP route/backend. Request supervised scoped discovery refresh without restarting or altering global CUPS/Avahi. Display the usable Ferriprint IPP URI, grant controls, and separate discovery state (`disabled`, `pending`, `published`, or `failed`) for the exact catalog/profile revision. The initial success page says saved/HTTP-active, discovery pending; it never says advertised before actual successful Avahi establishment. No native print is submitted by onboarding.

Wizard state is private, bounded, time-limited and tied to the administrator identity/revision; CSRF and stale-review tokens prevent cross-user/stale commits. Reinspect the queue, source binding and reviewed PPD digest before final commit. A changed/removed queue or changed native PPD requires review again. Duplicate names, no usable format intersection, unsupported defaults/recipes, invalid semantics confirmation or another administrator's competing commit leaves no active partial printer.

CLI setup uses the same inventory, profile/recipe validators, preview and commit functions; noninteractive choices remain explicit/confirmed. It is not a second implementation with relaxed validators. Existing offline inspection is still read-only. Before catalog rename, preflight all fallible PPD parsing, profile/recipe resolution, live format/decoder intersection, backend construction, identity uniqueness and portable discovery/TXT validation, and prepare the new runtime map/backend snapshot. Only the in-memory activation after catalog rename is infallible. Actual D-Bus/Avahi/network/collision registration remains fallible and runs in its supervised task using the committed snapshot. A refresh marks the new revision pending and withdraws stale advertising before replacing it; establishment marks that revision published, while a collision/daemon/network failure marks it failed/withdrawn with a sanitized reason and bounded retry. HTTP routes and the durable catalog stay active, administrator status shows the failure, and the coordinator observes task failure rather than silently losing supervision. A discovery error never rolls catalog authority back, fabricates advertisement success, or hides an already committed printer. Crash/restart reloads committed state and attempts actual registration again. No active backend identity is edited or removed through the wizard.

### Acceptance

From an empty managed catalog, use a real browser from trusted-server selection through mappings/defaults/recipes/review/commit with no per-printer TOML edit; inspect advertised attributes and submit only to an isolated nonprinting receiver. Restart and repeat endpoint/recipe/grant checks. Exercise stale review, duplicate name, changed PPD, missing queue, CUPS permission denial, malformed/foreign advertised URI, PPD size/decompression limits, CSRF and non-admin denial; verify no native queue/default/device change. Exercise equivalent explicit CLI choices through the same validators and actual executable.

## 5. Linux ARM64 artifacts and broader compatibility evidence

The release workstream builds the real Linux ARM64 binary and matching tar/Debian packages from the exact clean public source commit using pinned compiler/locked dependencies and complete source/notices/manifest metadata. Native Linux AArch64 execution is available through the inspected OrbStack environment (18 CPUs, 16.8 GB VM memory); use that real native environment, not cross-compilation-only evidence or an emulation label. Install/extract/run and exercise the actual AArch64 binary there. A cross-compile or CI YAML alone is not acceptance. The binary, package payload, `/source`, release manifest and checksums must agree. AMD64 package behavior is preserved and re-exercised once after integrated changes. Package tests use disposable environments, never host package/config changes.

Compatibility is an evidence matrix, not blanket certification: record actual OS/version/architecture, native print client, queue/driver/PPD, transport/TLS, inspected recipes, final resolved settings, input format and nonprinting-vs-physical scope. Broaden actual native clients/printer profiles where access exists, using distinct real installed queues and consenting test environments. Synthetic fixtures and isolated protocol receivers cover malformed/unsupported/transition cases but are labeled synthetic, never called another physical printer. An inaccessible additional physical printer/client is a concrete remaining hardware prerequisite and blocks that specific coverage claim, not completion of reachable implementation. No fake fallback or invented certification satisfies it.

## Integrated execution order and release gates

1. Approve this design and contracts. Both workstreams stay read-only before approval except this requested plan.
2. Implement explicit managed-state migration/init and current-authority checks, then account UI/ACL; preserve strict legacy credential FD validation.
3. Add complete recipes through the existing resolver/profile/IPP/browser paths.
4. Add trusted source inventory/private PPD/wizard and common CLI commit logic; integrate dynamic scheduler/discovery catalog handling.
5. Add persisted notification transitions and real webhook delivery/UI.
6. Add permanent behavior/security/transition tests, not wording/source-text/echo tests. Run the integrated formatter/linter/build/test/security suite once under the main operator's coordination, then actual CLI/browser/native nonprinting acceptance; fix observed failures and repeat only the affected failing check where needed.
7. Complete actual ARM64/AMD64 package exercises and compatibility matrix with truthful hardware limitations. Update existing docs/changelog/security/installer/manpage/examples to the operational contracts; regenerate locked notices if dependency inputs change. All final source/docs changes precede the final exact-source artifact build.
8. The coordinating operator alone authorizes any production mutation after published-artifact source/checksum, private-state backup, identity/ACL/migration review and rollback/readiness guards. No production deployment is implied by source completion.

Completion requires every reachable implementation and every named end-to-end acceptance result above, with exercised evidence separated from inference. Missing real hardware must be named precisely. A feature skeleton, status-only notification, editable arbitrary backend path, guessed recipe, unexecuted ARM64 artifact or protocol-only claim of a client GUI is not completion.
