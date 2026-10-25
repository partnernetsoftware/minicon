#!/usr/bin/env python3
"""Local observations for the test-only native auth context court. No browser is launched here.

The finite page protocol reports fixture-owned DOM/storage, never browser
profiles, provider grants, credentials, or arbitrary JavaScript evaluation.
Owner: PRD 28 / PRD 31.01.02. The protocol fixture alone is not native qualification.
"""
from __future__ import annotations

import argparse
from collections import Counter, deque
import hashlib
import hmac
import http.client
import http.cookies
import http.server
import io
import json
import os
from pathlib import Path
import re
import secrets
import socket
import socketserver
import threading
import time
from urllib.parse import unquote, urlsplit

MAX_BODY = 8192
MAX_HEADERS = 8192
MAX_REPORTS = 128
MAX_REQUESTS = 4096
MAX_DISTINCT_PATHS = 64
MAX_COMMANDS = 8
PHASES = ("ordinary", "a", "b", "a-reopen")
STEPS = ("ordinary", "a", "b", "a-reopen", "ordinary-return", "history-back", "history-forward")
HEX = re.compile(r"^[0-9a-f]{32}$")
ROOT = Path(__file__).resolve().parent


def digest(value: str | bytes) -> str:
    return hashlib.sha256(value.encode() if isinstance(value, str) else value).hexdigest()


def private_json(path: Path, data: dict) -> None:
    raw = (json.dumps(data, indent=2) + "\n").encode()
    temporary = path.with_name(path.name + "." + secrets.token_hex(8) + ".tmp")
    try:
        fd = os.open(temporary, os.O_CREAT | os.O_EXCL | os.O_WRONLY, 0o600)
        with os.fdopen(fd, "wb") as output:
            output.write(raw)
            output.flush()
            os.fsync(output.fileno())
        os.replace(temporary, path)
    finally:
        temporary.unlink(missing_ok=True)


def object_json(raw: bytes) -> dict:
    def unique(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError("duplicate-json-key")
            result[key] = value
        return result
    result = json.loads(raw.decode("utf-8"), object_pairs_hook=unique,
                        parse_constant=lambda _: (_ for _ in ()).throw(ValueError("nonfinite-json")))
    if not isinstance(result, dict):
        raise ValueError("json-object-required")
    return result


class Rejected(Exception):
    def __init__(self, status: int, code: str):
        self.status, self.code = status, code


class State:
    def __init__(self, deadline: float = 120):
        if not 0 < deadline <= 180:
            raise ValueError("deadline must be in (0, 180]")
        self.run = secrets.token_hex(16)
        self.driver_token = secrets.token_hex(16)
        self.tickets = {phase: secrets.token_hex(16) for phase in PHASES}
        self.key = "mc_court_" + self.run[:16]
        self.values = {"ordinary": "O:" + self.run, "a": "A:" + self.run, "b": "B:" + self.run}
        self.deadline = time.monotonic() + deadline
        self.lock = threading.RLock()
        self.index = 0
        self.acked = set()
        self.failure = None
        self.initial_storage_observations = {}
        self.complete = False
        self.commands = {phase: deque() for phase in PHASES}
        self.reports = deque()
        self.latest_seq = {}
        self.loaded = {}
        self.challenge = None
        self.checks = {}
        self.request_counts = Counter()
        self.path_counts = Counter()
        self.page_gets = Counter()
        self.origin = ""
        self.rejected = Counter()
        self.receipt_retrieved = False
        self.driver_receipt_snapshot = None

    def alive(self):
        if time.monotonic() >= self.deadline:
            if not self.complete and self.failure is None:
                self.failure = "fixture-deadline-expired"
            raise Rejected(410, "fixture-deadline-expired")

    def fail(self, code):
        self.failure = self.failure or code
        raise Rejected(422, code)

    def count(self, method, path, route):
        with self.lock:
            self.alive()
            if sum(self.request_counts.values()) >= MAX_REQUESTS:
                self.failure = "request-limit-exceeded"
                raise Rejected(429, self.failure)
            path_hash = digest(method + " " + path)
            if path_hash not in self.path_counts and len(self.path_counts) >= MAX_DISTINCT_PATHS:
                self.failure = "request-path-bound"
                raise Rejected(429, self.failure)
            self.request_counts[method + ":" + route] += 1
            self.path_counts[path_hash] += 1

    def identity(self, run, phase, ticket):
        if not all(isinstance(value, str) for value in (run, phase, ticket)) or run != self.run or phase not in self.tickets or not hmac.compare_digest(ticket, self.tickets[phase]):
            raise Rejected(403, "page-identity-rejected")

    def page_config(self, phase):
        role = "a" if phase == "a-reopen" else phase
        return dict(run=self.run, phase=phase, ticket=self.tickets[phase],
                    storage_key=self.key, write_value=self.values[role])

    def queue(self, op):
        queue = self.commands["ordinary"]
        if len(queue) >= MAX_COMMANDS:
            self.failure = "command-queue-full"
            raise Rejected(429, self.failure)
        self.challenge = secrets.token_hex(16)
        queue.append(dict(op=op, challenge=self.challenge))

    def status(self):
        with self.lock:
            self.alive()
            action = None
            if self.index <= 4 and self.index not in self.acked and not self.failure:
                phase = STEPS[self.index]
                action = dict(seq=self.index, phase=phase,
                              kind="ordinary" if self.index == 0 else "close-auth" if self.index == 4 else "present-auth")
                if self.index < 4:
                    action["url"] = f"{self.origin}/page/{self.run}/{phase}/{self.tickets[phase]}"
            return dict(schema=1, run=self.run, state="failed" if self.failure else "observations-complete" if self.complete else "observing",
                        failure=self.failure, step=STEPS[self.index] if self.index < len(STEPS) else "complete", action=action)

    def ack(self, data):
        with self.lock:
            self.alive()
            if type(data.get("seq")) is not int or data["seq"] != self.index or self.index > 4 or self.index in self.acked:
                raise Rejected(409, "driver-ack-out-of-order")
            self.acked.add(self.index)
            if self.index == 4:
                self.queue("report")
            self.advance()

    def advance(self):
        while self.index < 4 and self.index in self.acked and STEPS[self.index] in self.loaded and not self.failure:
            self.index += 1

    def report(self, phase, data, http_cookie):
        with self.lock:
            self.alive()
            self.identity(data.get("run"), data.get("phase"), data.get("ticket", ""))
            if data.get("phase") != phase or data.get("version") != 1:
                raise Rejected(400, "report-shape-rejected")
            instance, seq = data.get("instance"), data.get("seq")
            if not isinstance(instance, str) or not HEX.fullmatch(instance) or type(seq) is not int or not 1 <= seq <= MAX_REPORTS:
                raise Rejected(400, "report-instance-rejected")
            if data.get("event") not in ("loaded", "requested", "history") or data.get("error") not in (None, "storage-unavailable"):
                raise Rejected(400, "report-event-rejected")
            for name in ("input", "path", "query"):
                if not isinstance(data.get(name), str) or len(data[name]) > (256 if name != "input" else 128):
                    raise Rejected(400, "report-string-bound")
            for name in ("initial_cookie", "initial_storage", "cookie", "storage", "challenge", "history_instance"):
                if data.get(name) is not None and (not isinstance(data[name], str) or len(data[name]) > 128):
                    raise Rejected(400, "report-value-bound")
            if type(data.get("history_length")) is not int or not 1 <= data["history_length"] <= 1024:
                raise Rejected(400, "report-history-bound")
            if data.get("history_step") is not None and (type(data["history_step"]) is not int or data["history_step"] not in (1, 2)):
                raise Rejected(400, "report-history-bound")
            key = (phase, instance)
            if seq <= self.latest_seq.get(key, 0):
                raise Rejected(409, "report-replay-rejected")
            if len(self.reports) >= MAX_REPORTS:
                self.failure = "report-queue-full"
                raise Rejected(429, self.failure)
            self.latest_seq[key] = seq
            self.reports.append(dict(phase=phase, instance_sha256=digest(instance), body_sha256=digest(json.dumps(data, sort_keys=True))))
            role = "a" if phase == "a-reopen" else phase
            want = self.values[role]
            if data["error"]:
                self.fail("page-storage-unavailable")
            if data["cookie"] != want or data["storage"] != want or http_cookie != want:
                self.fail("context-cookie-storage-or-wire-mismatch")
            if phase not in self.loaded:
                if self.index > 3 or phase != STEPS[self.index]:
                    raise Rejected(409, "initial-report-out-of-order")
                previous = self.values["a"] if phase == "a-reopen" else None
                # Fixed categories distinguish isolation from missing persistence
                # without exposing fixture values, cookies or profile contents.
                self.initial_storage_observations[phase] = {
                    name: "matches" if data[name] == previous else "missing" if data[name] is None else "unexpected"
                    for name in ("initial_cookie", "initial_storage")
                }
                if data["initial_cookie"] != previous or data["initial_storage"] != previous:
                    self.fail("context-" + phase + "-inherited-storage")
                if data["event"] != "loaded":
                    self.fail("initial-page-report-missing")
                if data["input"] != "unsaved:" + instance:
                    self.fail("initial-dom-draft-mismatch")
                if phase == "ordinary" and (data["history_instance"] != instance or data["history_step"] != 2 or data["history_length"] < 2):
                    self.fail("ordinary-history-not-established")
                self.loaded[phase] = data.copy()
                self.checks[phase + "-initial-storage"] = True
                self.advance()
            elif phase == "ordinary" and self.index >= 4:
                original = self.loaded["ordinary"]
                if instance != original["instance"] or self.page_gets["ordinary"] != 1:
                    self.fail("ordinary-document-reloaded")
                if 4 not in self.acked:
                    return
                if data["challenge"] != self.challenge:
                    raise Rejected(409, "report-challenge-stale")
                if data["input"] != original["input"] or data["history_instance"] != original["instance"]:
                    self.fail("ordinary-unsaved-dom-lost")
                if data["history_length"] != original["history_length"]:
                    self.fail("ordinary-history-length-changed")
                if self.index == 4:
                    if data["event"] != "requested" or data["history_step"] != 2:
                        self.fail("ordinary-return-state-mismatch")
                    self.checks["ordinary-live-dom-cookie-storage-return"] = True
                    self.checks["ordinary-one-document-load"] = True
                    self.index = 5
                    self.queue("history-back")
                elif self.index == 5:
                    if data["event"] != "history" or data["history_step"] != 1 or data["query"]:
                        self.fail("ordinary-history-back-mismatch")
                    self.checks["ordinary-native-history-back"] = True
                    self.index = 6
                    self.queue("history-forward")
                elif self.index == 6:
                    if data["event"] != "history" or data["history_step"] != 2 or data["query"] != "?step=two":
                        self.fail("ordinary-history-forward-mismatch")
                    self.checks["ordinary-native-history-forward"] = True
                    self.index = 7
                    self.complete = True

    def receipt(self):
        with self.lock:
            return dict(kind="native-auth-court-fixture", schema=1,
                        observation_status="failed" if self.failure else "complete" if self.complete else "incomplete",
                        native_qualification="NOT_CLAIMED", native_execution_attested=False,
                        failure=self.failure, run_sha256=digest(self.run), checks=self.checks.copy(),
                        initial_storage_observations={phase: values.copy() for phase, values in self.initial_storage_observations.items()},
                        raw_request_path_sha256_counts=dict(self.path_counts), request_counts=dict(self.request_counts),
                        page_get_counts=dict(self.page_gets), rejected_requests=dict(self.rejected),
                        report_count=len(self.reports), report_hashes=list(self.reports),
                        unexecuted_or_unclaimed=["actual provider authorization", "proxy failure/no-direct", "late embedded_ready delivery",
                            "native map/visibility/focus", "physical keyboard/native IME/clipboard", "toolbar-driven history", "exact production artifact qualification"])


class BoundedServer(socketserver.ThreadingMixIn, http.server.HTTPServer):
    daemon_threads = True
    block_on_close = False

    def __init__(self, state):
        self.state = state
        self.slots = threading.BoundedSemaphore(8)
        super().__init__(("127.0.0.1", 0), Handler)
        state.origin = f"http://127.0.0.1:{self.server_port}"

    def process_request(self, request, address):
        if not self.slots.acquire(blocking=False):
            self.shutdown_request(request)
            return
        try:
            super().process_request(request, address)
        except BaseException:
            self.slots.release()
            raise

    def process_request_thread(self, request, address):
        try:
            super().process_request_thread(request, address)
        finally:
            self.slots.release()


class Handler(http.server.BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *_args):
        pass

    def line(self, limit):
        # A raw readline timeout can restart on every received fragment.
        # Recheck the absolute budget between bounded buffered-byte reads.
        value = bytearray()
        while len(value) < limit:
            left = self.request_deadline - time.monotonic()
            if left <= 0:
                raise Rejected(408, "request-deadline")
            self.connection.settimeout(left)
            byte = self.rfile.read(1)
            if not byte:
                break
            value += byte
            if byte == b"\n":
                break
        return bytes(value)

    def handle_one_request(self):
        self.close_connection = True
        self.request_version = "HTTP/1.1"
        self.requestline = ""
        self.request_deadline = min(self.server.state.deadline, time.monotonic() + 2)
        try:
            self.connection.settimeout(max(0.001, self.request_deadline - time.monotonic()))
            raw = self.line(2049)
            if not raw:
                return
            words = raw.decode("ascii").strip().split()
            if len(raw) > 2048 or len(words) != 3 or words[2] not in ("HTTP/1.0", "HTTP/1.1"):
                raise Rejected(400, "request-line-rejected")
            self.command, self.path, self.request_version = words
            if self.command not in ("GET", "POST"):
                raise Rejected(405, "method-rejected")
            self.requestline = " ".join(words)
            headers, total = [], 0
            while True:
                left = self.request_deadline - time.monotonic()
                if left <= 0:
                    raise Rejected(408, "request-deadline")
                self.connection.settimeout(left)
                line = self.line(MAX_HEADERS + 1)
                total += len(line)
                if total > MAX_HEADERS or not line or len(headers) >= 64:
                    raise Rejected(431, "headers-bound")
                if line == b"\r\n":
                    break
                if not line.endswith(b"\r\n") or line[:1] in (b" ", b"\t"):
                    raise Rejected(400, "headers-rejected")
                headers.append(line)
            self.headers = http.client.parse_headers(io.BytesIO(b"".join(headers) + b"\r\n"))
            if self.headers.get_all("Host", []) != [urlsplit(self.server.state.origin).netloc]:
                raise Rejected(400, "host-rejected")
            if self.headers.get("Transfer-Encoding") or len(self.headers.get_all("Content-Length", [])) > 1:
                raise Rejected(400, "framing-rejected")
            self.route()
        except Rejected as error:
            with self.server.state.lock:
                self.server.state.rejected[error.code] += 1
            self.respond(error.status, dict(error=error.code))
        except TimeoutError:
            self.respond(408, dict(error="request-deadline"))
        except (OSError, UnicodeError, ValueError, RecursionError, http.cookies.CookieError, http.client.HTTPException):
            self.respond(400, dict(error="request-rejected"))

    def respond(self, status, data=None, raw=None, content_type="application/json"):
        body = raw if raw is not None else json.dumps(data, separators=(",", ":")).encode()
        try:
            self.send_response(status)
            self.send_header("Content-Type", content_type)
            self.send_header("Content-Length", str(len(body)))
            self.send_header("Cache-Control", "no-store")
            self.send_header("Connection", "close")
            self.end_headers()
            self.wfile.write(body)
        except OSError:
            pass

    def body(self):
        size = self.headers.get("Content-Length", "")
        if not size.isdecimal() or not 0 < int(size) <= MAX_BODY:
            raise Rejected(413, "body-bound")
        if self.headers.get("Content-Type", "").split(";", 1)[0] != "application/json":
            raise Rejected(415, "json-required")
        left = self.request_deadline - time.monotonic()
        if left <= 0:
            raise Rejected(408, "request-deadline")
        self.connection.settimeout(left)
        raw = bytearray()
        while len(raw) < int(size):
            left = self.request_deadline - time.monotonic()
            if left <= 0:
                raise Rejected(408, "request-deadline")
            self.connection.settimeout(left)
            part = self.rfile.read1(min(2048, int(size) - len(raw)))
            if not part:
                raise Rejected(400, "body-truncated")
            raw.extend(part)
        return object_json(bytes(raw))

    def driver(self):
        if not hmac.compare_digest(self.headers.get("X-Court-Token", ""), self.server.state.driver_token):
            raise Rejected(403, "driver-identity-rejected")

    def route(self):
        state = self.server.state
        if not self.path.startswith("/") or self.path.startswith("//"):
            raise Rejected(400, "request-target-rejected")
        path = urlsplit(self.path).path
        parts = path.strip("/").split("/")
        state.count(self.command, self.path, parts[0] if parts[0] in ("driver", "page", "asset", "report", "command") else "unknown")
        if path in ("/driver/status", "/driver/ack", "/driver/receipt"):
            self.driver()
            if path == "/driver/status" and self.command == "GET":
                self.respond(200, state.status())
            elif path == "/driver/receipt" and self.command == "GET":
                with state.lock:
                    state.receipt_retrieved = state.complete
                    if state.complete and state.driver_receipt_snapshot is None:
                        state.driver_receipt_snapshot = state.receipt()
                    receipt = state.driver_receipt_snapshot or state.receipt()
                self.respond(200, receipt)
            elif path == "/driver/ack" and self.command == "POST":
                state.ack(self.body())
                self.respond(200, dict(acknowledged=True))
            else:
                raise Rejected(405, "method-rejected")
            return
        if len(parts) != 4 or parts[0] not in ("page", "asset", "report", "command"):
            raise Rejected(404, "route-rejected")
        kind, run, phase, ticket = parts
        state.identity(run, phase, ticket)
        if kind == "page" and self.command == "GET":
            with state.lock:
                state.page_gets[phase] += 1
            config = json.dumps(state.page_config(phase)).replace("<", "\\u003c")
            html = ("<!doctype html><meta charset=utf-8><title>MiniCon native context court</title>"
                    "<h1>Local context court</h1><label>Unsaved fixture input <input id=unsaved></label>"
                    f"<script type=application/json id=court-config>{config}</script>"
                    f"<script src='/asset/{run}/{phase}/{ticket}'></script>").encode()
            self.respond(200, raw=html, content_type="text/html; charset=utf-8")
        elif kind == "asset" and self.command == "GET":
            self.respond(200, raw=(ROOT / "page.js").read_bytes(), content_type="text/javascript; charset=utf-8")
        elif kind == "command" and self.command == "GET":
            with state.lock:
                command = state.commands[phase].popleft() if state.commands[phase] else dict(op="stop" if state.complete or state.failure else "wait")
            self.respond(200, command)
        elif kind == "report" and self.command == "POST":
            if self.headers.get_all("Origin", []) != [state.origin]:
                raise Rejected(403, "origin-rejected")
            cookie = http.cookies.SimpleCookie()
            cookie.load(self.headers.get("Cookie", ""))
            actual = unquote(cookie[state.key].value, errors="strict") if state.key in cookie else None
            state.report(phase, self.body(), actual)
            self.respond(200, dict(recorded=True))
        else:
            raise Rejected(405, "method-rejected")


class Fixture:
    def __init__(self, seconds=120):
        self.state = State(seconds)
        self.server = BoundedServer(self.state)
        self.worker = threading.Thread(target=self.server.serve_forever, kwargs={"poll_interval": 0.05}, daemon=True)

    def __enter__(self):
        self.worker.start()
        return self

    def __exit__(self, *_args):
        self.server.shutdown()
        self.server.server_close()
        self.worker.join(timeout=2)

    def manifest(self, output):
        profiles = {}
        for label in ("a", "b"):
            directory = output / "profiles" / label
            directory.mkdir(mode=0o700, parents=True, exist_ok=True)
            profiles[label] = dict(directory=str(directory.resolve()), store_identifier=secrets.token_hex(16))
        return dict(schema=1, test_only=True, origin=self.state.origin, run=self.state.run,
                    driver_token=self.state.driver_token, timeout_seconds=max(0.001, self.state.deadline - time.monotonic()),
                    pages={phase: f"{self.state.origin}/page/{self.state.run}/{phase}/{ticket}" for phase, ticket in self.state.tickets.items()},
                    profiles=profiles, output=str(output),
                    launch_environment={"MINICON_CONFIG_DIR": str(output / "config"), "MINICON_BROWSER_PROFILE_ROOT": str(output / "ordinary-profile")},
                    clear_environment=["WEBVIEW2_USER_DATA_FOLDER", "WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS", "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
                                       "MINICON_BROWSER_BIN", "MINICON_BROWSER_HTTP_PROXY", "MINICON_BROWSER_HTTPS_PROXY", "MINICON_BROWSER_NO_PROXY"],
                    scope="Test-only native driver; no provider authorization or production entry point.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=120)
    args = parser.parse_args()
    output = args.output.resolve()
    output.mkdir(mode=0o700, parents=True, exist_ok=False)
    with Fixture(args.timeout) as fixture:
        manifest = fixture.manifest(output)
        manifest["timeout_seconds"] = args.timeout
        private_json(output / "manifest.private.json", manifest)
        print(json.dumps(dict(kind="fixture-listening", manifest=str(output / "manifest.private.json"), native_browser_started=False)), flush=True)
        while time.monotonic() < fixture.state.deadline and not fixture.state.failure and not fixture.state.receipt_retrieved:
            # Keep the fixture available for the native driver's final receipt GET.
            time.sleep(0.05)
        private_json(output / "fixture-receipt.json", fixture.state.receipt())
    return 0 if fixture.state.complete else 2


if __name__ == "__main__":
    raise SystemExit(main())
