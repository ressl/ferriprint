#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 Robert Ressl
"""Extract the unique embedded AGPL source gzip without running the executable."""
import argparse
import io
from pathlib import Path
import tarfile
import zlib


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    data = args.binary.read_bytes()
    candidates = []
    offset = 0
    while True:
        offset = data.find(b"\x1f\x8b\x08", offset)
        if offset < 0:
            break
        try:
            decompressor = zlib.decompressobj(16 + zlib.MAX_WBITS)
            decoded = decompressor.decompress(data[offset:], 32 * 1024 * 1024)
            if decompressor.eof:
                with tarfile.open(fileobj=io.BytesIO(decoded), mode="r:") as archive:
                    members = archive.getmembers()
                    if members and all(member.isfile() and member.name.startswith("ferriprint/") for member in members) and any(member.name == "ferriprint/Cargo.toml" for member in members):
                        end = len(data) - len(decompressor.unused_data)
                        candidates.append(data[offset:end])
        except (zlib.error, tarfile.TarError, ValueError):
            pass
        offset += 3
    if len(candidates) != 1:
        raise SystemExit(f"expected one embedded source archive, found {len(candidates)}")
    with args.output.open("xb") as output:
        output.write(candidates[0])
    print(f"extracted embedded source archive: {args.output} ({len(candidates[0])} bytes)")


if __name__ == "__main__":
    main()
