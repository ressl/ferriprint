# Compatibility and evidence matrix

This is a dated evidence matrix, not vendor certification or a claim that an
untested platform works. The current published Linux release is0.3.0, immutable
source `503a7b90bc38a5d8b6917788ff7e237b2e684216`. Its
[downloaded release identities](release-and-installation.md#observed-release-030)
are distinct from the explicitly historical0.2 and pre-publication0.3 checkpoints
below. Later client observations never relabel an older source/build.

## Server and distribution targets

| Target | Build/distribution status | Discovery server | Evidence boundary |
|---|---|---|---|
| Linux amd64, glibc | Published0.3.0 native x86_64 ELF, tar and `.deb`; actual publisher passed native quality/source/notices and minimum-Rust gates | Linux Avahi backend; historical actual lifecycle/rollout observations are dated below | Rust1.99.1 release compiler and separate Rust1.88.0 builds; pinned inputs are not a general byte-reproducibility claim |
| Debian12, amd64 and arm64 | Actual0.3.0 package lifecycle plus installed package/tar runtimes passed on both native architectures | No new discovery or physical-client result inferred from packaging | Retained state/account/operator/CUPS sentinels and no auto-start; disposable containers, not booted systemd |
| Ubuntu26.04.1, amd64 and arm64 | Actual0.3.0 package/tar gates passed on both native architectures | Separate from the historical released-deployment DNS-SD/native Mac checks below | Eight installed runtimes across both distributions/architectures/package forms; not a new physical-output test |
| Linux arm64 | Published0.3.0 native AArch64 ELF, tar and `.deb`; both distribution gates passed in the sole publisher | No additional discovery-server result inferred from packaging | Actual machine183/native execution, not cross-compilation or hardware-client certification |
| macOS as server | No macOS server package or operational support is provided; a native source-built loopback fixture was exercised only for client QA | No macOS server Bonjour backend claim | The hosted Mac client result below does not create a supported server distribution |
| Windows/BSD as server | No native server packages or operational guarantees | Not claimed | A published Linux ELF in disposable WSL is not a native Windows server release |

The executable is self-contained with respect to Rust crates, not a fully static
operating-system image: glibc/libgcc and trusted CA data are prerequisites. The
build manifest records actual dynamic requirements. CUPS, a working native queue
and its driver/PPD are still required. No vendor driver/PPD is distributed.

## Post-release native-client observations (2026-10-28)

These checks used owned isolated destinations and public synthetic fixtures, not
another physical printer model. They issued no Print/Create/Send/Cancel request,
document or test page. Native metadata/settings consumption, GUI display, native
IPP login, an emitted job ticket and measured paper are separate evidence layers.

| Actual client | Observed result | Boundary |
| --- | --- | --- |
| Physical macOS27.0.1 arm64, fresh Default/Add/automatic Use | The real native panel displayed all three site recipes. Manual A3 zero-margin/Best-quality and duplex controls were observed; selecting the A3 named recipe changed quality/sides | Its observed Paper Size remainedA4 and Paper Handling destination was disabled. No complete A3 GUI ticket or output was produced; no Apple/server/driver root cause or universal preset behavior is inferred |
| Independent hosted native macOS15.7.9 arm64 | [Run40713530309](https://github.com/ressl/ferriprint/actions/runs/40713530309): actual local CUPS generated A3 29700x42000 with all margins0, High5 and both duplex options; a fresh ordinary user separately passed system-libcups GPA and three strict Validate requests | Actual `VirtualMac2,1` / `Apple M1 (Virtual)`, not another physical Mac. Source-built Mach-O from unchanged503, not the Linux release ELF or a macOS server package; no native Print-auth or output proof |
| Hosted Windows Server2025 x64 | [Run40769440761](https://github.com/ressl/ferriprint/actions/runs/40769440761): owned WSL1/direct-exec build, absolute-helper guard, exact public version, real managed init/runtime and HTTP role checks passed. Native Add automatically selected Microsoft IPP Class Driver | Full native settings acceptance **failed**: native capabilities omitted A3, quality and duplex; validated A4 tickets did not preserve the required quality/duplex or bordered imageable area. Native jobs, frontend ledger and refused-operation counts were measured zero; owned cleanup passed. This is not a Windows incompatibility verdict |

The preceding [Windows run40746710278](https://github.com/ressl/ferriprint/actions/runs/40746710278),
[classifier run40755062052](https://github.com/ressl/ferriprint/actions/runs/40755062052)
and [secret-free control40762760557](https://github.com/ressl/ferriprint/actions/runs/40762760557)
retain their unknown unrecorded error contents; equal lengths never prove them
identical to the later exposed error. The [bounded control40766425720](https://github.com/ressl/ferriprint/actions/runs/40766425720)
established a bare-name helper execution failure, not the actual PATH contents.
The reviewed absolute-executable correction worked in the native run above without
a global PATH, privilege or product workaround. Those earlier attempts cleaned
their owned resources and preserved the original queues/default/inbox driver.

The later [bounded observation run40783698573](https://github.com/ressl/ferriprint/actions/runs/40783698573)
passed real version/init/runtime/HTTP-role checks, then stopped on
`THREAD_COVERAGE_LOST` after an initial all-thread attachment snapshot.
No public-GPA canary was decoded/validated and native Add was not reached.
Coverage was incomplete: zero decoded bytes proves neither absence of requests
nor absence of future/asynchronous discovery. Detach, measured native/ledger/
refused-operation zeros and owned cleanup passed. The preceding native settings
failure remains the last actual native result; no loss cause or platform verdict
is inferred from the observation guard.

The subsequent [snapshot-boundary run40823203264](https://github.com/ressl/ferriprint/actions/runs/40823203264)
on 2026-10-29 again passed version/init/runtime and HTTP-role checks. Its protected
read-only ledger snapshot failed at the first count query with SQLite
`SQLITE_PROTOCOL` (15), after the credential and database-file guards passed.
This happened before the owned frontend stopped, a new process was released,
or native Add/capability collection began. The uncertain fixture was preserved,
not declared fully zero or cleaned. This is a WAL read-start diagnostic boundary,
not evidence of an underlying OS cause, a native settings fix or incompatibility.

The [derivative-reader run40833645412](https://github.com/ressl/ferriprint/actions/runs/40833645412)
used a separately built, explicitly patched SQLite3.50.2 QA reader, not a changed
published frontend or official release artifact. Its first protected read-only
count again failed with code15 before frontend stop or native Add. Fixed counters
recorded100 WAL-index mapping `BUSY` retries and one protocol-limit return; the
other10 counters were zero. These locate this reader's mapping branch, not a
competing lock holder, syscall errno, OS cause or native fix. The final frontend
ledger remained unknown and the fixture was preserved.

The [existing-call run40840342606](https://github.com/ressl/ferriprint/actions/runs/40840342606)
narrowed this QA reader to101 successful `F_GETLK` calls followed by the returned
write-lock-type guard, with100 mapping retries and one protocol-limit return.
The first returned-structure PID was0, explicitly unverified; neither targeted
advisory-failure counter advanced. This is not proof of an output-field write,
blocker identity or OS cause. The run again stopped before frontend stop/native
Add and preserved its unknown final ledger, without replacing the native failure.

The [output-sentinel run40844399095](https://github.com/ressl/ferriprint/actions/runs/40844399095)
initialized only the output-only PID field to-2 before the same call. Its first
recorded returned PID also equaled-2 and the write-lock-type guard still failed
the protected snapshot. Equality alone identifies neither a writer layer nor a
real holder. This bounded call chain has not supplied a trustworthy conflict
report usable for the protected observer; a conformant existing locking/output
path on an authorized environment is required without bypassing the corruption
guard. This is not a blanket Windows/WSL incompatibility or native-pass claim.
No frontend stop/native Add occurred and the unknown final ledger was preserved.

The physical Mac's missing generated-PPD `APPrinterPreset` entries did not prevent
those three recipes appearing. Its observed A4/disabled-destination limitation
remains explicit: recipe visibility or a changed quality control is not a
complete A3 media/job-ticket result. Cleanup restored the checked printing-domain
exports, queue URIs, defaults and options; it did not prove every volatile OS
preference unchanged.

The hosted Mac's immutable report retains `PASS_NATIVE_CLI` / `GUI_NOT_PROVEN`.
Separately, raw own-process events identified a visible `NSPanel` equal to the
actual `NSApp.modalWindow`, with the supplied `NSPrintInfo` identity. Its modal
flag serialized as integer1, while the summary required strict booleantrue.
That scoped modal observation is not full preset/A3-GUI acceptance: the recorded
paper name remained `na-letter`. Actual frontend/native job state, spool and
refused-operation counters were0; the owned queue/state/processes were removed,
both listener ports were closed, and other queues/defaults were preserved.

At the paper-diagnostic checkpoint, unknown tray counts (`-2`) remained unknown.
Ready-media metadata was not proof of loaded stock. `media-empty-report` was
informational/nonblocking while the printer/native queue were idle/accepting;
a reported empty rear source did not establish that all sources were empty.
No reason was reset/filtered and no physical stock or output was measured.

## Clients and ownership

**Historical0.2 client checkpoint, 2026-10-27.** The observations below keep
their original source, submission/cancellation and cleanup scope; they are not
the post-release zero-submission exercises above.

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

These earlier inventory observations are dated evidence, not new measurements
of the current physical ink or paper supply.

| Item | Actual known observation | Honesty requirement |
|---|---|---|
| Epson ET-16600 native CUPS/vendor PPD | Private installed PPD/driver; real native metadata inspection and reviewed setup install resolved 34 numeric media variants and explicit A3 borderless/high mappings | Not bundled; no claim every model/PPD is supported |
| Numeric media and quality | Native Get/Validate and setup previews accepted explicit A3 dimensions/zero margins/high quality; unsupported mappings are rejected | Mapping/Validate acceptance is not measured edge coverage or physical quality |
| Native queue ink markers | Verified IPPS queue actually returned Black/Cyan/Magenta/Yellow ink types, levels26/56/55/56, low15/high100 during inventory | Identify native queue source and its marker-change time; fetched now does not mean physically measured now |
| Native queue paper/status | Queue reported Letter/Legal media-ready and media-empty-report; no physical tray count observed | Queue metadata is not sensor-confirmed loaded paper; do not invent tray contents/percentages |
| Direct-device TLS/SNMP | No new direct-device trust or SNMP source was needed for existing queue-provided markers | Never bypass TLS or invent a community; unavailable readings stay unknown |
| Profile declared-ready | Operator declaration only | Never label it a live physical tray sensor |

## Final release evidence

**Historical0.2 release checkpoint, 2026-10-27.** Its native amd64 quality gate
included55 tests; that is not a0.3 test count. Current0.3 publication evidence is
linked above.

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

At the historical pre-publication checkpoint,0.3 candidate
`fe2611a421241ea3501ee84a2537a95e04619dc1` was built and exercised on two
independent native Docker engines:
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

At that earlier pre-publication checkpoint, the real Mac's native print-dialog/
preset-visibility gate remained unverified: the authorized session was locked
and no usable window was observed. No unlock or permission bypass was attempted.
That was not an unsupported-platform finding or a general inference from a lock
flag. The later actual unlocked Default/Add/Use and panel observations above
supersede that session-access blocker, with their separate A4/recipe limitation.

### Current remaining coverage prerequisites (2026-10-28)

| Additional requested target | Actual inventory/access evidence | Remaining acceptance prerequisite |
|---|---|---|
| A second physical Mac | An independent hosted native Mac VM passed the scoped check above. An existing private Darwin runner's matching online/idle metadata did not produce an assigned metadata job or establish usable capacity | Separately identified physical hardware and an authorized native session; no trusted execution/host-identity channel was established for that runner. Do not infer offline lifecycle from its label/contact age |
| Real iPhone | Bounded connected-device enumeration found no usable attached device; no authorized mirroring/physical native print UI/model/OS session was available | An already trusted accessible real iPhone native print session; no pairing, permission or app change was made |
| Real iPad | No usable attached iPad or authorized physical native print session was observed | An already trusted accessible real iPad session, separately from iPhone; iPhone Mirroring and simulators do not qualify |
| Windows native capabilities/settings | Real runtime/HTTP roles and automatic Microsoft IPP Class Driver Add passed; full native settings failed. The later protected observer stopped at an unusable/unverified GETLK conflict-report boundary before frontend stop or native Add | This observation path requires an authorized environment/call chain with trustworthy conformant locking output, without bypassing the safety guard. The original missing A3/quality/duplex/A4-border settings and driver-response evidence remain unresolved; the direct GPA is not the driver's response. No blanket Windows/WSL incompatibility or native pass is inferred |
| More real printer models / measured paper quality | The three real native destinations/backend paths are one Epson ET-16600; synthetic fixtures do not add models and unknown tray counts are not measured stock | Another identified model with its preexisting queue/driver; physical output needs separate approval, actual known stock and an available paper/ruler/scan handling path |

These are scoped evidence prerequisites, not permanent claims that platforms
are unreachable or incompatible. Hosted VMs do not become physical devices.
Post-release fixtures added only owned temporary queues and restored their
checked state; no physical-page, pairing or permission authority was introduced.
Private device identities, endpoints, credentials, vendor PPD bytes and
operator/job captures remain outside public release material.
