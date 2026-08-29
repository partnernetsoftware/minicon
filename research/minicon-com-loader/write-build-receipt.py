#!/usr/bin/env python3
"""Emit dist/build-receipt.json after pack.sh. Tool versions + payload digests."""
from __future__ import annotations

import hashlib
import json
import re
import struct
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
DIST = HERE / "dist"
CELLS = HERE / "dist" / "cells"
PAYLOAD = HERE / "payload-build"
LOADER_SOURCE = HERE / "loader.c"
AUTHENTICODE_PAD_SOURCE = HERE / "authenticode-pad.S"
AUTHENTICODE_PREPARE_SOURCE = HERE / "prepare-authenticode.py"
VERSION_RESOURCE_SOURCE = HERE / "ape-version.rc"


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def cmd(*args: str) -> str:
    try:
        return subprocess.check_output(args, text=True, stderr=subprocess.STDOUT).strip().splitlines()[0]
    except (OSError, subprocess.CalledProcessError):
        return ""


def git_sha() -> str:
    return subprocess.check_output(["git", "-C", str(ROOT), "rev-parse", "HEAD"], text=True).strip()


def source_identity() -> dict[str, object]:
    """HEAD plus a digest of the packed source tree (diff + non-build untracked)."""
    head = git_sha()
    porcelain = subprocess.check_output(
        ["git", "-C", str(ROOT), "status", "--porcelain", "-uall"], text=True
    )
    diff = subprocess.check_output(["git", "-C", str(ROOT), "diff", "HEAD"])
    h = hashlib.sha256()
    h.update(b"HEAD ")
    h.update(head.encode())
    h.update(b"\n")
    h.update(diff)
    skip_parts = {"payload-build", "dist", "target", "target-six"}
    for line in porcelain.splitlines():
        if not line.startswith("??"):
            continue
        rel = line[3:].strip()
        path = ROOT / rel
        if any(part in skip_parts for part in path.parts):
            continue
        if not path.is_file():
            continue
        h.update(b"\n?? ")
        h.update(rel.encode())
        h.update(b" ")
        h.update(sha256(path).encode())
    return {
        "source_sha": head,
        "source_dirty": bool(porcelain.strip()),
        "source_tree_digest": h.hexdigest(),
    }


def product_version() -> str:
    text = (ROOT / "Cargo.toml").read_text()
    match = re.search(r'(?m)^version = "([^"]+)"', text)
    if not match:
        raise SystemExit("Cargo.toml version missing")
    return match.group(1)


def payload_digests(expected_version: str) -> dict[str, dict[str, object]]:
    mapping = {
        "osx-aarch64": CELLS / "osx-aarch64" / "minicon",
        "osx-x86_64": CELLS / "osx-x86_64" / "minicon",
        "lnx-aarch64": CELLS / "lnx-aarch64" / "minicon",
        "lnx-x86_64": CELLS / "lnx-x86_64" / "minicon",
        "win-aarch64": CELLS / "win-aarch64" / "minicon.exe",
        "win-x86_64": CELLS / "win-x86_64" / "minicon.exe",
    }
    out = {}
    for cell, path in mapping.items():
        if path.is_file():
            markers = sorted(set(
                match.decode("ascii")
                for match in re.findall(rb"minicon 0\.[0-9]+\.[0-9]+", path.read_bytes())
            ))
            expected_marker = f"minicon {expected_version}"
            if markers != [expected_marker]:
                raise SystemExit(f"{cell}: stale or mixed version markers {markers}, expected {expected_marker}")
            out[cell] = {
                "path": path.relative_to(HERE).as_posix(),
                "sha256": sha256(path),
                "bytes": path.stat().st_size,
                "product_version": expected_version,
            }
    return out


def authenticode_layout(path: Path) -> dict[str, int | bool]:
    raw = path.read_bytes()
    pe = struct.unpack_from("<I", raw, 0x3C)[0]
    optional = pe + 24
    optional_size = struct.unpack_from("<H", raw, pe + 20)[0]
    directories = struct.unpack_from("<I", raw, optional + 108)[0]
    resource_rva, resource_size = struct.unpack_from("<II", raw, optional + 128)
    security_offset, security_size = struct.unpack_from("<II", raw, optional + 144)
    ready = optional_size == 0xF0 and directories == 16
    if not ready or security_offset or security_size or not resource_rva or not resource_size:
        raise SystemExit("unsigned APE lacks VERSIONINFO or an empty Authenticode Security Directory")
    table = pe + 24 + optional_size
    resource_offset = None
    sections = struct.unpack_from("<H", raw, pe + 6)[0]
    for index in range(sections):
        entry = table + index * 40
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from("<IIII", raw, entry + 8)
        if virtual_address <= resource_rva < virtual_address + max(virtual_size, raw_size):
            resource_offset = raw_offset + resource_rva - virtual_address
            break
    if resource_offset is None or resource_offset + resource_size > len(raw):
        raise SystemExit("APE VERSIONINFO Resource Directory is outside PE sections")
    resource = raw[resource_offset : resource_offset + resource_size]
    for value in ("ProductName", "MiniCon", "ProductVersion", "0.1.3"):
        if value.encode("utf-16le") not in resource:
            raise SystemExit(f"APE VERSIONINFO lacks {value}")
    return {
        "ready": ready,
        "optional_header_bytes": optional_size,
        "data_directory_count": directories,
        "security_file_offset": security_offset,
        "security_bytes": security_size,
        "resource_rva": resource_rva,
        "resource_bytes": resource_size,
        "product_name": "MiniCon",
        "product_version": "0.1.3",
    }


def main() -> None:
    com = DIST / "minicon.com"
    if not com.is_file():
        raise SystemExit("missing dist/minicon.com")
    com_sha = sha256(com)
    (DIST / "minicon.com.sha256").write_text(com_sha + "\n")
    ident = source_identity()
    version = product_version()
    receipt = {
        "schema": 2,
        "product_version": version,
        "source_sha": ident["source_sha"],
        "source_dirty": ident["source_dirty"],
        "source_tree_digest": ident["source_tree_digest"],
        "minicon_com_sha256": com_sha,
        "minicon_com_bytes": com.stat().st_size,
        "loader_source_sha256": sha256(LOADER_SOURCE),
        "authenticode_pad_source_sha256": sha256(AUTHENTICODE_PAD_SOURCE),
        "authenticode_prepare_source_sha256": sha256(AUTHENTICODE_PREPARE_SOURCE),
        "version_resource_source_sha256": sha256(VERSION_RESOURCE_SOURCE),
        "authenticode": authenticode_layout(com),
        "tools": {
            "rustc": cmd("rustc", "--version"),
            "zig": cmd("zig", "version"),
            "cargo_zigbuild": cmd("cargo-zigbuild", "--version") or cmd("cargo", "zigbuild", "--version"),
            "cargo_xwin": cmd("cargo", "xwin", "--version"),
            "cosmocc": cmd("cosmocc", "--version"),
        },
        "payloads": payload_digests(version),
    }
    empty = [k for k, v in receipt["tools"].items() if not v]
    if empty:
        raise SystemExit(f"empty tool identity fields: {empty}")
    if set(receipt["payloads"]) != {
        "osx-aarch64", "osx-x86_64", "lnx-aarch64", "lnx-x86_64", "win-aarch64", "win-x86_64",
    }:
        raise SystemExit(f"payload set {sorted(receipt['payloads'])}")
    (DIST / "build-receipt.json").write_text(json.dumps(receipt, indent=2) + "\n")
    print(json.dumps({
        "minicon_com_sha256": com_sha,
        "source_sha": receipt["source_sha"],
        "source_dirty": receipt["source_dirty"],
        "source_tree_digest": receipt["source_tree_digest"],
    }, indent=2))


if __name__ == "__main__":
    main()
