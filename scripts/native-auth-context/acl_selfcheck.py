#!/usr/bin/env python3
"""Controlled Python consumers of the ACL diagnostic coordinator/workflow.

Owner: PRD 28 browser-context lifecycle. These tests run Python child fixtures,
never Cargo, a native driver, browser, COM, ACL API or provider. Synthetic image
bytes are deliberately not executable. They do not qualify native ACL behavior.
"""
from __future__ import annotations

import argparse
import ast
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys
import tempfile
import textwrap
from types import SimpleNamespace
import unittest

sys.dont_write_bytecode = True
SUBJECT = Path(__file__).resolve().parents[2]
IMAGE = b"NOT-EXECUTABLE: controlled ACL coordinator image\n"


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def write(path, value):
    path.parent.mkdir(parents=True, exist_ok=True)
    path.write_text(json.dumps(value) + "\n", encoding="utf-8")


def child_fixture(phase, scenario_path, events_path, raw_argv):
    """Finite child fixture, not a second implementation of acl_contrast."""
    scenario = json.loads(Path(scenario_path).read_text(encoding="utf-8"))
    def arg(name):
        return raw_argv[raw_argv.index(name) + 1]
    event = {"phase": phase}
    if phase == "arm":
        event["mode"] = arg("--windows-profile-acl-mode")
        event["binary"] = arg("--binary")
        event["build_receipt"] = arg("--build-receipt")
    with Path(events_path).open("a", encoding="utf-8") as stream:
        stream.write(json.dumps(event) + "\n")
    if phase == "protocol":
        write(Path(arg("--output")) / "protocol-guard-receipt.json", {
            "restored_checks": 15, "expected_assertion_reds": ["synthetic-controller-fixture"],
            "byte_exact_source_restored": True,
        })
        return 0
    if phase == "build":
        image = Path(scenario["binary"])
        image.parent.mkdir(parents=True, exist_ok=True)
        image.write_bytes(IMAGE)
        return 0
    if phase != "arm":
        raise ValueError("unknown controlled phase")
    mode = arg("--windows-profile-acl-mode")
    baseline = mode == "owner-only"
    output = Path(arg("--output"))
    output.mkdir(parents=True, exist_ok=False)
    metadata = json.loads(Path(arg("--build-receipt")).read_text(encoding="utf-8"))
    if digest(Path(arg("--binary"))) != metadata["binary_sha256"]:
        return 71
    outcome = scenario["baseline"] if baseline else scenario["control"]
    if not outcome.get("omit_receipt"):
        write(output / "receipt.json", {
            "synthetic_controller_fixture": True,
            "status": outcome["status"], "native_execution_started": True,
            "actual_exit_code": outcome.get("actual_exit", 0),
            "checks": outcome["checks"], "failure_code": outcome.get("failure"),
            "fixture_failure_code": outcome.get("fixture_failure"),
            "driver_windows_profile_acl_mode": outcome.get("driver_mode", mode),
            "windows_profile_acl_observations": [],
        })
        write(output / "native-driver-receipt.json", {
            "failure_code": outcome.get("driver_failure"),
            "windows_cookie_metadata_observations": outcome.get("samples", []),
        })
    if baseline:
        mutation = scenario.get("after_baseline")
        if mutation in ("binary", "canonical", "source"):
            destination = Path(scenario[mutation])
            destination.write_bytes(destination.read_bytes() + b"changed-after-baseline\n")
        elif mutation == "missing-binary":
            Path(scenario["binary"]).unlink()
    return outcome.get("exit", 0)


def import_qualifier():
    spec = importlib.util.spec_from_file_location(
        "controlled_acl_qualifier", SUBJECT / "scripts/native-auth-context/qualify.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def public_oracles():
    source = ast.parse((SUBJECT / "scripts/native-auth-context/run.py").read_text(encoding="utf-8"))
    return next(ast.literal_eval(node.value) for node in source.body
                if isinstance(node, ast.Assign) and any(isinstance(t, ast.Name) and t.id == "CHECKS" for t in node.targets))


def finite_env(**extra):
    env = {name: value for name, value in os.environ.items()
           if name.upper() in {"SYSTEMROOT", "WINDIR", "PATH", "TEMP", "TMP", "HOME"}}
    return dict(env, PYTHONDONTWRITEBYTECODE="1", **extra)


class AclContrastConsumers(unittest.TestCase):
    def exercise(self, baseline=None, control=None, mutation=None):
        module = import_qualifier()
        checks = {name: True for name in public_oracles()}
        self.assertEqual(len(checks), 8, "The unchanged native court has eight original oracles")
        default = {"status": "PASS", "checks": checks}
        with tempfile.TemporaryDirectory(prefix="minicon-acl-controller-") as temporary:
            root = Path(temporary).resolve(strict=True)
            for name in module.INPUTS:
                path = root / name
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_bytes(("controlled-source:" + name).encode())
            module.ROOT = root
            module.SCRIPTS = root / "scripts/native-auth-context"
            binary, canonical = root / "dedicated-image.bin", root / "canonical-image.bin"
            canonical.write_bytes(b"NOT-EXECUTABLE: controlled canonical product\n")
            original_sources = module.hashes()
            output = root / "comparison"
            output.mkdir()
            scenario_path, events_path = root / "scenario.json", root / "events.jsonl"
            scenario = {"baseline": baseline or default, "control": control or default,
                        "binary": str(binary), "canonical": str(canonical),
                        "source": str(root / "Cargo.toml"), "after_baseline": mutation}
            write(scenario_path, scenario)
            original_command = module.command
            def controlled_command(argv, log, env, timeout):
                if len(argv) >= 2 and Path(argv[1]).name == "verify_protocol.py":
                    phase = "protocol"
                elif len(argv) >= 3 and argv[1:3] == ["scripts/build.sh", "dev"]:
                    phase = "build"
                elif len(argv) >= 2 and Path(argv[1]).name == "run.py":
                    phase = "arm"
                else:
                    self.fail("Unexpected command escaped the finite controlled child adapter")
                command = [sys.executable, str(Path(__file__).resolve()), "--controlled-child", phase,
                           str(scenario_path), str(events_path), *map(str, argv)]
                return original_command(command, log, env, min(timeout, 10))
            module.command = controlled_command
            code = module.acl_contrast(SimpleNamespace(target="x86_64-pc-windows-msvc", timeout=3),
                output, finite_env(), "e" * 40, binary, canonical, digest(canonical), original_sources, "controlled-bash")
            aggregate = json.loads((output / "acl-contrast-receipt.json").read_text(encoding="utf-8"))
            events = [json.loads(line) for line in events_path.read_text(encoding="utf-8").splitlines()]
            # Copy plain JSON out before the synthetic authority is removed.
            return code, aggregate, events

    def test_baseline_fail_or_missing_receipt_still_runs_control_and_outer_fails(self):
        partial = {public_oracles()[0]: True, public_oracles()[1]: False}
        samples = [{"phase": "a-first-before-navigation", "status": "observed", "count": 0}]
        for mode in ("FAIL", "BLOCKED", "missing"):
            with self.subTest(baseline=mode):
                baseline = {"status": mode, "checks": partial, "exit": 2,
                            "failure": "synthetic-original-oracle-failure", "samples": samples,
                            "omit_receipt": mode == "missing"}
                code, result, events = self.exercise(baseline=baseline)
                self.assertEqual([e["mode"] for e in events if e["phase"] == "arm"], ["owner-only", "inherit-default"])
                self.assertEqual(code, 1, "A good control cannot erase the original baseline failure")
                self.assertEqual(result["status"], "FAIL")
                self.assertEqual(len(result["arms"]), 2)
                arm = result["arms"][0]
                if mode == "missing":
                    self.assertEqual(arm["failure_code"], "arm-public-receipt-unavailable")
                    self.assertEqual(arm["observed_checks"], {})
                else:
                    self.assertEqual(arm["status"], mode)
                    self.assertEqual(arm["observed_checks"], partial)
                    self.assertEqual(arm["cookie_metadata_observations"], samples)
                self.assertNotIn(public_oracles()[-1], arm["observed_checks"], "Unreached original oracle must remain absent")

    def test_two_passes_are_diagnostic_only_one_build_and_same_image(self):
        code, result, events = self.exercise()
        self.assertEqual(code, 0)
        self.assertEqual(sum(e["phase"] == "build" for e in events), 1, "Actual child build invocation count, not a declared receipt count")
        arms = [event for event in events if event["phase"] == "arm"]
        self.assertEqual([a["mode"] for a in arms], ["owner-only", "inherit-default"])
        self.assertEqual(arms[0]["binary"], arms[1]["binary"])
        self.assertEqual(arms[0]["build_receipt"], arms[1]["build_receipt"])
        self.assertEqual({a["binary_sha256"] for a in result["arms"]}, {hashlib.sha256(IMAGE).hexdigest()})
        self.assertEqual(result["kind"], "native-auth-context-acl-diagnostic-comparison")
        for field in ("native_qualification", "production_artifact_qualification",
                      "qualification_negative_controls_executed", "qualification_restored_run_executed"):
            self.assertIs(result[field], False)

    def test_changed_source_or_bytes_refuses_second_child_and_missing_image_is_failure(self):
        for mutation in ("binary", "canonical", "source", "missing-binary"):
            with self.subTest(mutation=mutation):
                code, result, events = self.exercise(mutation=mutation)
                self.assertEqual([e["mode"] for e in events if e["phase"] == "arm"], ["owner-only"], "Changed authority must not launch a control child")
                self.assertEqual(code, 1)
                self.assertEqual(result["status"], "FAIL")
                self.assertEqual(result["arms"][1]["failure_code"], "exact-image-or-source-changed-before-arm")
                self.assertFalse(result["arms"][1]["native_execution_started"])

    def test_mode_mismatch_preserves_raw_arm_but_never_qualifies_it(self):
        checks = {name: True for name in public_oracles()}
        control = {"status": "PASS", "checks": checks, "driver_mode": "owner-only"}
        code, result, events = self.exercise(control=control)
        self.assertEqual(len([e for e in events if e["phase"] == "arm"]), 2)
        self.assertEqual(result["arms"][1]["status"], "PASS", "Do not rewrite the original arm verdict")
        self.assertEqual(result["arms"][1]["driver_mode"], "owner-only")
        self.assertEqual(result["arms"][1]["observed_checks"], checks)
        self.assertFalse(result["arms"][1]["valid_original_native_pass"])
        self.assertEqual(code, 1)

    def test_actual_workflow_plan_requires_exact_windows_x64_and_exclusive_mode(self):
        text = (SUBJECT / ".github/workflows/dev-loop-crosscheck.yml").read_text(encoding="utf-8")
        match = re.search(r"^          python3 - <<'PY' >> \"\$GITHUB_OUTPUT\"\n(.*?)^          PY\s*$", text, re.M | re.S)
        self.assertIsNotNone(match, "Actual workflow plan heredoc must be selected")
        code = textwrap.dedent(match.group(1))
        for cells, native in (("", "false"), ("all", "false"), ("win-aarch64", "false"),
                              ("lnx-x86_64", "false"), ("win-x86_64 lnx-x86_64", "false"),
                              ("win-x86_64 win-x86_64", "false"), ("win-x86_64", "true")):
            with self.subTest(cells=cells, native=native):
                result = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=10,
                    env=finite_env(CELLS=cells, ACL_CONTRAST="true", NATIVE_AUTH=native))
                self.assertNotEqual(result.returncode, 0, "Invalid diagnostic dispatch must fail, never silently skip")
                self.assertNotIn("matrix=", result.stdout)
        result = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=10,
            env=finite_env(CELLS="win-x86_64", ACL_CONTRAST="true", NATIVE_AUTH="false"))
        self.assertEqual(result.returncode, 0)
        matrix = json.loads(result.stdout.removeprefix("matrix="))["include"]
        self.assertEqual(matrix, [{"cell": "win-x86_64", "runner": "windows-2025", "triple": "x86_64-pc-windows-msvc"}])
        result = subprocess.run([sys.executable, "-c", code], capture_output=True, text=True, timeout=10,
            env=finite_env(CELLS="", ACL_CONTRAST="false", NATIVE_AUTH="true"))
        self.assertEqual(result.returncode, 0)
        self.assertEqual(len(json.loads(result.stdout.removeprefix("matrix="))["include"]), 4)


if __name__ == "__main__":
    if len(sys.argv) >= 3 and sys.argv[1] == "--controlled-child":
        raise SystemExit(child_fixture(sys.argv[2], sys.argv[3], sys.argv[4], sys.argv[5:]))
    parser = argparse.ArgumentParser(add_help=False)
    parser.add_argument("--subject-root", type=Path, default=SUBJECT)
    known, remaining = parser.parse_known_args()
    SUBJECT = known.subject_root.resolve(strict=True)
    unittest.main(argv=[sys.argv[0], *remaining], verbosity=2)
