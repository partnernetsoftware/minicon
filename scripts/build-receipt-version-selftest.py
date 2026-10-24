#!/usr/bin/env python3
"""Exercise the production receipt scanner's terminated version contract."""

from __future__ import annotations

import runpy
from pathlib import Path


def main() -> None:
    source = Path(__file__).resolve().parent.parent / "loader" / "write-build-receipt.py"
    validate = runpy.run_path(str(source))["validate_payload_version"]
    accepted = {
        "exact": b"minicon 0.2.23\n",
        "binary surroundings": b"\x7fELF\x00minicon 0.2.23\n\xff",
        "digit after terminator": b"minicon 0.2.23\n1",
        "repeated current version": b"minicon 0.2.23\n\x00minicon 0.2.23\n",
    }
    rejected = {
        "stale version": b"minicon 0.2.22\n",
        "longer version": b"minicon 0.2.231\n",
        "mixed stale version": b"minicon 0.2.23\n\x00minicon 0.2.22\n",
        "mixed longer version": b"minicon 0.2.23\n\x00minicon 0.2.231\n",
        "missing marker": b"\x7fELF\x00unrelated data",
        "unterminated current version": b"minicon 0.2.23",
        "pooled trailing digit": b"minicon 0.2.231\x00",
        "non-LF terminator": b"minicon 0.2.23\r\n",
        "unsupported suffix": b"minicon 0.2.23beta\n",
        "mixed unterminated longer version": b"minicon 0.2.23\n\x00minicon 0.2.231",
        "mixed unterminated current version": b"minicon 0.2.23\n\x00minicon 0.2.23",
    }
    for name, payload in accepted.items():
        try:
            validate(payload, "0.2.23", "fixture")
        except SystemExit as error:
            raise AssertionError(f"accepted fixture rejected: {name}: {error}") from error
    for name, payload in rejected.items():
        try:
            validate(payload, "0.2.23", "fixture")
        except SystemExit:
            continue
        raise AssertionError(f"rejected fixture accepted: {name}")
    print(f"PASS {len(accepted) + len(rejected)} terminated-version fixtures")


if __name__ == "__main__":
    main()
