#!/usr/bin/env python3
"""Size court for minicon.com.

Rehearsal fail-closed guard is 12 MiB and must not decide a Candidate.
Candidate hard ceiling is the stamped integer CANDIDATE_CEILING_BYTES.
Over that ceiling after G7 pack: fail and re-review; never auto-raise.
"""
from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import struct
import sys
from pathlib import Path

HERE = Path(__file__).resolve().parent
DIST = HERE / "dist"
CELLS = DIST / "cells"
COM = DIST / "minicon.com"
# Rehearsal-only. Never used as Candidate pass/fail.
REHEARSAL_GUARD_BYTES = 12 * 1024 * 1024  # 12582912
# cdx 2026-08-29: 9 MiB hard ceiling (rehearsal raw 8880268 + 556916).
# Horizon qjswasm/TinyVM (PRD) must not change this constant or mix into Candidate.
CANDIDATE_CEILING_BYTES = 9 * 1024 * 1024  # 9437184


def sha256(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def candidate_ok(nbytes: int) -> bool:
    return nbytes <= CANDIDATE_CEILING_BYTES


def zip_overlay_breakdown(data: bytes) -> dict:
    """Honest container split from the Zip overlay, not uncompressed payload sum."""
    ape = data.find(b"PK\x03\x04")
    eocd = data.rfind(b"PK\x05\x06")
    if ape < 0 or eocd < 0 or eocd + 22 > len(data):
        raise ValueError("minicon.com is not APE+zip")
    cd_size, cd_off = struct.unpack_from("<II", data, eocd + 12)
    if data[cd_off : cd_off + 4] == b"PK\x01\x02":
        cd_abs = cd_off
    elif data[ape + cd_off : ape + cd_off + 4] == b"PK\x01\x02":
        cd_abs = ape + cd_off
    else:
        raise ValueError("zip central directory not found")
    pos = cd_abs
    end = cd_abs + cd_size
    cells_comp = 0
    cells_uncomp = 0
    other_comp = 0
    n_cells = 0
    n_other = 0
    while pos + 46 <= end and data[pos : pos + 4] == b"PK\x01\x02":
        _hdr = struct.unpack_from("<HHHHHHIIIHHHHHII", data, pos + 4)
        csz, usz, nlen, elen, clen = _hdr[7], _hdr[8], _hdr[9], _hdr[10], _hdr[11]
        name = data[pos + 46 : pos + 46 + nlen].decode("utf-8", "replace")
        pos += 46 + nlen + elen + clen
        if name.startswith("cells/") and not name.endswith("/"):
            cells_comp += csz
            cells_uncomp += usz
            n_cells += 1
        elif not name.endswith("/"):
            other_comp += csz
            n_other += 1
    eocd_len = len(data) - eocd
    local_and_data = cd_abs - ape
    return {
        "ape_prefix_bytes": ape,
        "zip_local_and_filedata_bytes": local_and_data,
        "zip_central_directory_bytes": cd_size,
        "zip_eocd_bytes": eocd_len,
        "cells_entry_count": n_cells,
        "cells_compressed_bytes": cells_comp,
        "cells_uncompressed_bytes": cells_uncomp,
        "other_zip_file_compressed_bytes": other_comp,
        "other_zip_file_count": n_other,
        "container_minus_cells_compressed_bytes": len(data) - cells_comp,
    }


def self_test() -> int:
    if not candidate_ok(CANDIDATE_CEILING_BYTES):
        print("FAIL ceiling pass", file=sys.stderr)
        return 1
    if candidate_ok(CANDIDATE_CEILING_BYTES + 1):
        print("FAIL ceiling+1 should fail", file=sys.stderr)
        return 1
    if CANDIDATE_CEILING_BYTES != 9437184:
        print("FAIL stamped integer", file=sys.stderr)
        return 1
    if REHEARSAL_GUARD_BYTES != 12582912:
        print("FAIL rehearsal guard integer", file=sys.stderr)
        return 1
    import io
    import zipfile

    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", compression=zipfile.ZIP_DEFLATED) as zf:
        zf.writestr("cells/osx-aarch64/minicon", b"payload" * 50)
        zf.writestr("cells/lnx-x86_64/minicon", b"other" * 40)
    blob = b"APEPREFIX" + buf.getvalue()
    br = zip_overlay_breakdown(blob)
    if br["ape_prefix_bytes"] != 9:
        print("FAIL zip ape prefix", br, file=sys.stderr)
        return 1
    if br["cells_entry_count"] != 2 or br["cells_uncompressed_bytes"] != 7 * 50 + 5 * 40:
        print("FAIL zip cells", br, file=sys.stderr)
        return 1
    if br["cells_compressed_bytes"] <= 0 or br["cells_compressed_bytes"] >= br["cells_uncompressed_bytes"]:
        print("FAIL zip compressed vs raw", br, file=sys.stderr)
        return 1
    print(
        f"PASS size-court ceiling={CANDIDATE_CEILING_BYTES} "
        f"ceiling+1 reject rehearsal_guard={REHEARSAL_GUARD_BYTES} zip-cd"
    )
    return 0


def report(mode: str) -> int:
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
    body = {
        "schema": 2,
        "kind": mode,
        "minicon_com_bytes": len(raw),
        "minicon_com_sha256": hashlib.sha256(raw).hexdigest(),
        "minicon_com_gzip9_bytes": len(gz),
        "payload_bytes_sum": payload_sum,
        "zip_overlay": zip_overlay_breakdown(raw),
        "rehearsal_guard_bytes": REHEARSAL_GUARD_BYTES,
        "candidate_ceiling_bytes": CANDIDATE_CEILING_BYTES,
        "candidate_ceiling_stamped": True,
        "payloads": payloads,
    }
    out = DIST / "size-report.json"
    out.write_text(json.dumps(body, indent=2) + "\n")
    print(json.dumps(body, indent=2))
    if mode == "candidate":
        if not candidate_ok(len(raw)):
            print(
                f"candidate fail: {len(raw)} > ceiling {CANDIDATE_CEILING_BYTES}; re-review budget, do not raise",
                file=sys.stderr,
            )
            return 3
        return 0
    if len(raw) > REHEARSAL_GUARD_BYTES:
        print(f"rehearsal fail-closed: {len(raw)} > {REHEARSAL_GUARD_BYTES}", file=sys.stderr)
        return 3
    return 0


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--mode", choices=("rehearsal", "candidate"), default="rehearsal")
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        return self_test()
    return report(args.mode)


if __name__ == "__main__":
    raise SystemExit(main())
