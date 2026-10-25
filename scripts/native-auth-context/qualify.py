#!/usr/bin/env python3
"""Build and exercise dedicated native context bytes. Owner: PRD 28 / PRD 32.

Uses an allocated native court desktop. Never run on an owner's active desktop.
All build, source mutation and GUI steps are serialized; default product bytes
remain separate. Native observation failures, not build errors, prove controls.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[2]
SCRIPTS = Path(__file__).resolve().parent
INPUTS = (
    "Cargo.toml", "Cargo.lock", ".github/workflows/dev-loop-crosscheck.yml", "src/main.rs", "src/harness_auth_court.rs", "crates/minicon-web/tests/support/native_auth_fixture.rs",
    "src/harness_auth_ui.rs", "src/harness_settings_ui.rs",
    "src/harness_workspace_ui.rs", "src/browser_process.rs",
    "crates/minicon-web/Cargo.toml",
    "crates/minicon-web/src/lib.rs", "crates/minicon-web/src/native_page.rs",
    "crates/minicon-web/src/native_page/windows_release.rs",
    "crates/minicon-web/src/native_page/windows_cookie_diagnostics.rs",
    "crates/minicon-web/src/profile.rs", "crates/minicon-web/src/wry_host.rs",
    "scripts/native-auth-context/fixture.py", "scripts/native-auth-context/page.js",
    "scripts/native-auth-context/fixture_selfcheck.py", "scripts/native-auth-context/verify_protocol.py",
    "scripts/native-auth-context/acl_selfcheck.py",
    "scripts/native-auth-context/run.py", "scripts/native-auth-context/qualify.py",
    "scripts/build.sh", "scripts/test-plan.py",
)


def sha(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def hashes():
    return {name: sha(ROOT / name) for name in INPUTS}


def write(path, data):
    path.write_text(json.dumps(data, indent=2) + "\n", encoding="utf-8")


def command(args, log, env, timeout):
    with log.open("wb") as stream:
        result = subprocess.run(args, cwd=ROOT, env=env, stdout=stream,
                                stderr=subprocess.STDOUT, timeout=timeout, check=False)
    return result.returncode


def choose_bash():
    spec = importlib.util.spec_from_file_location("minicon_test_plan", ROOT / "scripts/test-plan.py")
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.choose_bash()


def linux_helper_release_controls(output, env, target, bash):
    """Actual owned child/pipe subjects; no desktop, provider or profile reads."""
    source = ROOT / "src/browser_process.rs"
    original = source.read_bytes()
    production, tests = original.decode("utf-8").split("#[cfg(test)]", 1)
    report = dict(status="FAIL", baseline_subjects=7, controls=[], source_restored_exact=False)
    receipt = output / "helper-release-controls.json"

    def subjects(label, pattern, passed, failed, required=()):
        log = output / f"helper-release-{label}.log"
        code = command([bash, "scripts/build.sh", "test-suite", "--target", target,
                        "--bin", "minicon", pattern, "--", "--test-threads=1"], log, env, 900)
        text = log.read_text(encoding="utf-8", errors="replace")
        results = re.findall(r"test result: (ok|FAILED)\. (\d+) passed; (\d+) failed;", text)
        expected = ("FAILED" if failed else "ok", str(passed), str(failed))
        if not results or results[-1] != expected or bool(code) != bool(failed):
            raise RuntimeError(f"{label}: helper subjects did not reach required assertions")
        if any(message not in text for message in required):
            raise RuntimeError(f"{label}: intended helper assertion was not observed")
        return dict(label=label, passed=passed, failed=failed, log=log.name, sha256=sha(log))

    try:
        report["baseline"] = subjects("baseline", "browser_process::tests::", 7, 0)
        variants = (
            ("progress-pid", 'if progress["pid"].as_u64() == Some(u64::from(pid))',
             "if progress.is_object()", "release_progress_requires_the_exact_child", 0, 1,
             ("a different helper must not report release progress",)),
            ("progress-is-not-ack", 'Some("native-close-signalled") => self.native_close_signalled = true,',
             'Some("native-close-signalled") => { self.native_close_signalled = true; self.closed = true; },',
             "release_progress_requires_the_exact_child", 0, 1,
             ("release progress is not a native close acknowledgement",)),
            ("closed-pid", 'if closed["pid"].as_u64() == Some(u64::from(pid))',
             "if closed.is_object()", "closed_frame_requires_the_exact_child", 0, 1,
             ("a different helper must not acknowledge native close",)),
            ("exit-kind", "if !exit.success() {", "if exit.success() {",
             "native_release_", 1, 3,
             ("native_release_requires_the_close_ack_and_actual_normal_child_exit",
              "normal exit alone is not a native close acknowledgement",
              "a matching acknowledgement cannot turn an abnormal exit into release")),
            ("deadline-cause", '"browser-context-release-timeout"',
             '"browser-native-release-abnormal-exit"',
             "native_release_deadline_", 0, 1, ("browser-context-release-timeout",)),
        )
        for label, before, after, pattern, passed, failed, messages in variants:
            if production.count(before) != 1:
                raise RuntimeError(f"{label}: helper mutation anchor changed")
            source.write_text(production.replace(before, after, 1) + "#[cfg(test)]" + tests,
                              encoding="utf-8", newline="\n")
            try:
                red = subjects(label + "-red", pattern, passed, failed, messages)
            finally:
                source.write_bytes(original)
            green = subjects(label + "-restored", "browser_process::tests::", 7, 0)
            report["controls"].append(dict(red=red, restored=green))
        report["status"] = "PASS"
    finally:
        source.write_bytes(original)
        report["source_restored_exact"] = source.read_bytes() == original
        write(receipt, report)
    return report



def acl_contrast(args, output, env, source, binary, canonical, canonical_hash, original_hashes, bash):
    """Diagnostic only: one build, two fresh arms, each original verdict retained."""
    result = dict(kind="native-auth-context-acl-diagnostic-comparison", schema=1,
                  source_commit=source, target=args.target, test_only=True,
                  status="FAIL", native_qualification=False, production_artifact_qualification=False,
                  qualification_negative_controls_executed=False, qualification_restored_run_executed=False,
                  source_sha256=original_hashes, canonical_product_sha256=canonical_hash,
                  controlled_variable="Whole create-new synthetic fixture directory permission policy",
                  timing_interference_possible=True, arms=[], failure_code=None)
    identity = None
    aggregate = output / "acl-contrast-receipt.json"
    def same_sources():
        try:
            return hashes() == original_hashes
        except OSError:
            return False
    def same_bytes(path, expected):
        try:
            return sha(path) == expected
        except OSError:
            return False
    try:
        protocol = command([sys.executable, str(SCRIPTS / "verify_protocol.py"), "--output", str(output / "protocol")],
                           output / "protocol.log", env, 120)
        if protocol != 0 or not same_sources():
            result["failure_code"] = "protocol-or-source-check-failed"
            return 1
        protocol_receipt = output / "protocol/protocol-guard-receipt.json"
        protocol_result = json.loads(protocol_receipt.read_text(encoding="utf-8"))
        if (type(protocol_result["restored_checks"]) is not int or protocol_result["restored_checks"] <= 0
                or not protocol_result["expected_assertion_reds"]
                or protocol_result["byte_exact_source_restored"] is not True):
            result["failure_code"] = "protocol-evidence-unavailable"
            return 1
        result["protocol_receipt_sha256"] = sha(protocol_receipt)
        result["protocol_restored_checks"] = protocol_result["restored_checks"]
        result["protocol_assertion_red_controls"] = len(protocol_result["expected_assertion_reds"])
        build = [bash, "scripts/build.sh", "dev", "--target", args.target,
                 "--bin", "minicon", "--features", "native-auth-court"]
        result["dedicated_build_invocations"] = 1
        if command(build, output / "acl-shared-build.log", env, 900) != 0:
            result["failure_code"] = "dedicated-build-failed-not-runtime-evidence"
            return 1
        identity = sha(binary)
        if not same_sources() or not same_bytes(canonical, canonical_hash):
            result["failure_code"] = "source-or-canonical-changed-after-build"
            return 1
        metadata = dict(schema=1, source_commit=source, test_only=True,
                        features=["default", "experimental-rust-console", "native-auth-court"],
                        diagnostic_features=["minicon-web/native-profile-diagnostics"],
                        binary_sha256=identity, helper_sha256=None, os="windows", isa="x86_64",
                        source_sha256=original_hashes, source_tree_clean=True,
                        variant="acl-shared-diagnostic-image", source_authority="serialized native CI build declaration")
        build_receipt = output / "acl-shared-build-receipt.json"
        write(build_receipt, metadata)
        result["shared_binary_sha256"] = identity
        result["shared_build_receipt_sha256"] = sha(build_receipt)
        for label, mode in (("acl-owner-only", "owner-only"), ("acl-inherit-default", "inherit-default")):
            arm_output = output / label
            log = output / f"{label}-run.log"
            arm = dict(label=label, mode=mode, status="FAIL", process_exit=None,
                       binary_sha256=identity, observed_checks={}, failure_code=None,
                       native_execution_started=False, diagnostic_stage_only=True)
            # A baseline observation FAIL must not prevent the second arm.
            # Identity/source failure is recorded separately; never run changed bytes.
            if not same_bytes(binary, identity) or not same_bytes(canonical, canonical_hash) or not same_sources():
                arm["failure_code"] = "exact-image-or-source-changed-before-arm"
                result["arms"].append(arm)
                continue
            argv = [sys.executable, str(SCRIPTS / "run.py"), "--binary", str(binary),
                    "--output", str(arm_output), "--timeout", str(args.timeout),
                    "--build-receipt", str(build_receipt), "--windows-profile-acl-mode", mode]
            try:
                arm["process_exit"] = command(argv, log, env, args.timeout + 30)
                receipt_path = arm_output / "receipt.json"
                receipt = json.loads(receipt_path.read_text(encoding="utf-8"))
                arm.update(status=receipt["status"], receipt=f"{label}/receipt.json",
                           receipt_sha256=sha(receipt_path), native_execution_started=receipt["native_execution_started"],
                           observed_checks=receipt.get("checks", {}), failure_code=receipt.get("failure_code"),
                           fixture_failure_code=receipt.get("fixture_failure_code"), actual_exit_code=receipt.get("actual_exit_code"),
                           driver_mode=receipt.get("driver_windows_profile_acl_mode"),
                           finite_acl_observations=receipt.get("windows_profile_acl_observations", []))
                driver_path = arm_output / "native-driver-receipt.json"
                if driver_path.exists():
                    driver = json.loads(driver_path.read_text(encoding="utf-8"))
                    arm["driver_failure_code"] = driver.get("failure_code")
                    arm["cookie_metadata_observations"] = driver.get("windows_cookie_metadata_observations", [])
                    arm["driver_receipt_sha256"] = sha(driver_path)
                arm["valid_original_native_pass"] = (arm["process_exit"] == 0 and arm["status"] == "PASS"
                    and arm["actual_exit_code"] == 0 and arm["driver_mode"] == mode)
            except subprocess.TimeoutExpired:
                arm.update(status="FAIL", failure_code="arm-controller-timeout", owned_cleanup_not_observed=True)
            except (OSError, ValueError, KeyError, TypeError):
                arm.update(status="FAIL", failure_code="arm-public-receipt-unavailable")
            if log.exists():
                arm["run_log_sha256"] = sha(log)
            arm["binary_unchanged_after"] = same_bytes(binary, identity)
            arm["source_unchanged_after"] = same_sources()
            result["arms"].append(arm)
            write(aggregate, result)
        initial_acl = []
        for arm in result["arms"]:
            snapshot = next((item for item in arm.get("finite_acl_observations", [])
                             if item.get("stage") == "allocated-before-driver-policy"), {})
            entries = [snapshot.get("output_root", {}), snapshot.get("profiles_parent", {})]
            entries.extend(snapshot.get("profiles", {}).get(label, {}) for label in ("a", "b"))
            if all(entry.get("status") == "observed" and "ace_count" in entry for entry in entries):
                fields = ("protected", "dacl_present", "null_dacl", "ace_count", "inherited_ace_count", "allow_ace_count", "other_ace_count")
                initial_acl.append([[entry.get(key) for key in fields] for entry in entries])
        result["initial_acl_structure_comparison"] = (
            "unavailable" if len(initial_acl) != 2 else
            "inconclusive-no-observed-structural-difference" if initial_acl[0] == initial_acl[1] else
            "structural-difference-observed-not-effective-access-proof")
        result["effective_child_access_or_cookie_flush_claimed"] = False
        if (len(result["arms"]) == 2 and all(arm.get("valid_original_native_pass") for arm in result["arms"])
                and all(arm.get("binary_unchanged_after") and arm.get("source_unchanged_after") for arm in result["arms"])):
            result["status"] = "PASS"
        else:
            result["failure_code"] = "one-or-more-original-arms-failed-or-unavailable"
    except (OSError, ValueError, KeyError, TypeError, subprocess.SubprocessError):
        result["failure_code"] = "comparison-setup-or-io-unavailable"
    finally:
        result["source_unchanged"] = same_sources()
        result["canonical_product_unchanged"] = same_bytes(canonical, canonical_hash)
        result["shared_binary_unchanged"] = identity is not None and same_bytes(binary, identity)
        if not all(result[name] for name in ("source_unchanged", "canonical_product_unchanged", "shared_binary_unchanged")):
            result["status"] = "FAIL"
            if result["failure_code"] is None:
                result["failure_code"] = "exact-image-source-or-canonical-changed"
        write(aggregate, result)
    return 0 if result["status"] == "PASS" else 1


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--helper", type=Path)
    parser.add_argument("--canonical-product", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=120)
    parser.add_argument("--windows-profile-acl-contrast", action="store_true",
                        help="Windows x64 diagnostic only: same-image fresh permission-policy arms; not qualification")
    args = parser.parse_args()
    targets = {f"{isa}-{suffix}": (system, isa) for isa in ("x86_64", "aarch64")
               for system, suffix in (("windows", "pc-windows-msvc"),
                                      ("linux", "unknown-linux-gnu"),
                                      ("macos", "apple-darwin"))}
    if args.target not in targets or not 0 < args.timeout <= 180:
        parser.error("requires a selected native OS/ISA target and timeout in (0, 180]")
    system, isa = targets[args.target]
    if args.windows_profile_acl_contrast and (sys.platform != "win32" or args.target != "x86_64-pc-windows-msvc" or args.helper):
        parser.error("fixture ACL contrast requires an actual Windows x64 host/target and no helper")
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    canonical = args.canonical_product.resolve(strict=True)
    canonical_hash = sha(canonical)
    helper = args.helper.resolve(strict=True) if args.helper else None
    if (system == "linux") != (helper is not None):
        parser.error("Linux requires exactly one explicit native helper")
    env = os.environ.copy()
    env["CARGO_TARGET_DIR"] = str(ROOT / "target/native-auth-court-build")
    source = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    if not re.fullmatch(r"[0-9a-f]{40}", source):
        raise RuntimeError("native court source identity is invalid")
    dirty = subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"],
                                    cwd=ROOT, text=True)
    if dirty:
        raise RuntimeError("native court requires an initially clean tracked source")
    original_hashes = hashes()
    bash = choose_bash()
    binary = Path(env["CARGO_TARGET_DIR"]) / args.target / "debug" / ("minicon.exe" if system == "windows" else "minicon")
    receipt = dict(kind="native-auth-context-qualification", schema=1,
                   source_commit=source, target=args.target, test_only=True,
                   production_artifact_qualification=False, status="FAIL",
                   source_sha256=original_hashes, canonical_product_sha256=canonical_hash,
                   runs=[], source_restored_exact=False)

    if args.windows_profile_acl_contrast:
        return acl_contrast(args, output, env, source, binary, canonical, canonical_hash, original_hashes, bash)

    def run(label, negative=False):
        print(f"native auth {label}: build dedicated feature", flush=True)
        build_log = output / f"{label}-build.log"
        build = [bash, "scripts/build.sh", "dev", "--target", args.target,
                 "--bin", "minicon", "--features", "native-auth-court"]
        if command(build, build_log, env, 900) != 0:
            raise RuntimeError(f"{label}: build failed; this is not a runtime counterexample")
        metadata = dict(schema=1, source_commit=source, test_only=True,
                        features=["default", "experimental-rust-console", "native-auth-court"],
                        diagnostic_features=["minicon-web/native-profile-diagnostics"] if system == "windows" else [],
                        binary_sha256=sha(binary),
                        helper_sha256=sha(helper) if helper else None,
                        os=system, isa=isa, source_sha256=hashes(),
                        source_tree_clean=not negative, variant=label,
                        source_authority="serialized native CI build declaration")
        build_receipt = output / f"{label}-build-receipt.json"
        write(build_receipt, metadata)
        run_output = output / label
        argv = [sys.executable, str(SCRIPTS / "run.py"), "--binary", str(binary),
                "--output", str(run_output), "--timeout", str(args.timeout),
                "--build-receipt", str(build_receipt)]
        if helper:
            argv.extend(["--helper", str(helper)])
        code = command(argv, output / f"{label}-run.log", env, args.timeout + 30)
        result = json.loads((run_output / "receipt.json").read_text(encoding="utf-8"))
        observed = json.loads((run_output / "fixture-receipt.json").read_text(encoding="utf-8")) if (run_output / "fixture-receipt.json").exists() else {}
        driver = json.loads((run_output / "native-driver-receipt.json").read_text(encoding="utf-8")) if (run_output / "native-driver-receipt.json").exists() else {}
        if negative:
            expected_failure = {"previous-account-context-reused": "context-b-inherited-storage",
                                "ordinary-reload-fault": "ordinary-document-reloaded"}[label]
            valid = (code != 0 and result["status"] == "FAIL"
                     and result["native_execution_started"] is True
                     and driver.get("failure_code") == "native-court-page-observation-failed"
                     and observed.get("observation_status") == "failed"
                     and observed.get("failure") == expected_failure)
        else:
            valid = code == 0 and result["status"] == "PASS" and result["actual_exit_code"] == 0
            if system == "linux":
                valid = valid and any(record.get("phase") == "a-reopen"
                    and record.get("native_host_present") is True
                    and record.get("owned_profile_release") == "matching-normal-exit"
                    for record in driver.get("linux_context_observations", []))
        receipt["runs"].append(dict(label=label, runtime_red=negative, valid=valid,
                                    process_exit=code, binary_sha256=metadata["binary_sha256"],
                                    receipt=f"{label}/receipt.json",
                                    receipt_sha256=sha(run_output / "receipt.json"),
                                    observed_failure=observed.get("failure")))
        write(output / "qualification-receipt.json", receipt)
        if not valid:
            if result["status"] == "BLOCKED":
                receipt["status"] = "BLOCKED"
            raise RuntimeError(f"{label}: missing required {'runtime RED' if negative else 'native PASS'}")
        if sha(canonical) != canonical_hash or sha(binary) != metadata["binary_sha256"]:
            raise RuntimeError("product or test binary changed during the court")
        print(f"native auth {label}: {'actual observation RED' if negative else '8 checks PASS'}", flush=True)

    paths = (ROOT / "src/harness_auth_ui.rs", ROOT / "src/harness_auth_court.rs")
    originals = {path: path.read_bytes() for path in paths}
    try:
        protocol = command([sys.executable, str(SCRIPTS / "verify_protocol.py"), "--output", str(output / "protocol")],
                           output / "protocol.log", env, 120)
        if protocol != 0 or hashes() != original_hashes:
            raise RuntimeError("fixture protocol controls failed or changed source")
        protocol_result = json.loads((output / "protocol/protocol-guard-receipt.json").read_text(encoding="utf-8"))
        checks = protocol_result["restored_checks"]
        controls = len(protocol_result["expected_assertion_reds"])
        if (type(checks) is not int or checks <= 0 or controls <= 0
                or protocol_result["byte_exact_source_restored"] is not True):
            raise RuntimeError("protocol evidence counts unavailable")
        receipt["protocol_restored_checks"] = checks
        receipt["protocol_assertion_red_controls"] = controls
        receipt["protocol_gate"] = f"{checks} checks and {controls} actual assertion RED controls; exact source restored"
        if system == "linux":
            helper_controls = linux_helper_release_controls(output, env, args.target, bash)
            receipt["helper_release_controls"] = {
                "receipt": "helper-release-controls.json",
                "sha256": sha(output / "helper-release-controls.json"),
                "status": helper_controls["status"],
            }
            if hashes() != original_hashes:
                raise RuntimeError("helper controls did not restore exact source")
        run("initial")
        # Reuse the previous account's explicit context in actual product
        # dispatch. Ordinary remains separate; B must observe A's storage.
        path = paths[0]
        text = originals[path].decode("utf-8").replace("\r\n", "\n")
        argument = "        context: browser::PageContext,\n        url: &str,"
        anchor = "        let current = self\n            .harness_settings"
        if text.count(argument) != 1 or text.count(anchor) != 1:
            raise RuntimeError("explicit-context mutation anchor changed")
        text = text.replace(argument, argument.replace("context:", "mut context:"), 1)
        text = text.replace(anchor, ("        if let Some(auth) = &self.browser_auth {\n"
                                     "            context = auth.context.clone();\n"
                                     "        }\n" + anchor), 1)
        path.write_bytes(text.encode("utf-8"))
        try:
            run("previous-account-context-reused", negative=True)
        finally:
            path.write_bytes(originals[path])
        # This is a declared test-driver fault injection after real detach,
        # not an adapter mutation: proving that URL reload cannot satisfy the
        # ordinary-document/unsaved-input preservation oracle.
        path = paths[1]
        text = originals[path].decode("utf-8").replace("\r\n", "\n")
        anchor = "                self.app.settings_open = false;\n                self.app.sync_embedded_page(window);"
        if text.count(anchor) != 1:
            raise RuntimeError("ordinary reload fault anchor changed")
        fault = ("                self.app.settings_open = false;\n"
                 "                let url = self.recipe.pages.get(\"ordinary\").unwrap().clone();\n"
                 "                self.app.stop_page_host();\n"
                 "                self.app.present_http_link(window, &url);\n"
                 "                self.app.sync_embedded_page(window);")
        path.write_bytes(text.replace(anchor, fault, 1).encode("utf-8"))
        try:
            run("ordinary-reload-fault", negative=True)
        finally:
            path.write_bytes(originals[path])
        if hashes() != original_hashes:
            raise RuntimeError("source changed outside the declared mutation")
        run("restored")
        receipt["status"] = "PASS"
    finally:
        for path, content in originals.items():
            path.write_bytes(content)
        receipt["source_restored_exact"] = hashes() == original_hashes
        receipt["canonical_product_unchanged"] = sha(canonical) == canonical_hash
        receipt["post_source_sha256"] = hashes()
        write(output / "qualification-receipt.json", receipt)
        if not receipt["source_restored_exact"] or not receipt["canonical_product_unchanged"]:
            raise RuntimeError("court failed exact source/product restoration")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
