#!/usr/bin/env python3
"""Fingerprint every tracked or unignored untracked worktree file."""

import hashlib
import json
import os
import subprocess
from pathlib import Path


def git_paths() -> list[bytes]:
    output = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"]
    )
    return sorted(set(output.split(b"\0")) - {b""})


digest = hashlib.sha256()
paths = git_paths()
for raw_path in paths:
    path = Path(os.fsdecode(raw_path))
    digest.update(len(raw_path).to_bytes(8, "big"))
    digest.update(raw_path)
    if path.is_symlink():
        kind = b"symlink"
        content = os.fsencode(os.readlink(path))
        executable = False
    elif path.is_file():
        kind = b"file"
        content = path.read_bytes()
        executable = bool(path.stat().st_mode & 0o111)
    else:
        kind = b"missing"
        content = b""
        executable = False
    digest.update(kind)
    digest.update(b"x" if executable else b"-")
    digest.update(len(content).to_bytes(8, "big"))
    digest.update(content)

print(json.dumps({"sha256": digest.hexdigest(), "files": len(paths)}, sort_keys=True))
