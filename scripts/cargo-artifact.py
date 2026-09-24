#!/usr/bin/env python3
"""Name the executable a Cargo build stands behind.

A round's cell directory is seeded from the previous fingerprint's directory,
so `deps/` holds every earlier round's `<suite>-<hash>` and the presence of a
file says nothing about which source produced it. Modification time says
nothing either: a fully cached build touches none of them.

Cargo's own JSON record does say it. It emits one `compiler-artifact` message
per target with the path it would hand to `cargo test`, whether it recompiled
the target or found it fresh, so this is the same answer Cargo would act on.

Prints the path, or nothing when this build named no such artifact -- which is
a blocked round, never an older answer quietly substituted for it.
"""

import json
import sys


def executable(log, name, kind):
    try:
        with open(log, encoding="utf-8", errors="replace") as record:
            lines = record.read().splitlines()
    except OSError:
        return None
    for line in lines:
        if not line.startswith("{"):
            continue
        try:
            message = json.loads(line)
        except ValueError:
            continue
        if message.get("reason") != "compiler-artifact":
            continue
        target = message.get("target", {})
        if target.get("name") != name or kind not in target.get("kind", []):
            continue
        # A binary target is compiled twice: once as the product and once as
        # its own test harness, under the same name and kind. Only the first
        # is the product.
        if kind == "bin" and message.get("profile", {}).get("test"):
            continue
        found = message.get("executable")
        if found:
            return found
    return None


def main(argv):
    if len(argv) != 4:
        print(f"usage: {argv[0]} BUILD_JSON TARGET_NAME KIND", file=sys.stderr)
        return 2
    found = executable(argv[1], argv[2], argv[3])
    if found is None:
        return 1
    print(found)
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv))
