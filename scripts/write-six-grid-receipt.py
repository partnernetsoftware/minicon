#!/usr/bin/env python3
"""Write the build receipt the six-grid packager reads.

`scripts/six-cell-qualify.sh` writes this receipt after cross-building every cell
on one host. The cloud build instead compiles each cell on its own native runner
and assembles them afterwards, so it needs the same receipt shape written from
the assembled result. Keeping the writer here rather than inline in the workflow
makes it testable and keeps YAML block scalars out of the picture.

The receipt is deliberately minimal: the packager checks that the source is not
dirty, that the tree fingerprint matches the current tree, that no stage FAILed,
and that the named build root holds the cell bodies. It does not read the stage
timings, so a native build records a PASS stage per cell with no duration.
"""

from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

CELLS = (
    "osx-aarch64",
    "osx-x86_64",
    "win-x86_64",
    "win-aarch64",
    "lnx-x86_64",
    "lnx-aarch64",
)


def source_fingerprint(repo: Path) -> str:
    """The tree fingerprint the qualification and packaging steps share."""
    raw = subprocess.check_output(
        [sys.executable, str(repo / "scripts" / "source-fingerprint.py")],
        cwd=repo,
        text=True,
    )
    return json.loads(raw)["sha256"]


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--build-root", required=True, help="directory holding <cell>/<triple>/…")
    parser.add_argument("--output", default="target-six/receipt.json")
    parser.add_argument("--stage", default="native-build")
    args = parser.parse_args()

    repo = Path(__file__).resolve().parent.parent
    identity = source_fingerprint(repo)
    sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=repo, text=True).strip()
    dirty = bool(subprocess.check_output(["git", "status", "--porcelain"], cwd=repo, text=True).strip())

    stages = []
    for cell in CELLS:
        # The stage records the outcome for this cell. A missing body is a FAIL
        # rather than an absent stage, because the packager refuses a receipt
        # containing FAIL and would otherwise report a confusing missing-file
        # error much later.
        present = (repo / args.build_root / cell).is_dir()
        stages.append(
            {
                "cell": cell,
                "stage": args.stage,
                "status": "PASS" if present else "FAIL",
                "duration_seconds": 0,
                "log": None,
                "detail": (
                    "assembled by the six-grid cloud build"
                    if present
                    else f"no staged tree at {args.build_root}/{cell}"
                ),
            }
        )

    receipt = {
        "schema_version": 1,
        "product": "minicon",
        "source_sha": sha,
        "source_dirty": dirty,
        "source_tree_sha256": identity,
        "build_root": args.build_root,
        "stages": stages,
        "artifacts": [],
    }
    receipt["summary"] = {
        key: sum(stage["status"] == key for stage in stages)
        for key in ("PASS", "FAIL", "BLOCKED")
    }

    output = (repo / args.output).resolve()
    output.parent.mkdir(parents=True, exist_ok=True)
    output.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    print(json.dumps({"receipt": str(output.relative_to(repo)), **receipt["summary"]}, sort_keys=True))
    raise SystemExit(1 if receipt["summary"]["FAIL"] else 0)


if __name__ == "__main__":
    main()
