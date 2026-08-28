#!/usr/bin/env python3
"""Package already-linked six-cell products and test harnesses for remote runtime courts."""

from __future__ import annotations

import argparse
import gzip
import hashlib
import json
import os
import re
import shutil
import stat
import subprocess
import tarfile
import tempfile
from pathlib import Path


CELLS = {
    "osx-aarch64": ("osx-aarch64/aarch64-apple-darwin", "minicon", "macos-runtime-qualify.sh"),
    "osx-x86_64": ("osx-x86_64/x86_64-apple-darwin", "minicon", "macos-runtime-qualify.sh"),
    "win-x86_64": ("win-x86_64/x86_64-pc-windows-msvc", "minicon.exe", "windows-runtime-qualify.ps1"),
    "win-aarch64": ("win-aarch64/aarch64-pc-windows-msvc", "minicon.exe", "windows-runtime-qualify.ps1"),
    "lnx-x86_64": ("lnx-x86_64/x86_64-unknown-linux-gnu", "minicon", "linux-runtime-qualify.sh"),
    "lnx-aarch64": ("lnx-aarch64/aarch64-unknown-linux-gnu", "minicon", "linux-runtime-qualify.sh"),
}

COMMON_TESTS = (
    "minicon",
    "minicon_core",
    "minicon_alignment",
    "minicon_load_portability",
    "minicon_console_agent",
    "minicon_control",
    "minicon_blackbox",
)
LINUX_TESTS = COMMON_TESTS + ("minicon_accessibility_linux",)
THROUGHPUT_TEST = ("minicon_throughput",)
MAX_CELL_ARCHIVE_BYTES = 64 * 1024 * 1024


def digest(path: Path) -> str:
    value = hashlib.sha256()
    with path.open("rb") as source:
        for chunk in iter(lambda: source.read(1024 * 1024), b""):
            value.update(chunk)
    return value.hexdigest()


def one_harness(deps: Path, prefix: str, windows: bool, product: Path | None = None) -> Path:
    suffix = r"\.exe" if windows else ""
    pattern = re.compile(rf"^{re.escape(prefix)}-[0-9a-f]+{suffix}$")
    matches = sorted(path for path in deps.glob(f"{prefix}-*") if path.is_file() and pattern.fullmatch(path.name))
    # Cargo also places a hash-named copy of the ordinary binary in deps. It
    # accepts product CLI arguments rather than libtest arguments. Exclude it
    # by exact byte identity; timestamps and filename hashes are not authority.
    if prefix == "minicon" and product is not None:
        product_digest = digest(product)
        matches = [path for path in matches if digest(path) != product_digest]
    if len(matches) != 1:
        names = [path.name for path in matches]
        raise SystemExit(f"expected exactly one {prefix} harness in {deps}, found {names}")
    return matches[0]


def copy_executable(source: Path, destination: Path) -> None:
    destination.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(source, destination)
    destination.chmod(destination.stat().st_mode | stat.S_IXUSR | stat.S_IXGRP | stat.S_IXOTH)


def write_deterministic_archive(root: Path, archive: Path) -> None:
    with archive.open("wb") as raw:
        with gzip.GzipFile(filename="", mode="wb", fileobj=raw, mtime=0) as compressed:
            with tarfile.open(fileobj=compressed, mode="w", format=tarfile.PAX_FORMAT) as target:
                for path in sorted(root.rglob("*")):
                    info = target.gettarinfo(str(path), arcname=path.relative_to(root).as_posix())
                    info.uid = info.gid = 0
                    info.uname = info.gname = ""
                    info.mtime = 0
                    info.mode = 0o755 if path.is_dir() or os.access(path, os.X_OK) else 0o644
                    if path.is_file():
                        with path.open("rb") as source:
                            target.addfile(info, source)
                    else:
                        target.addfile(info)


def package_cell(repo: Path, build_root: Path, output: Path, identity: str, cell: str) -> dict[str, object]:
    relative, product_name, driver_name = CELLS[cell]
    windows = cell.startswith("win-")
    linux = cell.startswith("lnx-")
    with tempfile.TemporaryDirectory(prefix=f"minicon-{cell}-") as temporary:
        root = Path(temporary) / "payload"
        tests_by_profile: dict[str, dict[str, str]] = {}
        for profile, prefixes in (("debug", LINUX_TESTS if linux else COMMON_TESTS), ("release-fast", THROUGHPUT_TEST)):
            # Linux debug harnesses carry 130+ MiB of DWARF each. Runtime
            # qualification needs executable semantics, not debug symbols, and
            # the owning build already links all release-fast test targets.
            source_profile_name = "release-fast" if linux else profile
            source_profile = build_root / relative / source_profile_name
            product = source_profile / product_name
            if not product.is_file():
                raise SystemExit(f"missing {cell} product: {product}")
            copy_executable(product, root / "target" / profile / product_name)
            for prefix in prefixes:
                harness = one_harness(source_profile / "deps", prefix, windows, product)
                copy_executable(harness, root / "target" / profile / "deps" / harness.name)
                tests_by_profile.setdefault(profile, {})[prefix] = harness.name

        shutil.copy2(repo / "scripts" / driver_name, root / driver_name)
        if not windows:
            (root / driver_name).chmod(0o755)

        if windows:
            (root / "target" / "test-manifest.json").write_text(
                json.dumps({
                    "schema": 2,
                    "source_tree_sha256": identity,
                    "profiles": {
                        profile: {"product": product_name, "tests": tests}
                        for profile, tests in tests_by_profile.items()
                    },
                }, indent=2) + "\n",
                encoding="utf-8",
            )

        source_root = root / "source"
        source_root.mkdir()
        for name in ("Cargo.toml", "alignment-contract.json", "evidence-registry.json"):
            shutil.copy2(repo / name, source_root / name)
        shutil.copytree(repo / "prd", source_root / "prd")
        shutil.copytree(repo / "tests", source_root / "tests")

        files = []
        for path in sorted(item for item in root.rglob("*") if item.is_file()):
            files.append({
                "path": path.relative_to(root).as_posix(),
                "bytes": path.stat().st_size,
                "sha256": digest(path),
            })
        (root / "bundle.json").write_text(
            json.dumps({"schema": 1, "cell": cell, "source_tree_sha256": identity, "files": files}, indent=2) + "\n",
            encoding="utf-8",
        )

        payload_bytes = sum(path.stat().st_size for path in root.rglob("*") if path.is_file())

        archive = output / f"minicon-six-grid-{identity}-{cell}.tar.gz"
        write_deterministic_archive(root, archive)
    archive_bytes = archive.stat().st_size
    if archive_bytes > MAX_CELL_ARCHIVE_BYTES:
        raise SystemExit(
            f"{cell} runtime body is {archive_bytes} bytes; "
            f"limit is {MAX_CELL_ARCHIVE_BYTES} (debug symbols or unrelated files leaked)"
        )
    compression_ratio = archive_bytes / payload_bytes if payload_bytes else 0.0
    print(json.dumps({
        "cell": cell,
        "payload_bytes": payload_bytes,
        "archive_bytes": archive_bytes,
        "compression_ratio": round(compression_ratio, 4),
    }))
    return {
        "cell": cell,
        "asset": archive.name,
        "payload_bytes": payload_bytes,
        "archive_bytes": archive_bytes,
        "compression_ratio": round(compression_ratio, 4),
        "bytes": archive_bytes,
        "sha256": digest(archive),
    }


def reusable_manifest(path: Path, receipt: dict[str, object], receipt_path: Path, output: Path) -> bool:
    if not path.is_file():
        return False
    try:
        manifest = json.loads(path.read_text(encoding="utf-8"))
        if manifest.get("source_sha") != receipt.get("source_sha"):
            return False
        if manifest.get("source_tree_sha256") != receipt.get("source_tree_sha256"):
            return False
        if manifest.get("build_receipt_sha256") != digest(receipt_path):
            return False
        assets = manifest.get("assets", [])
        if {asset.get("cell") for asset in assets} != set(CELLS):
            return False
        return all(
            (output / asset["asset"]).is_file()
            and digest(output / asset["asset"]) == asset["sha256"]
            and (output / asset["asset"]).stat().st_size == asset["bytes"]
            for asset in assets
        )
    except (KeyError, OSError, TypeError, ValueError, json.JSONDecodeError):
        return False


def main() -> None:
    parser = argparse.ArgumentParser()
    parser.add_argument("--receipt", default="target-six/receipt.json")
    parser.add_argument("--output", default="target-six/cloud-runtime")
    parser.add_argument("--allow-dirty", action="store_true", help="diagnostic packaging only; publisher never enables this")
    parser.add_argument("--force", action="store_true", help="rebuild deterministic archives even when verified outputs exist")
    args = parser.parse_args()

    repo = Path(__file__).resolve().parent.parent
    receipt_path = (repo / args.receipt).resolve()
    receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    if receipt.get("source_dirty") and not args.allow_dirty:
        raise SystemExit("refusing to package an authoritative cloud body from a dirty source tree")
    identity = receipt.get("source_tree_sha256", "")
    if not re.fullmatch(r"[0-9a-f]{64}", identity):
        raise SystemExit("receipt has no valid source_tree_sha256")
    if any(stage["status"] == "FAIL" for stage in receipt.get("stages", [])):
        raise SystemExit("refusing to package a build receipt containing FAIL")
    current_state = json.loads(subprocess.check_output(
        ["python3", str(repo / "scripts" / "source-fingerprint.py")], cwd=repo, text=True
    ))
    if current_state.get("sha256") != identity and not args.allow_dirty:
        raise SystemExit("build receipt does not represent the current source tree; rerun scripts/six-cell-qualify.sh")
    build_root_value = receipt.get("build_root")
    build_root = (repo / build_root_value).resolve() if build_root_value else receipt_path.parent / "builds" / identity
    output = (repo / args.output).resolve()
    output.mkdir(parents=True, exist_ok=True)

    manifest_path = output / f"minicon-six-grid-{identity}-manifest.json"
    if not args.force and reusable_manifest(manifest_path, receipt, receipt_path, output):
        print(json.dumps({"manifest": str(manifest_path.relative_to(repo)), "sha256": digest(manifest_path), "assets": 6, "reused": True}))
        return

    assets = [package_cell(repo, build_root, output, identity, cell) for cell in CELLS]
    manifest = {
        "schema": 1,
        "product": "minicon",
        "source_sha": receipt["source_sha"],
        "source_tree_sha256": identity,
        "build_receipt_sha256": digest(receipt_path),
        "assets": assets,
    }
    manifest_path.write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
    checksum_path = manifest_path.with_suffix(manifest_path.suffix + ".sha256")
    checksum_path.write_text(f"{digest(manifest_path)}  {manifest_path.name}\n", encoding="utf-8")
    print(json.dumps({"manifest": str(manifest_path.relative_to(repo)), "sha256": digest(manifest_path), "assets": len(assets)}))


if __name__ == "__main__":
    main()
