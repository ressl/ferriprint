# Security Policy

## Supported versions

| Version | Support |
| --- | --- |
| 0.2.x | Current pre-release; report vulnerabilities against the latest version/public-development revision |
| 0.1.x | Historical, unsupported configuration/authentication schema; migrate explicitly before upgrading |

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
- bootstrap, private PPD and managed-profile parsing/validation;
- bounded Argon2id authentication, network admission and verified job ownership;
- owner-filtered IPP, root HTML and JSON job views, admin authorization, exact
  Origin/CSRF protection and atomic managed-profile activation;
- TLS termination, upstream/telemetry trust and credential handling;
- private spool/durable metadata, uncertain outcomes, upstream reconciliation and
  actual upstream cancellation;
- lifecycle-bound, explicitly scoped Linux Avahi publication.

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

Frontend users in0.2 use named `user`/`admin` principals with protected **Argon2id
hash files**, not a singleton plaintext password environment variable. Generate
hashes through protected stdin or a hidden terminal; no plaintext password argv,
URL, committed configuration or diagnostic output. Keep personal printing and
administration credentials separate. Invalid supplied credentials never silently
fall back to anonymous admission.

Hash inputs must be regular non-symlink files effectively readable only by root
and the service UID, with no effective group/other/untrusted named-ACL access.
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

Live bootstrap, vendor PPDs/drivers, hashes/keys, spool/database and private
operator/network records must not enter public git history, `/source`, release
packages or logs. The embedded source offer contains the reviewed project source,
not private runtime state or vendored registry dependencies. Publication uses the
clean public-development branch; never merge historical private parents into it.
