# Changelog

All notable changes to Ferriprint are documented in this file. The format follows
[Keep a Changelog](https://keepachangelog.com/); versions follow
[Semantic Versioning](https://semver.org/).

## [0.2.0] - 2026-10-27

### Changed

- Breaking 0.2.0 configuration cutover: immutable queue/service/PPD/backend identity
  remains in the protected bootstrap; all mappings/defaults/formats/resolutions and
  declared-ready data move to one schema-versioned managed profile. No inline
  override layer or legacy aliases. Profile saves reject active/held/uncertain jobs
  and stale revisions; durable jobs retain their resolved option/backend snapshot.
- Replace singleton plaintext-environment frontend authentication with bounded
  Argon2id user/admin principals, protected non-symlink hash files and explicit
  anonymous source CIDRs. Invalid supplied credentials never fall back to anonymous.
  Preserve a prior anonymous owner string when retaining its job history.
- Allow only capability Get-Printer-Attributes and the AGPL source offer without
  credentials inside existing network/TLS admission. Every job operation,
  including Validate-Job, and web/job APIs still requires a verified user or
  explicitly shared anonymous source; invalid supplied credentials always401.
  Authentication attributes reflect the request source rather than forcing a
  preserved anonymous client through a global discovery auth hint.
- Linux discovery uses Rust Avahi registration with the actual TLS FQDN and explicit
  interface/family rather than the obsolete `.local` embedded backend or a Python
  publisher. Existing external deployment wiring requires one clean cutover.
  Non-printing real Avahi collision/restart/withdrawal and native macOS
  discovery/raw-root-dot Get/Validate were exercised on isolated endpoints.
- Generic packaged service uses `/usr/bin/ferriprint`; an immutable `/opt` deployment
  explicitly overrides ExecStart in its private host-owned drop-in.
  Optional Avahi/D-Bus dependencies are ordering-only: installing the package does
  not activate or restart global discovery daemons.
- Container CI explicitly trusts only its owned checkout path for Git-based source
  and dependency proofs, independent of the hosted runner's UID and HOME behavior;
  it does not trust arbitrary directories or change a persistent host's Git config.

### Added

- Read-only native queue/PPD inspection and guided or explicitly reviewed setup
  installation with shared mapping validation, option previews and protected
  no-overwrite bootstrap/profile outputs.
- Authenticated browser profile/default editing with durable immediate activation,
  owner-filtered root/IPP/job APIs and real upstream cancellation guarded by role,
  ownership, CSRF and exact Origin. Actual Chromium native-form acceptance caught
  and fixed the no-referrer/opaque-Origin conflict without accepting null Origin.
- Verified IPP supply/tray/status observations with source/fetch age, honest raw
  marker-change time, stale/unknown failure behavior and clear separation from
  unobserved operator-ready declarations.
- Six-point implementation/security/acceptance specification, actual non-printing
  browser regression runbook, factual compatibility matrix and clean public-branch
  workflow that preserves historical private source without importing its parents.
- Versioned Linux tar/Debian package tooling, manual/examples, pinned-input build
  manifest, locked-dependency SPDX and regenerated verbatim/deduplicated notices;
  exact all-member embedded-source proof and disposable install/remove/ownership
  gates. Packages never enable/start/restart the service or configure native CUPS.
- Verbatim copyright/license notices for the pinned compiler's Rust standard
  library, checked against that actual compiler and shipped in source/packages
  separately from the accurately scoped Cargo.lock SPDX inventory.
  A self-contained standard copyright file preserves all project/crate/Rust
  library notice text under minimal-image dpkg documentation filters, without
  overriding operator policy; raw upstream HTML remains in package/source/tar.

### Fixed

- Accept genuinely private systemd LoadCredential files through effective
  root/service-UID ACL checks rather than rejecting the ACL mask as group access.
  Keep root0600 originals, no-follow regular-file checks and rejection of
  untrusted users/groups; no path exception or permission weakening.
- Reconstruct per-printer configuration-change time from known profile/PPD
  metadata and advance it on durable profile activation rather than advertising
  a constant zero or an unrelated last-query time.
- Validate every Git archive member before portable extraction, rejecting
  traversal, links and special files while supporting the pinned Bookworm
  builder's Python3.11 extraction API.

## Earlier private pilot development

These entries describe work before the0.2 configuration/discovery cutover, not
current deployment instructions. The old external publisher and singleton
authentication configuration are superseded, not compatibility paths.

### Fixed

- Match local printer and job targets when native DNS-SD retains the DNS hostname's
  absolute-root dot or changes hostname case. The real macOS `cupsBackendDeviceURI`
  returned the dotted IPPS hostname; unmodified Get-Printer-Attributes and
  Validate-Job previously failed with `client-error-not-found`. Keep the canonical
  advertised URI, job URI and UUID unchanged, and retain exact scheme, userinfo,
  port and raw path/query/fragment boundaries without DNS alias lookups.

- Document verified native macOS driverless queue creation with an explicit IPPS
  device URI, distinguishing a complete printer URL from Add Printer's Address
  field. Avoid the failed full-URL/IP/Generic PostScript setup without changing
  the deployed server, its pinned source archive, or existing printer defaults.
  Record observed native print-dialog A3.Borderless and High quality choices,
  without submitting a print job or claiming physical-output acceptance.

### Added

- Document secure external Bonjour publication through interface-scoped Avahi
  D-Bus registration, a canonical certificate-bearing FQDN, current IPP-derived
  AirPrint TXT/subtype metadata, and frontend-bound withdrawal/republication.
  Keep built-in discovery disabled, the exact client ACL and printer sharing
  unchanged; discovery must not start a deliberately stopped pilot. No
  application or TLS workaround.

- Isolated Linux pilot deployment guide, portable least-privilege systemd unit using
  systemd credentials, and a digest-pinned container release-build script that records
  the exact source commit, toolchain, lockfile, and binary checksum without installing
  a compiler on the print server.
- Driverless IPP frontend that validates standard job attributes (`media-col` with numeric
  dimensions and explicit margins, `print-quality`, `print-color-mode`, media source,
  sides) against an operator-provided printer profile before submitting resolved native
  options to an existing native CUPS queue over real IPP. Explicit numeric media and each
  explicitly requested margin are honored; unsupported combinations are rejected rather
  than silently substituted.
- Printer profile layer mapping operator-declared media types, quality presets, color
  modes, trays, and sides onto PPD-validated native choices, with numeric media matching
  and standard dimensional-rounding normalization; mappings are validated against the
  actual PPD at startup.
- Durable job engine: held job creation, single-document ingestion into a private spool
  with size limits, bounded queued-job admission, upstream IPP submission, and job-state
  reconciliation against the real upstream queue — including real cancellation by
  upstream job id, page counters, and restart-safe recovery with an explicit offline
  `abandon` command for unknown-outcome submissions.
- IPP surface: Print-Job, Validate-Job, Create-Job/Send-Document, Cancel-Job,
  Get-Job-Attributes, Get-Jobs, Get-Printer-Attributes; strict envelope validation
  (version 1.1/2.0, charset, target URI canonicality, group ordering) with standard IPP
  status codes and unsupported-attribute reporting.
- IPPS listener (rustls, single `ring` provider; TLS verification never disabled), optional
  Basic authentication with ownership pinned to the configured principal, client network
  allow-lists, and a source-archive endpoint (`/source`) for AGPL source delivery.
- Operator configuration (TOML, unknown keys rejected) with per-printer PPD, upstream URI,
  upstream credentials from named environment variables only, optional upstream CA
  certificate, document-format intersection with the upstream queue, and ready-media
  declaration; offline `check` subcommand validates configuration and PPD mappings without
  any network access.
- Project governance: AGPL-3.0-only license, security policy, contribution guide,
  complete locked-dependency inventory (`THIRD_PARTY_NOTICES.md`, including bundled SQLite
  and cryptography-provider notes), upstream reference-design document, and CI pipelines
  for GitLab (`.gitlab-ci.yml`) and GitHub (`.github/workflows/ci.yml`).
