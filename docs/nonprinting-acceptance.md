# Non-printing acceptance, including real browser forms

Run this only against an isolated receiver that cannot reach a physical printer.
Names containing “test” are not a safety boundary: inspect the configured upstream
URI and ensure it is the loopback non-printing receiver before any job submission.
Never reuse these submission/cancellation steps on a physical-backed endpoint.

Use a real browser and the exact HTTP(S) origin configured by `public_url`; swapping
`127.0.0.1` for `localhost` changes the origin and invalidates this exercise.
The source contains a non-printing `receiver` example. In0.3, initialize its empty
managed state through the real `user init` command (hidden terminal/bounded stdin
password), then use browser account administration for distinct admin/two-user
identities and explicit printer grants. Configure only the literal-loopback trusted
CUPS source. Do not inject fabricated managed records, put credentials in argv/
URLs, or treat the historical0.2 credential/bootstrap setup as0.3 acceptance.

## Real native form regression

This deliberately exercises actual browser behavior rather than asserting a
Referrer-Policy header string or fabricating an Origin header in a unit test.
It caught a bug that all Rust tests initially missed: the response's former
`no-referrer` policy caused Chromium's ordinary form navigation to send
`Origin: null`, so strict same-origin CSRF correctly rejected a legitimate form.
The fix retains strict Origin/CSRF enforcement and uses a browser-compatible
same-origin referrer policy; accepting `null` is not the fix.

1. Authenticate as the isolated admin. Open `/admin`, then the existing printer's
   profile link. Verify the actual UI contains paper/source/material/quality/color/
   sides selectors, borderless checkbox, advanced native mappings and separate
   **Validate and preview only** / **Save and activate** buttons.
2. With no active/held/uncertain jobs, select A3, borderless and High on the checked-in
   synthetic PPD profile. Click the real **Validate** button. Expect HTTP200 and
   “Profile validated; not saved”, native `PageSize=TA3`, `Quality=Fine`, and four
   zero margins. The persisted profile/revision must still be unchanged.
3. Return to the active profile, choose the same settings and click the real
   **Save** button. Expect303→200, an incremented active revision and “durably saved
   and activated”. Read the sole managed profile and verify those defaults, then
   fetch live printer metadata and verify the active capability/default values.
   Do not count a source-file inspection or a hand-built HTTP POST as this check.
4. Inspect only sanitized network metadata: the browser's actual form POST has the
   exact configured HTTP(S) Origin and a populated CSRF field. Do not log Basic
   headers, tokens or passwords. If the normal form sends an opaque/null Origin,
   keep the security rejection and fix the browser flow; do not whitelist null.
5. Repeat after a frontend restart with a fresh form/CSRF token. Verify defaults
   and job identities persist. Never reuse a process-local stale CSRF token as a
   purported proof of normal form functionality.

Observed on the 0.2.0 isolated acceptance fixture: real Chromium Validate200,
Save303→200, revision0→1, TA3/Fine/zero margins and persisted A3/borderless/High.
The initial legitimate403 was fixed and re-exercised, not hidden or relabeled.

## Ownership, actual cancellation and active-profile gate

1. Submit one harmless document per isolated user to the non-printing receiver.
   Verify independent frontend IDs, principals and actual upstream IDs. Check the
   upstream received the browser-applied native options, not merely frontend JSON.
2. With a genuinely independent user session, root/IPP/API views expose only that
   user's job; `/admin` returns403. Browsers may cache Basic credentials across
   tabs: use distinct browser profiles/contexts or deliberately controlled headers
   and verify the page's displayed principal before interpreting a result. A new
   tab alone is not an independent identity.
3. Exercise real **native HTML form** denials: change only the hidden CSRF value of
   the user's own Cancel form and submit (403); reload a valid form, change its
   action to the other owner's job and submit (403). Neither upstream job changes.
   Page `fetch()` may be blocked by the restrictive CSP; that is not a server-side
   negative-CSRF test and must not be counted as one.
4. Submit the untouched own-job Cancel form. Expect real upstream Cancel-Job for
   that exact upstream ID, then durable terminal **Canceled**, not a relabeled
   local-only record. Independently query the receiver to verify only that job
   changed and the other owner's job is still processing.
5. Try profile Save while the other job is active:409, unchanged revision/profile.
   Gracefully restart only the isolated frontend; the same active upstream ID,
   owner and immutable resolved options must survive without duplicate submission.
6. An explicit admin may cancel the remaining job through the real browser form.
   Independently confirm the second upstream terminal state. A repeated terminal
   cancel must be reported honestly rather than pretending to cancel anew.

Observed isolated IDs were Alice frontend1/upstream700 and Bob2/upstream701.
Alice's invalid-CSRF and cross-owner form POSTs returned403; her own Cancel303
reconciled to terminal7 and independently canceled only upstream700. The other
active job made profile Save409; frontend restart retained both owners/IDs and the
immutable TA3/Fine option snapshots. These are receiver exercises, not two physical
Macs or physical printer output.

## Status honesty and source failure

Inspect an actual browser status page and `/api/printers`, not just a parser test.
Verify source URI/kind, fetch age, separate physical-measurement freshness and raw
marker-change time when present. A known trusted native queue can provide actual
ink markers even when no physical tray sensor exists: display unknown trays and
source-reported media-ready without claiming loaded paper. Profile declared-ready
is a separate unobserved operator declaration.

After all isolated jobs are terminal, stop **only the non-printing telemetry
receiver**, leave the frontend running, and wait past its configured stale limit.
Reload the browser once the limit has elapsed and verify stale/unknown rather than
invented zero ink or a healthy state. Restore or remove only the owned isolated
receiver. Never stop native CUPS or block a real printer in order to demonstrate a
status error.

## 0.3 real wizard, accounts, recipes and notifications

These are additional current-interface gates, not results inferred from the0.2
observations above. Use separate real browser contexts, inspect their displayed
principals and prove downstream effects against the nonprinting receiver.

1. Start with an initialized administrator and **empty** catalog. At
   `/admin/onboarding`, select the configured source/returned queue, acquire its
   synthetic PPD, complete identity/native mappings/defaults/full recipes/review
   and commit using actual controls with no manual per-printer TOML. Verify the
   HTTP route and persisted catalog/PPD/profile; keep saved/HTTP-active distinct
   from actual pending/published/failed discovery. Restart and repeat metadata/
   grant checks. Changed PPD/queue, stale review, duplicate identity, foreign
   resource, permission/size failure, non-admin and CSRF cases must leave no
   active partial binding or native queue/default change.
2. At `/admin/users`, create a normal user with only printer A. Log in separately,
   prove A access and B/job/status denial, rotate the password and prove the old
   password fails. Suspend/revoke during a controlled slow upload and queued
   pre-submission work: no unauthorized new receiver job may appear. Reactivate
   deliberately. Exercise own-password verification, reserved names, concurrent
   last-admin protection and restart durability without exposing PHCs.
3. Through `/admin/printers/NAME/recipes`, validate/save complete A4-normal,
   supported A3-borderless-high and duplex tickets. Inspect all fields/native
   options, standard `job-presets-supported` requested-attribute filtering and
   authorized export. A native client copies the selected standard attributes;
   do not invent a `job-preset-name` selector or fake GUI visibility. Observe
   actual downstream options, explicit-override precedence, rejection with no
   submission, unchanged historical jobs and stable tickets after default edits/
   restart. Profile/recipe saves must still respect active-job/revision gates.
4. Configure only the existing isolated webhook receiver in protected startup
   transport settings. At `/admin/notifications`, enable events and deliver a
   real diagnostic, failed/stopped-job, source-outage and source-reported
   paper-shortage transition plus observed recoveries. Verify actual received
   payloads, stable IDs, privacy fields and durable attempts. Repeated polls/
   restart must not duplicate transitions; unknown/stale observations must not
   resolve paper. Exercise non-2xx/timeout/pending restart/exhaustion/explicit
   retry and receiver deduplication. A queued row/button response is not delivery.
5. Stop the isolated0.2 fixture before migration. Give its old runtime PHC paths
   deliberately nonexistent systemd locations and retain protected persistent
   originals. Without the explicit credential-source map it must fail without
   changing jobs; with the exact map original passwords and historical ownership/
   IDs/sequence/time origin/resolved backend settings survive without replay.
   Test private permissions, symlinks, missing/extra keys and held-lock failures.

The example receiver's graceful SIGINT ledger is authoritative for actual
received documents/options and duplicate detection. Query known upstream IDs
with Get-Job-Attributes and independently confirm cancellation; do not send an
unsupported Get-Jobs operation to the receiver or call two principals two physical
Macs. Native UI/hardware coverage remains separate from browser/protocol evidence.

## Linux discovery and distribution

Use a distinct non-production service name/port/state with the non-printing
receiver for Linux Avahi tests. Observe actual native Mac DNS-SD Add/Remove,
resolved FQDN/port/TXT, collision behavior and exact raw resolver URI Get/Validate.
Do not restart global Avahi/CUPS or add a printer merely to test metadata.

Versioned package exercises run only in disposable digest-pinned Linux containers
as described in [installation](release-and-installation.md). Source member proof,
locked notices and package ownership/removal checks are independent release gates;
passing browser tests does not imply those gates passed.

For both native release architectures, the package exerciser installs/reconfigures/
removes/purges the actual package and performs the documented tar installation.
Each installed executable runs as the dedicated nonroot service UID through
genuine0.3 init/setup/check/serve, receiver GPA/Validate/Print/known-ID cancellation,
restart, served-source equality and exact one-job ledger. QA receiver binaries
stay outside release assets. These gates do not substitute for the real0.3
browser flows above or claim additional physical clients/printers.

## Observed0.3 native CUPS recipe application

This historical pre-publication exercise used candidate
`fe2611a421241ea3501ee84a2537a95e04619dc1`, not the published503/135 source.
It ran in a disposable native Linux amd64 Debian12 container with real
CUPS2.4.2-3+deb12u9 and cups-filters1.28.17-3+deb12u2. No host CUPS socket,
physical backend, published port or host queue was involved. The native daemon,
frontend and existing Rust receiver listened only on literal loopback addresses.
Explicit anonymous admission and the sole managed printer grant were confined
to that disposable loopback options fixture, not a public/authenticated QA policy.
Administrator credentials remained in protected memory/stdin/HTTP headers, never
URIs, argv, plaintext files or CUPS spool credentials.

After genuine0.3 init/setup, the authorized recipe API exported the complete
A3-borderless-high recipe. Its eight actual `cups_options` pairs were passed
unchanged as `lp -o` arguments to a real `lpadmin -m everywhere` CUPS queue; no
synthetic IPP Print payload or expected-map echo stood in for native application.
Actual CUPS queue inventory/inspection and HTTP PPD export also succeeded.
One native job reached frontend1/upstream700. The Rust receiver independently
observed `PageSize=TA3`, `Quality=Fine`, `InputSlot=Rear`, `MediaType=Photo`,
`ColorModel=RGB`, `Duplex=None`, copies2,300x300dpi and1106 actual PDF bytes.

Do **not** report raw ticket equality: CUPS normalized exported orientation4
to3 and scaling`fit` to`none` after its real PDF filter processed them:

- The source was one A4 page, `MediaBox=[0,0,595,842]`, rotation0,616 bytes,
  SHA256 `e4116626cdda1c1713cf65ab2b706fd35bf2deb5e953b531255403e98d088c09`.
- The actual incoming frontend spool PDF, whose1106 bytes matched the receiver's
  document count, had SHA256
  `d344f233f109e568225e5763ab321f2050c9cfaa2f3602d69d681817d9b50efe`.
  `pdfinfo`/`qpdf` showed landscape A3
  `MediaBox=[0,0,1190.551147,841.889771]`, page`Rotate=270`, and the source
  form's90-degree rotation matrix `[0,-1,1,0,0,595]`.
- Its content matrix `[1.413956,0,0,1.413956,0,0.292877]` applies the actual
  rotated-page fit: `min(1190.551147/842,841.889771/595)=1.4139562316`.
  Independent `pdftotext -bbox` text-width growth was1.413956; the original
  `Ferriprint NONPRINTING package acceptance` marker/content stream survived
  exactly. Nonempty bytes alone were not counted as orientation/scaling proof.

One explicit frontend Cancel reconciled to receiver state7; graceful SIGINT
produced exactly one received-document ledger entry. The owned local CUPS job
was then canceled for cleanup. These are observed software transformations and
nonprinting delivery, not a physical orientation/edge/color/quality measurement,
a native GUI/preset-visibility result or a second real printer/model.

## Post-release checks without submission (2026-10-28)

These later checks are metadata/settings-only. They do not reuse historical
one-job or cancellation authority and create no document or Print/Create/Send/
Cancel request. Their [dated compatibility matrix](compatibility.md#post-release-native-client-observations-2026-10-28)
separates the following layers:

1. Server GPA and recipe collections describe what Ferriprint exposes.
2. Generated native CUPS/driver capabilities describe what the actual new local
   destination consumed; copied server attributes are not a substitute.
3. A merged native ticket or strict nonspooling Validate proves only its actual
   settings at that boundary, not an emitted Print ticket.
4. Visible native controls and an actual own modal window are distinct from
   full preset selection, native IPP login and physical output.

On the physical macOS27.0.1 client, fresh Default/Add/automatic Use and an actual
panel displayed all three site recipes and manual A3-zero/Best/duplex controls.
Selecting the A3 recipe changed quality/sides while Paper Size remainedA4 and
Paper Handling destination was disabled. No complete A3 GUI job ticket was
produced and no root cause is attributed. Checked printing-domain exports,
queue URIs, defaults and options were restored, not all volatile global OS state.

The independent [hosted Mac run40713530309](https://github.com/ressl/ferriprint/actions/runs/40713530309)
used native macOS15.7.9 on `VirtualMac2,1` / virtual M1 and a source-built Mach-O
from unchanged503. Actual local CUPS generated A3 29700x42000/all margins0,
High5 and both duplex modes; a fresh ordinary user separately passed native GPA
and three strict Validate requests. Frontend/native jobs, spool and measured
refused operations stayed0, with owned queue/process/state cleanup and closed
listeners. The original `GUI_NOT_PROVEN` report is retained alongside the
separate exact own-modal facts; its `na-letter` panel state is not A3 GUI proof.
Neither this VM nor its QA-only Cargo receiver copy is a product distribution
or another physical device.

The [Windows native run40769440761](https://github.com/ressl/ferriprint/actions/runs/40769440761)
proved the absolute-helper guard, exact public version, real managed init/runtime
and separate HTTP administrator/ordinary-user roles. Native Add automatically
selected Microsoft IPP Class Driver, but the full native settings gate failed:
its actual capability XML lacked A3, quality and duplex, and validated A4 tickets
did not preserve required quality/duplex or bordered imageable areas.
`NoConflict` alone is not settings preservation. The separately captured direct
frontend GPA is not evidence of the driver's actual requests or responses.
Native jobs, frontend ledger and refused operations were measured zero; owned
cleanup and original queue/default/inbox-driver preservation passed. Frontend
filesystem spool count was not separately measured. Anonymous loopback IPP
functionality is distinct from the HTTP role checks and proves neither native
IPP authentication nor TLS. Earlier unknown errors remain unknown; the later
bare-helper execution error was corrected without a measured PATH claim.
The native failure remains agent-actionable diagnosis, not a platform verdict.

The later [bounded observation40783698573](https://github.com/ressl/ferriprint/actions/runs/40783698573)
again passed version/init/runtime/HTTP roles, but its initial all-thread
attachment was followed by `THREAD_COVERAGE_LOST`. No public-GPA canary was
decoded/validated, so the guard prevented native Add/capability collection.
The offending thread/state snapshot was not retained; thread exit, ptrace
behavior or another loss mechanism is not established. Incomplete coverage and
zero decoded bytes prove neither no request nor no future/asynchronous request.
Detach, measured native/ledger/refused-operation zeros and owned cleanup passed.
This does not replace the preceding actual native settings failure or prove a
Windows/product incompatibility, client response consumption or an original fix.

The later [snapshot-boundary run40823203264](https://github.com/ressl/ferriprint/actions/runs/40823203264)
on 2026-10-29 reached the published runtime and HTTP-role checks, but its first
protected read-only ledger count failed with SQLite `SQLITE_PROTOCOL` (15).
Credential and database-file guards passed. No owned frontend stop, gated
release, trace/canary or native Add occurred, so this run does not replace the
preceding actual native settings failure. Its final native job count was zero,
but the final frontend ledger was unproven and the fixture was preserved on
uncertainty; no full-zero or successful runtime-teardown claim is made.
Known-head-only source-branch/clean-clone cleanup was separate from that runtime
preservation and provider-managed VM disposal. The failing WAL retry mechanism
remains unestablished; no group/permission/OS cause or native fix is inferred.

The [derivative-reader run40833645412](https://github.com/ressl/ferriprint/actions/runs/40833645412)
used an explicitly attributed SQLite3.50.2 QA derivative with a separate Cargo
root/lock, leaving the published frontend unchanged. Its first read-only count
again failed with code15. Process-local counters recorded `INDEX_SHMMAP_BUSY=100`
and `PROTOCOL_LIMIT_REACHED=1`, with the other10 fixed counters zero. This locates
this modified reader's retry branch, not a kernel lock holder, errno, OS cause,
historical branch or native fix. No frontend stop, trace/canary or native Add was
reached. Final native jobs were zero, but the final frontend ledger was unknown
and the fixture was preserved. Known-head source-ref/clean-clone removal was
separate; this was not a full-zero runtime-cleanup or native acceptance result.

The [existing-call run40840342606](https://github.com/ressl/ferriprint/actions/runs/40840342606)
again failed before frontend stop, trace or native Add. Fixed observations were
`DMS_GETLK_REPORTED_WRLCK=101`, mapping `BUSY=100`, protocol-limit1, and no targeted
exclusive/shared advisory failure. The first returned-structure PID0 remained
`AS_KERNEL_REPORTED_UNVERIFIED`; both unobserved errno slots were null. Since
the caller already supplied write-lock type and had not initialized the PID
field, these observations do not prove a kernel output write or holder identity.
The race/corruption guard remained intact. Final native jobs0 did not prove the
unknown frontend ledger zero; runtime was preserved. Known-head ref/clean-clone
cleanup passed separately, and the preceding actual native failure remained.

The [output-sentinel run40844399095](https://github.com/ressl/ferriprint/actions/runs/40844399095)
set only output-only `l_pid=-2` immediately before the existing call, leaving
request type/range/whence and the corruption guard unchanged. The first recorded
returned PID equaled-2, with the same101 write-lock-type guard observations,
100 mapping retries and protocol-limit1. Neither equality nor these counts
identify a writer layer, actual holder, stub or OS cause. This call chain did
not provide a trustworthy conflict report usable for the protected observer.
A conformant existing locking/output path on an authorized environment is needed;
there is no source-proven safe fix or license to bypass the guard. No frontend
stop, trace/canary or native Add occurred. Final native jobs0 did not prove the
unknown final ledger zero; runtime was preserved and known-head source cleanup
passed separately. The last actual native settings failure remains unchanged.

A metadata-only receiver's fixed `jobs=[]` / `document_bytes=0` fields are design
invariants, not independently measured document counters. Real native/frontend
ledgers, spool state and refused-operation counts are recorded separately.
Unknown/nonzero runtime state prevents owned teardown and acceptance; preserve
the fixture without exporting jobs/credentials or canceling/retrying anything.
Cleanup of owned resources is separate from provider-managed VM disposal.
