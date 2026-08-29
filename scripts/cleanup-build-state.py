#!/usr/bin/env python3
"""Bound local build state without deleting qualification evidence by accident."""

from __future__ import annotations

import argparse
import json
import os
import re
import signal
import shutil
import time
from contextlib import contextmanager
from pathlib import Path


IDENTITY = re.compile(r"^[0-9a-f]{64}$")
CLOUD_IDENTITY = re.compile(r"^minicon-six-grid-([0-9a-f]{64})(?:-|\.)")


def env_int(name: str, default: int, minimum: int = 0) -> int:
    raw = os.environ.get(name, str(default))
    try:
        value = int(raw)
    except ValueError as error:
        raise SystemExit(f"{name} must be an integer") from error
    if value < minimum:
        raise SystemExit(f"{name} must be at least {minimum}")
    return value


def process_alive(pid: int) -> bool:
    try:
        os.kill(pid, 0)
        return True
    except ProcessLookupError:
        return False
    except PermissionError:
        return True


def newest_mtime(path: Path) -> float:
    # Snapshot roots are immutable identities after their owning build marker
    # disappears. Their root timestamp is therefore the lifecycle timestamp;
    # recursively walking 100+ GiB merely to rediscover it makes GC the load.
    return path.stat().st_mtime


def tree_bytes(path: Path) -> int:
    if path.is_file() or path.is_symlink():
        return path.lstat().st_size
    total = 0
    for child in path.rglob("*"):
        try:
            if child.is_file() and not child.is_symlink():
                total += child.stat().st_size
        except FileNotFoundError:
            pass
    return total


class Cleaner:
    def __init__(self, repo: Path, apply: bool, now: float) -> None:
        self.repo = repo
        self.apply = apply
        self.now = now
        self.reclaimed = 0
        self.actions = 0
        self.records: list[dict[str, object]] = []
        override = os.environ.get("MINICON_CLEANUP_FREE_BYTES_OVERRIDE")
        self.free_bytes = int(override) if override is not None else shutil.disk_usage(repo).free
        threshold = env_int("MINICON_DISK_PRESSURE_GIB", 64) * 1024 * 1024 * 1024
        self.disk_pressure = threshold > 0 and self.free_bytes < threshold

    def remove(self, path: Path, reason: str) -> None:
        try:
            path.relative_to(self.repo)
        except ValueError as error:
            raise SystemExit(f"refusing path outside repository: {path}") from error
        size = tree_bytes(path)
        mode = "REMOVE" if self.apply else "WOULD_REMOVE"
        print(f"[cleanup] {mode} bytes={size} reason={reason} path={path.relative_to(self.repo)}")
        self.actions += 1
        self.reclaimed += size
        self.records.append({"path": path.relative_to(self.repo).as_posix(), "bytes": size, "reason": reason})
        if not self.apply:
            return
        if path.is_dir() and not path.is_symlink():
            shutil.rmtree(path)
        else:
            path.unlink(missing_ok=True)


@contextmanager
def cleanup_lock(repo: Path, apply: bool, now: float):
    if not apply:
        yield
        return
    target_six = repo / "target-six"
    target_six.mkdir(exist_ok=True)
    lock = target_six / ".cleanup.lock"
    try:
        lock.mkdir()
    except FileExistsError:
        try:
            owner = json.loads((lock / "owner.json").read_text(encoding="utf-8"))
            owner_pid = int(owner["pid"])
        except (FileNotFoundError, KeyError, TypeError, ValueError, json.JSONDecodeError, OSError):
            owner_pid = -1
        if owner_pid > 0 and process_alive(owner_pid):
            raise SystemExit("another MiniCon cleanup owns target-six/.cleanup.lock")
        shutil.rmtree(lock)
        lock.mkdir()
    (lock / "owner.json").write_text(
        json.dumps({"pid": os.getpid(), "started_epoch": now}) + "\n", encoding="utf-8"
    )
    try:
        yield
    finally:
        shutil.rmtree(lock, ignore_errors=True)


def write_gc_receipt(cleaner: Cleaner, scope: str) -> None:
    if not cleaner.apply or not cleaner.records:
        return
    directory = cleaner.repo / "target-six" / "gc-receipts"
    directory.mkdir(parents=True, exist_ok=True)
    stamp = time.strftime("%Y%m%dT%H%M%SZ", time.gmtime(cleaner.now))
    destination = directory / f"{stamp}-{os.getpid()}.json"
    receipt = {
        "schema": 1,
        "scope": scope,
        "generated_at_epoch": cleaner.now,
        "reclaimed_bytes": cleaner.reclaimed,
        "actions": cleaner.records,
    }
    temporary = destination.with_suffix(".tmp")
    temporary.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    temporary.replace(destination)


def receipt_values(repo: Path) -> tuple[set[str], set[Path]]:
    identities: set[str] = set()
    roots: set[Path] = set()
    receipt_path = repo / "target-six" / "receipt.json"
    try:
        receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
    except (FileNotFoundError, json.JSONDecodeError, OSError):
        return identities, roots
    identity = receipt.get("source_tree_sha256")
    if isinstance(identity, str) and IDENTITY.fullmatch(identity):
        identities.add(identity)
    root = receipt.get("build_root")
    if isinstance(root, str):
        candidate = (repo / root).resolve()
        builds = (repo / "target-six" / "builds").resolve()
        if candidate == builds or builds in candidate.parents:
            roots.add(candidate)
    return identities, roots


def cleanup_build_snapshots(cleaner: Cleaner) -> None:
    builds = cleaner.repo / "target-six" / "builds"
    if not builds.is_dir():
        return
    default_ttl = 1 if cleaner.disk_pressure else 168
    default_keep = 2 if cleaner.disk_pressure else 3
    ttl = env_int("MINICON_BUILD_CACHE_TTL_HOURS", default_ttl, 1) * 3600
    keep = env_int("MINICON_BUILD_CACHE_KEEP", default_keep, 1)
    _, receipt_roots = receipt_values(cleaner.repo)
    protected = {root for root in receipt_roots}
    current = builds / "current"
    if current.is_symlink():
        protected.add(current.resolve())
    snapshots = [path for path in builds.iterdir() if path.is_dir() and IDENTITY.fullmatch(path.name)]
    snapshots.sort(key=newest_mtime, reverse=True)
    protected.update(snapshots[:keep])
    for snapshot in snapshots:
        marker = snapshot / ".minicon-build-active"
        if marker.exists():
            try:
                marker_pid = int(marker.read_text(encoding="utf-8").strip())
            except (OSError, ValueError):
                marker_pid = -1
            if marker_pid > 0 and process_alive(marker_pid):
                protected.add(snapshot.resolve())
        age = cleaner.now - newest_mtime(snapshot)
        if snapshot.resolve() not in protected and age >= ttl:
            cleaner.remove(snapshot, f"stale six-cell snapshot age_hours={int(age / 3600)}")


def cleanup_cloud_runtime(cleaner: Cleaner) -> None:
    cloud = cleaner.repo / "target-six" / "cloud-runtime"
    if not cloud.is_dir():
        return
    ttl = env_int("MINICON_CLOUD_CACHE_TTL_HOURS", 336, 1) * 3600
    keep = env_int("MINICON_CLOUD_CACHE_KEEP", 3, 1)
    receipt_identities, _ = receipt_values(cleaner.repo)
    groups: dict[str, list[Path]] = {}
    for path in cloud.iterdir():
        match = CLOUD_IDENTITY.match(path.name)
        if match and path.is_file():
            groups.setdefault(match.group(1), []).append(path)
    ordered = sorted(groups, key=lambda item: max(path.stat().st_mtime for path in groups[item]), reverse=True)
    protected = set(ordered[:keep]) | receipt_identities
    for identity in ordered:
        age = cleaner.now - max(path.stat().st_mtime for path in groups[identity])
        if identity in protected or age < ttl:
            continue
        archive_receipt = cloud / f"minicon-six-grid-{identity}-archive.json"
        try:
            archived = json.loads(archive_receipt.read_text(encoding="utf-8"))
        except (FileNotFoundError, json.JSONDecodeError, OSError):
            continue
        if archived.get("verified") is not True or archived.get("source_tree_sha256") != identity:
            continue
        # One identity is one evidence unit: archives, manifest, checksum and
        # publication receipt are removed together, never partially, and only
        # after an explicit immutable-archive receipt exists.
        for path in groups[identity]:
            cleaner.remove(path, f"stale cloud evidence group={identity} age_hours={int(age / 3600)}")


def cleanup_routine(cleaner: Cleaner) -> None:
    pycache = cleaner.repo / "scripts" / "__pycache__"
    if pycache.is_dir():
        cleaner.remove(pycache, "regenerable Python bytecode")
    target = cleaner.repo / "target"
    marker = target / ".minicon-build-active"
    ttl = env_int("MINICON_TARGET_TTL_HOURS", 336, 1) * 3600
    if target.is_dir() and not marker.exists():
        age = cleaner.now - newest_mtime(target)
        if age >= ttl:
            cleaner.remove(target, f"stale ordinary Cargo target age_hours={int(age / 3600)}")


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--apply", action="store_true", help="perform removals; default is a dry run")
    parser.add_argument("--scope", choices=("routine", "six-cell", "cloud", "all"), default="all")
    parser.add_argument("--now-epoch", type=float, default=time.time(), help=argparse.SUPPRESS)
    args = parser.parse_args()
    repo = Path(__file__).resolve().parent.parent
    if not (repo / ".git").exists() or not (repo / "Cargo.toml").is_file():
        raise SystemExit("cleanup owner is not a MiniCon repository root")
    cleaner = Cleaner(repo, args.apply, args.now_epoch)
    print(
        f"[cleanup] disk free_bytes={cleaner.free_bytes} "
        f"pressure={str(cleaner.disk_pressure).lower()}"
    )
    signal.signal(signal.SIGTERM, lambda _signum, _frame: (_ for _ in ()).throw(SystemExit(143)))
    with cleanup_lock(repo, args.apply, args.now_epoch):
        if args.scope in ("routine", "all"):
            cleanup_routine(cleaner)
        if args.scope in ("six-cell", "all"):
            cleanup_build_snapshots(cleaner)
        if args.scope in ("cloud", "all", "six-cell"):
            cleanup_cloud_runtime(cleaner)
        write_gc_receipt(cleaner, args.scope)
    print(f"[cleanup] summary apply={str(args.apply).lower()} actions={cleaner.actions} bytes={cleaner.reclaimed}")


if __name__ == "__main__":
    main()
