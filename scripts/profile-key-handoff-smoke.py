#!/usr/bin/env python3
"""Public selected-profile key handoff smoke. Owner: PRD_02_31_01_02_02_profile_keys.md.

Never use clipboard mode on an owner desktop. No paid/model POST, credentials
import, real login, native input, foreground activation or product test hook.
Only receipt.json is public; the private directory is never an upload input.
"""
from __future__ import annotations

import argparse
import ctypes
import hashlib
import http.client
import importlib.util
import json
import os
from pathlib import Path
import secrets
import socket
import subprocess
import sys
import threading
import time


class Failure(Exception):
    pass


def require(condition, code):
    if not condition:
        raise Failure(code)


def save(path, value):
    path.write_bytes((json.dumps(value, indent=2) + "\n").encode("utf-8"))


def sha(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def load(path, name):
    spec = importlib.util.spec_from_file_location(name, path)
    module = importlib.util.module_from_spec(spec)
    sys.modules[name] = module
    spec.loader.exec_module(module)
    return module


class RejectProxy:
    """Existing production proxy policy points here; never forwards any byte."""
    def __init__(self):
        self.socket = socket.socket()
        self.socket.bind(("127.0.0.1", 0))
        self.socket.listen(4)
        self.socket.settimeout(0.1)
        self.port = self.socket.getsockname()[1]
        self.count = 0
        self.error = False
        self.stop = threading.Event()
        self.worker = threading.Thread(target=self.run, name="k3-reject-proxy")
        self.worker.start()

    def run(self):
        while not self.stop.is_set():
            try:
                connection, _ = self.socket.accept()
            except socket.timeout:
                continue
            except OSError:
                if not self.stop.is_set():
                    self.error = True
                break
            self.count += 1
            connection.close()

    def close(self):
        self.stop.set()
        self.worker.join(2)
        self.socket.close()
        require(not self.worker.is_alive() and not self.error, "proxy-cleanup-failed")


def windows_clipboard():
    require(os.name == "nt" and os.environ.get("GITHUB_ACTIONS") == "true",
            "isolated-windows-ci-clipboard-only")
    from ctypes import wintypes as w
    user = ctypes.WinDLL("user32", use_last_error=True)
    kernel = ctypes.WinDLL("kernel32", use_last_error=True)
    user.OpenClipboard.argtypes, user.OpenClipboard.restype = [w.HWND], w.BOOL
    user.CloseClipboard.argtypes, user.CloseClipboard.restype = [], w.BOOL
    user.GetClipboardData.argtypes, user.GetClipboardData.restype = [w.UINT], w.HANDLE
    kernel.GlobalSize.argtypes, kernel.GlobalSize.restype = [w.HANDLE], ctypes.c_size_t
    kernel.GlobalLock.argtypes, kernel.GlobalLock.restype = [w.HANDLE], ctypes.c_void_p
    kernel.GlobalUnlock.argtypes, kernel.GlobalUnlock.restype = [w.HANDLE], w.BOOL
    return user, kernel


def read_clipboard():
    user, kernel = windows_clipboard()
    deadline = time.monotonic() + 2
    while not user.OpenClipboard(None):
        require(time.monotonic() < deadline, "clipboard-read-unavailable")
        time.sleep(0.01)
    try:
        handle = user.GetClipboardData(13)
        require(bool(handle), "clipboard-no-unicode")
        size = kernel.GlobalSize(handle)
        require(0 < size <= 65536 and size % 2 == 0, "clipboard-read-bound")
        pointer = kernel.GlobalLock(handle)
        require(bool(pointer), "clipboard-read-lock")
        try:
            raw = ctypes.string_at(pointer, size)
        finally:
            kernel.GlobalUnlock(handle)
        end = next((i for i in range(0, len(raw), 2) if raw[i:i + 2] == b"\0\0"), None)
        require(end is not None, "clipboard-string-not-terminated")
        text = raw[:end].decode("utf-16-le")
        require(len(text.encode("utf-8")) <= 65536, "clipboard-utf8-bound")
        return text
    finally:
        require(bool(user.CloseClipboard()), "clipboard-read-release")


_LOCK_CHILD_PHASES = ("opening", "holding", "release-observed", "closed")
_LOCK_FAILURE_CODES = frozenset((
    "lock-directory", "lock-authority", "clipboard-lock-unavailable",
    "clipboard-lock-deadline", "clipboard-lock-release", "clipboard-lock-not-ready",
    "clipboard-lock-ended-before-adapter-failure", "clipboard-lock-cleanup",
    "key-operation-no-terminal-feedback", "key-terminal-target-mismatch",
    "key-terminal-result-mismatch:clipboard_failed", "key-terminal-did-not-ack-new-request",
))


def lock_failure_code(error):
    if error is None:
        return None
    code = str(error) if isinstance(error, Failure) else None
    return code if code in _LOCK_FAILURE_CODES else "unexpected-error"


def clipboard_lock_child(directory, authority):
    """Finite status only; no clipboard payload or private log is published."""
    user, _ = windows_clipboard()
    require(directory.is_dir() and not directory.is_symlink(), "lock-directory")
    require(json.loads((directory / "authority.json").read_bytes()) == {"authority": authority},
            "lock-authority")
    result = {"phase": "opening", "ready": False, "release_observed": False,
              "closed": False, "failure": None, "cleanup_failure": None, "win32_error": None}
    primary_error = cleanup_error = None
    opened = False
    try:
        opened = bool(user.OpenClipboard(None))
        if not opened:
            result["win32_error"] = ctypes.get_last_error()
        require(opened, "clipboard-lock-unavailable")
        save(directory / "ready.json", {"locked": True})
        result.update(phase="holding", ready=True)
        deadline = time.monotonic() + 20
        while not (directory / "release").exists():
            require(time.monotonic() < deadline, "clipboard-lock-deadline")
            time.sleep(0.01)
        result.update(phase="release-observed", release_observed=True)
    except BaseException as error:
        primary_error = error
    finally:
        if opened:
            try:
                closed = bool(user.CloseClipboard())
                if not closed:
                    result["win32_error"] = ctypes.get_last_error()
                require(closed, "clipboard-lock-release")
                result.update(phase="closed", closed=True)
            except BaseException as error:
                cleanup_error = error
        result.update(failure=lock_failure_code(primary_error),
                      cleanup_failure=lock_failure_code(cleanup_error))
        try:
            save(directory / "result.json", result)
        except OSError:
            pass  # Diagnostic failure cannot replace the actual child result.
        if primary_error is not None:
            raise primary_error
        if cleanup_error is not None:
            raise cleanup_error
    return 0


def lock_child_result(directory):
    """Reconstruct the fixed whitelist; never copy arbitrary child JSON."""
    try:
        path = directory / "result.json"
        if path.is_symlink() or path.stat().st_size > 1024:
            return {"status": "unavailable"}
        with path.open("rb") as stream:
            raw = stream.read(1025)
        if len(raw) > 1024:
            return {"status": "unavailable"}
        value = json.loads(raw)
        if (set(value) != {"phase", "ready", "release_observed", "closed", "failure", "cleanup_failure", "win32_error"}
                or value["phase"] not in _LOCK_CHILD_PHASES
                or any(type(value[k]) is not bool for k in ("ready", "release_observed", "closed"))
                or any(value[k] not in (*_LOCK_FAILURE_CODES, "unexpected-error", None)
                       for k in ("failure", "cleanup_failure"))
                or not (value["win32_error"] is None or (type(value["win32_error"]) is int
                        and 0 <= value["win32_error"] <= 0xffffffff))):
            return {"status": "unavailable"}
        return {"status": "observed", **value}
    except (OSError, ValueError, TypeError, KeyError):
        return {"status": "unavailable"}


class Replay:
    def __init__(self, args, repo, owned_module, output, receipt):
        self.args, self.repo, self.module = args, repo, owned_module
        self.output, self.receipt = output, receipt
        self.private = output / "private"
        self.private.mkdir(mode=0o700)
        self.deadline = time.monotonic() + args.timeout
        self.commands, self.http_requests = 0, 0
        self.owned = None
        self.lock = None
        self.proxy = None
        self.root = None
        self.carrier = None
        self.profiles = ("default", "b" * 32, "c" * 32)
        self.aliases = dict(zip(self.profiles, ("minicon-selected", "k3-alpha", "k3-beta")))
        self.models = dict(zip(self.profiles, ("k3-default-wire", "k3-alpha-wire", "k3-beta-wire")))

    def check(self, name, **detail):
        self.receipt["checks"][name] = {"status": "PASS", **detail}
        save(self.output / "receipt.json", self.receipt)

    def require_time(self):
        require(time.monotonic() < self.deadline, "global-deadline")

    def cli(self, *arguments, startup=False):
        self.require_time()
        self.commands += 1
        require(self.commands <= 2048, "public-command-bound")
        result = subprocess.run([str(self.args.binary), "cli", "--control", self.endpoint,
                                 *map(str, arguments)], env=self.env, capture_output=True,
                                timeout=min(15, self.deadline - time.monotonic()))
        if result.returncode:
            if startup:
                return None
            raise Failure("public-command-nonzero:" + str(arguments[0]))
        require(len(result.stdout) <= 1024 * 1024, "public-response-bound")
        return json.loads(result.stdout)

    def wait(self, predicate, code, seconds=15):
        deadline = min(self.deadline, time.monotonic() + seconds)
        while time.monotonic() < deadline:
            require(self.owned is not None and not self.owned.exited(), "owned-gui-exited")
            value = predicate()
            if value:
                return value
            time.sleep(0.06)
        raise Failure(code)

    def geometry(self):
        return self.cli("ui-snapshot")["geometry"]

    def state(self):
        s = self.geometry()["harness_settings"]
        require(isinstance(s, dict), "harness-state-unavailable")
        require(not s["login_active"] and s["connection_check_active"] is None,
                "unexpected-login-or-model-check")
        require(s["connection_check_last_terminal"] is None, "unexpected-model-check-terminal")
        if self.proxy:
            require(self.proxy.count == 0 and not self.proxy.error, "unexpected-service-network")
        return s

    def click(self, rectangle, geometry):
        frame = geometry["frame"]
        require(rectangle["width"] > 0 and rectangle["height"] > 0, "empty-public-cell")
        require(0 <= rectangle["x"] and 0 <= rectangle["y"]
                and rectangle["x"] + rectangle["width"] <= frame["width"]
                and rectangle["y"] + rectangle["height"] <= frame["height"], "public-cell-outside-frame")
        scale = geometry["scale_permille"] / 1000
        require(scale > 0, "invalid-scale")
        x = round((rectangle["x"] + rectangle["width"] / 2) / scale)
        y = round((rectangle["y"] + rectangle["height"] / 2) / scale)
        require(0 <= x <= 65535 and 0 <= y <= 65535, "public-cell-coordinate-bound")
        self.cli("send-ui-mouse", "--action", "click", "--button", "left", "--column", x, "--row", y)

    def icon(self, name):
        geometry = self.geometry()
        self.click(geometry[name], geometry)

    def row(self, name, column=0):
        ascending, visited = False, set()
        for _ in range(128):
            geometry = self.geometry()
            s = geometry["harness_settings"]
            row = next((row for row in s["rows"] if row["id"] == name), None)
            if row is not None:
                require(column < len(row["cells"]), "public-column-missing")
                self.click(row["cells"][column], geometry)
                return
            if s["scroll"] == 0:
                ascending = True
            marker = ascending, s["scroll"]
            require(marker not in visited, "public-row-unreachable:" + name)
            visited.add(marker)
            self.click(s["next"] if ascending else s["previous"], geometry)
        raise Failure("public-row-budget")

    def switch(self, profile):
        state = self.state()
        index = next(i for i, form in enumerate(state["connection_profiles"]) if form["id"] == profile)
        self.row("connection_profile_" + str(index))
        self.wait(lambda: self.state()["connection_profile"] == profile, "profile-switch-unconfirmed")

    def key_path(self, profile):
        return self.root / ("client-key" if profile == "default" else "client-key-" + profile)

    def key(self, profile):
        path = self.key_path(profile)
        require(path.is_file() and not path.is_symlink(), "fixture-key-unavailable")
        raw = path.read_bytes()
        require(0 < len(raw) <= 4096, "fixture-key-bound")
        return raw.decode("utf-8").strip()

    def key_action(self, column, expected):
        before = self.state()
        profile = before["connection_profile"]
        old_feedback = before["client_key"]["feedback"]
        old_request = old_feedback["request"] if old_feedback and old_feedback["profile"] == profile else None
        self.row("key_actions", column)
        def done():
            s = self.state()
            k = s["client_key"]
            feedback = k["feedback"]
            return s if (k["pending"] is None and feedback and feedback["kind"] != "waiting") else None
        result = self.wait(done, "key-operation-no-terminal-feedback")
        key = result["client_key"]
        require(key["profile"] == profile and key["feedback"]["profile"] == profile,
                "key-terminal-target-mismatch")
        require(key["feedback"]["kind"] == expected, "key-terminal-result-mismatch:" + expected)
        request = key["feedback"]["request"]
        if expected != "apply_first":
            require(isinstance(request, int) and (old_request is None or request > old_request),
                    "key-terminal-did-not-ack-new-request")
        return result

    def launch(self, label, applied):
        self.carrier = self.private / label
        self.carrier.mkdir(mode=0o700)
        config_dir = self.carrier / "config"
        self.root = config_dir / "harness"
        self.root.mkdir(parents=True, mode=0o700)
        self.env = os.environ.copy()
        for name in ("MINICON_LOCAL_API_KEY", "DEEPSEEK_API_KEY", "OPENAI_API_KEY", "ANTHROPIC_API_KEY",
                     "OPENCODE_API_KEY", "WEBVIEW2_USER_DATA_FOLDER", "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER",
                     "MINICON_BROWSER_HTTP_PROXY", "MINICON_BROWSER_HTTPS_PROXY", "MINICON_BROWSER_NO_PROXY"):
            self.env.pop(name, None)
        self.env["MINICON_CONFIG_DIR"] = str(config_dir)
        self.env["MINICON_BROWSER_PROFILE_ROOT"] = str(self.carrier / "web")
        self.endpoint = (rf"pipe:\\.\pipe\minicon-k3-{secrets.token_hex(8)}" if os.name == "nt"
                         else "unix:" + str(self.carrier / "control.sock"))
        require(os.name == "nt" or len(os.fsencode(self.endpoint.removeprefix("unix:"))) <= 100,
                "owned-unix-endpoint-path-too-long")
        if applied:
            self.seed_applied()
        owned_logs = self.carrier / "owned"
        owned_logs.mkdir(mode=0o700)
        shell = ["cmd.exe", "/d", "/k"] if os.name == "nt" else ["/bin/sh"]
        self.owned = self.module.OwnedProcess(
            [str(self.args.binary), "--no-activate", "--control", self.endpoint, "-e", *shell],
            self.env, owned_logs)
        self.wait(lambda: self.cli("list-tabs", startup=True), "control-unavailable", 25)
        self.icon("settings")
        self.icon("settings_harness_route")
        self.wait(lambda: (s if (s := self.state()).get("connection_profiles") and not s["busy"] else None),
                  "forms-not-loaded")

    def seed_applied(self):
        self.proxy = RejectProxy()
        with socket.socket() as available:
            available.bind(("127.0.0.1", 0))
            self.port = available.getsockname()[1]
        require(self.port >= 1024, "configured-port-invalid")
        account = "a" * 32
        proxy = {"Proxy": "http://127.0.0.1:" + str(self.proxy.port)}
        accounts = self.root / "accounts"
        credentials = accounts / "credentials"
        credentials.mkdir(parents=True, mode=0o700)
        self.module.fixture.private_json(accounts / "accounts.json", {"version": 2, "accounts": [{
            "id": account, "provider": "deepseek", "label": "K3 synthetic local-only",
            "state": "AccountReady", "preferences": {
                "context_id": account, "service_generation": account, "enabled": True,
                "login": proxy, "service": proxy}}]})
        self.module.fixture.private_json(credentials / (account + ".json"), {
            "version": 1, "provider": "deepseek", "access_token": "synthetic-k3-never-sent",
            "refresh_token": None, "expires_unix_ms": None, "organization": None})
        routes = [{"id": "k3-route-" + str(i), "model_id": self.models[profile],
                   "provider": "deepseek", "protocol": "Messages",
                   "url": "https://api.deepseek.com/anthropic/v1/messages", "headers": []}
                  for i, profile in enumerate(self.profiles)]
        config = {"version": 2, "port": self.port, "identity": secrets.token_hex(16),
                  "selected_account": account, "model": routes[0]["id"], "models": routes,
                  "profiles": [{"id": profile, "label": "K3 synthetic " + str(i), "enabled": True,
                                "default_model": self.aliases[profile], "allocations": [{
                                    "alias": self.aliases[profile], "account": account,
                                    "route": routes[i]["id"]}]}
                               for i, profile in enumerate(self.profiles) if profile != "default"]}
        self.module.fixture.private_json(self.root / "gateway.json", config)
        self.disk_baseline = {
            path: path.read_bytes() for path in (self.root / "gateway.json",
                accounts / "accounts.json", credentials / (account + ".json"))}

    def close_gui(self):
        if self.owned is None:
            return
        try:
            if not self.owned.exited():
                self.cli("close-window")
                end = min(self.deadline, time.monotonic() + 10)
                while not self.owned.exited() and time.monotonic() < end:
                    time.sleep(0.02)
                require(self.owned.exited(), "owned-close-deadline")
                require(self.owned.process.wait(timeout=2) == 0, "owned-close-nonzero")
        finally:
            self.owned.close()
            self.owned = None

    def get(self, key, path, session="k3-shared-session"):
        self.require_time()
        require(path in ("/v1/models", "/v1/minicon/binding"), "http-get-allowlist")
        self.http_requests += 1
        require(self.http_requests <= 64, "http-request-bound")
        connection = http.client.HTTPConnection("127.0.0.1", self.port,
                                                timeout=min(3, self.deadline - time.monotonic()))
        try:
            connection.request("GET", path, headers={"Authorization": "Bearer " + key,
                                                    "x-minicon-session": session})
            reply = connection.getresponse()
            raw = reply.read(65537)
            require(len(raw) <= 65536, "http-response-bound")
            require(reply.status in (200, 401), "http-status-unexpected")
            self.receipt["http_receipts"].append({"request": self.http_requests,
                                                  "path": path, "status": reply.status})
            return reply.status, json.loads(raw)
        finally:
            connection.close()

    def binding(self, profile, key):
        form = next(form for form in self.state()["connection_profiles"] if form["id"] == profile)
        applied = form["applied"]
        require(applied is not None and applied["default_alias"] == self.aliases[profile]
                and any(row["alias"] == self.aliases[profile] and row["model"] == self.models[profile]
                        for row in applied["allocations"]), "public-applied-profile-mismatch")
        status, models = self.get(key, "/v1/models")
        require(status == 200 and {row["id"] for row in models["data"]} == {self.aliases[profile]},
                "profile-model-catalogue-mismatch")
        status, binding = self.get(key, "/v1/minicon/binding")
        require(status == 200 and binding["profile"] == profile
                and binding["alias"] == self.aliases[profile]
                and binding["model"] == self.models[profile], "profile-binding-mismatch")
        return binding

    def copy(self, profile):
        self.switch(profile)
        self.key_action(1, "copied")
        value = read_clipboard()
        require(value == self.key(profile), "clipboard-key-target-mismatch")
        self.binding(profile, value)

    def recipe(self, row, running):
        profile = self.state()["connection_profile"]
        self.row(row)
        result = self.wait(lambda: (s if (s := self.state())["client_key"]["feedback"]
                                   and s["client_key"]["feedback"]["kind"] != "waiting" else None),
                           "recipe-no-terminal-feedback")
        require(result["client_key"]["feedback"]["kind"] == "recipe_copied",
                "recipe-terminal-result-mismatch")
        text = read_clipboard()
        require(profile in text and "default alias: " + self.aliases[profile] in text,
                "recipe-applied-alias-mismatch")
        require("127.0.0.1:" + str(self.port) + "/v1" in text, "recipe-bound-port-mismatch")
        require(("Listener running" if running else "Listener stopped") in text,
                "recipe-listener-state-mismatch")
        require("dirty-k3-" not in text and all(self.key(p) not in text for p in self.profiles),
                "recipe-leaked-key-or-used-dirty-draft")
        require("MINICON_LOCAL_API_KEY" in text, "recipe-private-env-missing")
        if row == "recipe_pi":
            require('"id": "' + self.aliases[profile] + '"' in text
                    and "--model '" + self.aliases[profile] + "'" in text
                    and "openai-completions" in text and "MINICON_PI_SESSION" in text,
                    "pi-recipe-alias-or-session-mismatch")
        else:
            require("<project-path>" in text and "<task>" in text
                    and "<explicit-command>" in text, "worker-recipe-authority-placeholders")

    def run(self):
        self.launch("canonical-default", False)
        self.key_action(0, "created")
        require(self.key_path("default").exists() and self.state()["listener"] == "stopped",
                "canonical-default-create-started-listener")
        self.check("canonical_default_before_model_create", listener_started=False)
        self.close_gui()
        self.launch("applied-pool", True)
        created = {}
        for profile in self.profiles:
            self.switch(profile)
            before = {p: self.key_path(p).read_bytes() for p in created}
            self.key_action(0, "created")
            require(all(self.key_path(p).read_bytes() == raw for p, raw in before.items()),
                    "create-changed-unrelated-key")
            created[profile] = self.key(profile)
        self.switch(self.profiles[1])
        # Existing Create deliberately refuses rotation, rather than succeeding
        # as an idempotent replacement. Preserve that visible policy.
        self.key_action(0, "refused")
        require(self.key(self.profiles[1]) == created[self.profiles[1]], "repeat-create-rotated-key")
        self.check("selected_default_and_two_custom_create_isolated", profiles=3)
        self.row("listener_actions", 0)
        state = self.wait(lambda: (s if (s := self.state())["listener"] == "listening" else None),
                          "listener-not-started")
        require(state["actual_port"] == self.port, "actual-port-mismatch")
        bindings = {p: self.binding(p, created[p]) for p in self.profiles}
        self.check("three_actual_key_admissions_and_same_session_profile_isolation", profiles=3,
                   synthetic_ready_injected=True, provider_calls=0, actual_port=self.port)
        a = self.profiles[1]
        self.switch(a)
        self.key_action(2, "revoked")
        require(not self.key_path(a).exists(), "revoked-key-file-still-exists")
        require(self.get(created[a], "/v1/models")[0] == 401, "revoked-key-still-admitted")
        for other in ("default", self.profiles[2]):
            require(self.key(other) == created[other] and self.binding(other, created[other]) == bindings[other],
                    "revoke-changed-unrelated-profile")
        self.key_action(2, "revoked")
        require(not self.key_path(a).exists(), "repeat-revoke-created-key")
        self.key_action(0, "created")
        require(self.key(a) != created[a], "recreated-key-reused-old-secret")
        require(self.get(created[a], "/v1/models")[0] == 401, "recreated-key-resurrected-old-secret")
        self.binding(a, self.key(a))
        self.check("target_revoke_recreate_preserves_unrelated_bindings", old_key_status=401)
        unaffected = {p: self.binding(p, self.key(p)) for p in self.profiles[1:]}
        old_default = self.key("default")
        self.switch("default")
        self.key_action(2, "revoked")
        require(not self.key_path("default").exists()
                and self.get(old_default, "/v1/models")[0] == 401, "default-revoke-not-admitted")
        for other in self.profiles[1:]:
            require(self.binding(other, self.key(other)) == unaffected[other],
                    "default-revoke-changed-custom-binding")
        self.key_action(0, "created")
        require(self.key("default") != old_default
                and self.get(old_default, "/v1/models")[0] == 401, "default-recreate-reused-key")
        self.binding("default", self.key("default"))
        self.check("default_revoke_preserves_both_custom_profile_bindings", old_key_status=401)
        self.row("connection_profile_create")
        draft = self.wait(lambda: (p if (p := self.state()["connection_profile"]) not in self.profiles else None),
                          "new-draft-not-selected")
        self.key_action(0, "apply_first")
        self.key_action(2, "apply_first")
        require(not self.key_path(draft).exists(), "draft-created-orphan-key")
        self.check("process_only_draft_refuses_orphan_key")
        if self.args.isolated_ci_clipboard:
            self.clipboard_checks()
        self.switch(a)
        if self.state()["listener"] != "stopped":
            self.row("listener_actions", 1)
            self.wait(lambda: self.state()["listener"] == "stopped", "listener-not-stopped")
        require(self.proxy.count == 0, "provider-proxy-received-request")
        require(all(path.read_bytes() == raw for path, raw in self.disk_baseline.items()),
                "key-or-draft-operations-changed-applied-account-state")
        self.check("all_service_network_suppressed", rejecting_proxy_requests=0, http_posts=0)

    def clipboard_checks(self):
        for profile in self.profiles:
            self.copy(profile)
        self.check("actual_os_copy_targets_three_profiles", profiles=3)
        a, b = self.profiles[1:]
        self.switch(a)
        self.row("model_input")
        self.cli("send-ui-keys", "Ctrl+A")
        self.cli("send-ui-ime", "commit", "dirty-k3-model")
        self.row("connection_alias_0")
        self.cli("send-ui-keys", "Ctrl+A")
        self.cli("send-ui-ime", "commit", "dirty-k3-alias")
        def dirty_draft():
            state = self.state()
            form = next(form for form in state["connection_profiles"] if form["id"] == a)
            model_matches = state["model_editor"]["text"] == "dirty-k3-model"
            alias_matches = any(row["alias"] == "dirty-k3-alias" for row in form["allocations"])
            # Preserve finite public failure context without exporting editor text.
            self.receipt["dirty_draft_observation"] = {
                "selected_profile_matches": state["connection_profile"] == a,
                "model_matches": model_matches, "alias_matches": alias_matches,
                "model_focused": state["model_editor"]["focused"],
            }
            return state if model_matches and alias_matches else None
        self.wait(dirty_draft, "public-dirty-model-alias-not-present")
        self.recipe("recipe", True)
        self.recipe("recipe_pi", True)
        require(bool(dirty_draft()), "recipe-changed-dirty-draft")
        self.check("actual_recipe_sink_uses_applied_alias_and_bound_port", actual_port=self.port,
                   external_client_executed=False, dirty_drafts_applied=False)
        self.row("listener_actions", 1)
        self.wait(lambda: self.state()["listener"] == "stopped", "recipe-stop-not-observed")
        self.recipe("recipe", False)
        require(self.state()["listener"] == "stopped", "recipe-implicitly-started-listener")
        self.check("stopped_recipe_is_configured_only", port_zero_supported=False)
        self.row("listener_actions", 0)
        self.wait(lambda: self.state()["listener"] == "listening", "clipboard-restart-failed")
        directory = self.private / "clipboard-lock"
        directory.mkdir(mode=0o700)
        authority = secrets.token_hex(16)
        save(directory / "authority.json", {"authority": authority})
        logs = directory / "owned"
        logs.mkdir(mode=0o700)
        self.lock = self.module.OwnedProcess([sys.executable, str(Path(__file__).resolve()),
            "--clipboard-lock-child", str(directory), authority], self.env, logs)
        primary_error = cleanup_error = None
        observation = {"phase": "wait-ready", "ready": False, "release_requested": False}
        try:
            self.wait(lambda: (directory / "ready.json").exists() and not self.lock.exited(),
                      "clipboard-lock-not-ready", 5)
            observation.update(phase="copy-under-contention", ready=True)
            self.key_action(1, "clipboard_failed")
            require(not self.lock.exited(), "clipboard-lock-ended-before-adapter-failure")
        except BaseException as error:
            primary_error = error
            observation["primary_phase"] = observation["phase"]
        finally:
            try:
                (directory / "release").touch()
                observation.update(phase="release-wait", release_requested=True)
                end = min(self.deadline, time.monotonic() + 3)
                while not self.lock.exited() and time.monotonic() < end:
                    time.sleep(0.01)
                require(self.lock.exited() and self.lock.process.returncode == 0, "clipboard-lock-cleanup")
            except BaseException as error:
                cleanup_error = error
            observation.update(exited_before_close=self.lock.exited(),
                               exit_code_before_close=self.lock.process.returncode)
            try:
                self.lock.close()
            except BaseException as error:
                if cleanup_error is None:
                    cleanup_error = error
            else:
                self.lock = None
            observation.update(child=lock_child_result(directory),
                               primary_failure=lock_failure_code(primary_error),
                               cleanup_failure=lock_failure_code(cleanup_error))
            self.receipt["clipboard_lock_observation"] = observation
            if primary_error is not None:
                raise primary_error
            if cleanup_error is not None:
                raise cleanup_error
        for _ in range(3):
            require(self.state()["client_key"]["feedback"]["kind"] == "clipboard_failed",
                    "clipboard-failure-replayed-or-relabeled")
        self.copy(a)
        self.check("real_adapter_failure_requires_new_explicit_copy", adapter_timeout_ms=500,
                   failure_scope="Windows OpenClipboard contention")
        self.row("key_actions", 1)
        self.switch(b)
        self.switch(a)
        self.copy(a)
        self.row("key_actions", 1)
        self.icon("settings")
        self.icon("settings")
        self.icon("settings_harness_route")
        self.wait(lambda: isinstance(self.state(), dict), "settings-reopen-unavailable")
        self.copy(a)
        self.check("observed_public_selection_aba_and_settings_close_reopen", pending_copy_proven=False,
                   isolated_main_hook_counterproof=False, hardware_input=False)

    def close(self):
        failures = []
        if self.lock:
            try:
                self.lock.close()
            except BaseException:
                failures.append("owned-lock-cleanup")
            self.lock = None
        try:
            self.close_gui()
        except BaseException:
            failures.append("owned-gui-cleanup")
        if self.proxy:
            self.receipt["rejecting_proxy_requests"] = self.proxy.count
            try:
                self.proxy.close()
            except BaseException:
                failures.append("proxy-cleanup")
        self.receipt["cleanup_errors"] = failures
        return not failures


def main():
    if len(sys.argv) == 4 and sys.argv[1] == "--clipboard-lock-child":
        return clipboard_lock_child(Path(sys.argv[2]).resolve(strict=True), sys.argv[3])
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--repo", type=Path, required=True)
    parser.add_argument("--binary", type=Path, required=True)
    parser.add_argument("--binary-sha256", required=True)
    parser.add_argument("--output", type=Path, required=True)
    parser.add_argument("--timeout", type=float, default=360)
    parser.add_argument("--execute", action="store_true")
    parser.add_argument("--allow-owned-background-gui", action="store_true")
    parser.add_argument("--isolated-ci-clipboard", action="store_true")
    args = parser.parse_args()
    require(0 < args.timeout <= 600, "invalid-timeout")
    args.binary = args.binary.resolve(strict=True)
    require(sha(args.binary) == args.binary_sha256, "binary-pin-mismatch")
    repo = args.repo.resolve(strict=True)
    output = args.output.resolve()
    require(output.is_relative_to(repo) and "probe-logs" not in output.relative_to(repo).parts,
            "private-carrier-output-must-be-separate-from-broad-log-upload")
    ignored = subprocess.run(["git", "check-ignore", "--quiet", str(output)], cwd=repo)
    require(ignored.returncode == 0, "private-carrier-output-must-be-ignored")
    output.mkdir(parents=True, exist_ok=False)
    paths = ["src/main.rs", "src/control_dispatch.rs", "src/harness_settings_ui.rs",
             "src/harness_settings_ui/profile_keys.rs", "src/provider_runtime/profile_keys.rs",
             "src/provider_runtime.rs", "src/provider_server.rs", "src/provider_accounts.rs",
             "src/harness_settings_ui/connection_forms.rs", "src/harness_gateway.rs",
             "src/clipboard_status.rs", "crates/minicon-platform/src/clipboard.rs",
             "crates/minicon-platform/src/adapters/windows/clipboard.rs",
             "scripts/native-auth-context/run.py", "scripts/native-auth-context/fixture.py"]
    receipt = {"schema": 1, "status": "PREPARED_NOT_EXECUTED", "checks": {}, "http_receipts": [],
               "script_sha256": sha(Path(__file__)), "binary_sha256": args.binary_sha256,
               "source_sha256": {p: sha(repo / p) for p in paths},
               "source_head": subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo,
                    capture_output=True, text=True, check=True).stdout.strip(),
               "tracked_status_before": subprocess.run(
                    ["git", "status", "--porcelain", "--untracked-files=no"], cwd=repo,
                    capture_output=True, text=True, check=True).stdout,
               "scope": "Public CLI logical GUI and synthetic local GETs only; injected ready state is not provider authorization.",
               "clipboard_mode": args.isolated_ci_clipboard, "isolated_main_hook_counterproof": False,
               "public_upload_allowlist": ["receipt.json"],
               "private_carrier_upload_forbidden": True, "provider_posts": 0}
    save(output / "receipt.json", receipt)
    if not args.execute:
        return 0
    require(args.allow_owned_background_gui, "owned-gui-execution-authority-required")
    if args.isolated_ci_clipboard:
        require(os.name == "nt" and os.environ.get("GITHUB_ACTIONS") == "true",
                "isolated-windows-ci-clipboard-only")
    # Same tracked ownership seam used by CS04/CS05; never require their ignored
    # local scripts to exist in a clean CI checkout.
    directory = repo / "scripts/native-auth-context"
    sys.path.insert(0, str(directory))
    try:
        module = load(directory / "run.py", "k3_existing_owned_process")
    finally:
        sys.path.pop(0)
    replay = Replay(args, repo, module, output, receipt)
    success = False
    try:
        replay.run()
        require(sha(args.binary) == args.binary_sha256, "binary-pin-changed")
        require(all(sha(repo / p) == h for p, h in receipt["source_sha256"].items()), "source-changed")
        require(subprocess.run(["git", "rev-parse", "HEAD"], cwd=repo, capture_output=True,
                               text=True, check=True).stdout.strip() == receipt["source_head"],
                "source-head-changed")
        receipt["tracked_status_after"] = subprocess.run(
            ["git", "status", "--porcelain", "--untracked-files=no"], cwd=repo,
            capture_output=True, text=True, check=True).stdout
        require(receipt["tracked_status_after"] == receipt["tracked_status_before"],
                "source-index-worktree-status-changed")
        success = True
    except BaseException as error:
        receipt["failure"] = str(error) if isinstance(error, Failure) else type(error).__name__
    finally:
        success = replay.close() and success
        receipt["status"] = "PASS" if success else "FAIL"
        receipt["actual_public_checks"] = len(receipt["checks"])
        receipt["public_commands"] = replay.commands
        receipt["local_get_requests"] = replay.http_requests
        save(output / "receipt.json", receipt)
    return 0 if success else 1


if __name__ == "__main__":
    raise SystemExit(main())
