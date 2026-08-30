#!/usr/bin/env python3
"""Verify MiniCon's exact-byte trusted-signing boundary."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import tempfile
from pathlib import Path


SHA256_RE = re.compile(r"^[0-9a-f]{64}$")
SOURCE_RE = re.compile(r"^[0-9a-f]{40}$")
SIGNED_ASSETS = {
    "minicon.com": "minicon.com",
    "win-x86_64": "cells/win-x86_64/minicon.exe",
    "win-aarch64": "cells/win-aarch64/minicon.exe",
}
PROVIDER_IDENTITIES = {
    "azure-artifact-signing": ("PARTNERNET SOFTWARE PTY LTD", {"O"}),
    "signpath-foundation": ("SignPath Foundation", {"CN", "O"}),
}


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def load(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(value, dict):
        raise ValueError(f"{path.name}: expected JSON object")
    return value


def require_nonempty(row: dict, field: str) -> str:
    value = row.get(field)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"missing {field}")
    return value.strip()


def subject_has_publisher(subject: str, publisher: str, attributes: set[str]) -> bool:
    # Avoid accepting the publisher merely as a substring of an unrelated RDN.
    return any(
        part.strip().casefold() == f"{attribute}={publisher}".casefold()
        for attribute in attributes
        for part in subject.split(",")
    )


def verify_receipt(receipt: dict, root: Path) -> None:
    if receipt.get("schema") != 2 or receipt.get("kind") != "minicon-trusted-signing":
        raise ValueError("unsupported signing receipt")
    if not SOURCE_RE.fullmatch(str(receipt.get("source_sha"))):
        raise ValueError("invalid source SHA")
    product_version = require_nonempty(receipt, "product_version")
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", product_version):
        raise ValueError("invalid signing receipt product version")
    if not SHA256_RE.fullmatch(str(receipt.get("source_tree_digest"))):
        raise ValueError("invalid source tree digest")
    provider = receipt.get("signing_provider")
    identity = PROVIDER_IDENTITIES.get(provider)
    if identity is None:
        raise ValueError("unexpected signing provider")
    expected_publisher, subject_attributes = identity
    if receipt.get("publisher_organization") != expected_publisher:
        raise ValueError("unexpected signing provider/publisher pair")
    if receipt.get("file_digest") != "SHA256" or receipt.get("timestamp_digest") != "SHA256":
        raise ValueError("signing digest policy mismatch")
    require_nonempty(receipt, "timestamp_rfc3161")
    upstream = receipt.get("upstream")
    if not isinstance(upstream, dict) or not isinstance(upstream.get("run_id"), int):
        raise ValueError("missing upstream one-pack run identity")
    if not isinstance(upstream.get("run_attempt"), int):
        raise ValueError("missing upstream one-pack attempt")
    signing_run = receipt.get("signing_run")
    if not isinstance(signing_run, dict) or not isinstance(signing_run.get("id"), int):
        raise ValueError("missing signing run identity")
    if not isinstance(signing_run.get("attempt"), int):
        raise ValueError("missing signing run attempt")
    if provider == "signpath-foundation":
        request = receipt.get("provider_request")
        if not isinstance(request, dict):
            raise ValueError("missing SignPath request identity")
        require_nonempty(request, "id")
        require_nonempty(request, "web_url")

    assets = receipt.get("assets")
    if not isinstance(assets, dict) or set(assets) != set(SIGNED_ASSETS):
        raise ValueError("signing receipt must contain exactly three signed assets")
    for key, relative in SIGNED_ASSETS.items():
        row = assets[key]
        if not isinstance(row, dict) or row.get("path") != relative:
            raise ValueError(f"{key}: invalid signed path")
        before = str(row.get("before_sha256", ""))
        after = str(row.get("after_sha256", ""))
        if not SHA256_RE.fullmatch(before) or not SHA256_RE.fullmatch(after) or before == after:
            raise ValueError(f"{key}: invalid before/after digest")
        path = root / relative
        if not path.is_file() or path.stat().st_size != row.get("after_bytes") or sha256(path) != after:
            raise ValueError(f"{key}: signed bytes do not match receipt")
        if row.get("authenticode_status") != "Valid":
            raise ValueError(f"{key}: Authenticode status is not Valid")
        subject = require_nonempty(row, "signer_subject")
        if not subject_has_publisher(subject, expected_publisher, subject_attributes):
            raise ValueError(f"{key}: publisher identity mismatch")
        if row.get("file_product_name") != "MiniCon":
            raise ValueError(f"{key}: ProductName mismatch")
        if not re.fullmatch(re.escape(product_version) + r"(?:\.0)?", str(row.get("file_product_version", ""))):
            raise ValueError(f"{key}: ProductVersion mismatch")
        for field in (
            "signer_issuer", "signer_thumbprint", "signer_not_before",
            "signer_not_after", "timestamp_subject", "timestamp_issuer",
        ):
            require_nonempty(row, field)


def verify(args: argparse.Namespace) -> None:
    verify_receipt(load(args.receipt), args.root.resolve())
    print("PASS trusted signature + timestamp bound to exact three artifacts")


def self_test() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        assets = {}
        for index, (key, relative) in enumerate(SIGNED_ASSETS.items()):
            path = root / relative
            path.parent.mkdir(parents=True, exist_ok=True)
            path.write_bytes(f"signed-{index}".encode())
            assets[key] = {
                "path": relative,
                "before_sha256": hashlib.sha256(f"unsigned-{index}".encode()).hexdigest(),
                "after_sha256": sha256(path),
                "after_bytes": path.stat().st_size,
                "authenticode_status": "Valid",
                "signer_subject": "CN=SignPath Foundation, C=AT",
                "signer_issuer": "CN=Fixture CA",
                "signer_thumbprint": "A" * 40,
                "signer_not_before": "2026-01-01T00:00:00Z",
                "signer_not_after": "2027-01-01T00:00:00Z",
                "timestamp_subject": "CN=Fixture TSA",
                "timestamp_issuer": "CN=Fixture TSA CA",
                "file_product_name": "MiniCon",
                "file_product_version": "0.1.3",
            }
        receipt = {
            "schema": 2,
            "kind": "minicon-trusted-signing",
            "source_sha": "a" * 40,
            "source_tree_digest": "b" * 64,
            "product_version": "0.1.3",
            "signing_provider": "signpath-foundation",
            "publisher_organization": "SignPath Foundation",
            "file_digest": "SHA256",
            "timestamp_rfc3161": "http://timestamp.invalid",
            "timestamp_digest": "SHA256",
            "upstream": {"run_id": 7, "run_attempt": 1},
            "signing_run": {"id": 8, "attempt": 1},
            "provider_request": {"id": "fixture-request", "web_url": "https://example.invalid/request"},
            "assets": assets,
        }
        verify_receipt(receipt, root)
        assets["minicon.com"]["signer_subject"] = "CN=MiniCon, O=OTHER FOUNDATION"
        try:
            verify_receipt(receipt, root)
        except ValueError as exc:
            assert "identity mismatch" in str(exc)
        else:
            raise AssertionError("wrong publisher identity unexpectedly passed")
        print("PASS signing receipt exact-byte and publisher-identity courts")


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--self-test", action="store_true")
    parser.add_argument("--receipt", type=Path)
    parser.add_argument("--root", type=Path)
    args = parser.parse_args()
    if args.self_test:
        self_test()
    elif args.receipt and args.root:
        verify(args)
    else:
        parser.error("use --self-test or both --receipt and --root")


if __name__ == "__main__":
    main()
