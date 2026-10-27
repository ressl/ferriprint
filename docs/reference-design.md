# Reference design and upstream study

This document records the upstream components studied while designing Ferriprint and the
architectural decisions Ferriprint takes differently. Ferriprint is an independent
implementation: no C source from the studied projects is copied, translated, or mechanically
ported into this repository.

## Studied upstream components

### PAPPL (Michael R Sweet)

- Studied at master commit `be020bbb45a6ae07245d1bf1ca5dd62ca5377537`.
- License: Apache-2.0. The project's `NOTICE` defines an *optional* exception that permits
  combined GPLv2/LGPL2 linking (used with older CUPS); Ferriprint relies on none of it and is
  licensed AGPL-3.0-only.
- `pappl/client-ipp.c` — IPP request discipline used as inspiration:
  - reject unsupported IPP major/minor versions (`server-error-version-not-supported`),
    request ids ≤ 0, and empty requests;
  - enforce attribute-group ordering and require `attributes-charset`
    (us-ascii/utf-8), `attributes-natural-language`, and a system/printer/job URI before
    any operation routing;
  - resolve jobs strictly by URI/job-id and answer not-found for unknown ones;
  - drain leftover document data after failed requests instead of desynchronizing the
    connection;
  - report ignored and unsupported attributes in the unsupported-attributes group with the
    matching status codes (`successful-ok-ignored-or-substituted`,
    `client-error-attributes-or-values-not-supported`).
- `pappl/job-ipp.c` — job lifecycle inspiration: explicit
  pending/held/processing/stopped/canceled/aborted/completed states, held "incoming" jobs
  that leave the held state only after their document is fully received, and state reporting
  through `job-state`, `job-state-message`, and `job-state-reasons`.
- Immutable link:
  [client-ipp.c at be020bbb](https://github.com/michaelrsweet/pappl/blob/be020bbb45a6ae07245d1bf1ca5dd62ca5377537/pappl/client-ipp.c)

### pappl-retrofit (OpenPrinting)

- Studied at master commit `49ad525a3715a57aae24c715ef0767b55ffb1991`.
- License: Apache-2.0.
- `pappl-retrofit/print-job.c` — the PPD option mapping patterns that Ferriprint normalizes:
  - numeric `media-col` (x/y dimensions plus the four margins) is handed to the CUPS PPD
    cache to select a `PageSize` choice; `media-source` maps to `InputSlot` (respecting a
    driver-specific source option name); `media-type` maps to `MediaType`;
  - print presets are selected as the cross product of color mode (color vs. black) and
    print quality (draft/normal/high) and applied as native PPD options;
  - a sides bitmask selects the PPD duplex option's one-sided / two-sided-long-edge /
    two-sided-short-edge choices, with vendor options appended;
  - `orientation-requested` is forwarded as a filter option only within the valid IPP
    enum range;
  - the assembled option map is marked into the PPD and carried into the CUPS filter
    pipeline together with job id, user, title, copies, and a cancellation callback.
- Immutable link:
  [print-job.c at 49ad525](https://github.com/OpenPrinting/pappl-retrofit/blob/49ad525a3715a57aae24c715ef0767b55ffb1991/pappl-retrofit/print-job.c)

### ipp.rs (the `ipp` crate, Dmitry Pankratov)

- Version 8.0.0, dual-licensed MIT OR Apache-2.0.
- Used as a runtime dependency (IPP encoding/decoding), not as a design reference. Its
  bounded-string APIs are fallible and Ferriprint validates before encoding.
- <https://github.com/ancwrd1/ipp.rs> (tag `v8.0.0`)

### ipp-printer-app (crate)

- Inspected and deliberately not used: it lacks durable spooling, a TLS serving surface, and
  the color/PDF handling this project requires.

## Architectural differences

| Concern | PAPPL / pappl-retrofit | Ferriprint |
|---|---|---|
| Role | A printer application that *is* the printer, or wraps classic CUPS drivers inside one | A driverless IPP frontend in front of an existing, operator-managed native CUPS queue |
| Rasterization | Runs CUPS filters in-process | Not implemented; the native queue and its host-installed drivers rasterize |
| Media selection | Delegates `PageSize` selection from the numeric `media-col` to the CUPS PPD cache | Validates numeric media and every explicitly requested margin against the PPD's `PaperDimension`/`ImageableArea` and the operator profile; unsupported combinations are rejected, never silently substituted |
| Mapping source | Presets/constraints derived from the PPD at runtime | Operator-declared media-type/quality/color/side mappings plus PPD validation; standard dimensional rounding is normalized instead of failing exact matches |
| Job storage | Application-local job objects in memory/disk | Durable SQLite metadata plus a private spool directory; queued documents and IDs survive restarts and are reconciled against upstream job state |
| Cancellation | In-process cancel callback into the filter pipeline | Cancel is matched against the *upstream* CUPS job id/URI, not the local job id, and races between cancel, upload, and submission are handled explicitly |
| Completion | Job completes when the local pipeline finishes | A job is terminal only when the real upstream queue reports a terminal state with page counters; acceptance of a submission is not completion |
| Discovery/TLS | Embedded HTTP server, Avahi integration | axum server with rustls (verification never disabled) and mDNS via `mdns-sd` |

## Independently implemented code versus reused dependencies

- Independently implemented in this repository: the IPP frontend, the printer
  profile/PPD mapping layer, the durable job engine, and the upstream IPP submission
  bridge (all original Rust).
- Reused dependencies: crates.io crates under their own permissive licenses; the complete
  list with licenses is in `THIRD_PARTY_NOTICES.md`, with build-time versions pinned in
  `Cargo.lock`.
- Studied but not redistributed: PAPPL and pappl-retrofit (design references only).

## Vendor drivers

Vendor drivers and PPDs — Epson's among them — are installed separately on the operator's
host under the vendor's own license. Ferriprint consumes whatever native queue the operator
configures and neither bundles, modifies, nor redistributes vendor driver packages.
