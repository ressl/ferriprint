# Non-printing acceptance, including real browser forms

Run this only against an isolated receiver that cannot reach a physical printer.
Names containing “test” are not a safety boundary: inspect the configured upstream
URI and ensure it is the loopback non-printing receiver before any job submission.
Never reuse these submission/cancellation steps on a physical-backed endpoint.

Use a real browser and the exact HTTP(S) origin configured by `public_url`; swapping
`127.0.0.1` for `localhost` changes the origin and invalidates this exercise.
The source contains a non-printing `receiver` example; the setup/deployment guide
explains the separate bootstrap and managed-profile files. Provision isolated
admin and two user identities with distinct generated credentials and protected
Argon2id files. Do not put credentials in URLs, command arguments or shared logs.

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

## Linux discovery and distribution

Use a distinct non-production service name/port/state with the non-printing
receiver for Linux Avahi tests. Observe actual native Mac DNS-SD Add/Remove,
resolved FQDN/port/TXT, collision behavior and exact raw resolver URI Get/Validate.
Do not restart global Avahi/CUPS or add a printer merely to test metadata.

Versioned package exercises run only in disposable digest-pinned Linux containers
as described in [installation](release-and-installation.md). Source member proof,
locked notices and package ownership/removal checks are independent release gates;
passing browser tests does not imply those gates passed.
