#!/usr/bin/env python3
"""Fail closed when a public signing receipt leaks provider coordinates."""

from __future__ import annotations

import argparse
import copy
import hashlib
import json
import re
from pathlib import Path
from typing import Any


SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
SOURCE_RE = re.compile(r"^[0-9a-f]{40}$")
FORBIDDEN_KEYS = {
    "provider_resource",
    "endpoint",
    "account",
    "account_name",
    "signing_account",
    "certificate_profile",
    "profile_name",
    "resource_group",
    "validation_id",
    "application_id",
    "object_id",
    "federated_credential",
    "azure_client_id",
    "client_id",
    "azure_tenant_id",
    "tenant_id",
    "azure_subscription_id",
    "subscription_id",
    "client_secret",
    "access_token",
}


def load(path: Path) -> dict[str, Any]:
    value = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(value, dict):
        raise ValueError("receipt must be a JSON object")
    return value


def require_text(row: dict[str, Any], field: str) -> str:
    value = row.get(field)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"missing {field}")
    return value.strip()


def reject_protected_keys(value: Any, path: str = "$") -> None:
    if isinstance(value, dict):
        for key, child in value.items():
            normalized = str(key).strip().casefold().replace("-", "_")
            if normalized in FORBIDDEN_KEYS:
                raise ValueError(f"protected configuration key at {path}.{key}")
            reject_protected_keys(child, f"{path}.{key}")
    elif isinstance(value, list):
        for index, child in enumerate(value):
            reject_protected_keys(child, f"{path}[{index}]")


def audit(
    receipt: dict[str, Any],
    expected_provider: str,
    expected_publisher: str,
    release_eligible: bool | None,
) -> None:
    reject_protected_keys(receipt)
    schema = receipt.get("schema", receipt.get("schema_version"))
    if type(schema) is not int or schema < 1:
        raise ValueError("missing receipt schema/schema_version")
    require_text(receipt, "kind")
    if not SOURCE_RE.fullmatch(str(receipt.get("source_sha", ""))):
        raise ValueError("invalid source SHA")
    if receipt.get("signing_provider") != expected_provider:
        raise ValueError("unexpected signing provider")
    if receipt.get("publisher_organization") != expected_publisher:
        raise ValueError("unexpected publisher organization")
    if receipt.get("file_digest") != "SHA256":
        raise ValueError("unexpected file digest policy")
    if receipt.get("timestamp_rfc3161") != "http://timestamp.acs.microsoft.com":
        raise ValueError("unexpected RFC 3161 timestamp policy")
    if receipt.get("timestamp_digest") != "SHA256":
        raise ValueError("unexpected timestamp digest policy")
    version = receipt.get("product_version", receipt.get("version"))
    if not isinstance(version, str) or not version.strip():
        raise ValueError("missing product version")
    if not isinstance(receipt.get("release_eligible"), bool):
        raise ValueError("missing release eligibility")
    if release_eligible is not None and receipt.get("release_eligible") is not release_eligible:
        raise ValueError("release eligibility mismatch")
    run = receipt.get("signing_run", receipt.get("run"))
    if not isinstance(run, dict):
        raise ValueError("missing signing run identity")
    run_id = run.get("id")
    run_attempt = run.get("attempt")
    if type(run_id) is not int or run_id <= 0 or type(run_attempt) is not int or run_attempt <= 0:
        raise ValueError("invalid signing run identity")
    assets = receipt.get("assets")
    if not isinstance(assets, dict) or not assets:
        raise ValueError("missing signed assets")
    for name, row in assets.items():
        if not isinstance(row, dict):
            raise ValueError(f"{name}: asset receipt must be an object")
        path = require_text(row, "path")
        parts = Path(path.replace("\\", "/")).parts
        if (
            path.startswith(("/", "\\"))
            or re.match(r"^[A-Za-z]:", path)
            or "\\" in path
            or not parts
            or any(part in ("", ".", "..") for part in parts)
        ):
            raise ValueError(f"{name}: unsafe asset path")
        before = str(row.get("before_sha256", ""))
        after = str(row.get("after_sha256", ""))
        if not SHA256_RE.fullmatch(before) or not SHA256_RE.fullmatch(after) or before == after:
            raise ValueError(f"{name}: invalid before/after SHA-256")
        if type(row.get("after_bytes")) is not int or row["after_bytes"] <= 0:
            raise ValueError(f"{name}: invalid signed byte count")
        if row.get("authenticode_status") != "Valid":
            raise ValueError(f"{name}: Authenticode is not Valid")
        signer_subject = require_text(row, "signer_subject")
        publisher_pattern = r"(?:^|,\s*)O=" + re.escape(expected_publisher) + r"(?:,|$)"
        if not re.search(publisher_pattern, signer_subject):
            raise ValueError(f"{name}: signer organization mismatch")
        for field in (
            "signer_issuer",
            "signer_thumbprint",
            "signer_not_before",
            "signer_not_after",
            "timestamp_subject",
            "timestamp_issuer",
        ):
            require_text(row, field)


def self_test() -> None:
    asset = {
        "path": "product.exe",
        "before_sha256": hashlib.sha256(b"before").hexdigest(),
        "after_sha256": hashlib.sha256(b"after").hexdigest(),
        "after_bytes": 5,
        "authenticode_status": "Valid",
        "signer_subject": "CN=Example Corp, O=Example Corp, C=AU",
        "signer_issuer": "CN=Fixture CA",
        "timestamp_subject": "CN=Fixture TSA",
        "timestamp_issuer": "CN=Fixture TSA CA",
    }
    fixture = {
        "schema": 1,
        "kind": "fixture-signing",
        "source_sha": "a" * 40,
        "product_version": "0.0.0",
        "signing_provider": "azure-artifact-signing",
        "publisher_organization": "Example Corp",
        "file_digest": "SHA256",
        "timestamp_rfc3161": "http://timestamp.acs.microsoft.com",
        "timestamp_digest": "SHA256",
        "release_eligible": False,
        "signing_run": {"id": 1, "attempt": 1},
        "assets": {"product": asset},
    }
    asset.update(
        {
            "signer_thumbprint": "00",
            "signer_not_before": "2026-01-01T00:00:00Z",
            "signer_not_after": "2026-01-02T00:00:00Z",
        }
    )
    audit(fixture, "azure-artifact-signing", "Example Corp", False)
    rejected: list[tuple[dict[str, Any], str]] = []
    leaked = copy.deepcopy(fixture)
    leaked["provider_resource"] = {"account": "must-not-ship"}
    rejected.append((leaked, "protected configuration key"))
    wrong_signer = copy.deepcopy(fixture)
    wrong_signer["assets"]["product"]["signer_subject"] = "CN=Other Corp, O=Other Corp"
    rejected.append((wrong_signer, "signer organization mismatch"))
    missing_thumbprint = copy.deepcopy(fixture)
    del missing_thumbprint["assets"]["product"]["signer_thumbprint"]
    rejected.append((missing_thumbprint, "missing signer_thumbprint"))
    wrong_timestamp = copy.deepcopy(fixture)
    wrong_timestamp["timestamp_rfc3161"] = "http://example.invalid/timestamp"
    rejected.append((wrong_timestamp, "unexpected RFC 3161 timestamp policy"))
    unsafe_path = copy.deepcopy(fixture)
    unsafe_path["assets"]["product"]["path"] = "../product.exe"
    rejected.append((unsafe_path, "unsafe asset path"))
    for invalid, expected_message in rejected:
        try:
            audit(invalid, "azure-artifact-signing", "Example Corp", False)
        except ValueError as exc:
            assert expected_message in str(exc), (expected_message, str(exc))
        else:
            raise AssertionError(f"invalid receipt was accepted: {expected_message}")
    print("PASS public signing receipt redaction and signature-fact court")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("receipt", type=Path, nargs="?")
    parser.add_argument("--expected-provider", default="azure-artifact-signing")
    parser.add_argument("--expected-publisher", default="PARTNERNET SOFTWARE PTY LTD")
    parser.add_argument("--release-eligible", choices=("true", "false"))
    parser.add_argument("--self-test", action="store_true")
    args = parser.parse_args()
    if args.self_test:
        self_test()
        return
    if args.receipt is None:
        parser.error("receipt is required unless --self-test is used")
    expected_eligibility = None if args.release_eligible is None else args.release_eligible == "true"
    audit(load(args.receipt), args.expected_provider, args.expected_publisher, expected_eligibility)
    print("PASS public signing receipt contains no protected provider coordinates")


if __name__ == "__main__":
    main()
