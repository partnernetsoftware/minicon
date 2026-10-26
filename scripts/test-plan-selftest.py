#!/usr/bin/env python3
"""Offline guard for profile selection and empty-test refusal."""

import importlib.util
import json
from pathlib import Path
from unittest import mock


root = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("minicon_test_plan", root / "test-plan.py")
assert spec and spec.loader
planner = importlib.util.module_from_spec(spec)
spec.loader.exec_module(planner)
inventory = json.loads((root / "test-profiles.json").read_text(encoding="utf-8"))

for profile, names in inventory["profiles"].items():
    assert names, profile
    assert all(name in inventory["tasks"] for name in names), profile

edit = planner.plan(inventory, ["control"], "edit", {})
assert [task["name"] for task in edit["tasks"]] == ["control-units"]
assert set(edit["deferred"]) == {"control-cli", "gui-blackbox"}

integrated = planner.plan(inventory, ["control", "terminal"], "integration", {})
names = [task["name"] for task in integrated["tasks"]]
assert names.count("gui-blackbox") == 1, names
assert set(names) == {
    "control-units", "control-cli", "gui-blackbox",
    "terminal-paint-units", "console-agent",
}

profiles, reasons = planner.profiles_for_paths(
    inventory, ["src/control_pending.rs", "unknown/new_adapter.rs"]
)
assert profiles == ["full"] and reasons["full"] == ["path:unknown/new_adapter.rs"]
profiles, _ = planner.profiles_for_paths(inventory, ["src/control_pending.rs"])
assert profiles == ["control"]

with mock.patch.object(planner, "choose_bash", return_value="bash"), mock.patch.object(
    planner, "run_command", return_value="Finished test listing with no matching cases\n"
):
    try:
        planner.execute(edit)
    except RuntimeError as error:
        assert "no tests discovered" in str(error), error
    else:
        raise AssertionError("empty selected test set was accepted")

listed = "a: test\nb: test\n"
passed = "test result: ok. 1 passed; 0 failed; 1 ignored; 0 measured; 0 filtered out\n"
with mock.patch.object(planner, "choose_bash", return_value="bash"), mock.patch.object(
    planner, "run_command", side_effect=[listed, passed]
):
    planner.execute({"tasks": [{"name": "one-ignored", "kind": "cargo-test", "args": []}]})

calls = []
original_config = planner.os.environ.get("MINICON_CONFIG_DIR")
original_browser = planner.os.environ.get("MINICON_BROWSER_PROFILE_ROOT")


def exact_isolated_command(command, env=None):
    assert command.count("--") == 1 and "--exact" in command[command.index("--") + 1:], "exact selector must have one libtest boundary"
    assert env is not None and env["MINICON_CONFIG_DIR"] != "owner-config" and env["MINICON_BROWSER_PROFILE_ROOT"] != "owner-browser", "smoke must not inherit owner profiles"
    calls.append((command, dict(env)))
    return "one: test\n" if "--list" in command else "test result: ok. 1 passed; 0 failed; 0 ignored;\n"


with mock.patch.dict(planner.os.environ, {"MINICON_CONFIG_DIR": "owner-config", "MINICON_BROWSER_PROFILE_ROOT": "owner-browser"}), mock.patch.object(
    planner, "choose_bash", return_value="bash"
), mock.patch.object(planner, "run_command", side_effect=exact_isolated_command):
    planner.execute({"tasks": [{"name": "isolated-exact", "kind": "cargo-test", "isolated_config": True,
                               "args": ["--test", "example", "one", "--", "--exact"]}]})
assert len(calls) == 2 and "--list" in calls[0][0] and "--list" not in calls[1][0], "discover and execute exactly once"
assert planner.os.environ.get("MINICON_CONFIG_DIR") == original_config and planner.os.environ.get("MINICON_BROWSER_PROFILE_ROOT") == original_browser, "fixture environment must restore"

print("test-plan selftest: PASS")
