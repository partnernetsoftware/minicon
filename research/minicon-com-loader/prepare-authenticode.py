#!/usr/bin/env python3
"""Expand Cosmopolitan's compact PE header for an Authenticode directory."""

from __future__ import annotations

import argparse
import struct
from pathlib import Path


PE32_PLUS_FULL_OPTIONAL = 0xF0
VERSION_KEY = "VS_VERSION_INFO".encode("utf-16le")
RESOURCE_TREE_BYTES = 88


def file_offset_to_rva(raw: bytearray, pe: int, file_offset: int) -> int:
    coff = pe + 4
    sections = struct.unpack_from("<H", raw, coff + 2)[0]
    optional_size = struct.unpack_from("<H", raw, coff + 16)[0]
    table = coff + 20 + optional_size
    for index in range(sections):
        entry = table + index * 40
        virtual_size, virtual_address, raw_size, raw_offset = struct.unpack_from(
            "<IIII", raw, entry + 8
        )
        extent = max(virtual_size, raw_size)
        if raw_offset <= file_offset < raw_offset + extent:
            return virtual_address + file_offset - raw_offset
    raise ValueError("version resource is not covered by a PE section")


def activate_version_resource(raw: bytearray, pe: int, optional: int) -> tuple[int, int]:
    matches = []
    start = 0
    while True:
        found = raw.find(VERSION_KEY, start)
        if found < 0:
            break
        matches.append(found)
        start = found + 2
    if len(matches) != 1:
        raise ValueError(f"expected one VS_VERSION_INFO marker, found {len(matches)}")
    blob = matches[0] - 6
    root = blob - RESOURCE_TREE_BYTES
    if root < 0 or struct.unpack_from("<H", raw, blob + 4)[0] != 0:
        raise ValueError("invalid VERSIONINFO resource layout")
    blob_size = struct.unpack_from("<H", raw, blob)[0]
    if blob_size < 6 + len(VERSION_KEY) + 2 or blob + blob_size > len(raw):
        raise ValueError("invalid VERSIONINFO length")
    expected_tree = bytes.fromhex(
        "000000000000000000000000000001001000000018000080"
    )
    if raw[root : root + len(expected_tree)] != expected_tree:
        raise ValueError("unexpected VERSIONINFO resource tree")
    root_rva = file_offset_to_rva(raw, pe, root)
    struct.pack_into("<I", raw, root + 0x48, root_rva + RESOURCE_TREE_BYTES)
    resource_size = RESOURCE_TREE_BYTES + ((blob_size + 3) & ~3)
    resource_directory = optional + 112 + 2 * 8
    struct.pack_into("<II", raw, resource_directory, root_rva, resource_size)
    return root_rva, resource_size


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
    resource = optional + 112 + 2 * 8
    if old_size == PE32_PLUS_FULL_OPTIONAL and count == 16:
        if any(raw[security : security + 8]):
            raise ValueError("refusing to rewrite an already signed APE")
        resource_rva, resource_size = struct.unpack_from("<II", raw, resource)
        if not resource_rva or not resource_size:
            activate_version_resource(raw, pe, optional)
        output.write_bytes(raw)
        return
    if old_size != PE32_PLUS_FULL_OPTIONAL or count != 2:
        raise ValueError(
            f"APE lacks link-time Authenticode padding: optional={old_size:#x}, dirs={count}"
        )
    if any(raw[optional + 128 : optional + PE32_PLUS_FULL_OPTIONAL]):
        raise ValueError("reserved PE data directories are not zero")
    struct.pack_into("<I", raw, optional + 108, 16)
    resource_rva, resource_size = activate_version_resource(raw, pe, optional)
    output.write_bytes(raw)

    print(
        f"PASS {sections} PE sections; resource RVA {resource_rva:#x} bytes "
        f"{resource_size}; Authenticode slot exposed at {security}"
    )


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("source")
    parser.add_argument("output")
    args = parser.parse_args()
    prepare(Path(args.source), Path(args.output))


if __name__ == "__main__":
    main()
