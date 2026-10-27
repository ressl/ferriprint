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
| Linux arm64 | No published package unless actually built/exercised | Not claimed | Source portability is not an exercised binary release |
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
