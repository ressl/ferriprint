# Compatibility and evidence matrix

This is an evidence matrix, not vendor certification or a claim that an untested
platform works. The version release and its manifest identify the exact public
source commit and build inputs. The six-point acceptance plan tracks final gates;
rows below are updated from actual exercises before publication.

## Server and distribution targets

| Target | Build/distribution status | Discovery server | Evidence boundary |
|---|---|---|---|
| Linux amd64, glibc | Pre-release native build, fmt, strict all-target/all-feature clippy and 55 tests passed; release assets remain gated below | Real Avahi 0.8 system D-Bus collision, daemon restart/re-registration and shutdown withdrawal exercised in an isolated container | Rust 1.99.1 release compiler and actual Rust 1.88.0 library/executable check+build passed; no byte-reproducibility claim |
| Debian12 amd64 | Disposable `.deb` install/reconfigure/remove/purge and documented tar install/service-user offline check passed | Real Avahi 0.8/private D-Bus lifecycle exercised separately | Retained state/account/config and unchanged container CUPS sentinel verified; not a booted systemd test |
| Ubuntu26.04.1 amd64 | Disposable `.deb` install/reconfigure/remove/purge and documented tar install/service-user offline check passed; real isolated frontend host | Existing host Avahi registration/native macOS discovery observed; private-daemon restart/withdraw exercised separately | Actual ownership/no auto-start and CUPS sentinel invariants passed; host native CUPS/TLS/production ACL remained untouched |
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
| Native macOS IPPS/DNS-SD | Pre-release Rust publisher observed native SRV/resolve, fresh Remove/Add and Default/automatic Use; exact native raw root-dot URI passed system trust, GPA and A3 borderless/high Validate | No certification or physical-quality claim. Native Add can temporarily change defaults; isolated acceptance recorded restoration rather than claiming unchanged throughout |
| Native macOS authenticated queue | Strict and mixed/AIR-omitted Default Add succeeded; generated A3.Fullbleed/High and username/password keyword sets matched exactly; missing credentials401, verified credentials Get/Validate0; one authenticated native PDF reached the receiver and was actually cancelled | One physical Mac only. Temporary queues removed; original three destinations/default scopes/preferences restored, exact pre-Mixed user-default bytes and no fixture Keychain item; no second submission or physical print |
| Two authenticated actors, isolated receiver | Actual submissions, owner-filtered IPP/root/API views, cross-owner denial, browser cancellation propagated to the receiver, and durable restart without duplicate submission exercised | Two principals are not two physical Macs; no physical endpoint was submitted to |
| Additional trusted-network client | Actual distinct Linux127/8 TCP source allowed capability GPA but required credentials for protected operations; invalid provided credentials401 even on metadata/source. Native mixed-policy Mac with no anonymous eligibility completed setup and authenticated Validate | Reviewed network admission is not a user/admin grant; no guest/pentest/DMZ expansion or claim of a second physical Mac |
| Anonymous trusted-LAN source | One deliberately shared principal; existing anonymous behavior can be retained per source CIDR | No per-person privacy/cancel isolation for anonymous users |
| Browser administration | Actual native browser forms validated and durably saved existing profile/default changes; invalid CSRF and cross-owner cancellation 403, active-job profile save 409, own/admin cancellation and upstream terminal state observed | No arbitrary PPD upload or backend/identity/path edits; this does not protect jobs reachable through direct native CUPS access |

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

Exact-source build, Linux package exercises, isolated multi-user/profile/browser/
status exercises, real Avahi/macOS regression, public release URLs/checksums and
frontend-only host rollout are recorded here only after they are observed. Until
then the corresponding acceptance item remains an explicit release gate, not a
silently completed compatibility claim.
