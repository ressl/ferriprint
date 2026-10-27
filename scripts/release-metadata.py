#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 Robert Ressl
"""Generate lock-complete notices and SPDX from fetched, immutable Cargo packages.

Run in the build container after `cargo metadata --locked --format-version 1`.
The SBOM describes the complete lock graph (including build/dev/platform-only
packages), not a claim that every package is linked into the executable.
"""
import argparse
from datetime import datetime, timezone
import hashlib
import json
from pathlib import Path
import re
import subprocess
import tomllib
from urllib.parse import quote


def licenses(package):
    root = Path(package["manifest_path"]).parent
    explicit = package.get("license_file")
    found = []
    for entry in sorted(root.rglob("*")):
        if not entry.is_file() or entry.is_symlink():
            continue
        relative = entry.relative_to(root)
        if len(relative.parts) > 3 or any(part in {".git", "target", "tests", "test", "benches"} for part in relative.parts[:-1]):
            continue
        if entry.name.upper().startswith(("LICENSE", "LICENCE", "COPYING", "NOTICE", "COPYRIGHT")) or (explicit and relative.as_posix() == explicit):
            text = entry.read_text(encoding="utf-8", errors="strict")
            found.append((relative.as_posix(), text))
    if package["name"] == "libsqlite3-sys":
        for amalgamation in sorted(root.rglob("sqlite3.c")):
            with amalgamation.open(encoding="utf-8") as stream:
                header = stream.read(16384)
            marker = header.lower().find("disclaims copyright")
            if marker >= 0:
                start = header.rfind("/*", 0, marker)
                end = header.find("*/", marker)
                if start >= 0 and end >= 0:
                    found.append((amalgamation.relative_to(root).as_posix() + " (public-domain blessing)", header[start:end + 2]))
    if not found:
        # Match the project's existing notice convention: report a real absence,
        # never invent a copyright owner or claim a license file was packaged.
        declaration = {
            "name": package["name"],
            "version": package["version"],
            "license": package["license"],
            "authors": package.get("authors", []),
            "repository": package.get("repository"),
            "notice": "No standalone LICENSE/LICENCE/COPYING/NOTICE/COPYRIGHT file is shipped in this exact Cargo package. The license expression is its package declaration; standard texts for those permissive licenses are reproduced elsewhere in this inventory.",
        }
        found.append(("Cargo package metadata (no standalone license file)", json.dumps(declaration, indent=2)))
    return found


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--metadata", type=Path, required=True)
    parser.add_argument("--commit", required=True)
    parser.add_argument("--source-date-epoch", required=True, type=int)
    parser.add_argument("--notices", type=Path, required=True)
    parser.add_argument("--sbom", type=Path, required=True)
    parser.add_argument("--check-notices", action="store_true")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.commit):
        parser.error("--commit requires a full git commit")
    sysroot = Path(subprocess.check_output(["rustc", "--print", "sysroot"], text=True).strip())
    runtime_notice = sysroot / "share/doc/rust/COPYRIGHT-library.html"
    supplied_notice = Path(__file__).resolve().parents[1] / "install/RUST-STDLIB-NOTICES.html"
    with runtime_notice.open("rb") as stream:
        runtime_digest = hashlib.file_digest(stream, "sha256").hexdigest()
    with supplied_notice.open("rb") as stream:
        supplied_digest = hashlib.file_digest(stream, "sha256").hexdigest()
    if runtime_digest != supplied_digest:
        raise SystemExit("Rust library notices differ from the actual compiler; copy the verbatim notice from the pinned release image before building")
    print(f"rust_library_notices_sha256={runtime_digest} matched_compiler=PASS")
    metadata = json.loads(args.metadata.read_text())
    lock = tomllib.loads(Path("Cargo.lock").read_text())
    by_key = {(p["name"], p["version"]): p for p in metadata["packages"]}
    packages = []
    for locked in lock["package"]:
        if "source" not in locked:
            continue
        key = (locked["name"], locked["version"])
        if key not in by_key:
            raise SystemExit(f"Cargo metadata missing locked package: {key}")
        package = by_key[key]
        if not locked["source"].startswith("registry+") or not locked.get("checksum"):
            raise SystemExit(f"unreviewed non-registry dependency: {key}")
        packages.append((locked, package))
    packages.sort(key=lambda row: (row[0]["name"], row[0]["version"]))
    lines = ["# Third-Party Notices", "", "Ferriprint is AGPL-3.0-only; see LICENSE. This generated inventory covers every", "registry package in the committed Cargo.lock, including build, development and", "platform-specific dependencies. It is intentionally broader than the linked", "runtime subset. License expressions and verbatim license/copyright texts below", "come from the exact fetched Cargo packages and are checked against Cargo.lock.", "Dependencies are fetched at build time; this is not a vendored source bundle.", "", "PAPPL and pappl-retrofit are design references only; no code from them is bundled.", "", "## Locked dependency inventory", "", "| Package | Version | Declared license |", "| --- | --- | --- |"]
    for locked, package in packages:
        expression = package.get("license")
        if not expression:
            raise SystemExit(f"missing declared license: {locked['name']} {locked['version']}")
        lines.append(f"| [{locked['name']}](https://crates.io/crates/{locked['name']}/{locked['version']}) | {locked['version']} | {expression} |")
    lines.extend(["", "## Reproduced license and copyright notices", "", "Byte-identical texts are reproduced once and explicitly referenced below.", ""])
    spdx = []
    seen_texts = {}
    seen_ids = set()
    for locked, package in packages:
        name, version = locked["name"], locked["version"]
        identifier = "SPDXRef-" + re.sub(r"[^A-Za-z0-9.-]", "-", f"{name}-{version}")
        if identifier in seen_ids:
            raise SystemExit(f"duplicate normalized SPDX package identifier: {identifier}")
        seen_ids.add(identifier)
        lines.extend([f"### {name} {version}", ""])
        for filename, text in licenses(package):
            text_digest = hashlib.sha256(text.encode()).hexdigest()
            lines.extend([f"#### `{filename}`", ""])
            if text_digest in seen_texts:
                lines.extend([f"Identical to `{seen_texts[text_digest]}` above; not repeated.", ""])
            else:
                seen_texts[text_digest] = f"{name} {version} / {filename}"
                lines.extend(["````text", text.rstrip("\n"), "````", ""])
        purl = f"pkg:cargo/{name}@{quote(version, safe='')}"
        spdx.append({"SPDXID": identifier, "name": name, "versionInfo": version, "downloadLocation": f"https://static.crates.io/crates/{name}/{name}-{version}.crate", "checksums": [{"algorithm": "SHA256", "checksumValue": locked["checksum"]}], "filesAnalyzed": False, "licenseConcluded": "NOASSERTION", "licenseDeclared": package["license"].replace(" / ", " OR ").replace("/", " OR "), "copyrightText": "NOASSERTION", "externalRefs": [{"referenceCategory": "PACKAGE-MANAGER", "referenceType": "purl", "referenceLocator": purl}]})
    notices = "\n".join(lines)
    if args.check_notices:
        if args.notices.read_text() != notices:
            raise SystemExit("THIRD_PARTY_NOTICES.md does not match the locked fetched packages; regenerate before committing/building")
    else:
        args.notices.write_text(notices)
    root = tomllib.loads(Path("Cargo.toml").read_text())["package"]
    root_id = "SPDXRef-Ferriprint"
    spdx.insert(0, {"SPDXID": root_id, "name": "ferriprint", "versionInfo": root["version"], "downloadLocation": f"https://github.com/ressl/ferriprint/tree/{args.commit}", "filesAnalyzed": False, "licenseConcluded": "AGPL-3.0-only", "licenseDeclared": "AGPL-3.0-only", "copyrightText": "Copyright (C) 2026 Robert Ressl"})
    document = {
        "spdxVersion": "SPDX-2.3",
        "dataLicense": "CC0-1.0",
        "SPDXID": "SPDXRef-DOCUMENT",
        "name": f"ferriprint-{root['version']}-locked-dependencies",
        "documentNamespace": f"https://github.com/ressl/ferriprint/spdx/{args.commit}",
        "creationInfo": {
            "creators": ["Tool: ferriprint-release-metadata"],
            "created": datetime.fromtimestamp(args.source_date_epoch, timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
        },
        "comment": "Complete Cargo.lock registry inventory, including build/dev/platform-only packages; not an assertion of binary linkage or a toolchain/OS inventory. Crate archive checksums are Cargo.lock SHA-256. Verbatim Rust library runtime notices are distributed separately and checked against the release compiler. No proprietary driver or PPD is distributed.",
        "packages": spdx,
        "relationships": [{"spdxElementId": "SPDXRef-DOCUMENT", "relationshipType": "DESCRIBES", "relatedSpdxElement": root_id}]
        + [{"spdxElementId": root_id, "relationshipType": "DEPENDS_ON", "relatedSpdxElement": p["SPDXID"], "comment": "Member of the locked dependency graph; may be build/dev/platform-specific."} for p in spdx[1:]],
    }
    args.sbom.write_text(json.dumps(document, indent=2) + "\n")
    print(f"locked dependency packages={len(packages)} notices_sha256={hashlib.sha256(notices.encode()).hexdigest()}")


if __name__ == "__main__":
    main()
