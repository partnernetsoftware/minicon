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
    "Cargo.toml", "Cargo.lock", "src/main.rs", "src/harness_auth_court.rs",
    "src/harness_auth_ui.rs", "src/harness_settings_ui.rs",
    "src/harness_workspace_ui.rs",
    "crates/minicon-web/Cargo.toml",
    "crates/minicon-web/src/lib.rs", "crates/minicon-web/src/native_page.rs",
    "crates/minicon-web/src/native_page/windows_release.rs",
    "crates/minicon-web/src/native_page/windows_cookie_diagnostics.rs",
    "crates/minicon-web/src/profile.rs", "crates/minicon-web/src/wry_host.rs",
    "scripts/native-auth-context/fixture.py", "scripts/native-auth-context/page.js",
    "scripts/native-auth-context/fixture_selfcheck.py", "scripts/native-auth-context/verify_protocol.py",
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


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--helper", type=Path)
    parser.add_argument("--canonical-product", type=Path, required=True)
    parser.add_argument("--timeout", type=int, default=120)
    args = parser.parse_args()
    targets = {f"{isa}-{suffix}": (system, isa) for isa in ("x86_64", "aarch64")
               for system, suffix in (("windows", "pc-windows-msvc"),
                                      ("linux", "unknown-linux-gnu"),
                                      ("macos", "apple-darwin"))}
    if args.target not in targets or not 0 < args.timeout <= 180:
        parser.error("requires a selected native OS/ISA target and timeout in (0, 180]")
    system, isa = targets[args.target]
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
