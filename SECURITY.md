# Security Policy

## Supported versions

| Version | Support |
| --- | --- |
| 0.3 development | Breaking managed-state cutover; not a published release or certification; report against the exact public-development commit |
| 0.2.x | Latest immutable published release; explicit `migrate-0.2` is required before using0.3 |
| 0.1.x | Historical unsupported configuration/authentication schema |

Ferriprint is not a certified printing product. See the compatibility matrix and
license warranty terms rather than interpreting a test result as certification.

## Reporting a vulnerability

Report suspected vulnerabilities privately by email to **rr@canus.ch**.

- Include the affected version/commit, component and minimal reproduction steps.
- Do **not** include passwords, hashes, private keys, SNMP communities, document
  contents or real users' job titles in reports or shared logs.
- Do not open public issues for vulnerabilities. Coordinated disclosure follows
  investigation and an available fix.

## Frontend security scope

In scope:

- IPP/HTTP envelope, target and job-attribute parsing/validation;
- protected startup/trusted-CUPS configuration, private PPD acquisition and managed
  account/catalog/profile/complete-recipe parsing/validation;
- bounded Argon2id authentication, current account/role/per-printer grants,
  permanent owner identities, network admission and verified job ownership;
- owner-filtered IPP/root/JSON views, browser account/password/grant administration,
  same-origin CSRF, revision conflict handling and atomic profile/catalog cutover;
- TLS termination, upstream/telemetry trust and private credential-descriptor
  handling, including explicit legacy import without runtime fallbacks;
- private spool/durable metadata, uncertain outcomes, actual upstream
  reconciliation/cancellation and revocation boundaries before new submissions;
- fixed-destination privacy-minimal webhook delivery, persisted condition/outbox
  state, bounded retries and truthful acknowledgement semantics;
- lifecycle-bound, explicitly scoped Linux Avahi publication and honest
  saved/HTTP-active versus pending/published/failed discovery status.

CUPS/filter vulnerabilities, printer firmware and vendor PPD/driver packages belong
to their respective maintainers. Ferriprint deliberately relies on an existing
working native queue/renderer rather than replacing their security boundaries.

**Ownership isolation applies to requests through Ferriprint.** It does not secure
a separately reachable native CUPS queue, physical printer, driver-management UI
or another print path. If end-to-end user privacy is required, the operator must
separately restrict direct backend/device access. Ferriprint installation does not
change those firewall, queue or device policies, and this rollout does not claim
otherwise. A trusted-LAN anonymous source deliberately shares one principal;
`requesting-user-name` and source IP alone are not a person's identity.

Within configured network admission and TLS policy, unauthenticated clients may
read only capability Get-Printer-Attributes and the AGPL `/source` offer.
These exceptions are not an anonymous print principal and expose no job records,
documents or admin/profile actions. All other IPP operations, including
Validate-Job, and web/job APIs require a verified user or an explicitly admitted
shared anonymous source. **Supplied invalid Authorization remains401 even on the
metadata/source exceptions**; there is no bad-credential fallback. Authentication
attributes reflect the request source, not a blanket policy that silently forces
an existing anonymous client to authenticate.

## Secrets and private access

Frontend users in0.3 live only in private service-owned `state_dir/access.toml`.
It contains Argon2id PHCs, never plaintext. Initialize the first administrator
offline with `ferriprint --config PATH user init --username NAME`; use hidden
terminal input or bounded stdin, never password argv/URLs/logs. Browser account
and own-password forms use bounded same-origin POSTs, current authority and CSRF.
Keep everyday printing and global-administration credentials separate.

Suspended usernames and historical owners remain permanently reserved: no
rename/delete/reuse operation may transfer old jobs to a new person. Ordinary
users require an explicit printer grant; a known unauthorized user never falls
back to the shared anonymous identity. The final enabled administrator is
protected against suspension/demotion, including concurrent mutations.

Legacy0.2 `access.users`/`[[printers]]` are rejected at runtime. The stopped-service
`migrate-0.2` import preserves existing ownership/history and uses the original
effective-private PHC FD validator. If systemd has removed runtime credentials,
`--credential-sources` must provide a protected exact username-to-persistent-PHC
path map; there is no inferred path or missing-secret fallback. Offline
`user recover-admin` uses the service lock/private-state validators, not a network
backdoor. Imported managed access/catalog state stays service-owned, not
group-readable configuration.

Legacy PHC import inputs and optional webhook bearer-secret files must be regular
non-symlink files effectively readable only by root and the service UID, with no
effective group/other/untrusted named-ACL access.
Simple owner-only0600 files are suitable. Linux systemd LoadCredential may expose
a root-owned0440 file whose named service-UID ACL grants read while the owning
**group entry has no access**: the group mode bits can represent an ACL mask, not
permission for a group. Such credentials require validation of the effective ACL,
not a pathname-based exemption or permission broadening. Keep originals root0600
under root0700 and inspect metadata only; never dump PHC/secret contents to prove
permissions. TLS keys similarly stay in protected operator storage/runtime
credentials, outside repositories and release assets.

Optional upstream Basic credentials still come from explicitly named environment
variables; only their names/username belong in protected bootstrap configuration.
Native/explicit CA trust is verified; disabling certificate/hostname checks or
blindly accepting a new device certificate is not an onboarding procedure.

Trusted CUPS server origins and optional CA/auth references are protected startup
configuration. Browser onboarding selects only freshly enumerated queues and
bounded PPD metadata from that exact source; no arbitrary backend URL, shell
command, device URI, filesystem path or vendor-driver installation is accepted.
This does not grant the browser access to native queue administration.

The webhook destination and secret reference are likewise operator-controlled,
not browser-editable. HTTPS verifies certificates and forbids redirects; literal
loopback HTTP exists only for an explicitly isolated receiver. Events exclude
document bytes/titles, owners and credentials. Delivery is at least once: a stable
event ID permits receiver deduplication;2xx is an acknowledgement, not proof of a
human notification. An outage/unknown observation never invents physical paper
contents or a healthy recovery.

Live bootstrap, vendor PPDs/drivers, hashes/keys, spool/database and private
operator/network records must not enter public git history, `/source`, release
packages or logs. The embedded source offer contains the reviewed project source,
not private runtime state or vendored registry dependencies. Publication uses the
clean public-development branch; never merge historical private parents into it.
