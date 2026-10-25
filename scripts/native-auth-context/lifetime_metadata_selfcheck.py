#!/usr/bin/env python3
"""Controlled fresh-file/HTTP checks only; no browser, COM, SQLite or Cookie data."""
import copy
import http.client
import json
from pathlib import Path
import tempfile
import time
from types import SimpleNamespace
import unittest
from unittest.mock import patch

import fixture
import lifetime
import run as native


class BeforeReleaseMetadataChecks(unittest.TestCase):
    def setUp(self):
        self.scratch = tempfile.TemporaryDirectory()
        self.addCleanup(self.scratch.cleanup)
        self.output = Path(self.scratch.name).resolve(strict=True)
        self.court = fixture.Fixture(10)
        self.addCleanup(self.court.server.server_close)
        self.manifest = self.court.manifest(self.output)
        self.profile = self.output / "profiles/a/EBWebView/Default"
        (self.profile / "Network").mkdir(parents=True)
        self.court.state.index = 2
        self.court.state.acked.update({0, 1})
        self.action = self.court.state.status()["action"]
        self.payload = {"run": self.court.state.run, "action": self.action, "locator": {
            "schema": 1, "test_only": True, "run": self.court.state.run, "phases": {"a": {
                "relative_directory": "EBWebView/Default",
                "store_identifier": list(bytes.fromhex(self.manifest["profiles"]["a"]["store_identifier"]))}}}}
        self.observer = lifetime.BeforeReleaseFiles(self.court.state, self.manifest, self.output)

    def test_fixed_six_slots_report_only_metadata_without_content_reads(self):
        (self.profile / "Network/Cookies").write_bytes(b"")
        (self.profile / "Network/Cookies-wal").write_bytes(b"synthetic-file-payload-must-not-escape")
        with patch.object(Path, "open", side_effect=AssertionError("pre-close metadata opened file contents")), \
             patch.object(native, "_cookie_database_observation", side_effect=AssertionError("SQLite reader reached")), \
             patch.object(native, "_local_state_observation", side_effect=AssertionError("Local State reader reached")):
            result = self.observer.observe(self.payload)
        self.assertEqual(result, {"status": "observed", "slots": {
            "network": "regular-empty", "network-wal": "regular-nonempty", "network-shm": "absent",
            "legacy": "absent", "legacy-wal": "absent", "legacy-shm": "absent"}})
        public = json.dumps(result)
        self.assertNotIn(str(self.output), public)
        self.assertNotIn("synthetic-file-payload-must-not-escape", public)

    def test_wrong_store_identity_is_refused_before_any_fixed_slot(self):
        bad = copy.deepcopy(self.payload)
        bad["locator"]["phases"]["a"]["store_identifier"][0] ^= 1
        with patch.object(native, "_fixed_slot_metadata", return_value="absent") as inspect:
            self.assertEqual(self.observer.observe(bad), {"status": "unavailable"})
            inspect.assert_not_called()

    def test_pending_action_is_exact_and_successful_request_is_once_only(self):
        bad = copy.deepcopy(self.payload)
        bad["action"]["url"] += "?changed"
        self.assertEqual(self.observer.observe(bad), {"status": "unavailable"})
        with patch.object(native, "_fixed_slot_metadata", return_value="absent") as inspect:
            self.assertEqual(self.observer.observe(self.payload)["status"], "observed")
            self.assertEqual(inspect.call_count, 6)
            self.assertEqual(self.observer.observe(self.payload), {"status": "unavailable"})
            self.assertEqual(inspect.call_count, 6)

    def test_changed_action_or_expired_budget_cannot_publish_late_metadata(self):
        def changed(_root, _relative):
            self.court.state.index = 3
            return "regular-nonempty"
        with patch.object(native, "_fixed_slot_metadata", side_effect=changed):
            self.assertEqual(self.observer.observe(self.payload), {"status": "unavailable"})
        self.court.state.index = 2
        with patch.object(native, "_fixed_slot_metadata", side_effect=AssertionError("expired metadata still inspected files")):
            self.assertEqual(native.before_release_file_metadata(
                self.manifest, self.output, self.payload["locator"], time.monotonic() - 1), {"status": "unavailable"})

    def test_existing_entry_guard_refuses_reparse_metadata(self):
        target = self.profile / "Cookies"
        target.write_bytes(b"synthetic")
        original = Path.lstat
        def reparse(path, *args, **kwargs):
            info = original(path, *args, **kwargs)
            if path == target:
                return SimpleNamespace(st_mode=info.st_mode, st_nlink=info.st_nlink,
                                       st_size=info.st_size, st_file_attributes=0x400)
            return info
        with patch.object(Path, "lstat", reparse):
            self.assertEqual(native._fixed_slot_metadata(self.profile, "Cookies"), "refused-link")

    def test_loopback_route_requires_driver_token_and_never_acknowledges_b(self):
        self.court.server.before_release_files = self.observer
        self.court.server.RequestHandlerClass = lifetime.LifetimeMetadataHandler
        with self.court:
            def request(token):
                connection = http.client.HTTPConnection("127.0.0.1", self.court.server.server_port, timeout=2)
                try:
                    connection.request("POST", "/driver/lifetime-before-release", json.dumps(self.payload),
                                       {"X-Court-Token": token, "Content-Type": "application/json"})
                    response = connection.getresponse()
                    return response.status, json.loads(response.read(8192))
                finally:
                    connection.close()
            self.assertEqual(request("synthetic-wrong-token")[0], 403)
            code, result = request(self.court.state.driver_token)
            self.assertEqual(code, 200)
            self.assertEqual(result["observation"]["status"], "observed")
            self.assertEqual(result["seq"], 2)
            self.assertEqual(result["phase"], "a-before-context-release")
            self.assertNotIn(2, self.court.state.acked)
            self.assertEqual(self.court.state.index, 2)
            self.assertIsNone(self.court.state.failure)


if __name__ == "__main__":
    unittest.main()
