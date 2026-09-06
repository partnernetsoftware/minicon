#!/usr/bin/env python3
"""List PE import DLLs. Used to prove delay-load of gdiplus.dll."""

from __future__ import annotations

import struct
import sys
from pathlib import Path


def import_dlls(path: Path) -> list[str]:
    data = path.read_bytes()
    e_lfanew = struct.unpack_from("<I", data, 0x3C)[0]
    magic = struct.unpack_from("<H", data, e_lfanew + 24)[0]
    opt = e_lfanew + 24
    dd = opt + 112 if magic == 0x20B else opt + 96
    import_rva = struct.unpack_from("<I", data, dd + 8)[0]
    nsec = struct.unpack_from("<H", data, e_lfanew + 6)[0]
    optsz = struct.unpack_from("<H", data, e_lfanew + 20)[0]
    sec0 = e_lfanew + 24 + optsz
    secs = []
    for i in range(nsec):
        off = sec0 + i * 40
        vsz, va, rsz, raw = struct.unpack_from("<IIII", data, off + 8)
        secs.append((va, vsz, raw, rsz))

    def rva_to_off(rva: int) -> int | None:
        for va, vsz, raw, rsz in secs:
            if va <= rva < va + max(vsz, rsz):
                return raw + (rva - va)
        return None

    off = rva_to_off(import_rva)
    if off is None:
        return []
    out: list[str] = []
    while True:
        _ilt, _td, _fwd, name_rva, _iat = struct.unpack_from("<IIIII", data, off)
        if name_rva == 0:
            break
        name_off = rva_to_off(name_rva)
        if name_off is None:
            break
        out.append(data[name_off:].split(b"\0", 1)[0].decode("ascii", "replace").lower())
        off += 20
    return out


def main() -> int:
    if len(sys.argv) < 2:
        print("usage: pe_imports.py PE [--forbid DLL]", file=sys.stderr)
        return 2
    path = Path(sys.argv[1])
    dlls = import_dlls(path)
    print("imports", " ".join(dlls))
    if "--forbid" in sys.argv:
        forbid = sys.argv[sys.argv.index("--forbid") + 1].lower()
        if forbid in dlls:
            print(f"FAIL static import {forbid}", file=sys.stderr)
            return 1
        print(f"PASS no static import {forbid}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
