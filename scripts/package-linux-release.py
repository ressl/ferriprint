#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 Robert Ressl
"""Package an exact committed Linux amd64 build; never configure/start a service."""
import argparse
import gzip
import hashlib
from html.parser import HTMLParser
import io
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import subprocess
import tarfile
import tempfile
import tomllib


def digest(path):
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


class NoticeText(HTMLParser):
    """Keep all upstream HTML text in the Debian copyright notice."""

    def __init__(self):
        super().__init__(convert_charrefs=True)
        self.parts = []

    def handle_data(self, data):
        self.parts.append(data)

    def handle_starttag(self, tag, attrs):
        if tag in {"br", "p", "pre", "li", "tr", "td", "th", "h1", "h2", "h3", "summary"}:
            self.parts.append("\n")


def archive_tree(root, output, prefix, epoch):
    with output.open("xb") as raw:
        with gzip.GzipFile(filename="", fileobj=raw, mode="wb", mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w") as archive:
                for path in sorted(root.rglob("*")):
                    if path.is_symlink() or not path.is_file():
                        continue
                    data = path.read_bytes()
                    info = tarfile.TarInfo(prefix + path.relative_to(root).as_posix())
                    info.size = len(data)
                    info.mode = 0o755 if os.access(path, os.X_OK) else 0o644
                    info.uid = info.gid = 0
                    info.mtime = epoch
                    archive.addfile(info, io.BytesIO(data))


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("commit")
    parser.add_argument("build_directory", type=Path)
    parser.add_argument("output_directory", type=Path)
    args = parser.parse_args()
    if not re.fullmatch(r"[0-9a-f]{40}", args.commit):
        parser.error("full commit required")
    repository = Path(__file__).resolve().parents[1]
    tree = subprocess.check_output(["git", "archive", "--format=tar", args.commit], cwd=repository)
    epoch = int(subprocess.check_output(["git", "show", "-s", "--format=%ct", args.commit], cwd=repository))
    build = args.build_directory.resolve()
    binary = build / "ferriprint"
    manifest = (build / "build-manifest.txt").read_text()
    if f"commit={args.commit}\n" not in manifest or digest(binary) not in manifest:
        raise SystemExit("build manifest commit/binary checksum mismatch")
    with binary.open("rb") as executable:
        header = executable.read(20)
    if header[:5] != b"\x7fELF\x02" or int.from_bytes(header[18:20], "little") != 62:
        raise SystemExit("package target must be ELF64 Linux x86_64")
    for required in ["ferriprint-source.tar.gz", "ferriprint.spdx.json"]:
        if not (build / required).is_file():
            raise SystemExit(f"missing release input: {required}")
    subprocess.run(["python3", str(repository / "scripts/verify-source.py"), args.commit, str(build / "ferriprint-source.tar.gz")], cwd=repository, check=True)
    args.output_directory.mkdir(parents=True, exist_ok=True)
    out = args.output_directory.resolve()
    if any(out.iterdir()):
        raise SystemExit("release output directory must be empty; refusing existing checksums or unrelated files")
    with tempfile.TemporaryDirectory(prefix="ferriprint-package-") as temporary:
        work = Path(temporary)
        source = work / "source"
        source.mkdir()
        with tarfile.open(fileobj=io.BytesIO(tree), mode="r:") as archive:
            # The pinned Bookworm builder has Python3.11 without extraction
            # filters. Git input needs only regular files/directories: reject
            # traversal and every link/special member before extracting any.
            members = archive.getmembers()
            for member in members:
                relative = PurePosixPath(member.name)
                if (
                    not relative.parts
                    or relative.is_absolute()
                    or ".." in relative.parts
                    or not (member.isfile() or member.isdir())
                ):
                    raise SystemExit(f"unsafe source archive member: {member.name!r}")
            archive.extractall(source, members=members)
        package = tomllib.loads((source / "Cargo.toml").read_text())["package"]
        version = package["version"]
        maintainer = package["authors"][0]
        if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+(?:-[0-9A-Za-z.-]+)?", version):
            raise SystemExit("unsupported package version")
        stem = f"ferriprint-{version}-linux-amd64"
        stage = work / "tar"
        (stage / "bin").mkdir(parents=True)
        shutil.copy2(binary, stage / "bin/ferriprint")
        (stage / "lib/systemd/system").mkdir(parents=True)
        shutil.copy2(source / "install/ferriprint.service", stage / "lib/systemd/system/ferriprint.service")
        (stage / "share/man/man1").mkdir(parents=True)
        shutil.copy2(source / "install/ferriprint.1", stage / "share/man/man1/ferriprint.1")
        documentation = stage / "share/doc/ferriprint"
        documentation.mkdir(parents=True)
        for name in ["LICENSE", "NOTICE", "THIRD_PARTY_NOTICES.md", "README.md", "CHANGELOG.md", "SECURITY.md"]:
            shutil.copy2(source / name, documentation / name)
        shutil.copy2(source / "install/RUST-STDLIB-NOTICES.html", documentation / "RUST-STDLIB-NOTICES.html")
        shutil.copytree(source / "docs", documentation / "docs")
        (documentation / "examples").mkdir()
        for example in (source / "examples").rglob("*.toml"):
            destination = documentation / "examples" / example.relative_to(source / "examples")
            destination.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(example, destination)
        shutil.copy2(source / "tests/fixtures/native.ppd", documentation / "examples/native.ppd")
        for name in ["build-manifest.txt", "ferriprint.spdx.json"]:
            shutil.copy2(build / name, documentation / name)
        runtime_text = NoticeText()
        runtime_text.feed((source / "install/RUST-STDLIB-NOTICES.html").read_text())
        runtime_text.close()
        # Debian/slim intentionally filters most documentation but preserves this
        # standard pathname. Keep complete licensing here as well as raw files.
        (documentation / "copyright").write_text(
            "Ferriprint project and dependency copyright/license notices\n\n"
            + (source / "NOTICE").read_text() + "\n\n"
            + (source / "LICENSE").read_text() + "\n\n"
            + (source / "THIRD_PARTY_NOTICES.md").read_text()
            + "\n\nRust standard-library notices (text rendering of the verbatim "
            "RUST-STDLIB-NOTICES.html supplied in this package/source):\n\n"
            + "".join(runtime_text.parts)
        )
        archive_tree(stage, out / f"{stem}.tar.gz", f"{stem}/", epoch)
        deb = work / "deb"
        shutil.copytree(stage / "share", deb / "usr/share")
        (deb / "usr/bin").mkdir(parents=True)
        shutil.copy2(binary, deb / "usr/bin/ferriprint")
        (deb / "usr/lib/systemd/system").mkdir(parents=True)
        shutil.copy2(source / "install/ferriprint.service", deb / "usr/lib/systemd/system/ferriprint.service")
        control = deb / "DEBIAN"
        control.mkdir()
        debversion = version.replace("-", "~", 1) + "-1"
        installed_size = sum(path.stat().st_size for path in deb.rglob("*") if path.is_file()) // 1024
        (control / "control").write_text(f"""Package: ferriprint
Version: {debversion}
Section: net
Priority: optional
Architecture: amd64
Maintainer: {maintainer}
Depends: libc6 (>= 2.36), libgcc-s1, ca-certificates, adduser
Suggests: avahi-daemon
Installed-Size: {installed_size}
Homepage: https://github.com/ressl/ferriprint
Description: secure IPP frontend for an existing native CUPS queue
 Self-contained Rust executable; a working local or remote native CUPS queue
 and its operator-installed driver remain required. Installation never
 configures a queue, enables or starts the service.
""")
        postinst = control / "postinst"
        postinst.write_text('''#!/bin/sh
set -eu
if [ "$1" = configure ]; then
  if getent passwd ferriprint >/dev/null && ! getent -s files passwd ferriprint >/dev/null; then
    printf '%s\\n' 'Refusing an NSS-only/DynamicUser identity; migrate explicitly while stopped.' >&2
    exit 1
  fi
  if getent group ferriprint >/dev/null && ! getent -s files group ferriprint >/dev/null; then
    printf '%s\\n' 'Refusing an NSS-only group; configure a reviewed local service group.' >&2
    exit 1
  fi
  if ! getent group ferriprint >/dev/null; then
    addgroup --system ferriprint
  fi
  if ! getent passwd ferriprint >/dev/null; then
    adduser --system --ingroup ferriprint --home /var/lib/ferriprint --no-create-home --shell /usr/sbin/nologin ferriprint
  fi
  install -d -m 0750 -o root -g ferriprint /etc/ferriprint
  if [ ! -e /var/lib/ferriprint ]; then
    install -d -m 0700 -o ferriprint -g ferriprint /var/lib/ferriprint
  fi
  if [ -d /run/systemd/system ]; then systemctl daemon-reload; fi
  printf '%s\\n' 'Ferriprint installation did not enable, start or restart the service. Configure/check first; an existing process keeps its old binary until an explicitly controlled restart.'
fi
''')
        postinst.chmod(0o755)
        prerm = control / "prerm"
        prerm.write_text('''#!/bin/sh
set -eu
case "$1" in
  remove|deconfigure)
    if [ -d /run/systemd/system ] && systemctl is-active --quiet ferriprint.service; then
      printf '%s\\n' 'Stop Ferriprint gracefully after checking active jobs before removing the package.' >&2
      exit 1
    fi
    ;;
esac
''')
        prerm.chmod(0o755)
        postrm = control / "postrm"
        postrm.write_text('''#!/bin/sh
set -eu
if [ -d /run/systemd/system ]; then systemctl daemon-reload; fi
# Deliberately retain operator configuration, credentials, user and durable jobs.
''')
        postrm.chmod(0o755)
        environment = dict(os.environ, SOURCE_DATE_EPOCH=str(epoch))
        subprocess.run(["dpkg-deb", "--root-owner-group", "--build", str(deb), str(out / f"ferriprint_{debversion}_amd64.deb")], env=environment, check=True)
        for source_name, target_name in [("ferriprint-source.tar.gz", f"ferriprint-{version}-source.tar.gz"), ("ferriprint.spdx.json", f"ferriprint-{version}.spdx.json"), ("build-manifest.txt", f"ferriprint-{version}-build-manifest.txt")]:
            shutil.copy2(build / source_name, out / target_name)
        assets = sorted(path for path in out.iterdir() if path.is_file())
        (out / "SHA256SUMS").write_text("".join(f"{digest(path)}  {path.name}\n" for path in assets))
        print(json.dumps({"version": version, "commit": args.commit, "architecture": "amd64", "assets": [path.name for path in assets]}, indent=2))


if __name__ == "__main__":
    main()
