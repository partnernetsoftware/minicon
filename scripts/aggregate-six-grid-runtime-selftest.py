#!/usr/bin/env python3
from __future__ import annotations

import json
import hashlib
import subprocess
import tempfile
from pathlib import Path


CELLS = ("lnx-x86_64", "lnx-aarch64", "win-x86_64", "win-aarch64", "osx-x86_64", "osx-aarch64")
BUNDLE = "ghcr.io/example/minicon@sha256:" + "a" * 64
SOURCE = "b" * 40
TREE = "c" * 64
WORKFLOW = "d" * 40
LOG = b"runtime log\n"


def receipt(
    cell: str,
    attempt: int,
    status: str,
    probe: str = "none",
    probe_injected: bool = False,
) -> dict:
    family = "Windows" if cell.startswith("win") else "macOS" if cell.startswith("osx") else "Linux"
    arch = "ARM64" if "aarch64" in cell else "X64"
    return {
        "schema": 2, "cell": cell, "run_id": 123, "run_attempt": attempt,
        "runner_arch": arch, "expected_runner_arch": arch,
        "runner_os": family, "expected_runner_os": family,
        "bundle_ref": BUNDLE, "source_sha": SOURCE, "source_tree_sha256": TREE,
        "suite": "test", "job_status": status, "workflow_sha": WORKFLOW,
        "evidence_probe_cell": probe,
        "evidence_probe_injected": probe_injected,
        "runtime_log_bytes": len(LOG), "runtime_log_sha256": hashlib.sha256(LOG).hexdigest(),
    }


def run(root: Path, expected: int, probe: str = "none") -> dict:
    output = root / "aggregate.json"
    completed = subprocess.run([
        "python3", str(Path(__file__).with_name("aggregate-six-grid-runtime.py")),
        "--receipts", str(root), "--output", str(output), "--bundle-ref", BUNDLE,
        "--source-sha", SOURCE, "--source-tree-sha256", TREE, "--suite", "test",
        "--evidence-probe-cell", probe,
    ], check=False)
    assert completed.returncode == expected
    return json.loads(output.read_text())


def write(root: Path, value: dict) -> None:
    path = root / f"{value['cell']}-attempt-{value['run_attempt']}"
    path.mkdir()
    (path / "runtime-receipt.json").write_text(json.dumps(value) + "\n")
    (path / "runtime-body.log").write_bytes(LOG)


with tempfile.TemporaryDirectory(prefix="minicon-runtime-aggregate-") as temporary:
    root = Path(temporary)
    for cell in CELLS:
        write(root, receipt(cell, 1, "success"))
    result = run(root, 0)
    assert result["verdict"] == "PASS"
    assert {value["verdict"] for value in result["cells"].values()} == {"pass"}

with tempfile.TemporaryDirectory(prefix="minicon-runtime-aggregate-") as temporary:
    root = Path(temporary)
    for cell in CELLS:
        write(root, receipt(
            cell,
            1,
            "failure" if cell == "osx-x86_64" else "success",
            "osx-x86_64",
            cell == "osx-x86_64",
        ))
    write(root, receipt("osx-x86_64", 2, "success", "osx-x86_64"))
    result = run(root, 0, "osx-x86_64")
    assert result["verdict"] == "PASS"
    assert result["cells"]["osx-x86_64"]["verdict"] == "reverified-pass"
    assert [item["job_status"] for item in result["cells"]["osx-x86_64"]["attempts"]] == ["failure", "success"]
    assert result["cells"]["osx-x86_64"]["attempts"][0]["failure_class"] == "intentional-probe"

with tempfile.TemporaryDirectory(prefix="minicon-runtime-aggregate-") as temporary:
    root = Path(temporary)
    for cell in CELLS:
        write(root, receipt(
            cell,
            1,
            "failure" if cell == "osx-x86_64" else "success",
            "osx-x86_64",
        ))
    result = run(root, 1, "osx-x86_64")
    assert result["cells"]["osx-x86_64"]["attempts"][0]["failure_class"] == "runtime-or-environment"

with tempfile.TemporaryDirectory(prefix="minicon-runtime-aggregate-") as temporary:
    root = Path(temporary)
    for cell in CELLS:
        write(root, receipt(cell, 1, "failure" if cell == "win-aarch64" else "success"))
    result = run(root, 1)
    assert result["verdict"] == "FAIL"
    assert result["cells"]["win-aarch64"]["verdict"] == "fail"
    assert result["errors"] == ["win-aarch64: latest attempt 1 is failure"]

with tempfile.TemporaryDirectory(prefix="minicon-runtime-aggregate-") as temporary:
    root = Path(temporary)
    for cell in CELLS:
        write(root, receipt(cell, 1, "success"))
    (root / "lnx-aarch64-attempt-1" / "runtime-body.log").write_text("tampered\n")
    result = run(root, 1)
    assert result["verdict"] == "FAIL"
    assert "lnx-aarch64 attempt 1: runtime log size mismatch" in result["errors"]
    assert "lnx-aarch64 attempt 1: runtime log hash mismatch" in result["errors"]

print("aggregate-six-grid-runtime-selftest: PASS")
