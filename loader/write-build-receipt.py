#!/usr/bin/env python3
"""Emit dist/build-receipt.json after pack.sh. Tool versions + payload digests."""
from __future__ import annotations

import hashlib
import json
import os
import re
import struct
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent
DIST = HERE / "dist"
CELLS = HERE / "dist" / "cells"
PAYLOAD = HERE / "payload-build"
# v0.2.27: the cosmocc polyglot lane is retired; loader.c and its pad/rc
# companions are frozen history (see loader/README.md). The live launcher is
# loader/com-rs (Rust), whose identity is the packed .com manifest itself.
LAUNCHER_SOURCES = [HERE / "com-rs" / "src" / "lib.rs"]


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


def validate_payload_version(payload: bytes, expected_version: str, cell: str) -> None:
    """Require every production version candidate to be a complete LF record."""
    markers = set()
    for match in re.finditer(rb"minicon [0-9]+\.[0-9]+\.[0-9]+", payload):
        if payload[match.end():match.end() + 1] != b"\n":
            raise SystemExit(f"{cell}: unterminated product version marker")
        markers.add(match.group().decode("ascii"))
    markers = sorted(markers)
    expected_marker = f"minicon {expected_version}"
    if markers != [expected_marker]:
        raise SystemExit(f"{cell}: stale or mixed version markers {markers}, expected {expected_marker}")


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
            payload_bytes = path.read_bytes()
            validate_payload_version(payload_bytes, expected_version, cell)
            out[cell] = {
                "path": path.relative_to(HERE).as_posix(),
                "sha256": sha256(path),
                "bytes": path.stat().st_size,
                "product_version": expected_version,
            }
            if cell.startswith("lnx-"):
                helper_base = Path(os.environ.get("MINICON_BROWSER_HELPERS", str(ROOT / "target" / "browser-helpers")))
                if not helper_base.is_absolute():
                    helper_base = ROOT / helper_base
                helper_root = helper_base / cell
                helper = helper_root / "minicon-web-host"
                receipt = json.loads((helper_root / "receipt.json").read_text())
                raw = helper.read_bytes()
                expected_machine = 62 if cell == "lnx-x86_64" else 183
                if (receipt["source_sha"] != git_sha() or receipt["cell"] != cell
                        or receipt["sha256"] != sha256(helper)
                        or receipt["bytes"] != helper.stat().st_size
                        or not 64 <= len(raw) <= 32 * 1024 * 1024
                        or raw[:7] != b"\x7fELF\x02\x01\x01"
                        or int.from_bytes(raw[18:20], "little") != expected_machine
                        or raw not in payload_bytes):
                    raise SystemExit(f"{cell}: browser helper identity mismatch")
                out[cell]["embedded_browser_helper"] = receipt
    return out


def rust_com_digests() -> dict[str, dict[str, object]]:
    """Per-cell Rust-launcher artifacts (loader/dist/rust): the v0.2.27
    single-file assets. Every present artifact must carry the container table
    (MINICOM1) and match the manifest pack-rust-all.sh wrote beside it."""
    rust_dir = DIST / "rust"
    manifest_path = rust_dir / "manifest.json"
    if not manifest_path.is_file():
        return {}
    manifest = json.loads(manifest_path.read_text())
    out: dict[str, dict[str, object]] = {}
    for name, identity in sorted(manifest.get("artifacts", {}).items()):
        path = rust_dir / name
        if not path.is_file():
            raise SystemExit(f"manifest names a missing artifact: {name}")
        raw = path.read_bytes()
        if raw.rfind(b"MINICOM1") < 0:
            raise SystemExit(f"{name}: no MINICOM1 container")
        if identity.get("sha256") != sha256(path) or identity.get("bytes") != len(raw):
            raise SystemExit(f"{name}: manifest identity mismatch")
        out[name] = {
            "sha256": identity["sha256"],
            "bytes": len(raw),
        }
    return out


def launcher_source_digests() -> dict[str, str]:
    return {path.name: sha256(path) for path in LAUNCHER_SOURCES if path.is_file()}


def main() -> None:
    rust_dir = DIST / "rust"
    if not (rust_dir / "manifest.json").is_file():
        raise SystemExit("missing dist/rust/manifest.json (run pack-rust-all.sh)")
    coms = rust_com_digests()
    if len(coms) < 6:
        raise SystemExit(f"expected at least six .com artifacts, found {len(coms)}")
    primary = coms.get("minicon-win-x86_64.com")
    if not primary:
        raise SystemExit("missing the primary minicon-win-x86_64.com artifact")
    ident = source_identity()
    version = product_version()
    receipt = {
        "schema": 3,
        "product_version": version,
        "source_sha": ident["source_sha"],
        "source_dirty": ident["source_dirty"],
        "source_tree_digest": ident["source_tree_digest"],
        "minicon_com_sha256": primary["sha256"],
        "minicon_com_bytes": primary["bytes"],
        "delivery_profiles": {
            "windows": "windows-release",
            "linux": "release",
            "macos": "release-fast",
        },
        "launcher_sources": launcher_source_digests(),
        "tools": {
            "rustc": cmd("rustc", "--version"),
            "zig": cmd("zig", "version"),
            "cargo_zigbuild": cmd("cargo-zigbuild", "--version") or cmd("cargo", "zigbuild", "--version"),
            "cargo_xwin": cmd("cargo", "xwin", "--version"),
        },
        "payloads": payload_digests(version),
        "rust_coms": coms,
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
        "minicon_com_sha256": primary["sha256"],
        "source_sha": receipt["source_sha"],
        "source_dirty": receipt["source_dirty"],
        "source_tree_digest": receipt["source_tree_digest"],
    }, indent=2))


if __name__ == "__main__":
    main()
