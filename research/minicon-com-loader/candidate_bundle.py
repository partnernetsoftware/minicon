#!/usr/bin/env python3
"""Seal six-cell MiniCon release coverage plus the exact APE."""
from __future__ import annotations

import argparse
import hashlib
import json
import re
import tempfile
import zipfile
from pathlib import Path

SHA_RE = re.compile(r"^[0-9a-f]{64}$")
SOURCE_RE = re.compile(r"^[0-9a-f]{40}$")
CANDIDATE_CEILING_BYTES = 9_437_184


def sha256(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1 << 20), b""):
            digest.update(chunk)
    return digest.hexdigest()


def asset_names(version: str, include_com: bool) -> list[str]:
    names = [
        f"minicon-{version}-windows-x86_64.zip",
        f"minicon-{version}-windows-arm64.zip",
        f"minicon-{version}-linux-x86_64.tar.gz",
        f"minicon-{version}-linux-arm64.tar.gz",
        f"minicon-{version}-macos-universal.tar.gz",
    ]
    if include_com:
        names.append("minicon.com")
    return names


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


def require_policy(policy: dict, version: str) -> tuple[bool, str]:
    if policy.get("schema") != 1 or policy.get("version") != version:
        raise ValueError("release policy schema/version mismatch")
    assets = policy.get("assets")
    signing = policy.get("signing")
    reputation = policy.get("reputation")
    if not isinstance(assets, dict) or assets.get("native_archives") is not True:
        raise ValueError("release policy must include native archives")
    include_com = assets.get("minicon_com")
    mode = signing.get("mode") if isinstance(signing, dict) else None
    if not isinstance(include_com, bool) or mode not in {"off", "required"}:
        raise ValueError("invalid release asset/signing policy")
    if mode == "required" and not include_com:
        raise ValueError("required signing without minicon.com is not a supported release shape")
    expected_reputation = ["windows-x86_64", "windows-arm64"]
    if include_com:
        expected_reputation.insert(0, "minicon.com")
    if not isinstance(reputation, dict) or reputation.get("mode") != "defender" or reputation.get("assets") != expected_reputation:
        raise ValueError("release reputation policy does not match its asset shape")
    return include_com, mode


def require_identity(build: dict, aggregate: dict, signing: dict | None, source: str,
                     version: str, signing_mode: str) -> None:
    if not SOURCE_RE.fullmatch(source):
        raise ValueError("source SHA must be 40 lowercase hex characters")
    if build.get("source_sha") != source or aggregate.get("source_sha") != source:
        raise ValueError("receipt source SHA mismatch")
    if signing_mode == "required" and (not isinstance(signing, dict) or signing.get("source_sha") != source):
        raise ValueError("signing receipt source SHA mismatch")
    if build.get("source_dirty") is not False:
        raise ValueError("dirty source receipt")
    if build.get("product_version") != version:
        raise ValueError("build receipt product version mismatch")
    tree = build.get("source_tree_digest")
    if not SHA_RE.fullmatch(str(tree)) or aggregate.get("source_tree_digest") != tree:
        raise ValueError("source tree digest mismatch")
    if not SHA_RE.fullmatch(str(build.get("loader_source_sha256"))):
        raise ValueError("missing loader source digest")
    if signing_mode == "required":
        if not isinstance(signing, dict) or signing.get("kind") != "minicon-trusted-signing" or signing.get("product_version") != version:
            raise ValueError("trusted signing identity/version mismatch")
        if signing.get("release_eligible") is not True:
            raise ValueError("qualification-only signing receipt cannot enter a Candidate")
        signing_run = signing.get("signing_run")
        upstream_run = signing.get("upstream")
        if not isinstance(signing_run, dict) or not isinstance(upstream_run, dict):
            raise ValueError("signing/upstream run identity missing")
        if int(signing_run.get("id", -1)) != int(aggregate.get("run_id", -2)) or \
                int(signing_run.get("attempt", -1)) != int(aggregate.get("run_attempt", -2)):
            raise ValueError("signed aggregate/signing run identity mismatch")
        com_signing = signing.get("assets", {}).get("minicon.com", {})
        if com_signing.get("before_sha256") != build.get("minicon_com_sha256"):
            raise ValueError("signing receipt does not consume the unsigned build APE")
        if aggregate.get("minicon_com_sha256") != com_signing.get("after_sha256"):
            raise ValueError("signed aggregate/after-SHA mismatch")
        expected_kind = "minicon-signed-six-cell-aggregate"
    else:
        if signing is not None:
            raise ValueError("signing receipt supplied while signing policy is off")
        if aggregate.get("minicon_com_sha256") != build.get("minicon_com_sha256"):
            raise ValueError("unsigned aggregate/build APE mismatch")
        expected_kind = "minicon-six-cell-aggregate"
    cells = aggregate.get("cells")
    wanted = sorted(
        ["lnx-aarch64", "lnx-x86_64", "osx-aarch64", "osx-x86_64", "win-aarch64", "win-x86_64"]
    )
    if cells != wanted or aggregate.get("kind") != expected_kind:
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
    signing_path = args.signing_receipt.resolve() if args.signing_receipt else None
    policy_path = args.policy.resolve()
    g3_path = args.g3_receipt.resolve()
    build = read_json(build_path)
    aggregate = read_json(aggregate_path)
    policy = read_json(policy_path)
    include_com, signing_mode = require_policy(policy, args.version)
    signing = read_json(signing_path) if signing_path else None
    g3 = read_json(g3_path)
    require_identity(build, aggregate, signing, args.source_sha, args.version, signing_mode)
    require_g3(build, g3, g3_path, args.source_sha)

    expected = asset_names(args.version, include_com)
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

    if include_com:
        com = next(item for item in assets if item["name"] == "minicon.com")
        if com["bytes"] > CANDIDATE_CEILING_BYTES:
            raise ValueError("minicon.com exceeds the stamped 9 MiB Candidate ceiling")
        if signing_mode == "required":
            signed_com = signing["assets"]["minicon.com"]
            if com["sha256"] != signed_com.get("after_sha256") or com["bytes"] != signed_com.get("after_bytes"):
                raise ValueError("minicon.com asset does not match signed after-SHA receipt")
        elif com["sha256"] != build.get("minicon_com_sha256") or com["bytes"] != build.get("minicon_com_bytes"):
            raise ValueError("unsigned minicon.com asset does not match the one-build receipt")

    manifest = {
        "schema": 1,
        "kind": "minicon-release-candidate",
        "version": args.version,
        "expected_tag": f"v{args.version}",
        "source_sha": args.source_sha,
        "source_tree_digest": build["source_tree_digest"],
        "release_policy": {"sha256": sha256(policy_path), **policy},
        "candidate_run": {"id": args.candidate_run_id, "attempt": args.candidate_run_attempt},
        "upstream_run": {
            "id": int(aggregate["run_id"]),
            "attempt": int(aggregate["run_attempt"]),
            "artifact_id": aggregate.get("artifact_id"),
        },
        "receipts": {
            "build": {"name": build_path.name, "sha256": sha256(build_path)},
            "aggregate": {"name": aggregate_path.name, "sha256": sha256(aggregate_path)},
            "g3": {"name": g3_path.name, "sha256": sha256(g3_path)},
        },
        "signing": {"mode": signing_mode},
        "assets": assets,
    }
    reputation_assets = {}
    for key in policy["reputation"]["assets"]:
        if key == "minicon.com":
            row = next(item for item in assets if item["name"] == "minicon.com")
            reputation_assets[key] = {"container": "minicon.com", "sha256": row["sha256"]}
            continue
        platform = {"windows-x86_64": "windows-x86_64", "windows-arm64": "windows-arm64"}[key]
        archive_name = f"minicon-{args.version}-{platform}.zip"
        member = f"minicon-{args.version}-{platform}/minicon.exe"
        with zipfile.ZipFile(payload / archive_name) as zipped:
            binary = zipped.read(member)
        reputation_assets[key] = {
            "container": archive_name, "member": member,
            "sha256": hashlib.sha256(binary).hexdigest(), "bytes": len(binary),
        }
    manifest["reputation_assets"] = reputation_assets
    if signing_mode == "required":
        manifest["unsigned_one_pack_run"] = signing.get("upstream")
        manifest["receipts"]["signing"] = {"name": signing_path.name, "sha256": sha256(signing_path)}
        manifest["signing"].update({
            "provider": signing.get("signing_provider"),
            "publisher_organization": signing.get("publisher_organization"),
            "signed_after_sha256": {
                key: signing["assets"][key]["after_sha256"]
                for key in ("minicon.com", "win-x86_64", "win-aarch64")
            },
        })
    args.output.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    verify_manifest(manifest, payload, policy_path)
    print(json.dumps({"manifest": args.output.name, "assets": len(assets), "source_sha": args.source_sha}, indent=2))


def verify_manifest(manifest: dict, payload: Path, policy_path: Path | None = None) -> None:
    if manifest.get("schema") != 1 or manifest.get("kind") != "minicon-release-candidate":
        raise ValueError("unsupported Candidate manifest")
    version = manifest.get("version")
    if manifest.get("expected_tag") != f"v{version}" or not SOURCE_RE.fullmatch(str(manifest.get("source_sha"))):
        raise ValueError("invalid Candidate identity")
    policy = manifest.get("release_policy")
    if not isinstance(policy, dict):
        raise ValueError("Candidate lacks release policy")
    policy_digest = policy.pop("sha256", None)
    include_com, signing_mode = require_policy(policy, version)
    policy["sha256"] = policy_digest
    if not SHA_RE.fullmatch(str(policy_digest)):
        raise ValueError("invalid release policy digest")
    if policy_path is not None and sha256(policy_path) != policy_digest:
        raise ValueError("release policy file does not match Candidate manifest")
    rows = manifest.get("assets")
    if not isinstance(rows, list) or [row.get("name") for row in rows] != asset_names(version, include_com):
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
    reputation_assets = manifest.get("reputation_assets")
    wanted_reputation = policy["reputation"]["assets"]
    if not isinstance(reputation_assets, dict) or list(reputation_assets) != wanted_reputation:
        raise ValueError("Candidate reputation asset set/order mismatch")
    for key, row in reputation_assets.items():
        if key == "minicon.com":
            raw = (payload / "minicon.com").read_bytes()
        else:
            container, member = row.get("container"), row.get("member")
            with zipfile.ZipFile(payload / str(container)) as zipped:
                raw = zipped.read(str(member))
        if hashlib.sha256(raw).hexdigest() != row.get("sha256") or len(raw) != row.get("bytes", len(raw)):
            raise ValueError(f"{key}: reputation bytes mismatch")
    signing = manifest.get("signing")
    if signing_mode == "off":
        if signing != {"mode": "off"} or "signing" in manifest.get("receipts", {}):
            raise ValueError("unsigned Candidate carries signing identity")
        return
    allowed_publishers = {"azure-artifact-signing": "PARTNERNET SOFTWARE PTY LTD"}
    if not isinstance(signing, dict) or signing.get("mode") != "required" or allowed_publishers.get(signing.get("provider")) != signing.get("publisher_organization"):
        raise ValueError("missing or mismatched trusted signing identity")
    after = signing.get("signed_after_sha256")
    if not isinstance(after, dict) or set(after) != {"minicon.com", "win-x86_64", "win-aarch64"}:
        raise ValueError("missing signed after-SHA set")
    if sha256(payload / "minicon.com") != after["minicon.com"]:
        raise ValueError("sealed minicon.com is not the signed after-SHA")
    if (payload / "minicon.com").stat().st_size > CANDIDATE_CEILING_BYTES:
        raise ValueError("sealed minicon.com exceeds the stamped 9 MiB Candidate ceiling")
    for cell, platform in (("win-x86_64", "windows-x86_64"), ("win-aarch64", "windows-arm64")):
        archive = payload / f"minicon-{version}-{platform}.zip"
        member = f"minicon-{version}-{platform}/minicon.exe"
        with zipfile.ZipFile(archive) as zipped:
            try:
                binary = zipped.read(member)
            except KeyError as exc:
                raise ValueError(f"{archive.name}: missing signed minicon.exe") from exc
        if hashlib.sha256(binary).hexdigest() != after[cell]:
            raise ValueError(f"{archive.name}: embedded PE is not the signed after-SHA")


def verify(args: argparse.Namespace) -> None:
    verify_manifest(read_json(args.manifest), args.payload.resolve(), args.policy.resolve() if args.policy else None)
    print("PASS exact six-cell release set + sidecars")


def self_test() -> None:
    with tempfile.TemporaryDirectory() as raw:
        root = Path(raw)
        payload = root / "payload"
        payload.mkdir()
        version = "0.1.3"
        signed_windows = {
            f"minicon-{version}-windows-x86_64.zip": b"win-x86",
            f"minicon-{version}-windows-arm64.zip": b"win-arm",
        }
        policy_path = root / "release-policy.json"
        policy = {"schema": 1, "version": version,
                  "assets": {"native_archives": True, "minicon_com": True},
                  "signing": {"mode": "required"},
                  "reputation": {"mode": "defender", "assets": [
                      "minicon.com", "windows-x86_64", "windows-arm64"]}}
        policy_path.write_text(json.dumps(policy))
        for index, name in enumerate(asset_names(version, True)):
            path = payload / name
            if name in signed_windows:
                platform = name.removeprefix(f"minicon-{version}-").removesuffix(".zip")
                with zipfile.ZipFile(path, "w") as zipped:
                    zipped.writestr(f"minicon-{version}-{platform}/minicon.exe", signed_windows[name])
            else:
                path.write_bytes(f"asset-{index}".encode())
            (payload / f"{name}.sha256").write_text(f"{sha256(path)}  {name}\n")
        source = "a" * 40
        tree = "b" * 64
        com = payload / "minicon.com"
        build_path = root / "build-receipt.json"
        aggregate_path = root / "aggregate-receipt.json"
        signing_path = root / "signing-receipt.json"
        g3_path = root / "g3-receipt.json"
        output = root / "candidate-manifest.json"
        g3_path.write_text(json.dumps({
            "schema": 1, "kind": "minicon-com-g3-courts", "source_sha": source,
            "job_status": "success", "courts": {
                "reaper": "5 passed, 0 failed", "lifecycle": "3 passed, 0 failed",
                "cosmocc_swap": "PASS rc=2 rollback",
            },
        }))
        unsigned_com_sha = hashlib.sha256(b"unsigned-com").hexdigest()
        build_path.write_text(json.dumps({
            "source_sha": source, "source_dirty": False, "source_tree_digest": tree,
            "product_version": version, "loader_source_sha256": "c" * 64,
            "minicon_com_sha256": unsigned_com_sha, "minicon_com_bytes": 12,
            "g3": {"sha256": sha256(g3_path), "courts": {
                "reaper": "5 passed, 0 failed", "lifecycle": "3 passed, 0 failed",
                "cosmocc_swap": "PASS rc=2 rollback",
            }},
        }))
        signing_path.write_text(json.dumps({
            "kind": "minicon-trusted-signing", "source_sha": source,
            "product_version": version, "signing_provider": "azure-artifact-signing",
            "publisher_organization": "PARTNERNET SOFTWARE PTY LTD", "assets": {
                "minicon.com": {"before_sha256": unsigned_com_sha, "after_sha256": sha256(com),
                                 "after_bytes": com.stat().st_size},
                "win-x86_64": {"after_sha256": hashlib.sha256(b"win-x86").hexdigest()},
                "win-aarch64": {"after_sha256": hashlib.sha256(b"win-arm").hexdigest()},
            },
            "upstream": {"run_id": 6, "run_attempt": 1},
            "signing_run": {"id": 7, "attempt": 1},
            "release_eligible": True,
        }))
        aggregate_path.write_text(json.dumps({
            "kind": "minicon-signed-six-cell-aggregate", "source_sha": source,
            "source_tree_digest": tree,
            "minicon_com_sha256": sha256(com), "run_id": "7", "run_attempt": 1,
            "artifact_id": "9", "cells": sorted([
                "lnx-aarch64", "lnx-x86_64", "osx-aarch64", "osx-x86_64",
                "win-aarch64", "win-x86_64",
            ]),
        }))
        seal(argparse.Namespace(
            payload=payload, build_receipt=build_path, aggregate_receipt=aggregate_path,
            signing_receipt=signing_path, policy=policy_path,
            g3_receipt=g3_path,
            source_sha=source, version=version, candidate_run_id=11,
            candidate_run_attempt=1, output=output,
        ))
        signing_fixture = read_json(signing_path)
        signing_fixture["release_eligible"] = False
        signing_path.write_text(json.dumps(signing_fixture))
        try:
            seal(argparse.Namespace(
                payload=payload, build_receipt=build_path,
                aggregate_receipt=aggregate_path,
                signing_receipt=signing_path, policy=policy_path,
                g3_receipt=g3_path, source_sha=source, version=version,
                candidate_run_id=11, candidate_run_attempt=1, output=output,
            ))
        except ValueError as exc:
            assert "qualification-only" in str(exc)
            print("PASS qualification-only signing cannot enter Candidate")
        else:
            raise SystemExit("qualification-only signing receipt entered Candidate")
        signing_fixture["release_eligible"] = True
        signing_path.write_text(json.dumps(signing_fixture))
        manifest = read_json(output)
        original_com = (payload / "minicon.com").read_bytes()
        (payload / "minicon.com").write_bytes(b"tampered")
        try:
            verify_manifest(manifest, payload)
        except ValueError:
            print("PASS candidate bundle seal/tamper court")
        else:
            raise SystemExit("APE tamper court did not fail")
        (payload / "minicon.com").write_bytes(original_com)
        win = payload / f"minicon-{version}-windows-x86_64.zip"
        with zipfile.ZipFile(win, "w") as zipped:
            zipped.writestr(f"minicon-{version}-windows-x86_64/minicon.exe", b"tampered")
        row = next(item for item in manifest["assets"] if item["name"] == win.name)
        row["bytes"] = win.stat().st_size
        row["sha256"] = sha256(win)
        sidecar = payload / row["sidecar"]["name"]
        sidecar.write_text(f"{row['sha256']}  {win.name}\n")
        row["sidecar"]["sha256"] = sha256(sidecar)
        try:
            verify_manifest(manifest, payload)
        except ValueError as exc:
            assert str(exc) in {
                "windows-x86_64: reputation bytes mismatch",
                f"{win.name}: embedded PE is not the signed after-SHA",
            }
            print("PASS signed Windows archive substitution court")
        else:
            raise SystemExit("signed Windows archive substitution court did not fail")

        unsigned = root / "unsigned"
        unsigned.mkdir()
        unsigned_policy_path = root / "unsigned-policy.json"
        unsigned_policy = {"schema": 1, "version": version,
                           "assets": {"native_archives": True, "minicon_com": True},
                           "signing": {"mode": "off"},
                           "reputation": {"mode": "defender", "assets": [
                               "minicon.com", "windows-x86_64", "windows-arm64"]}}
        unsigned_policy_path.write_text(json.dumps(unsigned_policy))
        for name in asset_names(version, True):
            source_path = payload / name
            target = unsigned / name
            target.write_bytes(b"unsigned-com" if name == "minicon.com" else source_path.read_bytes())
            (unsigned / f"{name}.sha256").write_text(f"{sha256(target)}  {name}\n")
        unsigned_aggregate = dict(read_json(aggregate_path))
        unsigned_aggregate["kind"] = "minicon-six-cell-aggregate"
        unsigned_aggregate["minicon_com_sha256"] = unsigned_com_sha
        aggregate_path.write_text(json.dumps(unsigned_aggregate))
        seal(argparse.Namespace(
            payload=unsigned, build_receipt=build_path, aggregate_receipt=aggregate_path,
            signing_receipt=None, policy=unsigned_policy_path, g3_receipt=g3_path,
            source_sha=source, version=version, candidate_run_id=12,
            candidate_run_attempt=1, output=output,
        ))
        unsigned_manifest = read_json(output)
        assert unsigned_manifest["signing"] == {"mode": "off"}
        assert len(unsigned_manifest["assets"]) == 6
        verify_manifest(unsigned_manifest, unsigned, unsigned_policy_path)
        print("PASS unsigned APE plus five-archive Candidate policy court")


def parser() -> argparse.ArgumentParser:
    top = argparse.ArgumentParser()
    top.add_argument("--self-test", action="store_true")
    sub = top.add_subparsers(dest="command")
    make = sub.add_parser("seal")
    make.add_argument("--payload", type=Path, required=True)
    make.add_argument("--build-receipt", type=Path, required=True)
    make.add_argument("--aggregate-receipt", type=Path, required=True)
    make.add_argument("--signing-receipt", type=Path)
    make.add_argument("--policy", type=Path, required=True)
    make.add_argument("--g3-receipt", type=Path, required=True)
    make.add_argument("--source-sha", required=True)
    make.add_argument("--version", required=True)
    make.add_argument("--candidate-run-id", type=int, required=True)
    make.add_argument("--candidate-run-attempt", type=int, required=True)
    make.add_argument("--output", type=Path, required=True)
    check = sub.add_parser("verify")
    check.add_argument("--manifest", type=Path, required=True)
    check.add_argument("--payload", type=Path, required=True)
    check.add_argument("--policy", type=Path)
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
