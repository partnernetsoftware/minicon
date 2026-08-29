#!/usr/bin/env python3
"""Bind Defender and operator-supplied 360 evidence to one Candidate APE."""

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


def candidate_identity(manifest: dict) -> tuple[str, dict, str]:
    if manifest.get("kind") != "minicon-release-candidate":
        raise ValueError("wrong Candidate manifest kind")
    asset = next((row for row in manifest.get("assets", []) if row.get("name") == "minicon.com"), None)
    if not asset:
        raise ValueError("Candidate manifest lacks minicon.com")
    expected_sha = require_text(asset, "sha256")
    candidate_run = manifest.get("candidate_run")
    if not isinstance(candidate_run, dict):
        raise ValueError("Candidate run identity missing")
    source_sha = require_text(manifest, "source_sha")
    return source_sha, candidate_run, expected_sha


def qualify(args: argparse.Namespace) -> None:
    manifest_path = pathlib.Path(args.manifest)
    defender_path = pathlib.Path(args.defender)
    court360_path = pathlib.Path(args.court360)
    screenshot_path = pathlib.Path(args.screenshot)
    manifest = load(manifest_path)
    defender = load(defender_path)
    court360 = load(court360_path)

    source_sha, candidate_run, expected_sha = candidate_identity(manifest)

    if defender.get("kind") != "minicon-defender-court" or defender.get("verdict") != "clean":
        raise ValueError("Defender verdict is not clean")
    if defender.get("minicon_com_sha256") != expected_sha:
        raise ValueError("Defender evidence targets a different APE")
    if defender.get("candidate_run") != candidate_run:
        raise ValueError("Defender evidence targets a different Candidate run")
    for field in ("provider", "product_version", "engine_version", "signature_version", "scanned_at"):
        require_text(defender, field)

    if court360.get("schema") != 1 or court360.get("kind") != "minicon-360-court":
        raise ValueError("wrong 360 evidence kind")
    if court360.get("verdict") != "clean":
        raise ValueError("360 verdict is not clean")
    if court360.get("minicon_com_sha256") != expected_sha:
        raise ValueError("360 evidence targets a different APE")
    if court360.get("candidate_run") != candidate_run:
        raise ValueError("360 evidence targets a different Candidate run")
    for field in ("provider", "product_version", "engine_version", "signature_version", "scanned_at"):
        require_text(court360, field)
    if court360.get("screenshot_sha256") != digest(screenshot_path):
        raise ValueError("360 screenshot digest mismatch")

    result = {
        "schema": 1,
        "kind": "minicon-reputation-qualification",
        "source_sha": source_sha,
        "candidate_run": candidate_run,
        "minicon_com_sha256": expected_sha,
        "verdict": "clean",
        "courts": {
            "defender": {"receipt_sha256": digest(defender_path)},
            "360": {
                "receipt_sha256": digest(court360_path),
                "screenshot_sha256": digest(screenshot_path),
            },
        },
    }
    pathlib.Path(args.output).write_text(json.dumps(result, indent=2, sort_keys=True) + "\n", encoding="utf-8")


def verify_qualification(args: argparse.Namespace) -> None:
    manifest = load(pathlib.Path(args.manifest))
    qualification = load(pathlib.Path(args.qualification))
    source_sha, candidate_run, expected_sha = candidate_identity(manifest)
    if qualification.get("schema") != 1 or qualification.get("kind") != "minicon-reputation-qualification":
        raise ValueError("wrong reputation qualification kind")
    if qualification.get("verdict") != "clean":
        raise ValueError("reputation qualification is not clean")
    if qualification.get("source_sha") != source_sha:
        raise ValueError("qualification source SHA mismatch")
    if qualification.get("candidate_run") != candidate_run:
        raise ValueError("qualification Candidate run mismatch")
    if qualification.get("minicon_com_sha256") != expected_sha:
        raise ValueError("qualification APE digest mismatch")
    courts = qualification.get("courts")
    if not isinstance(courts, dict) or set(courts) != {"defender", "360"}:
        raise ValueError("qualification must contain Defender and 360 courts")
    wanted = (("defender", "receipt_sha256"), ("360", "receipt_sha256"), ("360", "screenshot_sha256"))
    for court, field in wanted:
        row = courts.get(court)
        if not isinstance(row, dict) or not SHA_RE.fullmatch(str(row.get(field))):
            raise ValueError(f"invalid {court} {field}")
    print("PASS reputation qualification bound to exact Candidate")


def selftest() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = pathlib.Path(raw)
        screenshot = root / "360.png"
        screenshot.write_bytes(b"synthetic screenshot fixture")
        run = {"id": "42", "attempt": "1"}
        sha = "a" * 64
        manifest = {"kind": "minicon-release-candidate", "source_sha": "b" * 40,
                    "candidate_run": run, "assets": [{"name": "minicon.com", "sha256": sha}]}
        common = {"verdict": "clean", "candidate_run": run, "minicon_com_sha256": sha,
                  "provider": "fixture", "product_version": "1", "engine_version": "1",
                  "signature_version": "1", "scanned_at": "2026-01-01T00:00:00Z"}
        defender = {"kind": "minicon-defender-court", **common}
        court360 = {"schema": 1, "kind": "minicon-360-court",
                    "screenshot_sha256": digest(screenshot), **common}
        paths = {}
        for name, value in (("manifest", manifest), ("defender", defender), ("court360", court360)):
            paths[name] = root / f"{name}.json"
            paths[name].write_text(json.dumps(value), encoding="utf-8")
        output = root / "qualification.json"
        args = argparse.Namespace(**paths, screenshot=screenshot, output=output)
        qualify(args)
        assert load(output)["verdict"] == "clean"
        verify_qualification(argparse.Namespace(manifest=paths["manifest"], qualification=output))
        court360["minicon_com_sha256"] = "c" * 64
        paths["court360"].write_text(json.dumps(court360), encoding="utf-8")
        try:
            qualify(args)
        except ValueError as exc:
            assert "different APE" in str(exc)
        else:
            raise AssertionError("mismatched SHA unexpectedly qualified")
        print("PASS reputation evidence exact-SHA court")


def main() -> None:
    parser = argparse.ArgumentParser()
    sub = parser.add_subparsers(dest="command", required=True)
    court = sub.add_parser("qualify")
    court.add_argument("--manifest", required=True)
    court.add_argument("--defender", required=True)
    court.add_argument("--court360", required=True)
    court.add_argument("--screenshot", required=True)
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
