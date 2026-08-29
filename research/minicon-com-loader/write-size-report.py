#!/usr/bin/env python3
"""Rehearsal size table for minicon.com. 12 MiB is a fail-closed guard only,
not the Candidate ceiling (that must be stamped in source+PRD before G1)."""
from __future__ import annotations

import gzip
import hashlib
import json
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DIST = HERE / "dist"
CELLS = DIST / "cells"
COM = DIST / "minicon.com"
REHEARSAL_GUARD = 12 * 1024 * 1024  # 12582912; not Candidate ceiling


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def main() -> int:
    if not COM.is_file():
        print("missing dist/minicon.com", file=sys.stderr)
        return 2
    raw = COM.read_bytes()
    gz = gzip.compress(raw, compresslevel=9)
    payloads = {}
    mapping = {
        "osx-aarch64": CELLS / "osx-aarch64" / "minicon",
        "osx-x86_64": CELLS / "osx-x86_64" / "minicon",
        "lnx-aarch64": CELLS / "lnx-aarch64" / "minicon",
        "lnx-x86_64": CELLS / "lnx-x86_64" / "minicon",
        "win-aarch64": CELLS / "win-aarch64" / "minicon.exe",
        "win-x86_64": CELLS / "win-x86_64" / "minicon.exe",
    }
    payload_sum = 0
    for cell, path in mapping.items():
        if not path.is_file():
            print(f"missing payload {path}", file=sys.stderr)
            return 2
        n = path.stat().st_size
        payload_sum += n
        payloads[cell] = {"bytes": n, "sha256": sha256(path)}
    report = {
        "schema": 1,
        "kind": "rehearsal",
        "minicon_com_bytes": len(raw),
        "minicon_com_sha256": hashlib.sha256(raw).hexdigest(),
        "minicon_com_gzip9_bytes": len(gz),
        "payload_bytes_sum": payload_sum,
        "overlay_saved_bytes": payload_sum - len(raw),
        "rehearsal_guard_bytes": REHEARSAL_GUARD,
        "candidate_ceiling_stamped": False,
        "payloads": payloads,
    }
    out = DIST / "size-report.json"
    out.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps(report, indent=2))
    if len(raw) > REHEARSAL_GUARD:
        print(f"rehearsal fail-closed: {len(raw)} > {REHEARSAL_GUARD}", file=sys.stderr)
        return 3
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
