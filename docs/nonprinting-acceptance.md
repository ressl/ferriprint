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
