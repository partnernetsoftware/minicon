#!/usr/bin/env python3
"""Bind Microsoft Defender evidence to the policy-selected Candidate assets."""

from __future__ import annotations

import argparse
import hashlib
import json
import pathlib
import re
import tempfile


SHA_RE = re.compile(r"^[0-9a-f]{64}$")


def load(path: pathlib.Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8-sig"))
    if not isinstance(value, dict):
        raise ValueError(f"{path}: expected object")
    return value


def digest(path: pathlib.Path) -> str:
    return hashlib.sha256(path.read_bytes()).hexdigest()


def require_text(row: dict, name: str) -> str:
    value = row.get(name)
    if not isinstance(value, str) or not value.strip():
        raise ValueError(f"missing {name}")
    return value.strip()


def candidate_identity(manifest: dict) -> tuple[str, dict, dict[str, str], str | None]:
    if manifest.get("kind") != "minicon-release-candidate":
        raise ValueError("wrong Candidate manifest kind")
    policy = manifest.get("release_policy")
    reputation = policy.get("reputation") if isinstance(policy, dict) else None
    wanted = reputation.get("assets") if isinstance(reputation, dict) else None
    if not isinstance(reputation, dict) or reputation.get("mode") != "defender" or not isinstance(wanted, list) or not wanted:
        raise ValueError("Candidate lacks Defender reputation policy")
    rows = manifest.get("reputation_assets")
    if not isinstance(rows, dict):
        raise ValueError("Candidate lacks reputation byte identities")
    expected = {}
    for key in wanted:
        row = rows.get(key)
        if not isinstance(row, dict):
            raise ValueError(f"Candidate lacks reputation asset {key}")
        expected[key] = require_text(row, "sha256")
    candidate_run = manifest.get("candidate_run")
    if not isinstance(candidate_run, dict):
        raise ValueError("Candidate run identity missing")
    source_sha = require_text(manifest, "source_sha")
    signing = manifest.get("signing")
    signing_sha = None
    if isinstance(signing, dict) and signing.get("mode") == "required":
        signing_receipt = manifest.get("receipts", {}).get("signing", {})
        signing_sha = require_text(signing_receipt, "sha256")
        if not SHA_RE.fullmatch(signing_sha):
            raise ValueError("invalid signing receipt digest")
        allowed_publishers = {
            "azure-artifact-signing": "PARTNERNET SOFTWARE PTY LTD",
            "signpath-foundation": "SignPath Foundation",
        }
        if allowed_publishers.get(signing.get("provider")) != signing.get("publisher_organization"):
            raise ValueError("Candidate lacks a valid trusted signing identity")
    elif signing != {"mode": "off"}:
        raise ValueError("invalid Candidate signing mode")
    return source_sha, candidate_run, expected, signing_sha


def qualify(args: argparse.Namespace) -> None:
    manifest_path = pathlib.Path(args.manifest)
    defender_path = pathlib.Path(args.defender)
    manifest = load(manifest_path)
    defender = load(defender_path)

    source_sha, candidate_run, expected, signing_sha = candidate_identity(manifest)

    if defender.get("kind") != "minicon-defender-court" or defender.get("verdict") != "clean":
        raise ValueError("Defender verdict is not clean")
    evidence = defender.get("assets")
    if not isinstance(evidence, dict) or set(evidence) != set(expected):
        raise ValueError("Defender evidence asset set mismatch")
    for key, expected_sha in expected.items():
        row = evidence[key]
        if not isinstance(row, dict) or row.get("sha256") != expected_sha or row.get("post_scan_sha256") != expected_sha:
            raise ValueError(f"Defender evidence targets different bytes for {key}")
    if defender.get("candidate_run") != candidate_run:
        raise ValueError("Defender evidence targets a different Candidate run")
    for field in ("provider", "product_version", "engine_version", "signature_version", "scanned_at"):
        require_text(defender, field)

    result = {
        "schema": 1,
        "kind": "minicon-reputation-qualification",
        "source_sha": source_sha,
        "candidate_run": candidate_run,
        "asset_sha256s": expected,
        "verdict": "clean",
        "courts": {"defender": {"receipt_sha256": digest(defender_path)}},
    }
    if signing_sha:
        result["signing_receipt_sha256"] = signing_sha
    pathlib.Path(args.output).write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def verify_qualification(args: argparse.Namespace) -> None:
    manifest = load(pathlib.Path(args.manifest))
    qualification = load(pathlib.Path(args.qualification))
    source_sha, candidate_run, expected, signing_sha = candidate_identity(manifest)
    if qualification.get("schema") != 1 or qualification.get("kind") != "minicon-reputation-qualification":
        raise ValueError("wrong reputation qualification kind")
    if qualification.get("verdict") != "clean":
        raise ValueError("reputation qualification is not clean")
    if qualification.get("source_sha") != source_sha:
        raise ValueError("qualification source SHA mismatch")
    if qualification.get("candidate_run") != candidate_run:
        raise ValueError("qualification Candidate run mismatch")
    if qualification.get("asset_sha256s") != expected:
        raise ValueError("qualification asset digest mismatch")
    if qualification.get("signing_receipt_sha256") != signing_sha:
        raise ValueError("qualification signing receipt mismatch")
    courts = qualification.get("courts")
    if not isinstance(courts, dict) or set(courts) != {"defender"}:
        raise ValueError("qualification must contain exactly the Defender court")
    wanted = (("defender", "receipt_sha256"),)
    for court, field in wanted:
        row = courts.get(court)
        if not isinstance(row, dict) or not SHA_RE.fullmatch(str(row.get(field))):
            raise ValueError(f"invalid {court} {field}")
    print("PASS reputation qualification bound to exact Candidate")


def selftest() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = pathlib.Path(raw)
        run = {"id": "42", "attempt": "1"}
        sha = "a" * 64
        x86, arm = "e" * 64, "f" * 64
        manifest = {"kind": "minicon-release-candidate", "version": "0.1.4", "source_sha": "b" * 40,
                    "candidate_run": run, "assets": [{"name": "minicon.com", "sha256": sha}],
                    "reputation_assets": {
                        "minicon.com": {"sha256": sha},
                        "windows-x86_64": {"sha256": x86},
                        "windows-arm64": {"sha256": arm},
                    },
                    "release_policy": {"reputation": {"mode": "defender", "assets": [
                        "minicon.com", "windows-x86_64", "windows-arm64"]}},
                    "signing": {"mode": "required", "provider": "signpath-foundation",
                                "publisher_organization": "SignPath Foundation",
                                "signed_after_sha256": {
                                    "minicon.com": sha, "win-x86_64": x86, "win-aarch64": arm}},
                    "receipts": {"signing": {"sha256": "d" * 64}}}
        common = {"verdict": "clean", "candidate_run": run,
                  "assets": {
                      "minicon.com": {"sha256": sha, "post_scan_sha256": sha},
                      "windows-x86_64": {"sha256": x86, "post_scan_sha256": x86},
                      "windows-arm64": {"sha256": arm, "post_scan_sha256": arm},
                  },
                  "provider": "fixture", "product_version": "1", "engine_version": "1",
                  "signature_version": "1", "scanned_at": "2026-01-01T00:00:00Z"}
        defender = {"kind": "minicon-defender-court", **common}
        paths = {}
        for name, value in (("manifest", manifest), ("defender", defender)):
            paths[name] = root / f"{name}.json"
            paths[name].write_text(json.dumps(value), encoding="utf-8")
        output = root / "qualification.json"
        args = argparse.Namespace(**paths, output=output)
        qualify(args)
        assert load(output)["verdict"] == "clean"
        verify_qualification(argparse.Namespace(manifest=paths["manifest"], qualification=output))
        defender["assets"]["minicon.com"]["sha256"] = "c" * 64
        paths["defender"].write_text(json.dumps(defender), encoding="utf-8")
        try:
            qualify(args)
        except ValueError as exc:
            assert "different bytes" in str(exc)
        else:
            raise AssertionError("mismatched SHA unexpectedly qualified")
        print("PASS reputation evidence exact-SHA court")

        unsigned_manifest = {
            "kind": "minicon-release-candidate", "version": "0.1.3", "source_sha": "b" * 40,
            "candidate_run": run,
            "assets": [
                {"name": "minicon-0.1.3-windows-x86_64.zip", "sha256": x86},
                {"name": "minicon-0.1.3-windows-arm64.zip", "sha256": arm},
            ],
            "reputation_assets": {
                "windows-x86_64": {"sha256": x86}, "windows-arm64": {"sha256": arm},
            },
            "release_policy": {"reputation": {"mode": "defender", "assets": ["windows-x86_64", "windows-arm64"]}},
            "signing": {"mode": "off"},
        }
        unsigned_defender = {
            "kind": "minicon-defender-court", "verdict": "clean", "candidate_run": run,
            "assets": {
                "windows-x86_64": {"sha256": x86, "post_scan_sha256": x86},
                "windows-arm64": {"sha256": arm, "post_scan_sha256": arm},
            },
            "provider": "fixture", "product_version": "1", "engine_version": "1",
            "signature_version": "1", "scanned_at": "2026-01-01T00:00:00Z",
        }
        paths["manifest"].write_text(json.dumps(unsigned_manifest))
        paths["defender"].write_text(json.dumps(unsigned_defender))
        qualify(args)
        verify_qualification(argparse.Namespace(manifest=paths["manifest"], qualification=output))
        assert "signing_receipt_sha256" not in load(output)
        print("PASS unsigned native Windows Defender policy court")


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    court = sub.add_parser("qualify")
    court.add_argument("--manifest", required=True)
    court.add_argument("--defender", required=True)
    court.add_argument("--output", required=True)
    verify = sub.add_parser("verify")
    verify.add_argument("--manifest", required=True)
    verify.add_argument("--qualification", required=True)
    sub.add_parser("self-test")
    args = parser.parse_args()
    if args.command == "self-test":
        selftest()
    elif args.command == "qualify":
        qualify(args)
    else:
        verify_qualification(args)


if __name__ == "__main__":
    main()
