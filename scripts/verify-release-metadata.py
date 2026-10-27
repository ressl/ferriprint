#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 Robert Ressl
"""Validate SPDX2.3 against its pinned official schema and the exact source lock.

QA prerequisite: python3-jsonschema (not a Ferriprint runtime dependency).
Run from a source checkout or the byte-verified corresponding-source archive.
"""
import argparse
import hashlib
import json
from pathlib import Path
import re
import tomllib
from urllib.parse import quote
from urllib.request import urlopen

import jsonschema

SCHEMA_URL = "https://raw.githubusercontent.com/spdx/spdx-spec/v2.3/schemas/spdx-schema.json"
SCHEMA_CANONICAL_SHA256 = "2db834e9c0d0c937dd1d9cb1700a76f8f380ff64eec7f8b6d8d1af0881986277"


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("commit")
    parser.add_argument("sbom", type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.commit):
        parser.error("full commit required")
    with urlopen(SCHEMA_URL, timeout=20) as response:
        raw = response.read(1024 * 1024 + 1)
    if len(raw) > 1024 * 1024:
        raise SystemExit("SPDX schema exceeds bounded download size")
    schema = json.loads(raw)
    canonical = json.dumps(schema, sort_keys=True, separators=(",", ":")).encode()
    if hashlib.sha256(canonical).hexdigest() != SCHEMA_CANONICAL_SHA256:
        raise SystemExit("official SPDX schema content changed; review before updating the pin")
    document = json.loads(args.sbom.read_text())
    jsonschema.Draft7Validator(schema).validate(document)
    repository = Path(__file__).resolve().parents[1]
    root = tomllib.loads((repository / "Cargo.toml").read_text())["package"]
    lock = tomllib.loads((repository / "Cargo.lock").read_text())
    locked = {(p["name"], p["version"]): p["checksum"] for p in lock["package"] if "source" in p}
    expected_namespace = f"https://github.com/ressl/ferriprint/spdx/{args.commit}"
    if document["documentNamespace"] != expected_namespace or document["spdxVersion"] != "SPDX-2.3":
        raise SystemExit("SBOM version/source namespace mismatch")
    packages = document["packages"]
    ids = [p["SPDXID"] for p in packages]
    if len(ids) != len(set(ids)) or any(not re.fullmatch(r"SPDXRef-[A-Za-z0-9.-]+", item) for item in ids):
        raise SystemExit("invalid or duplicate SPDX package identifier")
    root_packages = [p for p in packages if p["SPDXID"] == "SPDXRef-Ferriprint"]
    if len(root_packages) != 1 or root_packages[0]["versionInfo"] != root["version"] or root_packages[0]["downloadLocation"] != f"https://github.com/ressl/ferriprint/tree/{args.commit}":
        raise SystemExit("root package/source identity mismatch")
    dependencies = [p for p in packages if p["SPDXID"] != "SPDXRef-Ferriprint"]
    declared = {(p["name"], p["versionInfo"]): p for p in dependencies}
    if len(declared) != len(dependencies) or set(declared) != set(locked):
        raise SystemExit("SPDX dependency membership differs from the complete Cargo.lock registry graph")
    for (name, version), checksum in locked.items():
        package = declared[(name, version)]
        if {"algorithm": "SHA256", "checksumValue": checksum} not in package.get("checksums", []):
            raise SystemExit(f"crate checksum mismatch: {name} {version}")
        purl = f"pkg:cargo/{name}@{quote(version, safe='')}"
        if not any(ref.get("referenceType") == "purl" and ref.get("referenceLocator") == purl for ref in package.get("externalRefs", [])):
            raise SystemExit(f"crate package URL mismatch: {name} {version}")
    print(f"spdx_schema=PASS locked_packages={len(locked)} commit={args.commit} complete_inventory=PASS")


if __name__ == "__main__":
    main()
