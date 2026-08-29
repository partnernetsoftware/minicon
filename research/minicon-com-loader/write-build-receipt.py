#!/usr/bin/env python3
"""Emit dist/build-receipt.json after pack.sh. Tool versions + payload digests."""
from __future__ import annotations

import hashlib
import json
import re
import subprocess
from pathlib import Path

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
DIST = HERE / "dist"
CELLS = HERE / "dist" / "cells"
PAYLOAD = HERE / "payload-build"


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


def payload_digests() -> dict[str, str]:
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
            out[cell] = {"path": path.relative_to(HERE).as_posix(), "sha256": sha256(path), "bytes": path.stat().st_size}
    return out


def main() -> None:
    com = DIST / "minicon.com"
    if not com.is_file():
        raise SystemExit("missing dist/minicon.com")
    com_sha = sha256(com)
    (DIST / "minicon.com.sha256").write_text(com_sha + "\n")
    ident = source_identity()
    receipt = {
        "schema": 2,
        "product_version": product_version(),
        "source_sha": ident["source_sha"],
        "source_dirty": ident["source_dirty"],
        "source_tree_digest": ident["source_tree_digest"],
        "minicon_com_sha256": com_sha,
        "minicon_com_bytes": com.stat().st_size,
        "tools": {
            "rustc": cmd("rustc", "--version"),
            "zig": cmd("zig", "version"),
            "cargo_zigbuild": cmd("cargo-zigbuild", "--version") or cmd("cargo", "zigbuild", "--version"),
            "cargo_xwin": cmd("cargo", "xwin", "--version"),
            "cosmocc": cmd("cosmocc", "--version"),
        },
        "payloads": payload_digests(),
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
