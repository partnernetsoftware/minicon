#!/usr/bin/env python3
"""macOS owned-group regression and guard controls; owner: PRD 02.32."""
from __future__ import annotations

import argparse
import hashlib
import json
import subprocess
import sys
from pathlib import Path


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    if sys.platform != "darwin":
        raise SystemExit("BLOCKED: retained-group native court requires macOS")
    root = Path(__file__).resolve().parent.parent
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=True)
    source = root / "crates/minicon-platform/src/adapters/unix/contained_process.rs"
    original = source.read_bytes()
    digest = hashlib.sha256(original).hexdigest()
    # Match test names by their unique suffix, independent of adapter aliases.
    normal = "normal_exit_with_retained_zombie_group_preserves_status_and_reaps"
    descendant = "exited_leader_does_not_hide_a_live_descendant_or_a_truncated_inventory"
    receipts: list[dict[str, object]] = []

    def run(label: str, selection: str, negative: bool) -> None:
        command = [
            "bash", "scripts/build.sh", "test-suite", "--target", args.target,
            "-p", "minicon-platform", "--features", "contained-process-spawn",
            "--lib", selection, "--", "--nocapture", "--test-threads", "1",
        ]
        log = output / f"{label}.log"
        with log.open("wb") as stream:
            result = subprocess.run(
                command, cwd=root, stdout=stream, stderr=subprocess.STDOUT,
                timeout=180, check=False,
            )
        text = log.read_text(encoding="utf-8", errors="replace")
        if negative:
            valid = result.returncode != 0 and "test result: FAILED. 0 passed; 1 failed;" in text
        else:
            valid = result.returncode == 0 and "test result: ok. 2 passed; 0 failed; 0 ignored;" in text
        if not valid:
            raise SystemExit(f"{label}: missing required runtime result; inspect {log}")
        receipts.append({"court": label, "runtime_red": negative, "log": log.name})
        print(f"{label}: {'runtime RED' if negative else '2 native cases passed'}", flush=True)

    run("initial", "retained_group_cleanup_tests", False)
    changes = [
        (
            "zombie-only-error-guard", normal,
            b"Ok(true) if self.root.peek_exit()? => return Ok(()),",
            b"Ok(true) if self.root.peek_exit()? => return Err(error),",
        ),
        (
            "live-descendant-kill", descendant,
            b"if self.root_reaped {\n            return Ok(());\n        }\n        self.root.peek_exit()?;",
            b"if self.root_reaped || self.root.peek_exit()? {\n            return Ok(());\n        }\n        self.root.peek_exit()?;",
        ),
    ]
    for label, selection, before, after in changes:
        if original.count(before) != 1:
            raise SystemExit(f"{label}: source guard changed; audit this control")
        try:
            source.write_bytes(original.replace(before, after))
            run(label, selection, True)
        finally:
            source.write_bytes(original)
            if hashlib.sha256(source.read_bytes()).hexdigest() != digest:
                raise SystemExit("source restoration failed")
    run("restored", "retained_group_cleanup_tests", False)
    receipt = {
        "kind": "macos-retained-group-native-court",
        "source_sha": subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip(),
        "source_file_sha256": digest,
        "target": args.target,
        "source_restored": True,
        "results": receipts,
    }
    (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")


if __name__ == "__main__":
    main()
