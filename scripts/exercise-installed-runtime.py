#!/usr/bin/env python3
# SPDX-License-Identifier: AGPL-3.0-only
# Copyright (C) 2026 Robert Ressl
"""Exercise an installed 0.3 executable as its service UID, only against loopback."""
import argparse
import base64
import hashlib
import http.client
import json
import os
from pathlib import Path
import platform
import pwd
import re
import secrets
import signal
import socket
import struct
import subprocess
import tempfile
import time
import tomllib


FRONTEND_PORT = 18531
RECEIVER_PORT = 18631
FRONTEND_URI = f"ipp://127.0.0.1:{FRONTEND_PORT}/ipp/print/demo"
RECEIVER_URI = f"ipp://127.0.0.1:{RECEIVER_PORT}/printers/receiver"


def require(condition, message):
    if not condition:
        raise RuntimeError(message)


def elf_machine(path):
    with path.open("rb") as stream:
        header = stream.read(20)
    require(header[:6] == b"\x7fELF\x02\x01", "native smoke requires little-endian ELF64")
    return int.from_bytes(header[18:20], "little")


def attribute(tag, name, value):
    values = value if isinstance(value, list) else [value]
    result = bytearray()
    for index, item in enumerate(values):
        key = name.encode() if index == 0 else b""
        if tag in {0x21, 0x23}:
            encoded = struct.pack(">i", item)
        elif tag == 0x22:
            encoded = bytes([int(item)])
        else:
            encoded = item.encode()
        result.extend(bytes([tag]) + struct.pack(">H", len(key)) + key + struct.pack(">H", len(encoded)) + encoded)
    return result


def response_attributes(body, request_id):
    require(len(body) >= 9, "truncated IPP response")
    status, actual_id = struct.unpack(">HI", body[2:8])
    require(actual_id == request_id, "IPP response request ID mismatch")
    require(status < 0x100, f"IPP operation rejected: 0x{status:04x}")
    attributes = {}
    offset = 8
    name = ""
    while offset < len(body):
        tag = body[offset]
        offset += 1
        if tag == 3:
            return attributes
        if tag < 16:
            name = ""
            continue
        require(offset + 2 <= len(body), "truncated IPP attribute name")
        size = struct.unpack(">H", body[offset:offset + 2])[0]
        offset += 2
        require(offset + size + 2 <= len(body), "truncated IPP attribute")
        if size:
            name = body[offset:offset + size].decode("utf-8")
        offset += size
        length = struct.unpack(">H", body[offset:offset + 2])[0]
        offset += 2
        require(offset + length <= len(body), "truncated IPP value")
        value = body[offset:offset + length]
        offset += length
        if tag in {0x21, 0x23} and length == 4:
            value = struct.unpack(">i", value)[0]
        elif tag == 0x22 and length == 1:
            value = bool(value[0])
        elif 0x41 <= tag <= 0x49:
            value = value.decode("utf-8")
        attributes.setdefault(name, []).append(value)
    raise RuntimeError("IPP response omitted end-of-attributes")


class Client:
    def __init__(self, password):
        self.authorization = "Basic " + base64.b64encode(("admin:" + password).encode()).decode()
        self.sequence = 0

    def request(self, port, method, path, body=None, authenticated=True):
        connection = http.client.HTTPConnection("127.0.0.1", port, timeout=5)
        headers = {}
        if authenticated:
            headers["Authorization"] = self.authorization
        if method == "POST":
            headers["Content-Type"] = "application/ipp"
        try:
            connection.request(method, path, body=body, headers=headers)
            response = connection.getresponse()
            data = response.read(32 * 1024 * 1024 + 1)
            require(response.status == 200, f"loopback {method} {path} returned HTTP {response.status}")
            require(len(data) <= 32 * 1024 * 1024, "unbounded loopback response")
            return data
        finally:
            connection.close()

    def ipp(self, operation, job_id=None, document=None, upstream=False):
        self.sequence += 1
        uri = RECEIVER_URI if upstream else FRONTEND_URI
        operations = [(0x47, "attributes-charset", "utf-8"), (0x48, "attributes-natural-language", "en"),
                      (0x45, "printer-uri", uri), (0x42, "requesting-user-name", "admin")]
        if job_id is not None:
            operations.append((0x21, "job-id", job_id))
        if operation in {2, 4}:
            operations.extend([(0x49, "document-format", "application/pdf"), (0x42, "job-name", "native package nonprinting acceptance")])
        if operation == 11:
            operations.append((0x44, "requested-attributes", ["printer-name", "printer-state", "document-format-supported"]))
        body = struct.pack(">BBHI", 2, 0, operation, self.sequence) + b"\x01"
        body += b"".join(attribute(*item) for item in operations) + b"\x03"
        if document is not None:
            body += document
        port, path = (RECEIVER_PORT, "/printers/receiver") if upstream else (FRONTEND_PORT, "/ipp/print/demo")
        return response_attributes(self.request(port, "POST", path, body, authenticated=not upstream), self.sequence)


def wait_for(predicate, phase, process=None):
    deadline = time.monotonic() + 30
    while time.monotonic() < deadline:
        if process is not None:
            require(process.poll() is None, f"{phase}: owned process exited ({process.returncode})")
        try:
            if predicate():
                return
        except (ConnectionError, OSError, http.client.HTTPException):
            pass
        time.sleep(0.1)
    raise RuntimeError(f"{phase}: bounded readiness/state deadline exceeded")


def command(binary, config, *arguments, password=None):
    result = subprocess.run([str(binary), "--config", str(config), *arguments],
                            input=None if password is None else (password + "\n").encode(),
                            stdout=subprocess.PIPE, stderr=subprocess.PIPE, timeout=45)
    require(result.returncode == 0, f"installed CLI {' '.join(arguments[:2])} failed ({result.returncode}); output withheld to protect credentials")


def stop(process, signum):
    if process is None:
        return b""
    if process.poll() is None:
        process.send_signal(signum)
    try:
        stdout, _ = process.communicate(timeout=20)
    except subprocess.TimeoutExpired:
        process.kill()
        process.communicate()
        raise RuntimeError("owned acceptance process failed to stop gracefully")
    require(process.returncode in {0, -signum}, f"owned acceptance process exited unexpectedly ({process.returncode})")
    return stdout or b""


def document():
    content = b"BT /F1 12 Tf 20 760 Td (Ferriprint NONPRINTING package acceptance) Tj ET\n"
    objects = [b"<< /Type /Catalog /Pages 2 0 R >>", b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
               b"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 5 0 R >> >> /Contents 4 0 R >>",
               b"<< /Length " + str(len(content)).encode() + b" >>\nstream\n" + content + b"endstream",
               b"<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>"]
    data = bytearray(b"%PDF-1.4\n")
    offsets = [0]
    for number, value in enumerate(objects, 1):
        offsets.append(len(data))
        data.extend(f"{number} 0 obj\n".encode() + value + b"\nendobj\n")
    xref = len(data)
    data.extend(f"xref\n0 {len(offsets)}\n0000000000 65535 f \n".encode())
    for offset in offsets[1:]:
        data.extend(f"{offset:010d} 00000 n \n".encode())
    data.extend(f"trailer\n<< /Size {len(offsets)} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode())
    return bytes(data)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    for name in ["binary", "receiver", "fixture", "profile", "source-archive"]:
        parser.add_argument("--" + name, type=Path, required=True)
    parser.add_argument("--installation", choices=["package", "tar"], required=True)
    args = parser.parse_args()
    require(os.getuid() != 0 and pwd.getpwuid(os.getuid()).pw_name == "ferriprint", "run the installed binary as the dedicated nonroot service UID")
    architecture = os.environ["FERRIPRINT_TEST_ARCHITECTURE"]
    expected_machine = {"amd64": 62, "arm64": 183}[architecture]
    require(elf_machine(args.binary) == expected_machine and elf_machine(args.receiver) == expected_machine,
            "installed executable/QA receiver ELF machine differs from native package architecture")
    require(platform.machine() == {"amd64": "x86_64", "arm64": "aarch64"}[architecture], "native kernel machine differs from package")
    for port in [FRONTEND_PORT, RECEIVER_PORT]:
        with socket.socket() as probe:
            probe.bind(("127.0.0.1", port))
    password = secrets.token_urlsafe(32)
    client = Client(password)
    pdf = document()
    frontend = receiver = None
    with tempfile.TemporaryDirectory(prefix="ferriprint-installed-runtime-") as temporary:
        root = Path(temporary)
        state = root / "state"
        state.mkdir(mode=0o700)
        config = root / "ferriprint.toml"
        config.write_text(f'listen = "127.0.0.1:{FRONTEND_PORT}"\npublic_url = "ipp://127.0.0.1:{FRONTEND_PORT}/"\nstate_dir = "{state}"\npoll_interval_ms = 100\nbackend_timeout_secs = 5\n[[cups_sources]]\nid = "isolated"\nserver_uri = "ipp://127.0.0.1:{RECEIVER_PORT}/"\nusername = "ferriprint"\n')
        config.chmod(0o600)
        choices = root / "choices.toml"
        lines = ["confirmed_native_mappings = true", "[settings]"]
        for line in args.profile.read_text().splitlines():
            match = re.fullmatch(r"(\[\[?)([^\]\n]+)(\]\]?)", line)
            lines.append(match[1] + "settings." + match[2] + match[3] if match else line)
        choices.write_text("\n".join(lines) + "\n")
        choices.chmod(0o600)
        try:
            receiver = subprocess.Popen([str(args.receiver)], cwd=root, stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            wait_for(lambda: client.ipp(11, upstream=True), "native receiver GPA", receiver)
            command(args.binary, config, "user", "init", "--username", "admin", password=password)
            command(args.binary, config, "setup", "install", "--source", "isolated", "--queue", "receiver",
                    "--printer", "demo", "--display-name", "Ferriprint package acceptance",
                    "--service-name", "Ferriprint package acceptance", "--location", "NONPRINTING loopback receiver",
                    "--choices", str(choices), "--confirm")
            for path in [state / "access.toml", state / "printers.toml", state / "profiles/demo.toml"]:
                require(path.is_file() and path.stat().st_uid == os.getuid() and path.stat().st_mode & 0o077 == 0,
                        "CLI-managed files must be service-owned and private")
            catalog = tomllib.loads((state / "printers.toml").read_text())
            require(len(catalog["printers"]) == 1 and catalog["printers"][0]["upstream_uri"] == RECEIVER_URI,
                    "actual CLI catalog did not retain the sole literal loopback receiver")
            ppd = Path(catalog["printers"][0]["ppd"]).resolve()
            require(ppd.is_relative_to(state.resolve()) and ppd.read_bytes() == args.fixture.read_bytes(),
                    "CLI must privately acquire the exact synthetic PPD through the trusted receiver")
            command(args.binary, config, "check")
            frontend = subprocess.Popen([str(args.binary), "--config", str(config), "serve"], cwd=root,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            wait_for(lambda: client.ipp(11), "installed frontend GPA", frontend)
            client.ipp(4)
            source = client.request(FRONTEND_PORT, "GET", "/source", authenticated=False)
            source_hash = hashlib.sha256(source).hexdigest()
            require(source == args.source_archive.read_bytes(), "installed HTTP /source differs from the exact release source asset")
            submitted = client.ipp(2, document=pdf)
            job_id = submitted["job-id"][0]
            wait_for(lambda: client.ipp(9, job_id)["job-state"][0] == 5, "frontend actual submission", frontend)
            require(client.ipp(9, 700, upstream=True)["job-state"][0] == 5, "known upstream job700 must really be processing")
            client.ipp(8, job_id)
            wait_for(lambda: client.ipp(9, 700, upstream=True)["job-state"][0] == 7, "actual upstream cancellation", receiver)
            wait_for(lambda: client.ipp(9, job_id)["job-state"][0] == 7, "durable frontend cancellation", frontend)
            stop(frontend, signal.SIGTERM)
            frontend = None
            frontend = subprocess.Popen([str(args.binary), "--config", str(config), "serve"], cwd=root,
                                        stdout=subprocess.PIPE, stderr=subprocess.PIPE)
            wait_for(lambda: client.ipp(9, job_id)["job-state"][0] == 7, "installed durable restart", frontend)
            require(client.ipp(9, 700, upstream=True)["job-state"][0] == 7, "restart must not replay the canceled upstream job")
            stop(frontend, signal.SIGTERM)
            frontend = None
            ledger = stop(receiver, signal.SIGINT).decode()
            receiver = None
            jobs = [line for line in ledger.splitlines() if line.startswith("job=")]
            require(len(jobs) == 1 and jobs[0].startswith(f"job=700 format=application/pdf bytes={len(pdf)} native="),
                    "graceful real receiver ledger must contain exactly the one submitted document")
            settings = tomllib.loads(args.profile.read_text())
            defaults = settings["defaults"]
            native = {"PageSize": "A4", "InputSlot": settings["sources"][defaults["source"]]}
            native.update(settings["media_types"][defaults["media_type"]][{3: "draft", 4: "normal", 5: "high"}[defaults["quality"]]])
            native.update(settings["color_modes"][defaults["color_mode"]])
            native.update(settings["sides"][defaults["sides"]])
            require(all(json.dumps(key) + ": " + json.dumps(value) in jobs[0] for key, value in native.items()),
                    "actual receiver ledger differs from the reviewed synthetic native defaults")
            print(f"installed_runtime=PASS installation={args.installation} architecture={architecture} machine={platform.machine()} elf_machine={expected_machine} uid={os.getuid()} gid={os.getgid()} managed_cli_init_catalog=PASS offline_check=PASS GPA=PASS Validate=PASS local_job={job_id} upstream_job=700 upstream_canceled=PASS durable_restart=PASS receiver_ledger_jobs=1 source_sha256={source_hash} document_sha256={hashlib.sha256(pdf).hexdigest()} physical_prints=0")
        finally:
            try:
                stop(frontend, signal.SIGTERM)
            finally:
                stop(receiver, signal.SIGINT)


if __name__ == "__main__":
    main()
