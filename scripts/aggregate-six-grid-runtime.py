#!/usr/bin/env python3
"""Aggregate immutable six-grid runtime receipts without erasing prior attempts."""

from __future__ import annotations

import argparse
import hashlib
import json
import re
from pathlib import Path


CELLS = (
    "lnx-x86_64",
    "lnx-aarch64",
    "win-x86_64",
    "win-aarch64",
    "osx-x86_64",
    "osx-aarch64",
)
STATUSES = {"success", "failure", "cancelled"}
PROBE_CELLS = ("none",) + CELLS


def valid_digest_ref(value: str) -> bool:
    return bool(re.fullmatch(r"ghcr\.io/[a-z0-9_.-]+/[a-z0-9_.-]+@sha256:[0-9a-f]{64}", value))


def aggregate(args: argparse.Namespace) -> tuple[dict, bool]:
    paths = sorted(Path(args.receipts).rglob("runtime-receipt.json"))
    receipts = [(path, json.loads(path.read_text(encoding="utf-8"))) for path in paths]
    errors: list[str] = []
    histories: dict[str, list[dict]] = {cell: [] for cell in CELLS}
    seen: set[tuple[str, int]] = set()

    if not valid_digest_ref(args.bundle_ref):
        errors.append("bundle_ref is not immutable")
    if not re.fullmatch(r"[0-9a-f]{40}", args.source_sha):
        errors.append("source_sha is invalid")
    if not re.fullmatch(r"[0-9a-f]{64}", args.source_tree_sha256):
        errors.append("source_tree_sha256 is invalid")

    for receipt_path, receipt in receipts:
        cell = receipt.get("cell")
        attempt = receipt.get("run_attempt")
        if cell not in histories:
            errors.append(f"unexpected cell {cell!r}")
            continue
        if not isinstance(attempt, int) or attempt < 1:
            errors.append(f"{cell}: invalid run_attempt {attempt!r}")
            continue
        key = (cell, attempt)
        if key in seen:
            errors.append(f"{cell}: duplicate attempt {attempt}")
            continue
        seen.add(key)
        expected = {
            "bundle_ref": args.bundle_ref,
            "source_sha": args.source_sha,
            "source_tree_sha256": args.source_tree_sha256,
            "suite": args.suite,
            "evidence_probe_cell": args.evidence_probe_cell,
        }
        for field, value in expected.items():
            if receipt.get(field) != value:
                errors.append(f"{cell} attempt {attempt}: mismatched {field}")
        if receipt.get("runner_arch") != receipt.get("expected_runner_arch"):
            errors.append(f"{cell} attempt {attempt}: runner architecture mismatch")
        if receipt.get("runner_os") != receipt.get("expected_runner_os"):
            errors.append(f"{cell} attempt {attempt}: runner OS mismatch")
        if receipt.get("job_status") not in STATUSES:
            errors.append(f"{cell} attempt {attempt}: invalid job status")
        if not isinstance(receipt.get("run_id"), int) or receipt["run_id"] < 1:
            errors.append(f"{cell} attempt {attempt}: invalid run ID")
        if not re.fullmatch(r"[0-9a-f]{40}", str(receipt.get("workflow_sha", ""))):
            errors.append(f"{cell} attempt {attempt}: invalid workflow SHA")
        log_bytes = receipt.get("runtime_log_bytes")
        log_sha = receipt.get("runtime_log_sha256")
        log_path = receipt_path.with_name("runtime-body.log")
        if (log_bytes is None) != (log_sha is None):
            errors.append(f"{cell} attempt {attempt}: incomplete runtime log identity")
        elif log_bytes is None:
            if receipt.get("job_status") == "success":
                errors.append(f"{cell} attempt {attempt}: successful cell has no runtime log")
        elif not isinstance(log_bytes, int) or log_bytes < 0:
            errors.append(f"{cell} attempt {attempt}: invalid runtime log size")
        elif not re.fullmatch(r"[0-9a-f]{64}", str(log_sha)):
            errors.append(f"{cell} attempt {attempt}: invalid runtime log SHA-256")
        elif not log_path.is_file():
            errors.append(f"{cell} attempt {attempt}: runtime log is missing")
        else:
            actual = log_path.read_bytes()
            if len(actual) != log_bytes:
                errors.append(f"{cell} attempt {attempt}: runtime log size mismatch")
            if hashlib.sha256(actual).hexdigest() != log_sha:
                errors.append(f"{cell} attempt {attempt}: runtime log hash mismatch")
        histories[cell].append(receipt)

    cells: dict[str, dict] = {}
    for cell in CELLS:
        history = sorted(histories[cell], key=lambda receipt: receipt["run_attempt"])
        if not history:
            errors.append(f"{cell}: no receipt")
            cells[cell] = {"verdict": "missing", "attempts": []}
            continue
        latest = history[-1]
        had_failure = any(receipt["job_status"] != "success" for receipt in history[:-1])
        if latest["job_status"] == "success":
            verdict = "reverified-pass" if had_failure else "pass"
        else:
            verdict = "fail"
            errors.append(f"{cell}: latest attempt {latest['run_attempt']} is {latest['job_status']}")
        cells[cell] = {
            "verdict": verdict,
            "selected_attempt": latest["run_attempt"],
            "attempts": [
                {
                    "run_attempt": receipt["run_attempt"],
                    "job_status": receipt["job_status"],
                    "run_id": receipt["run_id"],
                    "workflow_sha": receipt["workflow_sha"],
                    "runtime_log_bytes": receipt.get("runtime_log_bytes"),
                    "runtime_log_sha256": receipt.get("runtime_log_sha256"),
                    "failure_class": (
                        "intentional-probe"
                        if receipt["job_status"] != "success"
                        and receipt.get("evidence_probe_cell") == cell
                        and receipt["run_attempt"] == 1
                        else "runtime-or-environment"
                        if receipt["job_status"] != "success"
                        else None
                    ),
                }
                for receipt in history
            ],
        }

    passed = not errors and all(
        cell["verdict"] in {"pass", "reverified-pass"} for cell in cells.values()
    )
    result = {
        "schema": 2,
        "verdict": "PASS" if passed else "FAIL",
        "bundle_ref": args.bundle_ref,
        "source_sha": args.source_sha,
        "source_tree_sha256": args.source_tree_sha256,
        "suite": args.suite,
        "evidence_probe_cell": args.evidence_probe_cell,
        "cells": cells,
        "errors": errors,
    }
    return result, passed


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--receipts", required=True)
    parser.add_argument("--output", required=True)
    parser.add_argument("--bundle-ref", required=True)
    parser.add_argument("--source-sha", required=True)
    parser.add_argument("--source-tree-sha256", required=True)
    parser.add_argument("--suite", choices=("status", "test", "full"), required=True)
    parser.add_argument("--evidence-probe-cell", choices=PROBE_CELLS, default="none")
    args = parser.parse_args()
    try:
        result, passed = aggregate(args)
    except (OSError, TypeError, ValueError, json.JSONDecodeError) as error:
        result = {"schema": 2, "verdict": "FAIL", "errors": [str(error)]}
        passed = False
    Path(args.output).write_text(json.dumps(result, indent=2) + "\n", encoding="utf-8")
    return 0 if passed else 1


if __name__ == "__main__":
    raise SystemExit(main())
