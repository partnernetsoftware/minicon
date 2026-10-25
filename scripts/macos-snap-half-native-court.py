#!/usr/bin/env python3
"""Explicit macOS owned-window geometry and source control. Owner: PRD28/32."""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import os
from pathlib import Path
import re
import struct
import subprocess
import sys
import tempfile
import time

EXAMPLE = "macos-snap-half-owned"
FEATURES = ["portable-pixel-window", "input", "ime"]
ADAPTER = "crates/minicon-platform/src/adapters/unix/window_host.rs"
ORACLE = "crates/minicon-platform/examples/macos_snap_half_owned.rs"
MAPPING = "crates/minicon-web/src/native_page/macos_shortcuts.rs"
MAPPING_TEST = "native_page::macos_shortcuts::tests::mac_window_half_arrows_have_distinct_exact_command_ownership"
MAPPING_GUARD = "        0x7b => Some('←'),".encode()
MAPPING_MUTANT = "        0x7b => Some('<'),".encode()
MAPPING_ASSERTION = "assert_eq!(physical_shortcut_character(code), Some(arrow));"
GUARD = b"    native.setFrame_display(target_frame, false);"
REMOVED = b"    // Native half-frame application removed by this temporary source court."
CONTENT_GUARD = b'''    if !content.size.width.is_finite()
        || !content.size.height.is_finite()
        || content.size.width <= 0.0
        || content.size.height <= 0.0
        || content.size.width < content_minimum.width
        || content.size.height < content_minimum.height
        || content.size.width > content_maximum.width
        || content.size.height > content_maximum.height
    {'''
CONTENT_REMOVED = CONTENT_GUARD.replace(b"    if ", b"    if false && (", 1).replace(b"\n    {", b"\n    ) {", 1)
ASSERTION = '''assert!(
                    close(actual, expected),
                    "owned snap frame mismatch at {stage}: actual={actual:?}, expected={expected:?}"
                );'''
CONTENT_ASSERTION = '''assert!(
                close(after_refusal, before_refusal),
                "content-size refusal moved the owned window: before={before_refusal:?}, after={after_refusal:?}"
            );'''
INPUTS = (
    "Cargo.toml", "Cargo.lock", "crates/minicon-platform/Cargo.toml",
    "crates/minicon-platform/src/lib.rs", "crates/minicon-platform/src/contract/window_host.rs",
    "crates/minicon-platform/src/window_host.rs", ADAPTER,
    "crates/minicon-platform/src/adapters/macos/window.rs", ORACLE,
    "scripts/macos-snap-half-native-court.py", "scripts/build.sh",
    "scripts/private-publication-native-court.py", "scripts/native-auth-context/run.py",
    "scripts/native-auth-context/fixture.py", "scripts/test-plan.py",
    "crates/minicon-web/Cargo.toml", "crates/minicon-web/src/lib.rs",
    "crates/minicon-web/src/native_page.rs", MAPPING,
)


class CourtError(Exception):
    pass


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def snapshot(root):
    return {path: digest((root / path).read_bytes()) for path in INPUTS}


def save(path, value):
    with path.open("xb") as stream:
        stream.write((json.dumps(value, indent=2) + "\n").encode())


def close(a, b):
    return (isinstance(a, list) and isinstance(b, list) and len(a) == len(b) == 4
            and all(type(x) in (int, float) and type(y) in (int, float)
                    and math.isfinite(x) and math.isfinite(y) and abs(x - y) <= 1.0
                    for x, y in zip(a, b)))


def validate_success(text):
    value = json.loads(text)
    if (value.get("status") != "PASS" or value.get("scope") != "owned-macos-adapter"
            or value.get("steps") != 3 or value.get("no_activate") is not True
            or value.get("tolerance_points") != 1
            or value.get("content_constraint_refused") is not True):
        raise CourtError("positive-native-receipt-invalid")
    rows = value.get("samples")
    if not isinstance(rows, list) or [row.get("stage") for row in rows] != ["left", "right", "repeat-right"]:
        raise CourtError("positive-native-sequence-missing")
    for index, row in enumerate(rows):
        visible = row.get("visible")
        if not close(visible, visible) or visible[2] <= 0 or visible[3] <= 0:
            raise CourtError("positive-visible-area-invalid")
        target = [visible[0] + (visible[2] / 2 if index else 0), visible[1], visible[2] / 2, visible[3]]
        if row.get("pass") is not True or not close(row.get("expected"), target) or not close(row.get("actual"), target):
            raise CourtError("positive-independent-frame-mismatch")
    if not close(rows[1]["actual"], rows[2]["actual"]):
        raise CourtError("positive-repeat-right-mismatch")
    return value


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", required=True, choices=["aarch64-apple-darwin", "x86_64-apple-darwin"])
    parser.add_argument("--output", required=True, type=Path)
    args = parser.parse_args()
    if sys.platform != "darwin":
        print(json.dumps({"status": "UNSUPPORTED", "reason": "macos-native-owned-window-required"}))
        return 2
    sys.dont_write_bytecode = True
    root = Path(__file__).resolve().parents[1]
    output = args.output.resolve()
    output.relative_to(root / "probe-logs")
    output.mkdir(parents=True, exist_ok=False)
    original = (root / ADAPTER).read_bytes()
    mapping_original = (root / MAPPING).read_bytes()
    mapping_mutant = mapping_original.replace(MAPPING_GUARD, MAPPING_MUTANT)
    source = snapshot(root)
    source_sha = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
    if re.fullmatch(r"[0-9a-f]{40}", source_sha) is None:
        raise CourtError("actual-checkout-identity-unavailable")
    oracle = (root / ORACLE).read_text(encoding="utf-8")
    if (original.count(GUARD) != 1 or oracle.count(ASSERTION) != 1
            or original.count(CONTENT_GUARD) != 1 or oracle.count(CONTENT_ASSERTION) != 1
            or mapping_original.count(MAPPING_GUARD) != 1
            or mapping_original.decode().count(MAPPING_ASSERTION) != 1):
        raise CourtError("review-required-source-guard-or-original-assertion-changed")
    offset = oracle.index(ASSERTION)
    point = {"path": ORACLE, "line": oracle.count("\n", 0, offset) + 1,
             "column": offset - oracle.rfind("\n", 0, offset), "statement_sha256": digest(ASSERTION.encode())}
    offset = oracle.index(CONTENT_ASSERTION)
    content_point = {"path": ORACLE, "line": oracle.count("\n", 0, offset) + 1,
                     "column": offset - oracle.rfind("\n", 0, offset),
                     "statement_sha256": digest(CONTENT_ASSERTION.encode())}
    mapping_text = mapping_original.decode()
    offset = mapping_text.index(MAPPING_ASSERTION)
    mapping_point = {"path": MAPPING, "line": mapping_text.count("\n", 0, offset) + 1,
                     "column": offset - mapping_text.rfind("\n", 0, offset),
                     "statement_sha256": digest(MAPPING_ASSERTION.encode())}
    mutant = original.replace(GUARD, REMOVED)
    content_mutant = original.replace(CONTENT_GUARD, CONTENT_REMOVED)
    spec = importlib.util.spec_from_file_location("snap_existing_native_process", root / "scripts/private-publication-native-court.py")
    helpers = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(helpers)
    owned_type = helpers.owned_process_class(root)
    bash = helpers.choose_bash(root)
    env = dict(os.environ, CARGO_BUILD_JOBS="4", CARGO_TERM_COLOR="never", PYTHONDONTWRITEBYTECODE="1")
    env["CARGO_TARGET_DIR"] = str(root / "target")
    report = {"kind": "macos-owned-snap-half-native-court", "status": "FAIL", "target": args.target,
              "source_sha256": source, "source_hash_scope": "listed source/support inputs; not a whole-dependency build attestation",
              "source_sha": source_sha, "controller_sha": os.environ.get("GITHUB_SHA"),
              "actual_host_os": sys.platform, "actual_host_isa": os.uname().machine,
              "original_assertion": point,
              "content_original_assertion": content_point,
              "mapping_original_assertion": mapping_point,
              "mutant_adapter_sha256": digest(mutant),
              "content_mutant_adapter_sha256": digest(content_mutant), "phases": [], "failures": [],
              "restore_errors": [], "scope": "owned native adapter only; no hardware/WK focus/release qualification"}
    active_adapter_mutant = None
    mapping_installed = False

    def stable(adapter, mapping=None):
        expected = dict(source)
        expected[ADAPTER] = digest(adapter)
        expected[MAPPING] = digest(mapping if mapping is not None else mapping_original)
        if snapshot(root) != expected:
            raise CourtError("source-drift-refusing-operation")
        if subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip() != source_sha:
            raise CourtError("checkout-identity-changed")

    def run(label, command, timeout):
        directory = output / label
        directory.mkdir()
        record = {"phase": label, "command": command, "timeout_seconds": timeout,
                  "exit_code": None, "timed_out": False, "cleanup_ok": False}
        owned = None
        started = time.monotonic()
        try:
            owned = owned_type(command, env, directory)
            while not owned.exited():
                if time.monotonic() - started >= timeout:
                    record["timed_out"] = True
                    raise CourtError("owned-phase-timeout")
                if sum(p.stat().st_size for p in directory.glob("driver.*.log")) > 32 * 1024 * 1024:
                    raise CourtError("owned-log-bound-exceeded")
                time.sleep(0.05)
        finally:
            if owned is not None:
                try:
                    owned.close()
                    record["cleanup_ok"] = True
                except BaseException as error:
                    record["cleanup_error"] = str(error)
                finally:
                    record["exit_code"] = owned.process.returncode
                    record["cleanup"] = owned.cleanup
            record["seconds"] = round(time.monotonic() - started, 3)
            for name in ["driver.stdout.log", "driver.stderr.log"]:
                if (directory / name).exists():
                    record[name + "_sha256"] = digest((directory / name).read_bytes())
            save(directory / "process.json", record)
        return record, (directory / "driver.stdout.log").read_text(encoding="utf-8"), (directory / "driver.stderr.log").read_text(encoding="utf-8")

    def build(label, work):
        canonical = root / "target" / args.target / "debug/examples" / EXAMPLE
        command = [bash, str(root / "scripts/build.sh"), "dev", "--target", args.target,
                   "-p", "minicon-platform", "--no-default-features", "--features", ",".join(FEATURES),
                   "--example", EXAMPLE, "--message-format=json-render-diagnostics",
                   "--config", 'build.rustflags=["-D","dead_code","-D","unused_variables","-D","unused_must_use"]']
        record, stdout, _stderr = run(label + "-build", command, 180)
        if record["exit_code"] != 0 or record["timed_out"] or not record["cleanup_ok"]:
            raise CourtError("build-failed-not-runtime-RED")
        artifacts, features = set(), set()
        for line in stdout.splitlines():
            try:
                item = json.loads(line)
            except ValueError:
                continue
            if item.get("reason") != "compiler-artifact":
                continue
            target = item.get("target", {})
            if target.get("name") == EXAMPLE and "example" in target.get("kind", []) and item.get("executable"):
                artifacts.add(Path(item["executable"]).resolve())
            if target.get("name") == "minicon_platform" and "lib" in target.get("kind", []):
                features.add(tuple(sorted(item.get("features", []))))
        if artifacts != {canonical.resolve(strict=True)} or len(features) != 1 or not set(FEATURES).issubset(next(iter(features))):
            raise CourtError("exact-cargo-example-or-feature-identity-missing")
        raw = canonical.read_bytes()
        expected_cpu = 0x0100000C if args.target.startswith("aarch64") else 0x01000007
        if len(raw) < 8 or raw[:4] != b"\xcf\xfa\xed\xfe" or struct.unpack_from("<I", raw, 4)[0] != expected_cpu:
            raise CourtError("example-thin-MachO-ISA-mismatch")
        pinned = work / label
        with pinned.open("xb") as stream:
            stream.write(raw)
        pinned.chmod(0o700)
        identity = {"sha256": digest(raw), "format": "Mach-O64", "isa": args.target.split("-")[0],
                    "canonical_relative_path": str(canonical.relative_to(root)), "actual_features": list(next(iter(features)))}
        save(output / (label + "-image.json"), identity)
        return pinned, identity

    def phase(label, work, negative=None):
        image, identity = build(label, work)
        record, stdout, stderr = run(label + "-native", [str(image)], 20)
        if digest(image.read_bytes()) != identity["sha256"] or record["timed_out"] or not record["cleanup_ok"]:
            raise CourtError("native-image-or-owned-cleanup-invalid")
        value = json.loads(stdout)
        result = {"phase": label, "image": identity, "process": record, "native": value, "runtime_RED": False}
        if negative:
            panics = list(re.finditer(r"^thread '([^']+)'[^\r\n]*panicked at ([^\r\n]+):(\d+):(\d+):\r?\n", stderr, re.M))
            valid = len(panics) == 1
            if valid:
                hit = panics[0]
                body = stderr[hit.end():].replace("\r\n", "\n").split("\n\n", 1)[0]
                first = body.splitlines()[0] if body else ""
                if negative == "content":
                    geometry = re.fullmatch(r"content-size refusal moved the owned window: before=(\[[^\n]+\]), after=(\[[^\n]+\])", first)
                    expected_point = content_point
                else:
                    geometry = re.fullmatch(r"owned snap frame mismatch at left: actual=(\[[^\n]+\]), expected=(\[[^\n]+\])", first)
                    expected_point = point
                valid = bool(geometry and hit[1] == "main" and hit[2].replace("\\", "/") == expected_point["path"]
                             and (int(hit[3]), int(hit[4])) == (expected_point["line"], expected_point["column"]))
                if valid:
                    first_frame, second_frame = json.loads(geometry[1]), json.loads(geometry[2])
                    valid = close(first_frame, first_frame) and close(second_frame, second_frame) and not close(first_frame, second_frame)
            if not (valid and record["exit_code"] == 1 and value == {"status": "FAIL", "reason": "owned-native-window-error"}):
                raise CourtError("negative-missing-original-geometry-assertion-not-RED")
            result["runtime_RED"] = True
            result["negative_kind"] = negative
        else:
            if record["exit_code"] != 0:
                raise CourtError("positive-native-exit-failed")
            validate_success(stdout)
        report["phases"].append(result)

    def mapping_phase(label, negative=False):
        command = [bash, str(root / "scripts/build.sh"), "test-suite", "--target", args.target,
                   "-p", "minicon-web", "--features", "native-page-host", "--lib", MAPPING_TEST,
                   "--", "--exact", "--nocapture", "--test-threads=1"]
        record, stdout, stderr = run(label, command, 180)
        summaries = re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", stdout, re.M)
        if record["timed_out"] or not record["cleanup_ok"]:
            raise CourtError("mapping-owned-run-invalid")
        expected_isa = "aarch64" if args.target.startswith("aarch64") else "x86_64"
        record["test_image"] = helpers.test_image(root, stdout + "\n" + stderr, expected_isa)
        if negative:
            panics = list(re.finditer(r"^thread '([^']+)'[^\r\n]*panicked at ([^\r\n]+):(\d+):(\d+):\r?\n", stderr, re.M))
            valid = len(panics) == 1
            if valid:
                hit = panics[0]
                body = stderr[hit.end():].replace("\r\n", "\n").split("\n\n", 1)[0]
                valid = (hit[1] == MAPPING_TEST and hit[2].replace("\\", "/") == MAPPING
                         and (int(hit[3]), int(hit[4])) == (mapping_point["line"], mapping_point["column"])
                         and body.startswith("assertion `left == right` failed")
                         and re.search(r"^\s*right: Some\('←'\)$", body, re.M) is not None)
            if not (valid and record["exit_code"] == 101 and summaries == [("FAILED", "0", "1", "0")]):
                raise CourtError("mapping-negative-not-original-Option-assertion-RED")
        elif (record["exit_code"] != 0 or summaries != [("ok", "1", "0", "0")]
              or re.findall(r"^test (\S+) \.\.\. ok$", stdout, re.M) != [MAPPING_TEST]):
            raise CourtError("mapping-positive-missing-exact-one-pass")
        report["phases"].append({"phase": label, "process": record, "runtime_RED": negative,
                                 "scope": "pure AppKit physical-code/command mapping; not WK focus or hardware events"})

    try:
        stable(original)
        # Pins are outside public probe-logs and are not uploaded.
        (root / "target").mkdir(exist_ok=True)
        with tempfile.TemporaryDirectory(prefix="macos-snap-half-", dir=root / "target") as temporary:
            work = Path(temporary)
            phase("initial", work)
            stable(original)
            active_adapter_mutant = mutant
            (root / ADAPTER).write_bytes(mutant)
            try:
                stable(mutant)
                phase("removed-set-frame", work, negative="frame")
                stable(mutant)
            except BaseException as error:
                report["failures"].append("frame-control:" + str(error))
            finally:
                try:
                    current = (root / ADAPTER).read_bytes()
                    if current == mutant:
                        (root / ADAPTER).write_bytes(original)
                    elif current != original:
                        raise CourtError("unexpected-source-refusing-restore-overwrite")
                    stable(original)
                    active_adapter_mutant = None
                except BaseException as error:
                    report["restore_errors"].append(str(error))
                if not report["restore_errors"]:
                    phase("restored", work)
                    stable(original)
            if report["restore_errors"]:
                raise CourtError("frame-source-restoration-failed")
            # Separate mutant: the real setFrame call and frame-size preflight
            # remain intact. Only the complete content preflight is bypassed.
            active_adapter_mutant = content_mutant
            (root / ADAPTER).write_bytes(content_mutant)
            try:
                stable(content_mutant)
                phase("removed-content-preflight", work, negative="content")
                stable(content_mutant)
            except BaseException as error:
                report["failures"].append("content-control:" + str(error))
            finally:
                try:
                    current = (root / ADAPTER).read_bytes()
                    if current == content_mutant:
                        (root / ADAPTER).write_bytes(original)
                    elif current != original:
                        raise CourtError("unexpected-content-mutant-source-refusing-overwrite")
                    stable(original)
                    active_adapter_mutant = None
                except BaseException as error:
                    report["restore_errors"].append(str(error))
                if not report["restore_errors"]:
                    phase("content-restored", work)
                    stable(original)
            if report["restore_errors"]:
                raise CourtError("content-source-restoration-failed")
            mapping_phase("mapping-initial")
            stable(original)
            mapping_installed = True
            (root / MAPPING).write_bytes(mapping_mutant)
            try:
                stable(original, mapping_mutant)
                mapping_phase("mapping-changed-left", negative=True)
                stable(original, mapping_mutant)
            except BaseException as error:
                report["failures"].append("mapping-control:" + str(error))
            finally:
                try:
                    current = (root / MAPPING).read_bytes()
                    if current == mapping_mutant:
                        (root / MAPPING).write_bytes(mapping_original)
                    elif current != mapping_original:
                        raise CourtError("unexpected-mapping-source-refusing-overwrite")
                    stable(original)
                    mapping_installed = False
                except BaseException as error:
                    report["restore_errors"].append(str(error))
                if not report["restore_errors"]:
                    mapping_phase("mapping-restored")
                    stable(original)
    except BaseException as error:
        report["failures"].append(str(error))
    finally:
        if mapping_installed:
            try:
                current = (root / MAPPING).read_bytes()
                if current == mapping_mutant:
                    (root / MAPPING).write_bytes(mapping_original)
                elif current != mapping_original:
                    raise CourtError("unexpected-mapping-source-refusing-final-overwrite")
            except BaseException as error:
                report["restore_errors"].append(str(error))
        if active_adapter_mutant is not None:
            try:
                current = (root / ADAPTER).read_bytes()
                if current == active_adapter_mutant:
                    (root / ADAPTER).write_bytes(original)
                elif current != original:
                    raise CourtError("unexpected-source-refusing-final-restore-overwrite")
            except BaseException as error:
                report["restore_errors"].append(str(error))
        try:
            report["source_restored"] = snapshot(root) == source
        except BaseException as error:
            report["source_restored"] = False
            report["restore_errors"].append("source-verification:" + str(error))
        if (not report["failures"] and not report["restore_errors"] and report["source_restored"]
                and [p["phase"] for p in report["phases"]] == ["initial", "removed-set-frame", "restored",
                    "removed-content-preflight", "content-restored",
                    "mapping-initial", "mapping-changed-left", "mapping-restored"]):
            report["status"] = "PASS"
        report["public_log_sha256"] = {str(path.relative_to(output)): digest(path.read_bytes())
                                        for path in output.rglob("*") if path.is_file()
                                        and (path.name.endswith(".log") or path.name == "process.json")}
        save(output / "receipt.json", report)
    print(json.dumps({"status": report["status"], "scope": report["scope"]}))
    return 0 if report["status"] == "PASS" else 1


if __name__ == "__main__":
    raise SystemExit(main())
