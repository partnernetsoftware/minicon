#!/usr/bin/env python3
"""Explain and run small, explicit development test profiles.

Changed-path inference is deliberately plan-only until dependency omissions
have a mutation court. The complete local gate remains test-tier.sh l3.
"""

from __future__ import annotations

import argparse
import fnmatch
import json
import os
from pathlib import Path
import re
import shutil
import subprocess
import sys
import tempfile
import time


ROOT = Path(__file__).resolve().parent.parent
INVENTORY = ROOT / "scripts" / "test-profiles.json"


def git_paths(*args: str) -> set[str]:
    output = subprocess.check_output(["git", *args, "-z"], cwd=ROOT)
    return {os.fsdecode(item).replace("\\", "/") for item in output.split(b"\0") if item}


def changed_paths(base: str) -> set[str]:
    # --no-renames exposes both sides of a rename to the same conservative rule.
    paths = git_paths("diff", "--no-renames", "--name-only", base)
    paths |= git_paths("ls-files", "--others", "--exclude-standard")
    return paths


def match_rule(path: str, pattern: str) -> bool:
    if pattern.endswith("/**"):
        return path.startswith(pattern[:-3] + "/")
    return fnmatch.fnmatchcase(path, pattern)


def profiles_for_paths(inventory: dict, paths: list[str]) -> tuple[list[str], dict[str, list[str]]]:
    profiles: list[str] = []
    reasons: dict[str, list[str]] = {}
    for path in paths:
        matches = [
            rule for rule in inventory["changed_path_rules"]
            if match_rule(path, rule["glob"])
        ]
        if not matches:
            matches = [{"profiles": ["full"]}]
        for rule in matches:
            for profile in rule["profiles"]:
                if profile not in profiles:
                    profiles.append(profile)
                reasons.setdefault(profile, []).append(f"path:{path}")
    if "full" in profiles:
        return ["full"], {"full": reasons["full"]}
    return profiles, reasons


def choose_bash() -> str:
    if os.name != "nt":
        return shutil.which("bash") or "bash"
    candidates: list[Path] = []
    override = os.environ.get("MINICON_TEST_BASH")
    if override:
        candidates.append(Path(override))
    git = shutil.which("git")
    if git:
        git_parent = Path(git).resolve().parent
        candidates.extend([git_parent / "bash.exe", git_parent.parent / "bin" / "bash.exe"])
    candidates.extend(
        [Path(r"C:\Program Files\Git\bin\bash.exe"), Path(r"C:\app\Git\bin\bash.exe")]
    )
    for candidate in candidates:
        if candidate.is_file() and "system32" not in str(candidate).lower():
            return str(candidate)
    raise RuntimeError("Git Bash was not found; set MINICON_TEST_BASH")


def plan(inventory: dict, profiles: list[str], phase: str, reasons: dict[str, list[str]]) -> dict:
    phase_index = inventory["phases"].index(phase)
    unknown = sorted(set(profiles) - set(inventory["profiles"]))
    if unknown:
        raise ValueError(f"unknown profile(s): {', '.join(unknown)}")
    selected: dict[str, dict] = {}
    deferred: list[str] = []
    for profile in profiles:
        for name in inventory["profiles"][profile]:
            task = inventory["tasks"][name]
            if inventory["phases"].index(task["phase"]) > phase_index:
                deferred.append(name)
                continue
            selected.setdefault(name, {"name": name, **task, "reasons": []})
            selected[name]["reasons"].extend(reasons.get(profile, [f"profile:{profile}"]))
    if not selected:
        raise ValueError("no tasks selected at this phase; choose a later phase")
    return {
        "schema": 1,
        "phase": phase,
        "profiles": profiles,
        "tasks": list(selected.values()),
        "deferred": sorted(set(deferred)),
        "scope": "development feedback; release still needs l3 and exact-artifact courts",
    }


def run_command(command: list[str], env: dict[str, str] | None = None) -> str:
    print("+ " + " ".join(command), flush=True)
    completed = subprocess.run(
        command, cwd=ROOT, env=env, stdout=subprocess.PIPE, stderr=subprocess.STDOUT,
        text=True, encoding="utf-8", errors="replace", check=False,
    )
    if completed.returncode:
        print(completed.stdout, file=sys.stderr)
        raise RuntimeError(f"task failed with exit {completed.returncode}")
    return completed.stdout


def execute(inventory_plan: dict) -> None:
    bash = choose_bash()
    with tempfile.TemporaryDirectory(prefix="minicon-test-plan-") as temporary:
        for task in inventory_plan["tasks"]:
            environment = None
            if task.get("isolated_config"):
                environment = dict(os.environ)
                environment["MINICON_CONFIG_DIR"] = str(Path(temporary) / task["name"] / "config")
                environment["MINICON_BROWSER_PROFILE_ROOT"] = str(Path(temporary) / task["name"] / "browser")
            started = time.monotonic()
            if task["kind"] == "cargo-test":
                base = [bash, "scripts/build.sh", "test-suite", *task["args"]]
                listing = run_command([*base, *([] if "--" in task["args"] else ["--"]), "--list"], env=environment)
                discovered = [line for line in listing.splitlines() if line.endswith(": test")]
                if not discovered:
                    raise RuntimeError(f"{task['name']}: no tests discovered; refusing an empty pass")
                output = run_command(base, env=environment)
                match = re.search(r"test result: ok\. (\d+) passed; 0 failed; (\d+) ignored;", output)
                if not match or int(match.group(1)) == 0:
                    raise RuntimeError(f"{task['name']}: no executed passing tests")
                observed = int(match.group(1))
                ignored = int(match.group(2))
                if observed + ignored != len(discovered):
                    raise RuntimeError(
                        f"{task['name']}: listed {len(discovered)} tests but observed {observed} passed and {ignored} ignored"
                    )
                print(f"PASS {task['name']}: {observed} passed, {ignored} ignored, {time.monotonic() - started:.1f}s")
            else:
                command = [bash, task["path"], *task.get("args", [])]
                run_command(command, env=environment)
                print(f"PASS {task['name']}: {time.monotonic() - started:.1f}s")


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--list", action="store_true", help="list profiles and tasks")
    parser.add_argument("--plan", action="store_true", help="show selection without execution")
    parser.add_argument("--profile", action="append", default=[], help="explicit profile (repeatable)")
    parser.add_argument("--changed-since", help="Git base for conservative plan-only selection")
    parser.add_argument("--phase", choices=["edit", "integration", "release"], default="edit")
    args = parser.parse_args()
    inventory = json.loads(INVENTORY.read_text(encoding="utf-8"))
    if args.list:
        print(json.dumps(inventory["profiles"], indent=2))
        return 0
    if args.changed_since and not args.plan:
        parser.error("--changed-since is plan-only until omission mutations qualify selection")
    reasons = {name: [f"profile:{name}"] for name in args.profile}
    profiles = list(dict.fromkeys(args.profile))
    paths: list[str] = []
    if args.changed_since:
        paths = sorted(changed_paths(args.changed_since))
        selected, path_reasons = profiles_for_paths(inventory, paths)
        for profile in selected:
            if profile not in profiles:
                profiles.append(profile)
            reasons.setdefault(profile, []).extend(path_reasons[profile])
        if "full" in profiles:
            profiles = ["full"]
            reasons = {"full": reasons.get("full", ["unknown or broad change"])}
    if args.changed_since and not profiles:
        print(json.dumps({"schema": 1, "changed_paths": paths, "tasks": [],
                          "selection_mode": "plan-only; no changed source"}, indent=2))
        return 0
    if "full" in profiles:
        profiles = ["full"]
        reasons = {"full": reasons.get("full", ["profile:full"])}
    if not profiles:
        parser.error("select --profile or --changed-since")
    try:
        phase = "release" if "full" in profiles else args.phase
        result = plan(inventory, profiles, phase, reasons)
        if args.plan:
            if args.changed_since:
                result["changed_paths"] = paths
                result["selection_mode"] = "plan-only; automatic execution not qualified"
            print(json.dumps(result, indent=2, ensure_ascii=False))
        else:
            execute(result)
    except (ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"test-plan: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
