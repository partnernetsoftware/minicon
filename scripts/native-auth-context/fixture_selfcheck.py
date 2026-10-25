#!/usr/bin/env python3
"""Protocol checks only: no JS engine, browser, CLI, Cargo, or provider runs."""
import argparse
import hashlib
import http.client
import json
from pathlib import Path
import secrets
import time
import tempfile
import unittest
from unittest.mock import patch
from urllib.parse import quote

import fixture


def request(court, method, path, body=None, headers=None, raw=None):
    conn = http.client.HTTPConnection("127.0.0.1", court.server.server_port, timeout=3)
    head = {"X-Court-Token": court.state.driver_token}
    if body is not None:
        raw = json.dumps(body).encode()
    if raw is not None:
        head["Content-Type"] = "application/json"
    head.update(headers or {})
    conn.request(method, path, body=raw, headers=head)
    response = conn.getresponse()
    data = response.read(fixture.MAX_BODY * 8)
    conn.close()
    return response.status, data


def page_path(state, phase, kind="page"):
    return f"/{kind}/{state.run}/{phase}/{state.tickets[phase]}"


def report_data(state, phase, instance=None):
    role = "a" if phase == "a-reopen" else phase
    previous = state.values["a"] if phase == "a-reopen" else None
    instance = instance or secrets.token_hex(16)
    return dict(version=1, run=state.run, phase=phase, ticket=state.tickets[phase],
                instance=instance, seq=1, event="loaded", challenge=None, error=None,
                input="unsaved:" + instance, initial_cookie=previous, initial_storage=previous,
                cookie=state.values[role], storage=state.values[role], history_length=2,
                history_instance=instance if phase == "ordinary" else None,
                history_step=2 if phase == "ordinary" else None,
                path=page_path(state, phase), query="?step=two" if phase == "ordinary" else "")


def report(court, phase, data):
    role = "a" if phase == "a-reopen" else phase
    return request(court, "POST", page_path(court.state, phase, "report"), data,
                   {"Origin": court.state.origin,
                    "Cookie": court.state.key + "=" + quote(court.state.values[role], safe="")})


def initial_trace(court):
    ordinary = None
    for index, phase in enumerate(fixture.PHASES):
        assert request(court, "GET", page_path(court.state, phase))[0] == 200
        data = report_data(court.state, phase)
        if phase == "ordinary":
            ordinary = data.copy()
        assert report(court, phase, data)[0] == 200
        assert request(court, "POST", "/driver/ack", {"seq": index})[0] == 200
    return ordinary


class ProtocolChecks(unittest.TestCase):
    def test_loopback_fixture_never_resolves_a_host_name(self):
        with patch("socket.getfqdn", side_effect=AssertionError("loopback fixture attempted DNS")):
            with fixture.Fixture() as court:
                self.assertEqual(court.server.server_name, "127.0.0.1")
                self.assertEqual(request(court, "GET", "/driver/status")[0], 200)

    def test_finite_synthetic_trace_and_redacted_receipt(self):
        with fixture.Fixture() as court:
            with tempfile.TemporaryDirectory() as directory:
                manifest = court.manifest(Path(directory).resolve())
                self.assertEqual(manifest["schema"], 1)
                self.assertIs(manifest["test_only"], True)
                self.assertNotIn("prototype", manifest)
                self.assertNotIn("helper", manifest)
            original = initial_trace(court)
            self.assertEqual(request(court, "POST", "/driver/ack", {"seq": 4})[0], 200)
            for seq, event, step in [(2, "requested", 2), (3, "history", 1), (4, "history", 2)]:
                status, raw = request(court, "GET", page_path(court.state, "ordinary", "command"))
                self.assertEqual(status, 200)
                command = json.loads(raw)
                data = dict(original, seq=seq, event=event, challenge=command["challenge"],
                            history_step=step, query="?step=two" if step == 2 else "")
                self.assertEqual(report(court, "ordinary", data)[0], 200)
            receipt = court.state.receipt()
            self.assertEqual(receipt["observation_status"], "complete")
            self.assertEqual(len(receipt["checks"]), 8)
            self.assertEqual(receipt["native_qualification"], "NOT_CLAIMED")
            self.assertFalse(receipt["native_execution_attested"])
            status, raw = request(court, "GET", "/driver/receipt")
            self.assertEqual(status, 200)
            snapshot = json.loads(raw)
            self.assertEqual(snapshot, court.state.driver_receipt_snapshot)
            self.assertEqual(request(court, "GET", "/driver/status")[0], 200)
            self.assertEqual(court.state.driver_receipt_snapshot, snapshot)
            text = json.dumps(receipt)
            for private in [court.state.run, court.state.driver_token, *court.state.tickets.values(),
                            *court.state.values.values(), original["instance"], original["input"]]:
                self.assertNotIn(private, text)

    def test_host_origin_and_nonce_are_checked_over_http(self):
        with fixture.Fixture() as court:
            self.assertEqual(request(court, "GET", "/driver/status", headers={"X-Court-Token": "wrong"})[0], 403)
            self.assertEqual(request(court, "GET", "/driver/status", headers={"Host": "other.invalid"})[0], 400)
            data = report_data(court.state, "ordinary")
            self.assertEqual(request(court, "POST", page_path(court.state, "ordinary", "report"), data,
                                     {"Origin": "http://other.invalid"})[0], 403)
            data["ticket"] = None
            self.assertEqual(report(court, "ordinary", data)[0], 403)

    def test_body_framing_and_json_are_bounded(self):
        with fixture.Fixture() as court:
            self.assertEqual(request(court, "POST", "/driver/ack", raw=b"{}", headers={"Content-Length": str(fixture.MAX_BODY + 1)})[0], 413)
            self.assertEqual(request(court, "POST", "/driver/ack", raw=b'{"seq":0,"seq":1}')[0], 400)
            self.assertEqual(request(court, "POST", "/driver/ack", raw=b"[]")[0], 400)
            self.assertEqual(request(court, "POST", "/driver/ack", raw=b"{}", headers={"Transfer-Encoding": "chunked"})[0], 400)

    def test_context_contamination_is_an_observation_failure(self):
        with fixture.Fixture() as court:
            original = report_data(court.state, "ordinary")
            self.assertEqual(report(court, "ordinary", original)[0], 200)
            self.assertEqual(request(court, "POST", "/driver/ack", {"seq": 0})[0], 200)
            contaminated = report_data(court.state, "a")
            contaminated["initial_storage"] = court.state.values["ordinary"]
            self.assertEqual(report(court, "a", contaminated)[0], 422)
            self.assertEqual(court.state.failure, "context-a-inherited-storage")
            self.assertEqual(court.state.receipt()["initial_storage_observations"]["a"],
                             {"initial_cookie": "matches", "initial_storage": "unexpected"})

    def test_reopen_requires_both_cookie_and_local_storage_persistence(self):
        for missing in ("initial_cookie", "initial_storage"):
            with fixture.Fixture() as court:
                for index, phase in enumerate(fixture.PHASES[:3]):
                    self.assertEqual(report(court, phase, report_data(court.state, phase))[0], 200)
                    self.assertEqual(request(court, "POST", "/driver/ack", {"seq": index})[0], 200)
                data = report_data(court.state, "a-reopen")
                data[missing] = None
                self.assertEqual(report(court, "a-reopen", data)[0], 422)
                self.assertEqual(court.state.failure, "context-a-reopen-inherited-storage")
                observed = court.state.receipt()["initial_storage_observations"]["a-reopen"]
                self.assertEqual(observed[missing], "missing")
                self.assertEqual(observed["initial_storage" if missing == "initial_cookie" else "initial_cookie"], "matches")

    def test_ordinary_document_replacement_is_rejected(self):
        for acknowledged in (False, True):
            with fixture.Fixture() as court:
                original = initial_trace(court)
                challenge = None
                if acknowledged:
                    request(court, "POST", "/driver/ack", {"seq": 4})
                    _, raw = request(court, "GET", page_path(court.state, "ordinary", "command"))
                    challenge = json.loads(raw)["challenge"]
                data = dict(original, instance=secrets.token_hex(16), seq=2,
                            event="requested" if acknowledged else "loaded", challenge=challenge)
                self.assertEqual(report(court, "ordinary", data)[0], 422)
                self.assertEqual(court.state.failure, "ordinary-document-reloaded")

    def test_ack_and_report_replay_do_not_advance(self):
        with fixture.Fixture() as court:
            self.assertEqual(request(court, "POST", "/driver/ack", {"seq": 3})[0], 409)
            self.assertEqual(court.state.index, 0)
            data = report_data(court.state, "ordinary")
            self.assertEqual(report(court, "ordinary", data)[0], 200)
            self.assertEqual(report(court, "ordinary", data)[0], 409)

    def test_report_queue_never_silently_overwrites(self):
        with fixture.Fixture() as court:
            court.state.reports.extend({"synthetic": True} for _ in range(fixture.MAX_REPORTS))
            self.assertEqual(report(court, "ordinary", report_data(court.state, "ordinary"))[0], 429)
            self.assertEqual(len(court.state.reports), fixture.MAX_REPORTS)
            self.assertEqual(court.state.failure, "report-queue-full")

    def test_command_queue_never_silently_overwrites(self):
        state = fixture.State()
        for _ in range(fixture.MAX_COMMANDS):
            state.queue("report")
        with self.assertRaises(fixture.Rejected):
            state.queue("report")
        self.assertEqual(len(state.commands["ordinary"]), fixture.MAX_COMMANDS)
        self.assertEqual(state.failure, "command-queue-full")

    def test_fixture_has_an_absolute_deadline(self):
        state = fixture.State(0.01)
        time.sleep(0.02)
        with self.assertRaises(fixture.Rejected):
            state.alive()
        self.assertEqual(state.failure, "fixture-deadline-expired")


    def test_cookie_diagnostic_feature_identity_is_bound_before_accepting_driver_result(self):
        import run as native_runner

        declared_features = ["default", "experimental-rust-console", "native-auth-court"]
        diagnostic = ["minicon-web/native-profile-diagnostics"]
        declaration = dict(schema=1, source_commit="a" * 40, test_only=True,
                           features=declared_features, diagnostic_features=diagnostic,
                           os="windows", isa="x86_64", binary_sha256="b" * 64,
                           helper_sha256=None)
        observed = dict(checks={name: True for name in native_runner.CHECKS},
                        run_sha256="c" * 64,
                        page_get_counts={name: 1 for name in fixture.PHASES},
                        report_count=8, report_hashes={}, observation_status="complete",
                        failure=None)
        driver = dict(kind="native-auth-court-test-build", test_only=True,
                      production_test_build=True, features=declared_features,
                      diagnostic_features=diagnostic, os="windows", isa="x86_64",
                      status="native-observations-complete", fixture=observed)
        with tempfile.TemporaryDirectory() as scratch, patch.object(native_runner, "host_os", return_value="windows"):
            path = Path(scratch) / "build.json"
            fixture.private_json(path, declaration)
            build = native_runner.metadata(path, "b" * 64, None)
            self.assertEqual(build["diagnostic_features"], diagnostic)
            self.assertIsNone(native_runner.validate(driver, observed, build, observed))
            for changed in [[], ["other"], diagnostic + ["undeclared"]]:
                fixture.private_json(path, dict(declaration, diagnostic_features=changed))
                with self.assertRaisesRegex(native_runner.CourtError, "build-metadata-invalid"):
                    native_runner.metadata(path, "b" * 64, None)
                for status in ["native-observations-complete", "FAIL"]:
                    mismatched = dict(driver, diagnostic_features=changed, status=status)
                    with self.assertRaisesRegex(native_runner.CourtError, "driver-test-build-identity-mismatch"):
                        native_runner.validate(mismatched, observed, build, observed)

    def test_common_diagnostic_root_guard_blocks_all_reads_but_missing_carrier_does_not(self):
        import run as native_runner
        from types import SimpleNamespace
        with tempfile.TemporaryDirectory() as scratch:
            output = Path(scratch).resolve(strict=True)
            roots = {label: output / "profiles" / label for label in ("a", "b")}
            for root in roots.values():
                root.mkdir(parents=True)
            manifest = {"schema": 1, "test_only": True, "run": "1" * 32,
                        "profiles": {label: {"directory": str(root), "store_identifier": "2" * 32}
                                     for label, root in roots.items()}}
            clock = (time.time(), time.monotonic())
            # A missing or wrong-run carrier does not revoke valid fresh-root
            # authority for the independent fixed-slot comparison.
            for wrong_run in (False, True):
                if wrong_run:
                    fixture.private_json(output / "native-profile-locator.private.json",
                                         {"schema": 1, "test_only": True, "run": "3" * 32,
                                          "phases": {"a": {"store_identifier": [0x22] * 16,
                                                           "relative_directory": "Default"}}})
                with patch.object(native_runner, "_cookie_database_observation", return_value={"status": "synthetic"}) as db, \
                     patch.object(native_runner, "_local_state_observation", return_value={"status": "synthetic"}) as local:
                    observed = native_runner.windows_cookie_snapshot(manifest, output, "synthetic", clock)
                    self.assertEqual(observed["status"], "diagnostics-only")
                    self.assertEqual(observed["verified_profile_databases"]["status"], "private-locator-unavailable")
                    self.assertEqual(db.call_count, 8)
                    self.assertEqual(local.call_count, 4)
            original_lstat = Path.lstat
            def lstat(path, *args, **kwargs):
                if path == output.parent:
                    return SimpleNamespace(st_mode=native_runner.stat.S_IFDIR, st_file_attributes=0x400)
                return original_lstat(path, *args, **kwargs)
            with patch.object(Path, "lstat", new=lstat), \
                 patch.object(native_runner, "_verified_locator_snapshot") as locator, \
                 patch.object(native_runner, "_cookie_database_observation") as db, \
                 patch.object(native_runner, "_local_state_observation") as local:
                observed = native_runner.windows_cookie_snapshot(manifest, output, "synthetic", clock)
                self.assertEqual(observed["status"], "fresh-profile-authority-unavailable")
                locator.assert_not_called()
                db.assert_not_called()
                local.assert_not_called()

    def test_private_locator_is_bound_and_reports_no_path(self):
        import run as native_runner
        import tempfile
        with tempfile.TemporaryDirectory() as scratch:
            output = Path(scratch).resolve(strict=True)
            roots = {label: output / "profiles" / label for label in ("a", "b")}
            for root in roots.values():
                (root / "unreported-profile-name").mkdir(parents=True)
            manifest = {"run": "1" * 32, "profiles": {
                label: {"store_identifier": ("2" if label == "a" else "3") * 32}
                for label in roots}}
            phases = {phase: {"store_identifier": list(bytes.fromhex(manifest["profiles"]["b" if phase == "b" else "a"]["store_identifier"])),
                              "relative_directory": "unreported-profile-name"}
                      for phase in ("a", "b", "a-reopen")}
            carrier = {"schema": 1, "test_only": True, "run": manifest["run"], "phases": phases}
            fixture.private_json(output / "native-profile-locator.private.json", carrier)
            if native_runner.sqlite3 is not None:
                location = roots["a"] / "unreported-profile-name" / "Network"
                location.mkdir()
                with native_runner.sqlite3.connect(location / "Cookies") as database:
                    database.execute("CREATE TABLE cookies (is_persistent INTEGER, has_expires INTEGER, expires_utc INTEGER)")
                    database.execute("INSERT INTO cookies VALUES (1, 1, ?)",
                                     (int((time.time() + 600 + 11644473600) * 1000000),))
                database.close()
            observed = native_runner._verified_locator_snapshot(manifest, output, roots, time.time())
            self.assertEqual(observed["status"], "verified-private-locator")
            self.assertEqual(observed["a_reopen_directory_relation"], "same")
            if native_runner.sqlite3 is not None:
                self.assertEqual(observed["phases"]["a"]["network"]["status"], "read-only-main-db-observed")
                self.assertEqual(observed["phases"]["a"]["network"]["counts"]["persistent"], 1)
            encoded = json.dumps(observed)
            self.assertNotIn("unreported-profile-name", encoded)
            self.assertNotIn(str(output), encoded)
            carrier["run"] = "4" * 32
            fixture.private_json(output / "native-profile-locator.private.json", carrier)
            self.assertEqual(native_runner._verified_locator_snapshot(manifest, output, roots, time.time())["status"],
                             "private-locator-unavailable")

    def test_private_locator_reopen_change_and_outside_are_distinct(self):
        import run as native_runner
        import tempfile
        with tempfile.TemporaryDirectory() as scratch:
            output = Path(scratch).resolve(strict=True)
            root = output / "profiles" / "a"
            for name in ("first", "second"):
                (root / name).mkdir(parents=True)
            manifest = {"run": "1" * 32, "profiles": {"a": {"store_identifier": "2" * 32}}}
            phases = {phase: {"store_identifier": [0x22] * 16, "relative_directory": name}
                      for phase, name in (("a", "first"), ("a-reopen", "second"))}
            carrier = {"schema": 1, "test_only": True, "run": manifest["run"], "phases": phases}
            fixture.private_json(output / "native-profile-locator.private.json", carrier)
            observed = native_runner._verified_locator_snapshot(manifest, output, {"a": root}, time.time())
            self.assertEqual(observed["a_reopen_directory_relation"], "different")
            phases["a-reopen"]["relative_directory"] = "../a/first"
            fixture.private_json(output / "native-profile-locator.private.json", carrier)
            self.assertEqual(native_runner._verified_locator_snapshot(manifest, output, {"a": root}, time.time())["status"],
                             "private-locator-unavailable")


if __name__ == "__main__":
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    suite = unittest.defaultTestLoader.loadTestsFromTestCase(ProtocolChecks)
    result = unittest.TextTestRunner(verbosity=2).run(suite)
    root = Path(__file__).resolve().parent
    data = dict(kind="fixture-protocol-self-checks-only", checks_run=result.testsRun,
                failures=len(result.failures), errors=len(result.errors),
                status="protocol-checks-completed" if result.wasSuccessful() else "protocol-checks-failed",
                native_browser_executed=False, page_javascript_executed=False,
                rust_driver_compiled=False, native_qualification="NOT_CLAIMED",
                source_sha256={name: hashlib.sha256((root / name).read_bytes()).hexdigest()
                               for name in ("fixture.py", "page.js", "fixture_selfcheck.py", "run.py")})
    fixture.private_json(output / "protocol-selfcheck-receipt.json", data)
    raise SystemExit(0 if result.wasSuccessful() else 1)
