#!/usr/bin/env python3
"""Black-box safety checks for cleanup-build-state.py."""

from __future__ import annotations

import json
import os
import shutil
import subprocess
import tempfile
from pathlib import Path


NOW = 2_000_000_000
OLD = NOW - 30 * 24 * 3600


def touch_tree(path: Path, value: bytes = b"x") -> None:
    path.mkdir(parents=True)
    (path / "payload").write_bytes(value)
    os.utime(path / "payload", (OLD, OLD))
    os.utime(path, (OLD, OLD))


def main() -> None:
    source = Path(__file__).with_name("cleanup-build-state.py")
    with tempfile.TemporaryDirectory(prefix="minicon-cleanup-") as temporary:
        repo = Path(temporary)
        (repo / ".git").mkdir()
        (repo / "Cargo.toml").write_text("[package]\nname='fixture'\n", encoding="utf-8")
        scripts = repo / "scripts"
        scripts.mkdir()
        shutil.copy2(source, scripts / source.name)
        builds = repo / "target-six" / "builds"
        current_id, receipt_id, active_id, stale_id = (character * 64 for character in "abcd")
        for identity in (current_id, receipt_id, active_id, stale_id):
            touch_tree(builds / identity)
        os.utime(builds / current_id, (OLD + 300, OLD + 300))
        os.utime(builds / receipt_id, (OLD + 200, OLD + 200))
        os.utime(builds / stale_id, (OLD, OLD))
        (builds / "current").symlink_to(current_id)
        marker = builds / active_id / ".minicon-build-active"
        marker.write_text(f"{os.getpid()}\n", encoding="utf-8")
        os.utime(marker, (NOW, NOW))
        receipt = {
            "source_tree_sha256": receipt_id,
            "build_root": f"target-six/builds/{receipt_id}",
        }
        (repo / "target-six" / "receipt.json").write_text(json.dumps(receipt), encoding="utf-8")
        cloud = repo / "target-six" / "cloud-runtime"
        cloud.mkdir()
        for identity in (receipt_id, stale_id):
            for suffix in ("-manifest.json", "-lnx-x86_64.tar.gz"):
                path = cloud / f"minicon-six-grid-{identity}{suffix}"
                path.write_bytes(b"evidence")
                os.utime(path, (OLD, OLD))
        archive = cloud / f"minicon-six-grid-{stale_id}-archive.json"
        archive.write_text(
            json.dumps({"verified": True, "source_tree_sha256": stale_id}), encoding="utf-8"
        )
        os.utime(archive, (OLD, OLD))
        env = os.environ | {
            "MINICON_BUILD_CACHE_KEEP": "1",
            "MINICON_BUILD_CACHE_TTL_HOURS": "1",
            "MINICON_CLOUD_CACHE_KEEP": "1",
            "MINICON_CLOUD_CACHE_TTL_HOURS": "1",
        }
        command = ["python3", str(scripts / source.name), "--scope", "all", "--now-epoch", str(NOW)]
        dry = subprocess.run(command, cwd=repo, env=env, check=True, text=True, capture_output=True)
        assert "WOULD_REMOVE" in dry.stdout and (builds / stale_id).exists()
        subprocess.run(command + ["--apply"], cwd=repo, env=env, check=True, text=True, capture_output=True)
        assert not (builds / stale_id).exists()
        assert (builds / current_id).exists()
        assert (builds / receipt_id).exists()
        assert (builds / active_id).exists()
        assert list(cloud.glob(f"minicon-six-grid-{receipt_id}*"))
        assert not list(cloud.glob(f"minicon-six-grid-{stale_id}*"))
    print("cleanup-build-state-selftest: PASS")


if __name__ == "__main__":
    main()
