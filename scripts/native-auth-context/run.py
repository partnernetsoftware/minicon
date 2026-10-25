#!/usr/bin/env python3
"""Run an already-built, test-only native auth context court. Owner: PRD 28.

This launches a native window: use only an explicitly allocated court desktop.
It never builds, logs into a provider, reads accounts, or uses the clipboard.
"""
from __future__ import annotations

import argparse
import ctypes
import contextlib
import hashlib
import json
import os
from pathlib import Path
import platform
import re
import signal
import stat
import subprocess
import sys
import time

import fixture

try:
    import sqlite3
except ImportError:
    # Optional diagnostic support cannot prevent the independent native court.
    sqlite3 = None

CHECKS = (
    "ordinary-initial-storage", "a-initial-storage", "b-initial-storage",
    "a-reopen-initial-storage", "ordinary-live-dom-cookie-storage-return",
    "ordinary-one-document-load", "ordinary-native-history-back",
    "ordinary-native-history-forward",
)
ROOT = Path(__file__).resolve().parent
LIMIT = 1024 * 1024


class CourtError(Exception):
    def __init__(self, code, blocked=False):
        self.code, self.blocked = code, blocked


def sha(path):
    value = hashlib.sha256()
    with path.open("rb") as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b""):
            value.update(block)
    return value.hexdigest()


def read_json(path):
    with path.open("rb") as stream:
        raw = stream.read(LIMIT + 1)
    if len(raw) > LIMIT:
        raise CourtError("receipt-size-bound")
    try:
        return fixture.object_json(raw)
    except (ValueError, UnicodeError, RecursionError):
        raise CourtError("receipt-json-invalid") from None


def executable(value):
    try:
        path = value.expanduser().resolve(strict=True)
    except OSError:
        raise CourtError("executable-missing", True) from None
    if not path.is_file() or (os.name != "nt" and not os.access(path, os.X_OK)):
        raise CourtError("executable-not-runnable", True)
    return path


def host_os():
    return {"win32": "windows", "darwin": "macos", "linux": "linux"}.get(sys.platform, "unsupported")


def metadata(path, binary_hash, helper_hash):
    if path is None:
        raise CourtError("build-provenance-missing", True)
    data = read_json(path)
    if (data.get("schema") != 1 or data.get("test_only") is not True
            or not isinstance(data.get("source_commit"), str)
            or not re.fullmatch(r"[0-9a-f]{40}", data["source_commit"])
            or not isinstance(data.get("features"), list)
            or "native-auth-court" not in data["features"]
            or not all(isinstance(item, str) and re.fullmatch(r"[a-z0-9_-]{1,64}", item) for item in data["features"])
            or data.get("os") != host_os()
            or data.get("isa") not in ("x86_64", "aarch64")):
        raise CourtError("build-metadata-invalid")
    if data.get("binary_sha256") != binary_hash or data.get("helper_sha256") != helper_hash:
        raise CourtError("build-metadata-byte-identity-mismatch")
    return {name: data[name] for name in ("source_commit", "features", "os", "isa", "binary_sha256")} | {
        "helper_sha256": helper_hash,
        "build_metadata_verified_byte_identity": True,
        "source_authority": "external-build-declaration-bound-to-measured-bytes",
    }



# Diagnostic authority is this run's fresh A/B manifest only. Never enumerate a
# user's profiles, decrypt, select cookie values, or change the court verdict.
_COOKIE_DB_SLOTS = (
    ("webview2-network", "EBWebView/Default/Network/Cookies"),
    ("webview2-legacy", "EBWebView/Default/Cookies"),
    ("chromium-network", "Default/Network/Cookies"),
    ("chromium-legacy", "Default/Cookies"),
)
_LOCAL_STATE_SLOTS = (("webview2", "EBWebView/Local State"), ("chromium", "Local State"))
_DIAGNOSTIC_FILE_LIMIT = 8 * 1024 * 1024
_DIAGNOSTIC_ROW_LIMIT = 4096


def _diagnostic_error(error):
    if isinstance(error, PermissionError):
        return "permission-denied"
    if isinstance(error, FileNotFoundError):
        return "not-found"
    if sqlite3 is not None and isinstance(error, sqlite3.Error):
        # Never persist exception text, SQL error text or native path strings.
        name = getattr(error, "sqlite_errorname", "")
        return {"SQLITE_BUSY": "sqlite-busy", "SQLITE_LOCKED": "sqlite-locked",
                "SQLITE_CORRUPT": "sqlite-corrupt", "SQLITE_NOTADB": "sqlite-not-database",
                "SQLITE_CANTOPEN": "sqlite-open-failed", "SQLITE_INTERRUPT": "sqlite-work-bound",
                "SQLITE_ERROR": "sqlite-schema-or-query-unavailable"}.get(name, "sqlite-other-error")
    return "diagnostic-read-unavailable"


def _diagnostic_entry(root, relative):
    path = root
    for component in Path(relative).parts:
        path = path / component
        metadata = path.lstat()
        if (stat.S_ISLNK(metadata.st_mode)
                or getattr(metadata, "st_file_attributes", 0) & 0x400):
            raise ValueError("link-like-entry")
    if not stat.S_ISREG(metadata.st_mode) or metadata.st_nlink != 1:
        raise ValueError("non-private-regular-entry")
    return path, metadata


def _cookie_database_observation(root, relative, now):
    result = {"status": "unavailable", "read_open": False}
    if sqlite3 is None:
        result["status"] = "sqlite-module-unavailable"
        return result
    try:
        path, metadata = _diagnostic_entry(root, relative)
        result["bytes"] = metadata.st_size
        if metadata.st_size > _DIAGNOSTIC_FILE_LIMIT:
            result["status"] = "file-size-bound"
            return result
        with path.open("rb") as stream:
            result["read_open"] = True
            if stream.read(16) != b"SQLite format 3\0":
                result["status"] = "sqlite-header-unavailable"
                return result
        # immutable guarantees no DB/SHM writes but deliberately ignores WAL.
        # A missing table or zero rows is INCONCLUSIVE when WAL exists.
        uri = path.as_uri() + "?mode=ro&immutable=1"
        with contextlib.closing(sqlite3.connect(uri, uri=True, timeout=0)) as connection:
            connection.execute("PRAGMA query_only=ON")
            ticks = [0]
            def bounded_sqlite_work():
                ticks[0] += 1
                return ticks[0] > 100
            connection.set_progress_handler(bounded_sqlite_work, 1000)
            rows = connection.execute(
                "SELECT is_persistent, has_expires, expires_utc FROM cookies LIMIT 4097"
            ).fetchall()
        if len(rows) > _DIAGNOSTIC_ROW_LIMIT:
            result["status"] = "row-count-bound"
            return result
        counts = dict(rows=len(rows), persistent=0, session=0, invalid_flags=0,
                      expires_after_now=0, expired_or_zero=0, invalid_expiry=0,
                      expiry_within_15_minutes=0)
        chromium_now = int((now + 11644473600) * 1000000)
        for persistent, has_expires, expires in rows:
            if type(persistent) is not int or persistent not in (0, 1) or type(has_expires) is not int or has_expires not in (0, 1):
                counts["invalid_flags"] += 1
            elif persistent == 1:
                counts["persistent"] += 1
            else:
                counts["session"] += 1
            if type(expires) is not int:
                counts["invalid_expiry"] += 1
            elif expires > chromium_now:
                counts["expires_after_now"] += 1
                if expires <= chromium_now + 900000000:
                    counts["expiry_within_15_minutes"] += 1
            else:
                counts["expired_or_zero"] += 1
        result.update(status="read-only-main-db-observed", counts=counts)
    except ValueError:
        result["status"] = "entry-authority-refused"
    except (OSError, sqlite3.Error, OverflowError) as error:
        result["status"] = _diagnostic_error(error)
    for suffix, label in (("-wal", "wal"), ("-shm", "shm")):
        try:
            _, metadata = _diagnostic_entry(root, relative + suffix)
            result[label] = {"status": "present", "bytes": metadata.st_size}
        except ValueError:
            result[label] = {"status": "entry-authority-refused"}
        except OSError as error:
            result[label] = {"status": _diagnostic_error(error)}
    return result


def _local_state_observation(root, relative):
    result = {"status": "unavailable", "read_open": False}
    try:
        path, metadata = _diagnostic_entry(root, relative)
        if metadata.st_size > LIMIT:
            result["status"] = "file-size-bound"
            return result
        with path.open("rb") as stream:
            raw = stream.read(LIMIT + 1)
        result["read_open"] = True
        if len(raw) > LIMIT:
            result["status"] = "file-size-bound"
            return result
        parsed = fixture.object_json(raw)
        crypt = parsed.get("os_crypt")
        result.update(status="configuration-observed", os_crypt_object=isinstance(crypt, dict),
                      encrypted_key_configured=isinstance(crypt, dict) and isinstance(crypt.get("encrypted_key"), str) and bool(crypt["encrypted_key"]),
                      app_bound_key_configured=isinstance(crypt, dict) and isinstance(crypt.get("app_bound_encrypted_key"), str) and bool(crypt["app_bound_encrypted_key"]))
    except ValueError:
        result["status"] = "entry-or-json-unavailable"
    except (OSError, UnicodeError, RecursionError) as error:
        result["status"] = _diagnostic_error(error)
    return result


def _diagnostic_directory_ancestry(root):
    for path in (root, *root.parents):
        info = path.lstat()
        if (not stat.S_ISDIR(info.st_mode) or stat.S_ISLNK(info.st_mode)
                or getattr(info, "st_file_attributes", 0) & 0x400):
            raise ValueError("diagnostic-root-link-like")


def _verified_locator_directories(manifest, output, roots):
    # Private transport only, never returned by a public receipt. Bind every
    # locator before opening any DB. No recursive discovery or arbitrary files.
    for root in (output, *roots.values()):
        _diagnostic_directory_ancestry(root)
    carrier, metadata = _diagnostic_entry(output, "native-profile-locator.private.json")
    if metadata.st_size > 16_384:
        raise ValueError("locator-size-bound")
    with carrier.open("rb") as stream:
        raw = stream.read(16_385)
    if len(raw) > 16_384:
        raise ValueError("locator-size-bound")
    data = fixture.object_json(raw)
    if (set(data) != {"schema", "test_only", "run", "phases"}
            or data["schema"] != 1 or data["test_only"] is not True
            or data["run"] != manifest["run"] or not isinstance(data["phases"], dict)
            or not 1 <= len(data["phases"]) <= 3
            or not set(data["phases"]) <= {"a", "b", "a-reopen"}):
        raise ValueError("locator-authority-invalid")
    locations = {}
    for phase, entry in data["phases"].items():
        label = "b" if phase == "b" else "a"
        if (not isinstance(entry, dict) or set(entry) != {"store_identifier", "relative_directory"}
                or entry["store_identifier"] != list(bytes.fromhex(manifest["profiles"][label]["store_identifier"]))
                or not isinstance(entry["relative_directory"], str)
                or not 1 <= len(entry["relative_directory"].encode("utf-8")) <= 2048):
            raise ValueError("locator-profile-invalid")
        relative = Path(entry["relative_directory"])
        if (relative.is_absolute() or relative.drive or relative.root
                or not 1 <= len(relative.parts) <= 16
                or any(part in ("", ".", "..") for part in relative.parts)):
            raise ValueError("locator-path-invalid")
        root = roots[label]
        location = root / relative
        # Include root ancestry, not merely the suffix underneath the fresh root.
        _diagnostic_directory_ancestry(location)
        if location.resolve(strict=True) != location or not location.is_relative_to(root) or location == root:
            raise ValueError("locator-outside-profile")
        locations[phase] = location
    return locations


def _verified_locator_snapshot(manifest, output, roots, now):
    try:
        locations = _verified_locator_directories(manifest, output, roots)
        relation = "unavailable"
        if "a" in locations and "a-reopen" in locations:
            relation = "same" if locations["a"] == locations["a-reopen"] else "different"
        return {"status": "verified-private-locator", "a_reopen_directory_relation": relation,
                "phases": {phase: {name: _cookie_database_observation(root, slot, now)
                                    for name, slot in (("network", "Network/Cookies"), ("legacy", "Cookies"))}
                           for phase, root in locations.items()}}
    except Exception:
        # No exception text, carrier bytes, path, or directory names escape.
        return {"status": "private-locator-unavailable", "a_reopen_directory_relation": "unavailable"}


def windows_cookie_snapshot(manifest, output, stage, clock):
    result = {"stage": stage, "profiles": {}, "status": "diagnostics-only"}
    try:
        # output was create-new before manifest creation. Bind BOTH profiles
        # before reading any file; refuse links/reparse points and unexpected paths.
        if manifest.get("schema") != 1 or manifest.get("test_only") is not True:
            raise ValueError("not-owned-test-manifest")
        roots = {}
        for label in ("a", "b"):
            root = output
            for component in ("profiles", label):
                root = root / component
                metadata = root.lstat()
                if (not stat.S_ISDIR(metadata.st_mode) or stat.S_ISLNK(metadata.st_mode)
                        or getattr(metadata, "st_file_attributes", 0) & 0x400):
                    raise ValueError("profile-authority-refused")
            if Path(manifest["profiles"][label]["directory"]) != root or root.resolve(strict=True) != root:
                raise ValueError("profile-authority-refused")
            roots[label] = root
        # Shared authority gate precedes ALL carrier, fixed DB and Local State
        # reads. A rejected ancestry must not become a locator-only warning.
        for root in (output, *roots.values()):
            _diagnostic_directory_ancestry(root)
        wall = time.time()
        result["verified_profile_databases"] = _verified_locator_snapshot(manifest, output, roots, wall)
        drift = (wall - clock[0]) - (time.monotonic() - clock[1])
        result["clock"] = {"wall_vs_monotonic": "stable-within-two-seconds" if abs(drift) <= 2 else "wall-clock-step-observed",
                           "fixture_cookie_max_age_seconds": 600}
        for label, root in roots.items():
            result["profiles"][label] = {
                "manifest_identity_verified": True,
                "cookie_databases": {name: _cookie_database_observation(root, relative, wall) for name, relative in _COOKIE_DB_SLOTS},
                "local_state": {name: _local_state_observation(root, relative) for name, relative in _LOCAL_STATE_SLOTS}}
    except (OSError, ValueError, KeyError, TypeError):
        result["status"] = "fresh-profile-authority-unavailable"
    except Exception:
        # Diagnostics never alter the court's already measured verdict.
        result = {"stage": stage, "status": "diagnostic-internal-error"}
    return result


class OwnedProcess:
    """Own one child and its new session/Windows Job, never arbitrary PIDs."""
    def __init__(self, command, env, output):
        self.process = None
        self.job = None
        self.logs = []
        self.cleanup = "not-started"
        self.exit_observed = False
        try:
            for name in ("driver.stdout.log", "driver.stderr.log"):
                self.logs.append((output / name).open("wb"))
            options = dict(env=env, stdin=subprocess.DEVNULL, stdout=self.logs[0], stderr=self.logs[1], cwd=output)
            if os.name == "nt":
                self._windows_start(command, options)
            else:
                if not all(hasattr(os, name) for name in ("waitid", "WNOWAIT", "WEXITED")):
                    raise CourtError("retained-child-observation-unavailable", True)
                self.process = subprocess.Popen(command, start_new_session=True, **options)
            self.cleanup = "owned-process-started"
        except BaseException:
            self.close()
            raise

    def _windows_start(self, command, options):
        # Start suspended; no helper can escape before assignment to this Job.
        from ctypes import wintypes as w

        class Basic(ctypes.Structure):
            _fields_ = [("PerProcessUserTimeLimit", ctypes.c_longlong), ("PerJobUserTimeLimit", ctypes.c_longlong),
                        ("LimitFlags", w.DWORD), ("MinimumWorkingSetSize", ctypes.c_size_t),
                        ("MaximumWorkingSetSize", ctypes.c_size_t), ("ActiveProcessLimit", w.DWORD),
                        ("Affinity", ctypes.c_size_t), ("PriorityClass", w.DWORD), ("SchedulingClass", w.DWORD)]

        class Io(ctypes.Structure):
            _fields_ = [(name, ctypes.c_ulonglong) for name in ("ReadOperationCount", "WriteOperationCount", "OtherOperationCount",
                                                               "ReadTransferCount", "WriteTransferCount", "OtherTransferCount")]

        class Extended(ctypes.Structure):
            _fields_ = [("BasicLimitInformation", Basic), ("IoInfo", Io), ("ProcessMemoryLimit", ctypes.c_size_t),
                        ("JobMemoryLimit", ctypes.c_size_t), ("PeakProcessMemoryUsed", ctypes.c_size_t), ("PeakJobMemoryUsed", ctypes.c_size_t)]

        class ThreadEntry(ctypes.Structure):
            _fields_ = [("dwSize", w.DWORD), ("cntUsage", w.DWORD), ("th32ThreadID", w.DWORD),
                        ("th32OwnerProcessID", w.DWORD), ("tpBasePri", w.LONG), ("tpDeltaPri", w.LONG), ("dwFlags", w.DWORD)]

        kernel = ctypes.WinDLL("kernel32", use_last_error=True)
        signatures = {
            "CreateJobObjectW": ([ctypes.c_void_p, w.LPCWSTR], w.HANDLE),
            "SetInformationJobObject": ([w.HANDLE, ctypes.c_int, ctypes.c_void_p, w.DWORD], w.BOOL),
            "AssignProcessToJobObject": ([w.HANDLE, w.HANDLE], w.BOOL),
            "CreateToolhelp32Snapshot": ([w.DWORD, w.DWORD], w.HANDLE),
            "Thread32First": ([w.HANDLE, ctypes.POINTER(ThreadEntry)], w.BOOL),
            "Thread32Next": ([w.HANDLE, ctypes.POINTER(ThreadEntry)], w.BOOL),
            "OpenThread": ([w.DWORD, w.BOOL, w.DWORD], w.HANDLE),
            "ResumeThread": ([w.HANDLE], w.DWORD),
            "TerminateJobObject": ([w.HANDLE, w.UINT], w.BOOL),
            "CloseHandle": ([w.HANDLE], w.BOOL),
        }
        for name, (arguments, result) in signatures.items():
            function = getattr(kernel, name)
            function.argtypes, function.restype = arguments, result
        self.kernel = kernel
        self.job = kernel.CreateJobObjectW(None, None)
        limits = Extended()
        limits.BasicLimitInformation.LimitFlags = 0x2000  # JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE
        if not self.job or not kernel.SetInformationJobObject(self.job, 9, ctypes.byref(limits), ctypes.sizeof(limits)):
            raise CourtError("owned-job-unavailable", True)
        self.process = subprocess.Popen(command, creationflags=0x08000004, **options)  # NO_WINDOW | SUSPENDED
        if not kernel.AssignProcessToJobObject(self.job, w.HANDLE(int(self.process._handle))):
            self.process.kill()
            raise CourtError("owned-job-assignment-failed", True)
        snapshot = kernel.CreateToolhelp32Snapshot(0x4, 0)  # TH32CS_SNAPTHREAD
        if snapshot == w.HANDLE(-1).value:
            raise CourtError("owned-thread-enumeration-failed")
        thread = None
        try:
            entry = ThreadEntry()
            entry.dwSize = ctypes.sizeof(entry)
            present = kernel.Thread32First(snapshot, ctypes.byref(entry))
            inspected = 0
            while present:
                inspected += 1
                if inspected > 8192:
                    raise CourtError("owned-thread-enumeration-bound")
                if entry.th32OwnerProcessID == self.process.pid:
                    thread = kernel.OpenThread(0x2, False, entry.th32ThreadID)  # THREAD_SUSPEND_RESUME
                    break
                present = kernel.Thread32Next(snapshot, ctypes.byref(entry))
            if not thread or kernel.ResumeThread(thread) == 0xFFFFFFFF:
                raise CourtError("owned-main-thread-resume-failed")
        finally:
            if thread:
                kernel.CloseHandle(thread)
            kernel.CloseHandle(snapshot)

    def exited(self):
        if os.name == "nt":
            self.exit_observed = self.process.poll() is not None
        else:
            # Preserve the owned PID/PGID identity until timeout cleanup is done.
            self.exit_observed = os.waitid(os.P_PID, self.process.pid, os.WEXITED | os.WNOHANG | os.WNOWAIT) is not None
        return self.exit_observed

    def close(self):
        failure = None
        if self.process is not None:
            try:
                if os.name == "nt":
                    if self.job and not self.kernel.TerminateJobObject(self.job, 125):
                        failure = CourtError("owned-job-cleanup-failed")
                    self.cleanup = "owned-job-terminated"
                elif not self.exit_observed:
                    # Root is not reaped. Its group identity cannot be reused.
                    try:
                        os.killpg(self.process.pid, signal.SIGKILL)
                    except ProcessLookupError:
                        pass
                    self.cleanup = "owned-session-killed-before-root-reap"
                else:
                    # The native driver executes its real detach path before exit.
                    # No global process scan or PID-based cleanup after root reap.
                    self.cleanup = "driver-exited-normal-detach;descendant-absence-not-independently-observed"
            except OSError:
                failure = CourtError("owned-process-cleanup-failed")
            finally:
                if self.job:
                    self.kernel.CloseHandle(self.job)
                    self.job = None
                # Reap even if signalling failed; retain the typed cleanup error.
                try:
                    self.process.wait(timeout=5)
                except (OSError, subprocess.TimeoutExpired):
                    failure = CourtError("owned-process-cleanup-failed")
        elif self.job:
            self.kernel.CloseHandle(self.job)
            self.job = None
        for stream in self.logs:
            stream.close()
        if failure:
            raise failure


def validate(driver, observed, build, driver_snapshot):
    if (driver.get("kind") != "native-auth-court-test-build"
            or driver.get("test_only") is not True or driver.get("production_test_build") is not True
            or driver.get("features") != build["features"]
            or driver.get("os") != build["os"] or driver.get("isa") != build["isa"]):
        raise CourtError("driver-test-build-identity-mismatch")
    if driver.get("status") == "BLOCKED" and driver.get("failure_code") == "native-court-native-host-unsupported":
        raise CourtError("native-court-native-host-unsupported", True)
    if driver.get("status") != "native-observations-complete":
        raise CourtError("driver-observations-failed")
    if driver_snapshot is None or driver.get("fixture") != driver_snapshot:
        raise CourtError("driver-fixture-receipt-mismatch")
    for name in ("checks", "run_sha256", "page_get_counts", "report_count", "report_hashes"):
        if driver_snapshot.get(name) != observed.get(name):
            raise CourtError("fixture-observations-changed-after-driver-receipt")
    if (observed.get("observation_status") != "complete" or observed.get("failure") is not None
            or set(observed.get("checks", {})) != set(CHECKS)
            or any(observed["checks"][name] is not True for name in CHECKS)):
        raise CourtError("eight-live-observation-checks-incomplete")
    if observed.get("page_get_counts") != {name: 1 for name in fixture.PHASES}:
        raise CourtError("page-navigation-count-mismatch")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True, help="fresh output directory")
    parser.add_argument("--helper", type=Path, help="explicit Linux native page helper")
    parser.add_argument("--timeout", type=float, default=120)
    parser.add_argument("--build-receipt", type=Path)
    args = parser.parse_args()
    if not 0 < args.timeout <= 180:
        parser.error("--timeout must be in (0, 180]")
    output = args.output.expanduser().resolve()
    try:
        output.mkdir(mode=0o700, parents=True, exist_ok=False)
    except OSError:
        parser.error("--output must be a fresh writable directory")
    result = dict(kind="native-auth-context-court", schema=1, test_only=True,
                  production_test_build=True, status="FAIL", failure_code=None,
                  native_execution_started=False, actual_exit_code=None, checks={},
                  runner_host={"os":host_os(), "machine":platform.machine(), "python_bits":ctypes.sizeof(ctypes.c_void_p) * 8},
                  source_authority="unverified", production_artifact_qualification=False,
                  provider_authorization_claimed=False,
                  unclaimed=["native map/visibility/focus", "proxy/no-direct", "delayed embedded_ready",
                             "physical keyboard/native IME/clipboard", "toolbar-driven history", "macOS native store deletion"],
                  script_sha256={name:sha(ROOT / name) for name in ("run.py", "fixture.py", "page.js")})
    owned = None
    court = None
    manifest = None
    diagnostic_clock = (time.time(), time.monotonic())
    cookie_diagnostics = []
    try:
        binary = executable(args.binary)
        helper = executable(args.helper) if args.helper else None
        if helper and host_os() != "linux":
            raise CourtError("helper-requires-linux")
        if host_os() == "linux" and helper is None:
            raise CourtError("explicit-linux-helper-required", True)
        if host_os() == "unsupported":
            raise CourtError("unsupported-native-host", True)
        result["binary_sha256"] = sha(binary)
        result["helper_sha256"] = sha(helper) if helper else None
        build = metadata(args.build_receipt, result["binary_sha256"], result["helper_sha256"])
        result["build"] = build
        result["source_authority"] = build["source_authority"]
        result["build_receipt_sha256"] = sha(args.build_receipt)
        if host_os() == "linux" and not os.environ.get("DISPLAY"):
            raise CourtError("x11-display-unavailable", True)
        with fixture.Fixture(args.timeout) as court:
            manifest = court.manifest(output)
            if helper:
                manifest["helper"] = str(helper)
            fixture.private_json(output / "manifest.private.json", manifest)
            env = dict(os.environ)
            for name in manifest["clear_environment"]:
                env.pop(name, None)
            env.update(manifest["launch_environment"])
            if helper:
                env["MINICON_BROWSER_BIN"] = str(helper)
            owned = OwnedProcess([str(binary), "--native-auth-court", str(output / "manifest.private.json")], env, output)
            result["native_execution_started"] = True
            while not owned.exited():
                if time.monotonic() >= court.state.deadline:
                    raise CourtError("native-driver-deadline-expired")
                if any(path.stat().st_size > LIMIT for path in (output / "driver.stdout.log", output / "driver.stderr.log")):
                    raise CourtError("native-driver-output-bound")
                time.sleep(0.05)
            # Preserve the measured normal exit code before Windows job cleanup.
            result["actual_exit_code"] = owned.process.wait(timeout=1)
            if host_os() == "windows":
                cookie_diagnostics.append(windows_cookie_snapshot(manifest, output, "driver-exited-before-job-cleanup", diagnostic_clock))
            observed = court.state.receipt()
            fixture.private_json(output / "fixture-receipt.json", observed)
            result["checks"] = observed["checks"]
            driver = read_json(output / "native-driver-receipt.json")
            validate(driver, observed, build, court.state.driver_receipt_snapshot)
            if result["actual_exit_code"] != 0:
                raise CourtError("native-driver-nonzero-exit")
            if sha(binary) != result["binary_sha256"] or (helper and sha(helper) != result["helper_sha256"]):
                raise CourtError("executable-changed-during-court")
            result["status"] = "PASS"
    except CourtError as error:
        result["status"] = "BLOCKED" if error.blocked else "FAIL"
        result["failure_code"] = error.code
    except (OSError, ValueError, subprocess.SubprocessError):
        # Do not serialize exception text: it can contain private paths or URLs.
        result["failure_code"] = "court-runtime-or-receipt-unavailable"
    except KeyboardInterrupt:
        result["failure_code"] = "court-interrupted"
    finally:
        if owned is not None:
            try:
                owned.close()
            except CourtError as error:
                result["status"], result["failure_code"] = "FAIL", error.code
            result["cleanup"] = owned.cleanup
            if owned.process.returncode is not None:
                result["actual_exit_code"] = owned.process.returncode
        if court is not None:
            observed = court.state.receipt()
            result["checks"] = observed["checks"]
            result["fixture_failure_code"] = observed["failure"]
            fixture.private_json(output / "fixture-receipt.json", observed)
        if host_os() == "windows" and manifest is not None:
            cookie_diagnostics.append(windows_cookie_snapshot(manifest, output, "after-owned-job-cleanup", diagnostic_clock))
            diagnostic = dict(kind="owned-fresh-windows-cookie-diagnostics", schema=1,
                              status="diagnostics-only", changes_court_verdict=False,
                              snapshots=cookie_diagnostics,
                              limitations=["Only this fresh run's exact manifest A/B directories, verified private locators and fixed Cookies slots; no profile search.",
                                           "SQLite immutable main DB only; WAL contents are NOT read or checkpointed. Missing rows/schema with WAL are inconclusive.",
                                           "A-reopen page writes its cookie again before reporting failure; post-exit observations cannot reconstruct the first A close.",
                                           "Normal driver exit precedes owned Job descendant cleanup; cleanup may interrupt browser persistence. Compare both stages.",
                                           "Read-open/permission categories are not a DACL audit, DPAPI decryption, SDK InPrivate observation or flush proof.",
                                           "Fresh private test authority is prechecked; no concurrent adversarial filesystem mutation court is claimed."])
            try:
                fixture.private_json(output / "windows-cookie-diagnostics.json", diagnostic)
            except (OSError, ValueError):
                result["cookie_diagnostic_write"] = "unavailable"
        artifacts = [output / name for name in ("driver.stdout.log", "driver.stderr.log", "native-driver-receipt.json", "fixture-receipt.json", "windows-cookie-diagnostics.json")]
        result["artifacts"] = {path.name:{"bytes":path.stat().st_size, "sha256":sha(path)}
                               for path in artifacts if path.is_file()}
        fixture.private_json(output / "receipt.json", result)
    print(json.dumps({name:result[name] for name in ("kind", "status", "failure_code")}))
    return 0 if result["status"] == "PASS" else 2 if result["status"] == "BLOCKED" else 1


if __name__ == "__main__":
    raise SystemExit(main())
