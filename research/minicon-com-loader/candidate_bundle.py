#!/usr/bin/env python3
"""Seal six-cell MiniCon release coverage plus the exact APE."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import tempfile
from pathlib import Path

SHA_RE = re.compile(r"^[0-9a-f]{64}$")
SOURCE_RE = re.compile(r"^[0-9a-f]{40}$")


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def asset_names(version: str) -> list[str]:
    return [
        f"minicon-{version}-windows-x86_64.zip",
        f"minicon-{version}-windows-arm64.zip",
        f"minicon-{version}-linux-x86_64.tar.gz",
        f"minicon-{version}-linux-arm64.tar.gz",
        f"minicon-{version}-macos-universal.tar.gz",
        "minicon.com",
    ]


def read_json(path: Path) -> dict:
    value = json.loads(path.read_text(encoding="utf-8"))
    if not isinstance(value, dict):
        raise ValueError(f"{path.name}: expected JSON object")
    return value


def sidecar_digest(path: Path) -> str:
    words = path.read_text(encoding="utf-8-sig").split()
    if not words or not SHA_RE.fullmatch(words[0].lower()):
        raise ValueError(f"{path.name}: invalid SHA-256 sidecar")
    return words[0].lower()


def require_identity(build: dict, aggregate: dict, source: str, version: str) -> None:
    if not SOURCE_RE.fullmatch(source):
        raise ValueError("source SHA must be 40 lowercase hex characters")
    if build.get("source_sha") != source or aggregate.get("source_sha") != source:
        raise ValueError("receipt source SHA mismatch")
    if build.get("source_dirty") is not False or aggregate.get("source_dirty") is not False:
        raise ValueError("dirty source receipt")
    if build.get("product_version") != version:
        raise ValueError("build receipt product version mismatch")
    tree = build.get("source_tree_digest")
    if not SHA_RE.fullmatch(str(tree)) or aggregate.get("source_tree_digest") != tree:
        raise ValueError("source tree digest mismatch")
    if not SHA_RE.fullmatch(str(build.get("loader_source_sha256"))):
        raise ValueError("missing loader source digest")
    if aggregate.get("minicon_com_sha256") != build.get("minicon_com_sha256"):
        raise ValueError("aggregate/build minicon.com digest mismatch")
    cells = aggregate.get("cells")
    wanted = sorted(
        ["lnx-aarch64", "lnx-x86_64", "osx-aarch64", "osx-x86_64", "win-aarch64", "win-x86_64"]
    )
    if cells != wanted:
        raise ValueError("aggregate does not contain exactly six cells")


def require_g3(build: dict, g3: dict, g3_path: Path, source: str) -> None:
    if g3.get("schema") != 1 or g3.get("kind") != "minicon-com-g3-courts":
        raise ValueError("unsupported G3 receipt")
    if g3.get("source_sha") != source or g3.get("job_status") != "success":
        raise ValueError("G3 receipt identity/status mismatch")
    expected = {
        "reaper": "5 passed, 0 failed",
        "lifecycle": "3 passed, 0 failed",
        "cosmocc_swap": "PASS rc=2 rollback",
    }
    if g3.get("courts") != expected:
        raise ValueError("G3 court set/result mismatch")
    bound = build.get("g3")
    if not isinstance(bound, dict) or bound.get("sha256") != sha256(g3_path):
        raise ValueError("build/G3 receipt digest mismatch")
    if bound.get("courts") != expected:
        raise ValueError("build/G3 court summary mismatch")


def seal(args: argparse.Namespace) -> None:
    payload = args.payload.resolve()
    build_path = args.build_receipt.resolve()
    aggregate_path = args.aggregate_receipt.resolve()
    g3_path = args.g3_receipt.resolve()
    build = read_json(build_path)
    aggregate = read_json(aggregate_path)
    g3 = read_json(g3_path)
    require_identity(build, aggregate, args.source_sha, args.version)
    require_g3(build, g3, g3_path, args.source_sha)

    expected = asset_names(args.version)
    allowed = set(expected + [f"{name}.sha256" for name in expected])
    actual = {path.name for path in payload.iterdir() if path.is_file()}
    if actual != allowed:
        raise ValueError(f"payload file set mismatch: expected={sorted(allowed)} actual={sorted(actual)}")

    assets = []
    for name in expected:
        path = payload / name
        sidecar = payload / f"{name}.sha256"
        digest = sha256(path)
        if sidecar_digest(sidecar) != digest:
            raise ValueError(f"{name}: sidecar mismatch")
        assets.append(
            {
                "name": name,
                "bytes": path.stat().st_size,
                "sha256": digest,
                "sidecar": {"name": sidecar.name, "sha256": sha256(sidecar)},
            }
        )

    com = next(item for item in assets if item["name"] == "minicon.com")
    if com["sha256"] != build.get("minicon_com_sha256") or com["bytes"] != build.get("minicon_com_bytes"):
        raise ValueError("minicon.com asset does not match build receipt")

    manifest = {
        "schema": 1,
        "kind": "minicon-release-candidate",
        "version": args.version,
        "expected_tag": f"v{args.version}",
        "source_sha": args.source_sha,
        "source_tree_digest": build["source_tree_digest"],
        "candidate_run": {"id": args.candidate_run_id, "attempt": args.candidate_run_attempt},
        "minicon_com_run": {
            "id": int(aggregate["run_id"]),
            "attempt": int(aggregate["run_attempt"]),
            "artifact_id": aggregate.get("artifact_id"),
        },
        "receipts": {
            "build": {"name": build_path.name, "sha256": sha256(build_path)},
            "aggregate": {"name": aggregate_path.name, "sha256": sha256(aggregate_path)},
            "g3": {"name": g3_path.name, "sha256": sha256(g3_path)},
        },
        "assets": assets,
    }
    args.output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    verify_manifest(manifest, payload)
    print(json.dumps({"manifest": args.output.name, "assets": len(assets), "source_sha": args.source_sha}, indent=2))


def verify_manifest(manifest: dict, payload: Path) -> None:
    if manifest.get("schema") != 1 or manifest.get("kind") != "minicon-release-candidate":
        raise ValueError("unsupported Candidate manifest")
    version = manifest.get("version")
    if manifest.get("expected_tag") != f"v{version}" or not SOURCE_RE.fullmatch(str(manifest.get("source_sha"))):
        raise ValueError("invalid Candidate identity")
    rows = manifest.get("assets")
    if not isinstance(rows, list) or [row.get("name") for row in rows] != asset_names(version):
        raise ValueError("Candidate asset order/set mismatch")
    allowed = {row["name"] for row in rows} | {row["sidecar"]["name"] for row in rows}
    actual = {path.name for path in payload.iterdir() if path.is_file()}
    if actual != allowed:
        raise ValueError("Candidate payload contains missing or extra files")
    for row in rows:
        path = payload / row["name"]
        sidecar = payload / row["sidecar"]["name"]
        if path.stat().st_size != row["bytes"] or sha256(path) != row["sha256"]:
            raise ValueError(f"{path.name}: sealed bytes mismatch")
        if sha256(sidecar) != row["sidecar"]["sha256"] or sidecar_digest(sidecar) != row["sha256"]:
            raise ValueError(f"{sidecar.name}: sealed sidecar mismatch")


def verify(args: argparse.Namespace) -> None:
    verify_manifest(read_json(args.manifest), args.payload.resolve())
    print("PASS exact six-cell release set + sidecars")


def self_test() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        payload = root / "payload"
        payload.mkdir()
        version = "0.1.3"
        for index, name in enumerate(asset_names(version)):
            path = payload / name
            path.write_bytes(f"asset-{index}".encode())
            (payload / f"{name}.sha256").write_text(f"{sha256(path)}  {name}\n")
        source = "a" * 40
        tree = "b" * 64
        com = payload / "minicon.com"
        build_path = root / "build-receipt.json"
        aggregate_path = root / "aggregate-receipt.json"
        g3_path = root / "g3-receipt.json"
        output = root / "candidate-manifest.json"
        g3_path.write_text(json.dumps({
            "schema": 1, "kind": "minicon-com-g3-courts", "source_sha": source,
            "job_status": "success", "courts": {
                "reaper": "5 passed, 0 failed", "lifecycle": "3 passed, 0 failed",
                "cosmocc_swap": "PASS rc=2 rollback",
            },
        }))
        build_path.write_text(json.dumps({
            "source_sha": source, "source_dirty": False, "source_tree_digest": tree,
            "product_version": version, "loader_source_sha256": "c" * 64,
            "minicon_com_sha256": sha256(com), "minicon_com_bytes": com.stat().st_size,
            "g3": {"sha256": sha256(g3_path), "courts": {
                "reaper": "5 passed, 0 failed", "lifecycle": "3 passed, 0 failed",
                "cosmocc_swap": "PASS rc=2 rollback",
            }},
        }))
        aggregate_path.write_text(json.dumps({
            "source_sha": source, "source_dirty": False, "source_tree_digest": tree,
            "minicon_com_sha256": sha256(com), "run_id": "7", "run_attempt": 1,
            "artifact_id": "9", "cells": sorted([
                "lnx-aarch64", "lnx-x86_64", "osx-aarch64", "osx-x86_64",
                "win-aarch64", "win-x86_64",
            ]),
        }))
        seal(argparse.Namespace(
            payload=payload, build_receipt=build_path, aggregate_receipt=aggregate_path,
            g3_receipt=g3_path,
            source_sha=source, version=version, candidate_run_id=11,
            candidate_run_attempt=1, output=output,
        ))
        manifest = read_json(output)
        (payload / "minicon.com").write_bytes(b"tampered")
        try:
            verify_manifest(manifest, payload)
        except ValueError:
            print("PASS candidate bundle seal/tamper court")
            return
        raise SystemExit("tamper court did not fail")


def parser() -> argparse.ArgumentParser:
    top = argparse.ArgumentParser()
    top.add_argument("--self-test", action="store_true")
    sub = top.add_subparsers(dest="command")
    make = sub.add_parser("seal")
    make.add_argument("--payload", type=Path, required=True)
    make.add_argument("--build-receipt", type=Path, required=True)
    make.add_argument("--aggregate-receipt", type=Path, required=True)
    make.add_argument("--g3-receipt", type=Path, required=True)
    make.add_argument("--source-sha", required=True)
    make.add_argument("--version", required=True)
    make.add_argument("--candidate-run-id", type=int, required=True)
    make.add_argument("--candidate-run-attempt", type=int, required=True)
    make.add_argument("--output", type=Path, required=True)
    check = sub.add_parser("verify")
    check.add_argument("--manifest", type=Path, required=True)
    check.add_argument("--payload", type=Path, required=True)
    return top


def main() -> None:
    args = parser().parse_args()
    if args.self_test:
        self_test()
    elif args.command == "seal":
        seal(args)
    elif args.command == "verify":
        verify(args)
    else:
        raise SystemExit("choose seal, verify, or --self-test")


if __name__ == "__main__":
    main()
