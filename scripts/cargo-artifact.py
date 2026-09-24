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


def matches(target, profile, name, kind):
    if target.get("name") != name:
        return False
    kinds = target.get("kind", [])
    if kind == "test":
        # An integration test is kind ["test"]; a library's own unit tests are
        # kind ["lib"] compiled with profile.test, and they are the executable
        # `cargo test` runs for that crate. Both are "the suite named <name>".
        return "test" in kinds or ("lib" in kinds and profile.get("test"))
    if kind not in kinds:
        return False
    # A binary target is compiled twice: once as the product and once as its
    # own test harness, under the same name and kind. Only the first is the
    # product.
    return not (kind == "bin" and profile.get("test"))


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
        if not matches(message.get("target", {}), message.get("profile", {}), name, kind):
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
