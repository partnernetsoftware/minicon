#!/usr/bin/env python3
"""Native private-publication source court and runtime controls. Owner: PRD 28.02/32.

Run serially in an allocated checkout: this temporarily mutates two owned source
files and restores their exact original bytes. No product/GUI/provider launches.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path
import platform
import re
import signal
import struct
import subprocess
import sys
import time

TEST_PREFIX = "filesystem_publish::tests::private_publication_"
TESTS = (
    TEST_PREFIX + "distinguishes_precommit_and_postcommit_failures",
    TEST_PREFIX + "keeps_staging_and_legacy_replacement_private",
    TEST_PREFIX + "refuses_nonregular_targets_and_preserves_io_category",
)
PUBLISH = "crates/minicon-platform/src/filesystem_publish.rs"
WINDOWS = "crates/minicon-platform/src/adapters/windows/filesystem.rs"
TARGETS = {
    "x86_64-unknown-linux-gnu": ("linux", "x86_64"),
    "aarch64-unknown-linux-gnu": ("linux", "aarch64"),
    "x86_64-pc-windows-msvc": ("windows", "x86_64"),
    "aarch64-pc-windows-msvc": ("windows", "aarch64"),
    "x86_64-apple-darwin": ("macos", "x86_64"),
    "aarch64-apple-darwin": ("macos", "aarch64"),
}


class CourtFailure(Exception):
    def __init__(self, code, blocked=False):
        self.code, self.blocked = code, blocked


def digest(raw):
    return hashlib.sha256(raw).hexdigest()


def file_digest(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def save_receipt(output, receipt):
    temporary = output / "receipt.json.tmp"
    temporary.write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
    os.replace(temporary, output / "receipt.json")


def host_os():
    return {"win32": "windows", "darwin": "macos", "linux": "linux"}.get(sys.platform, "unsupported")


def changes(windows):
    cases = [
        ("published-flag", PUBLISH, TESTS[0],
         '"sync published file parent failed",\n            error,\n        )\n        .after_publish()',
         '"sync published file parent failed",\n            error,\n        )'),
        ("legacy-errno", PUBLISH, TESTS[2],
         'pub(crate) fn into_private_io(mut self) -> io::Error {\n        if matches!(\n            self.kind,\n            FilePublishErrorKind::Inspect | FilePublishErrorKind::Create\n        ) && let Some(source) = self.source.take()\n        {\n            return source;\n        }',
         'pub(crate) fn into_private_io(self) -> io::Error {'),
    ]
    if windows:
        cases.append(("windows-private-inheritance", WINDOWS, TESTS[1],
                      "grfInheritance: SUB_CONTAINERS_AND_OBJECTS_INHERIT,",
                      "grfInheritance: 0 & SUB_CONTAINERS_AND_OBJECTS_INHERIT,"))
    else:
        cases.append(("unix-private-mode", PUBLISH, TESTS[1],
                      "match crate::filesystem::private_create_new_options().open(&temporary) {",
                      "match fs::OpenOptions::new().write(true).create_new(true).open(&temporary) {"))
    return cases


def runtime_result(text, exit_code, expected, negative, timed_out=False):
    """Classify actual libtest output, never infer RED from a Cargo failure."""
    summaries = re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", text, re.M)
    running = re.findall(r"^running (\d+) tests?\s*$", text, re.M)
    wanted = [("FAILED", "0", "1", "0")] if negative else [("ok", "3", "0", "0")]
    if timed_out or summaries != wanted or running != [str(len(expected))]:
        return False
    if re.search(r"^error(?:\[E\d+\])?:.*(?:could not compile|aborting due to|unused|dead_code)", text, re.M):
        return False
    if negative:
        return (exit_code not in (None, 0)
                and re.search(r"^failures:\s*\n\s+" + re.escape(expected[0]) + r"\s*$", text, re.M) is not None
                and re.search(r"panicked at [^\n]*filesystem_publish\.rs:\d+:\d+:", text) is not None
                and "assertion" in text)
    passed = re.findall(r"^test (\S+) \.\.\. ok\s*$", text, re.M)
    return exit_code == 0 and sorted(passed) == sorted(expected)


def selfcheck():
    """Parser contract checks only: no subprocesses or native tests execute."""
    red = ("running 1 test\nfailures:\n    " + TESTS[0] + "\n"
           "thread '" + TESTS[0] + "' panicked at crates/minicon-platform/src/filesystem_publish.rs:1:1:\n"
           "assertion failed: after.published()\n"
           "test result: FAILED. 0 passed; 1 failed; 0 ignored; 0 measured; 10 filtered out;\n")
    green = "running 3 tests\n" + "".join("test " + name + " ... ok\n" for name in TESTS)
    green += "test result: ok. 3 passed; 0 failed; 0 ignored; 0 measured; 10 filtered out;\n"
    assert runtime_result(red, 101, [TESTS[0]], True), "valid running assertion RED rejected"
    assert runtime_result(green, 0, TESTS, False), "valid three-test GREEN rejected"
    assert not runtime_result(red, 101, [TESTS[0]], True, timed_out=True), "timeout accepted as runtime RED"
    assert not runtime_result(red.replace("assertion", "nonassert"), 101, [TESTS[0]], True), "panic without assertion accepted as runtime RED"
    assert not runtime_result(red, 0, [TESTS[0]], True), "exit zero accepted as runtime RED"
    assert not runtime_result("error[E0308]: wrong type\nerror: could not compile minicon-platform\n", 101, [TESTS[0]], True), "compile failure accepted as runtime RED"
    assert not runtime_result(red + "error: could not compile minicon-platform\n", 101, [TESTS[0]], True), "mixed compile failure accepted as runtime RED"
    assert not runtime_result(red.replace("0 ignored", "1 ignored"), 101, [TESTS[0]], True), "ignored test accepted as runtime RED"
    assert not runtime_result(green.replace("test " + TESTS[0] + " ... ok\n", ""), 0, TESTS, False), "missing named test accepted as GREEN"
    assert not runtime_result(red.replace("running 1 test", "running 0 tests"), 101, [TESTS[0]], True), "zero running tests accepted as runtime RED"
    print(json.dumps({"kind":"private-publication-log-parser-selfcheck", "checks":10,
                      "native_tests_executed":False, "cargo_executed":False}))


def test_image(root, text, expected_isa):
    paths = re.findall(r"^\s*Running unittests [^\n]+ \(([^\n]+)\)\s*$", text, re.M)
    if len(paths) != 1:
        raise CourtFailure("test-image-observation-missing")
    path = Path(paths[0])
    if not path.is_absolute():
        path = root / path
    path = path.resolve(strict=True)
    with path.open("rb") as stream:
        header = stream.read(65_536)
    if header.startswith(b"\x7fELF\x02\x01") and len(header) >= 20:
        isa = {62: "x86_64", 183: "aarch64"}.get(struct.unpack_from("<H", header, 18)[0])
        image_format = "ELF64"
    elif header.startswith(b"MZ") and len(header) >= 64:
        pe = struct.unpack_from("<I", header, 60)[0]
        if pe + 6 > len(header) or header[pe:pe + 4] != b"PE\0\0":
            raise CourtFailure("test-image-header-invalid")
        isa = {0x8664: "x86_64", 0xAA64: "aarch64"}.get(struct.unpack_from("<H", header, pe + 4)[0])
        image_format = "PE"
    elif header.startswith(b"\xcf\xfa\xed\xfe") and len(header) >= 8:
        isa = {0x01000007: "x86_64", 0x0100000C: "aarch64"}.get(struct.unpack_from("<I", header, 4)[0])
        image_format = "Mach-O64"
    else:
        raise CourtFailure("test-image-header-unsupported")
    if isa != expected_isa:
        raise CourtFailure("test-image-isa-mismatch")
    return {"sha256": file_digest(path), "format": image_format, "isa": isa,
            "path_kind": "exact-cargo-reported-test-harness", "name": path.name}


def owned_process_class(root):
    # Reuse the existing private Job/new-session process owner. Loading this
    # module invokes neither its main() nor its native browser fixture.
    directory = root / "scripts/native-auth-context"
    sys.path.insert(0, str(directory))
    old_bytecode = sys.dont_write_bytecode
    sys.dont_write_bytecode = True
    try:
        spec = importlib.util.spec_from_file_location("private_publication_owned_process", directory / "run.py")
        if spec is None or spec.loader is None:
            raise CourtFailure("owned-process-module-unavailable", True)
        module = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(module)
        return module.OwnedProcess
    finally:
        sys.dont_write_bytecode = old_bytecode
        sys.path.pop(0)


def choose_bash(root):
    spec = importlib.util.spec_from_file_location("private_publication_test_plan", root / "scripts/test-plan.py")
    if spec is None or spec.loader is None:
        raise CourtFailure("bash-selection-module-unavailable", True)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module.choose_bash()


def run_case(root, output, label, selection, target, timeout, negative, results, process_type):
    directory = output / label
    directory.mkdir()
    options = ["test-suite", "--target", target, "-p", "minicon-platform", "--features", "filesystem", "--lib", selection,
               "--", "--nocapture", "--test-threads", "1"]
    if negative:
        options.append("--exact")
    build = (root / "scripts/build.sh").as_posix()
    bash = choose_bash(root)
    if os.name == "nt":
        command = [bash, build, *options]
    else:
        # Explicit isolated shell, not process-global umask mutation in Python.
        command = [bash, "-c", 'umask 022; exec "$@"', "private-publication", bash, build, *options]
    environment = dict(os.environ, CARGO_TERM_COLOR="never", PYTHONDONTWRITEBYTECODE="1")
    record = {"court": label, "expected_tests": list(TESTS) if not negative else [selection],
              "runtime_red_requested": negative, "runtime_red": False, "valid_runtime_result": False,
              "exit_code": None, "timed_out": False, "cleanup_ok": False,
              "umask": None if os.name == "nt" else "022", "command": command}
    results.append(record)
    owned = None
    error = None
    started = time.monotonic()
    try:
        owned = process_type(command, environment, directory)
        while not owned.exited():
            if time.monotonic() - started >= timeout:
                record["timed_out"] = True
                raise CourtFailure("strict-wrapper-timeout")
            if any(p.stat().st_size > 32 * 1024 * 1024 for p in directory.glob("driver.*.log")):
                raise CourtFailure("strict-wrapper-log-bound")
            time.sleep(0.05)
        record["exit_code"] = owned.process.wait(timeout=1)
    except CourtFailure as failure:
        error = failure
    except Exception:
        error = CourtFailure("strict-wrapper-or-owned-process-failed")
    finally:
        if owned is not None:
            try:
                owned.close()
                record["cleanup_ok"] = True
                record["cleanup"] = ("owned-windows-job-closed" if os.name == "nt" else
                    "owned-root-reaped;independent-descendant-absence-not-qualified")
            except Exception:
                error = CourtFailure("strict-wrapper-process-cleanup-failed")
            record["exit_code"] = owned.process.returncode
        record["seconds"] = round(time.monotonic() - started, 3)
        # Preserve both original streams and identify this nonchronological
        # concatenation explicitly; no merged log is called raw interleaving.
        log = output / (label + ".log")
        streams = [directory / "driver.stdout.log", directory / "driver.stderr.log"]
        parts = []
        for path in streams:
            if path.exists():
                with path.open("rb") as stream:
                    part = stream.read(32 * 1024 * 1024 + 1)
                if len(part) > 32 * 1024 * 1024:
                    error = CourtFailure("strict-wrapper-log-bound")
                    record["combined_log_truncated"] = True
                    part = part[:32 * 1024 * 1024]
                parts.append(part)
        raw = b"".join(parts)
        log.write_bytes(raw)
        record["log"] = log.name
        record["log_layout"] = "stdout-then-stderr; original streams retained"
        record["log_sha256"] = digest(raw)
        record["stream_sha256"] = {p.name: file_digest(p) for p in streams if p.exists()}
    if error:
        raise error
    text = raw.decode("utf-8", errors="replace")
    record["observed_summaries"] = [dict(status=status, passed=int(passed), failed=int(failed), ignored=int(ignored))
        for status, passed, failed, ignored in re.findall(r"^test result: (ok|FAILED)\. (\d+) passed; (\d+) failed; (\d+) ignored;", text, re.M)]
    record["observed_running_counts"] = [int(value) for value in re.findall(r"^running (\d+) tests?\s*$", text, re.M)]
    expected = [selection] if negative else TESTS
    record["valid_runtime_result"] = runtime_result(text, record["exit_code"], expected, negative, record["timed_out"])
    if not record["valid_runtime_result"]:
        raise CourtFailure("required-runtime-result-missing")
    record["test_image"] = test_image(root, text, TARGETS[target][1])
    record["runtime_red"] = negative
    print(f"{label}: {'1 actual assertion RED' if negative else '3 tests passed; zero ignored'}", flush=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--target", choices=TARGETS)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--self-check", action="store_true", help="parser checks only; never launches Cargo")
    parser.add_argument("--timeout", type=float, default=240, help="per strict-wrapper command, at most 600 seconds")
    args = parser.parse_args()
    if args.self_check:
        if args.target or args.output:
            parser.error("--self-check does not accept native-court arguments")
        selfcheck()
        return 0
    if not args.target or not args.output:
        parser.error("--target and --output are required for the native court")
    if not 0 < args.timeout <= 600:
        parser.error("--timeout must be in (0, 600]")
    root = Path(__file__).resolve().parent.parent
    output = args.output.resolve()
    output.mkdir(parents=True, exist_ok=False)
    receipt = {"kind": "private-publication-native-source-court", "schema": 1, "status": "FAIL",
               "failure_code": None, "target": args.target, "os": host_os(), "isa": TARGETS[args.target][1],
               "runner_machine": platform.machine(), "features": ["filesystem"], "default_features": True,
               "package": "minicon-platform", "cargo_cache_isolated": False,
               "source_restored_exact": False, "results": [],
               "scope": "native source mechanisms; not release-artifact/browser/provider or power-loss qualification"}
    originals = {}
    normalized = {}
    mutation_attempted = False
    failure = None
    process_type = None
    def interrupt(signum, _frame):
        # Soft cancellation enters the same child cleanup/source restoration
        # finally path. SIGKILL/host loss cannot produce terminal evidence.
        signal.signal(signum, signal.SIG_IGN)
        raise KeyboardInterrupt
    for signum in (signal.SIGINT, signal.SIGTERM):
        signal.signal(signum, interrupt)
    try:
        if host_os() != TARGETS[args.target][0]:
            raise CourtFailure("target-os-does-not-match-native-runner", True)
        receipt["source_sha"] = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=root, text=True).strip()
        receipt["tracked_dirty_at_start"] = bool(subprocess.check_output(["git", "status", "--porcelain", "--untracked-files=no"], cwd=root))
        receipt["source_binding"] = "HEAD plus listed working-tree preimage bytes; shared cache"
        paths = {item[1] for item in changes(os.name == "nt")}
        originals = {name: (root / name).read_bytes() for name in paths}
        normalized = {name: raw.decode("utf-8").replace("\r\n", "\n") for name, raw in originals.items()}
        receipt["source_file_sha256"] = {name: digest(raw) for name, raw in originals.items()}
        receipt["status"] = "RUNNING"
        save_receipt(output, receipt)
        receipt["support_file_sha256"] = {name: file_digest(root / name) for name in (
            "scripts/build.sh", "scripts/test-plan.py", "scripts/native-auth-context/run.py", "scripts/native-auth-context/fixture.py",
            "crates/minicon-platform/src/filesystem.rs", "crates/minicon-platform/Cargo.toml", "Cargo.lock",
            "crates/minicon-platform/src/adapters/" + {"windows": "windows", "linux": "linux", "macos": "macos"}[host_os()] + "/filesystem.rs",
        )}
        for label, name, _, before, _ in changes(os.name == "nt"):
            if normalized[name].count(before) != 1:
                raise CourtFailure("source-mutation-anchor-changed:" + label, True)
        process_type = owned_process_class(root)
        run_case(root, output, "initial", TEST_PREFIX, args.target, args.timeout, False, receipt["results"], process_type)
        for label, name, test, before, after in changes(os.name == "nt"):
            mutation_attempted = True
            try:
                mutant = normalized[name].replace(before, after, 1).encode("utf-8")
                receipt.setdefault("mutations", []).append({"court": label, "source": name,
                    "mutant_sha256": digest(mutant), "test": test})
                receipt["source_restored_exact"] = False
                save_receipt(output, receipt)
                (root / name).write_bytes(mutant)
                run_case(root, output, label, test, args.target, args.timeout, True, receipt["results"], process_type)
            finally:
                (root / name).write_bytes(originals[name])
                if (root / name).read_bytes() != originals[name]:
                    raise CourtFailure("source-byte-restoration-failed")
                receipt["source_restored_exact"] = True
                save_receipt(output, receipt)
    except CourtFailure as error:
        failure = error
    except KeyboardInterrupt:
        failure = CourtFailure("court-interrupted")
    except Exception:
        failure = CourtFailure("court-setup-or-runtime-failed")
    finally:
        restored = True
        for name, raw in originals.items():
            try:
                (root / name).write_bytes(raw)
                restored = restored and (root / name).read_bytes() == raw
            except OSError:
                restored = False
        receipt["source_restored_exact"] = bool(originals) and restored
        if not restored:
            failure = CourtFailure("source-byte-restoration-failed")
        # Even an invalid mutant gets a restored-source gate, if process cleanup
        # completed. A timeout/compile failure never becomes successful RED.
        if mutation_attempted and restored and (failure is None or failure.code != "court-interrupted") and all(row["cleanup_ok"] for row in receipt["results"]):
            try:
                run_case(root, output, "restored", TEST_PREFIX, args.target, args.timeout, False, receipt["results"], process_type)
            except CourtFailure as error:
                failure = failure or error
            except Exception:
                failure = failure or CourtFailure("restored-source-gate-failed")
        receipt["source_after_sha256"] = {name: file_digest(root / name) for name in originals if (root / name).is_file()}
        receipt["source_restored_exact"] = bool(originals) and restored and receipt["source_after_sha256"] == receipt.get("source_file_sha256")
        if failure is None and len(receipt["results"]) == 5 and all(row["valid_runtime_result"] for row in receipt["results"]) and receipt["source_restored_exact"]:
            receipt["status"] = "PASS"
        else:
            receipt["status"] = "BLOCKED" if failure and failure.blocked else "FAIL"
            receipt["failure_code"] = failure.code if failure else "incomplete-native-court"
        receipt["script_sha256"] = file_digest(Path(__file__))
        save_receipt(output, receipt)
    print(json.dumps({key: receipt[key] for key in ("kind", "status", "failure_code")}))
    return 0 if receipt["status"] == "PASS" else 2 if receipt["status"] == "BLOCKED" else 1


if __name__ == "__main__":
    raise SystemExit(main())
