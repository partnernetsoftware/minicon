#!/usr/bin/env python3
"""Expand Cosmopolitan's compact PE header for an Authenticode directory."""

from __future__ import annotations

import argparse
import struct
from pathlib import Path


PE32_PLUS_FULL_OPTIONAL = 0xF0


def prepare(source: Path, output: Path) -> None:
    raw = bytearray(source.read_bytes())
    if raw[:2] != b"MZ":
        raise ValueError("not an MZ/APE file")
    pe = struct.unpack_from("<I", raw, 0x3C)[0]
    if raw[pe : pe + 4] != b"PE\0\0":
        raise ValueError("missing PE signature")
    coff = pe + 4
    sections = struct.unpack_from("<H", raw, coff + 2)[0]
    old_size = struct.unpack_from("<H", raw, coff + 16)[0]
    optional = coff + 20
    if struct.unpack_from("<H", raw, optional)[0] != 0x20B:
        raise ValueError("expected PE32+ optional header")
    count = struct.unpack_from("<I", raw, optional + 108)[0]
    security = optional + 112 + 4 * 8
    if old_size == PE32_PLUS_FULL_OPTIONAL and count == 16:
        if any(raw[security : security + 8]):
            raise ValueError("refusing to rewrite an already signed APE")
        output.write_bytes(raw)
        return
    if old_size != PE32_PLUS_FULL_OPTIONAL or count != 2:
        raise ValueError(
            f"APE lacks link-time Authenticode padding: optional={old_size:#x}, dirs={count}"
        )
    if any(raw[optional + 128 : optional + PE32_PLUS_FULL_OPTIONAL]):
        raise ValueError("reserved PE data directories are not zero")
    struct.pack_into("<I", raw, optional + 108, 16)
    output.write_bytes(raw)

    print(f"PASS {sections} PE sections; Authenticode slot exposed at {security}")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source")
    parser.add_argument("output")
    args = parser.parse_args()
    prepare(Path(args.source), Path(args.output))


if __name__ == "__main__":
    main()
