# Compatibility and evidence matrix

This is an evidence matrix, not vendor certification or a claim that an untested
platform works. The immutable0.2.0 release and manifest identify the exact public
source/build inputs. Rows record actual exercises, including the downloaded
release and controlled nonprinting rollout; unsupported targets remain explicit.

## Server and distribution targets

| Target | Build/distribution status | Discovery server | Evidence boundary |
|---|---|---|---|
| Linux amd64, glibc | Published0.2.0 tar/`.deb`; native build, fmt, strict all-target/all-feature clippy and55 tests passed in the actual release workflow | Real Avahi0.8 D-Bus collision, restart/re-registration and withdrawal exercised separately; actual released deployment observed by native macOS | Rust1.99.1 release compiler and actual Rust1.88.0 library/executable builds passed; no whole-build byte-reproducibility claim |
| Debian12 amd64 | Disposable `.deb` install/reconfigure/remove/purge and documented tar install/service-user offline check passed | Real Avahi 0.8/private D-Bus lifecycle exercised separately | Retained state/account/config and unchanged container CUPS sentinel verified; not a booted systemd test |
| Ubuntu26.04.1 amd64 | Disposable package/tar exercises passed; actual downloaded release deployed through the reviewed unit/private host override | Rust publisher retained the prior service identity/FQDN and passed fresh native macOS resolve/Get/Validate | Static identity/state/native config/PPD/defaults/TLS preserved; approved client/auth policy changed deliberately. A concurrent scheduled CUPS logrotate restart is recorded below, not called an unchanged invocation |
| Linux arm64 | Unreleased0.3 native AArch64 ELF and both Debian/Ubuntu package/tar runtime gates exercised; no published ARM64 release yet | No additional discovery-server result inferred from packaging | Actual architecture/execution and installed-runtime evidence below; not cross-compilation or hardware-client certification |
| macOS as server | Not supported/tested for this release | No macOS server Bonjour backend claim | Native macOS is a client verification target only |
| Windows/BSD as server | Not supported/tested | Not claimed | No packages or operational guarantees |

The executable is self-contained with respect to Rust crates, not a fully static
operating-system image: glibc/libgcc and trusted CA data are prerequisites. The
build manifest records actual dynamic requirements. CUPS, a working native queue
and its driver/PPD are still required. No vendor driver/PPD is distributed.

## Clients and ownership

| Client/path | Exercised behavior | Not implied |
|---|---|---|
| Native macOS IPPS/DNS-SD | Released Rust publisher observed fresh Add/resolve; exact native raw root-dot URI passed system trust, GPA and A3 zero-margin/high Validate;34 served semantic/default/capability fields matched prior frontend | No certification or physical-quality claim. Isolated Default/Add acceptance restored its temporary default changes; final existing queues/both default scopes/preferences stayed unchanged |
| Native macOS authenticated queue | Strict and mixed/AIR-omitted Default Add succeeded; generated A3.Fullbleed/High and username/password keyword sets matched exactly; missing credentials401, verified credentials Get/Validate0; one authenticated native PDF reached the receiver and was actually cancelled | One physical Mac only. Temporary queues removed; original three destinations/default scopes/preferences restored, exact pre-Mixed user-default bytes and no fixture Keychain item; no second submission or physical print |
| Two authenticated actors, isolated receiver | Actual submissions, owner-filtered IPP/root/API views, cross-owner denial, browser cancellation propagated to the receiver, and durable restart without duplicate submission exercised | Two principals are not two physical Macs; no physical endpoint was submitted to |
| Additional trusted-network client | Actual distinct Linux127/8 TCP source allowed capability GPA but required credentials for protected operations; invalid provided credentials401 even on metadata/source. Native mixed-policy Mac with no anonymous eligibility completed setup and authenticated Validate | Reviewed network admission is not a user/admin grant; no guest/pentest/DMZ expansion or claim of a second physical Mac |
| Anonymous trusted-LAN source | Deliberately shared principal retained only for explicit legacy CIDRs; actual existing Mac Get/Validate and historical-owner view preserved; management401 challenge | No per-person privacy/cancel isolation for anonymous users |
| Browser administration | Actual isolated native forms validated/saved with CSRF/owner/active-job gates and real upstream cancellation; released deployment's real admin form and separate-context personal status view observed read-only | No arbitrary PPD/backend/identity/path editing; direct native CUPS is outside frontend isolation. HTTP Basic cache is browser-context-wide; use separate contexts for separate actors |

Native macOS discovery labeling is recorded literally: the existing pilot showed
Kind **Bonjour**, automatic **Use: Epson ET-16600 — Ferriprint pilot**, not the
literal word AirPrint. Ferriprint is not certified by Apple, a printer vendor or
an IPP certification program. A user-reported successful physical print is accepted
as that observation, not as measured dimensions/quality or a reason to print again.

## Observed printer/profile/status boundaries

| Item | Actual known observation | Honesty requirement |
|---|---|---|
| Epson ET-16600 native CUPS/vendor PPD | Private installed PPD/driver; real native metadata inspection and reviewed setup install resolved 34 numeric media variants and explicit A3 borderless/high mappings | Not bundled; no claim every model/PPD is supported |
| Numeric media and quality | Native Get/Validate and setup previews accepted explicit A3 dimensions/zero margins/high quality; unsupported mappings are rejected | Mapping/Validate acceptance is not measured edge coverage or physical quality |
| Native queue ink markers | Verified IPPS queue actually returned Black/Cyan/Magenta/Yellow ink types, levels26/56/55/56, low15/high100 during inventory | Identify native queue source and its marker-change time; fetched now does not mean physically measured now |
| Native queue paper/status | Queue reported Letter/Legal media-ready and media-empty-report; no physical tray count observed | Queue metadata is not sensor-confirmed loaded paper; do not invent tray contents/percentages |
| Direct-device TLS/SNMP | No new direct-device trust or SNMP source was needed for existing queue-provided markers | Never bypass TLS or invent a community; unavailable readings stay unknown |
| Profile declared-ready | Operator declaration only | Never label it a live physical tray sensor |

## Final release evidence

[Version0.2.0](https://github.com/ressl/ferriprint/releases/tag/v0.2.0) is immutable
commit `24ccd15668502e24854cdaea87b4500c8e767554`.
The [actual successful publisher](https://github.com/ressl/ferriprint/actions/runs/40594946394)
ran quality, minimum-Rust, privacy, exact-source/notices/SPDX and both package/tar
gates before release. All six actual downloaded assets matched uploaded digests
and checksums; the released ELF's56-member embedded archive matched the tag.
See [exact hashes](release-and-installation.md#observed-release-020).

Controlled rollout preserved existing durable records/IDs/sequence/time origin,
empty spool, static ownership and native/Mac queues/defaults/TLS. Real private
user/admin credentials passed ownership/role/challenge gates; actual browser
rendering showed source/fetch age, four actual ink markers and unknown physical
freshness/trays. No production Print/Create/Send or new physical job was issued.
An existing scheduled CUPS logrotate action restarted the native scheduler during
the window; this concurrent maintenance exception is explicitly recorded, not
misrepresented as unchanged native invocation. Global Avahi/Docker/containerd
invocations and native configuration/PPD/defaults/TLS stayed unchanged.

## 0.3 development evidence boundaries

Unreleased0.3 candidate `fe2611a421241ea3501ee84a2537a95e04619dc1`
was actually built and exercised on two independent native Docker engines:
Linux AArch64 on Apple Silicon and Linux x86_64 on Ubuntu26.04.1. Their pinned
Rust1.99.1 images reported matching native daemon/kernel/rustc hosts; the actual
ELFs were AArch64 machine183 and x86_64 machine62 and executed as version0.3.0.
This is not an old0.2 result relabeled or a published0.3 release.

| Native architecture | Debian12 | Ubuntu26.04.1 | Actual runtime boundary |
|---|---|---|---|
| amd64 / x86_64 | `.deb` lifecycle and documented tar installation passed | `.deb` lifecycle and documented tar installation passed | Each installed executable, dedicated nonroot UID100 and independent private state |
| arm64 / AArch64 | `.deb` lifecycle and documented tar installation passed | `.deb` lifecycle and documented tar installation passed | Each installed executable, dedicated nonroot UID100 and independent private state |

All eight installed-runtime runs used genuine0.3 initialization/catalog/setup,
offline checks, GPA/Validate/PDF submission, upstream700 Get/Cancel, durable
restart without duplicate submission, served-source equality and an exact
one-document graceful receiver ledger. Package install/reconfiguration/removal/
purge preserved the account/private state/operator sentinels and container CUPS
configuration, without automatic enable/start/restart. These were disposable
containers, not booted-systemd or physical-output tests.

Both candidate executables embedded the same72-member source archive
(`fc81304fd2ca9f94ece8a08f03d436c36f3b9cc3de419f0a433273f7d2b4bb53`).
The actual merger accepted byte-identical common source/281-package SPDX and
produced one eight-asset checksum set. Each architecture retained its own ELF,
tar, Debian package and native build manifest. The explicitly ignored Linux
root migration ownership regression was separately invoked as UID0 on each
native architecture: one passed, zero ignored per invocation, including actual
UID994 read-success/write-denial for the root-owned protected output.

Real native Debian CUPS2.4.2/cups-filters1.28.17 also applied all eight options
from the actual complete A3 recipe export through `lp` and its real IPP backend.
The nonprinting receiver observed TA3/Fine/Rear/Photo/RGB/None, copies2 and
300x300dpi. Native CUPS applied orientation/fit inside the PDF and normalized
their final ticket fields; the measured PDF geometry/matrices and preserved
source marker are recorded in [nonprinting acceptance](nonprinting-acceptance.md#observed03-native-cups-recipe-application).
This proves native software application, not print-dialog visibility, additional
hardware, measured margins/quality or physical paper output. A later artifact's
own manifest/source identifies its exact successor revision; this candidate is
not silently relabeled.

On that candidate, one actual macOS native-library application used all eight
saved recipe-export options through `cupsParseOptions`/`cupsEncodeOptions2`,
Validate0 and one `cupsDoFileRequest` Print0. A normal user created through the
real browser, granted only the isolated printer, authenticated with system TLS
trust and the native root-dot URI; no admin/anonymous print bypass was used.
The receiver observed TA3/Fine/Rear/Photo/RGB/None, copies2,orientation4,
300x300dpi,scaling`fit` and632 PDF bytes; frontend media was29700x42000 with
four zero margins and that user's ownership. The normal user's real browser
Cancel reconciled frontend/receiver to7 with one ledger entry. Existing three
queue URIs, both defaults, preference bytes and Keychain save state were not
changed. This was actual native-library application, not a local queue install
or a native print-dialog test.

On the already observed real Mac, the0.3 native print-dialog installation/preset
visibility gate remained **unverified / externally blocked**: the authorized
session was screen-locked and accessibility reported no usable windows.
Native-library/metadata success is not a GUI test. This is a session-access
prerequisite, not a claim that macOS is unsupported; no unlock or permission
bypass was attempted.

| Additional requested target | Actual inventory/access evidence | Remaining acceptance prerequisite |
|---|---|---|
| A second real Mac | Private inventory identifies additional M1 Max MacBook targets; an existing authenticated Darwin/arm64 shell runner is registered online/unpaused. One separately authorized bounded private metadata job never started and was canceled; no Mac command ran | Actual separately identified hardware/OS and a usable authorized native-client session; registration/presence is not execution or a print-dialog test |
| Real iPhone | Existing network/phone metadata indicates devices, but no usable native print UI or exact current model/OS was exercised | An already trusted accessible real iPhone native print session; no pairing/permissions/new app was introduced |
| Real iPad | Existing iPad inventory/network metadata, no usable native print UI/model/OS observation | An already trusted accessible physical iPad native print session; a simulator does not qualify |
| Windows | Authorized VM/runner inventories produced no usable Windows native test environment; registered active executor metadata was Darwin/Linux | An authorized existing real Windows computer/VM and native print session; no new VM/driver or spoofed-client claim |
| More real printer models / measured paper quality | Existing native destinations/queues resolve to the same Epson ET-16600; synthetic PPDs/receivers are explicitly synthetic | A separately identified real model with preexisting queue/driver; measured physical output requires separate approval and actual observations |

These are scoped evidence prerequisites, not permanent assertions that the
platforms are unreachable or incompatible. They block their own support/coverage
claims, not reachable software implementation. No extra physical page, native
queue/default change, device pairing or permission grant was made to fill the
matrix. Private device identities, addresses, credentials and operator/job logs
are not public release material.
