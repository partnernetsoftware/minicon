#!/usr/bin/env python3
"""Prove only fixture protocol guards; never start a browser or product CLI."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import subprocess
import sys

ROOT = Path(__file__).resolve().parent
SOURCE = ROOT / "fixture.py"
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
OUTPUT = args.output.resolve()
OUTPUT.mkdir(mode=0o700, parents=True, exist_ok=False)
original = SOURCE.read_bytes()
cases = [
    ("no-loopback-dns", "test_loopback_fixture_never_resolves_a_host_name", 'socketserver.TCPServer.server_bind(self)', 'http.server.HTTPServer.server_bind(self)'),
    ("receipt-redaction", "test_finite_synthetic_trace_and_redacted_receipt", 'run_sha256=digest(self.run)', 'run_sha256=self.run'),
    ("host-identity", "test_host_origin_and_nonce_are_checked_over_http", 'if self.headers.get_all("Host", []) != [urlsplit(self.server.state.origin).netloc]:', 'if False:'),
    ("duplicate-json", "test_body_framing_and_json_are_bounded", 'if key in result:', 'if False:'),
    ("context-inheritance", "test_context_contamination_is_an_observation_failure", 'if data["initial_cookie"] != previous or data["initial_storage"] != previous:', 'if False:'),
    ("reopen-persistence", "test_reopen_requires_both_cookie_and_local_storage_persistence", 'if data["initial_cookie"] != previous or data["initial_storage"] != previous:', 'if False:'),
    ("ordinary-instance", "test_ordinary_document_replacement_is_rejected", 'if instance != original["instance"] or self.page_gets["ordinary"] != 1:', 'if False:'),
    ("report-replay", "test_ack_and_report_replay_do_not_advance", 'if seq <= self.latest_seq.get(key, 0):', 'if False:'),
    ("report-bound", "test_report_queue_never_silently_overwrites", 'if len(self.reports) >= MAX_REPORTS:', 'if False:'),
    ("command-bound", "test_command_queue_never_silently_overwrites", 'if len(queue) >= MAX_COMMANDS:', 'if False:'),
    ("absolute-deadline", "test_fixture_has_an_absolute_deadline", 'if time.monotonic() >= self.deadline:', 'if False:'),
]
observations = []
failure = None
active_case = None
environment = dict(os.environ, PYTHONDONTWRITEBYTECODE="1")
try:
    for name, test, guard, mutant in cases:
        active_case = name
        text = original.decode()
        assert text.count(guard) == 1, name
        SOURCE.write_bytes(text.replace(guard, mutant, 1).encode("utf-8"))
        # A timed-out child is never assertion RED. Preserve a diagnostic
        # stack before its unchanged deadline, then retain captured output.
        child = ("import faulthandler, unittest; "
                 "faulthandler.dump_traceback_later(8, repeat=False); "
                 "unittest.main(module=None)")
        try:
            result = subprocess.run([sys.executable, "-B", "-c", child,
                                     f"fixture_selfcheck.ProtocolChecks.{test}"],
                                    cwd=ROOT, env=environment, capture_output=True, timeout=12)
        except subprocess.TimeoutExpired as error:
            (OUTPUT / f"protocol-red-{name}.log").write_bytes((error.stdout or b"") + (error.stderr or b""))
            raise
        log = result.stdout + result.stderr
        (OUTPUT / f"protocol-red-{name}.log").write_bytes(log)
        assert result.returncode != 0 and b"AssertionError" in log and b"Ran 1 test" in log, (name, log.decode(errors="replace"))
        observations.append(dict(guard=name, exact_test=test, returncode=result.returncode,
                                 assertion_red=True, log=f"protocol-red-{name}.log", log_sha256=hashlib.sha256(log).hexdigest()))
        SOURCE.write_bytes(original)
        assert SOURCE.read_bytes() == original
except BaseException as error:
    failure = dict(case=active_case, kind=type(error).__name__)
    raise
finally:
    SOURCE.write_bytes(original)
    if failure is not None:
        (OUTPUT / "protocol-guard-receipt.json").write_text(json.dumps(dict(
            kind="fixture-protocol-guard-controls", status="FAIL", failure=failure,
            native_qualification="NOT_CLAIMED", native_browser_executed=False,
            expected_assertion_reds=observations,
            byte_exact_source_restored=SOURCE.read_bytes() == original,
            fixture_sha256=hashlib.sha256(original).hexdigest(),
        ), indent=2) + "\n", encoding="utf-8")
restored = subprocess.run([sys.executable, "-B", "fixture_selfcheck.py", "--output", str(OUTPUT / "restored")], cwd=ROOT,
                          env=environment, capture_output=True, timeout=20)
(OUTPUT / "protocol-restored.log").write_bytes(restored.stdout + restored.stderr)
assert restored.returncode == 0
assert SOURCE.read_bytes() == original
restored_result = json.loads((OUTPUT / "restored/protocol-selfcheck-receipt.json").read_text(encoding="utf-8"))
restored_checks = restored_result["checks_run"]
assert type(restored_checks) is int and restored_checks > 0
assert restored_result["failures"] == 0 and restored_result["errors"] == 0
receipt = dict(kind="fixture-protocol-guard-controls", native_qualification="NOT_CLAIMED",
               native_browser_executed=False, javascript_executed=False, rust_compiled=False,
               expected_assertion_reds=observations, byte_exact_source_restored=True,
               fixture_sha256=hashlib.sha256(original).hexdigest(), restored_returncode=restored.returncode,
               restored_log="protocol-restored.log", restored_checks=restored_checks)
(OUTPUT / "protocol-guard-receipt.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
print(json.dumps(dict(protocol_assertion_reds=len(observations), byte_exact_restored=True,
                      final_protocol_checks=restored_checks, native_browser_executed=False, rust_compiled=False)))
