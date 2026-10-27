#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 Robert Ressl
"""Compare the binary-served /source archive to every file in an exact git tree."""
import argparse
import hashlib
from pathlib import PurePosixPath
import re
import subprocess
import tarfile


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("commit")
    parser.add_argument("archive")
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.commit):
        parser.error("full commit required")
    entries = subprocess.check_output(["git", "ls-tree", "-rz", args.commit]).split(b"\0")
    expected = {}
    for entry in entries:
        if not entry:
            continue
        metadata, name = entry.split(b"\t", 1)
        mode, kind, object_id = metadata.split()
        if kind != b"blob" or mode not in {b"100644", b"100755"}:
            raise SystemExit(f"unsupported public tree entry: {name!r}")
        expected["ferriprint/" + name.decode()] = object_id.decode()
    with tarfile.open(args.archive, "r:gz") as archive:
        members = archive.getmembers()
        actual = [member.name for member in members]
        if len(actual) != len(set(actual)):
            raise SystemExit("duplicate source archive member")
        if set(actual) != set(expected):
            raise SystemExit(f"source membership mismatch: missing={sorted(set(expected)-set(actual))} extra={sorted(set(actual)-set(expected))}")
        for member in members:
            if not member.isfile() or PurePosixPath(member.name).is_absolute() or ".." in PurePosixPath(member.name).parts:
                raise SystemExit(f"unsafe source member: {member.name}")
            data = archive.extractfile(member).read()
            wanted = subprocess.check_output(["git", "cat-file", "blob", expected[member.name]])
            if data != wanted:
                raise SystemExit(f"source bytes mismatch: {member.name}")
    with open(args.archive, "rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    print(f"source_commit={args.commit} members={len(expected)} sha256={digest} byte_for_byte=PASS")


if __name__ == "__main__":
    main()
