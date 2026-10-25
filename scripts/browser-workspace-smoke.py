#!/usr/bin/env python3
"""Public CLI/native page journey for exact desktop artifacts. Owner: PRD 25/27."""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import hashlib
import http.server
import json
import math
import os
import secrets
import zlib
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time


def host_png_samples(path: Path, points: list[tuple[int, int]]) -> dict:
    """Bounded RGB8/RGBA8 host-frame PNG observation, not compositor pixels."""
    with path.open("rb") as stream:
        raw = stream.read(16 * 1024 * 1024 + 1)
    assert len(raw) <= 16 * 1024 * 1024 and raw[:8] == b"\x89PNG\r\n\x1a\n", "host PNG unavailable or oversized"
    offset, header, compressed, ended = 8, None, bytearray(), False
    while offset < len(raw):
        assert offset + 12 <= len(raw), "truncated host PNG chunk"
        size = int.from_bytes(raw[offset:offset + 4], "big")
        kind = raw[offset + 4:offset + 8]
        assert offset + 12 + size <= len(raw), "truncated host PNG body"
        body = raw[offset + 8:offset + 8 + size]
        assert zlib.crc32(kind + body) & 0xffffffff == int.from_bytes(raw[offset + 8 + size:offset + 12 + size], "big"), "host PNG CRC mismatch"
        if kind == b"IHDR":
            assert header is None and size == 13, "invalid host PNG header"
            width, height = int.from_bytes(body[:4], "big"), int.from_bytes(body[4:8], "big")
            assert body[8] == 8 and body[9] in (2, 6) and body[10:] == b"\0\0\0", "unsupported host PNG format"
            header = (width, height, 3 if body[9] == 2 else 4)
        elif kind == b"IDAT":
            assert header is not None, "host PNG data before header"
            compressed.extend(body)
        elif kind == b"IEND":
            assert size == 0, "invalid host PNG end"
            ended = True
            offset += 12
            break
        offset += 12 + size
    assert ended and offset == len(raw) and header is not None, "incomplete host PNG"
    width, height, channels = header
    assert 0 < width <= 16384 and 0 < height <= 16384
    assert points and all(0 <= x < width and 0 <= y < height for x, y in points), "host PNG sample outside frame"
    stride = width * channels
    expected = (stride + 1) * height
    assert expected <= 64 * 1024 * 1024, "host PNG decoded bound"
    decoder = zlib.decompressobj()
    filtered = decoder.decompress(compressed, expected + 1)
    assert len(filtered) == expected and decoder.eof and not decoder.unused_data and not decoder.unconsumed_tail, "invalid bounded host PNG stream"
    previous, samples = bytearray(stride), {}
    for y in range(max(y for _, y in points) + 1):
        start = y * (stride + 1)
        mode = filtered[start]
        assert mode <= 4, "invalid host PNG row filter"
        row = bytearray(filtered[start + 1:start + 1 + stride])
        for i in range(stride):
            a, b = (row[i - channels] if i >= channels else 0), previous[i]
            c = previous[i - channels] if i >= channels else 0
            if mode == 1:
                predictor = a
            elif mode == 2:
                predictor = b
            elif mode == 3:
                predictor = (a + b) // 2
            elif mode == 4:
                p = a + b - c
                pa, pb, pc = abs(p - a), abs(p - b), abs(p - c)
                predictor = a if pa <= pb and pa <= pc else b if pb <= pc else c
            else:
                predictor = 0
            row[i] = (row[i] + predictor) & 255
        for x, sample_y in points:
            if sample_y == y:
                samples[f"{x},{y}"] = list(row[x * channels:x * channels + 3])
        previous = row
    return {"width": width, "height": height, "samples_rgb": samples}


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--timeout", type=float, default=45, help="navigation observation deadline in seconds (0 < timeout <= 120)")
    parser.add_argument("--physical", action="store_true", help="Windows interactive focus/shortcut and page capture court")
    parser.add_argument("--mac-window-shortcuts-only", action="store_true",
                        help="run only the owned single-display macOS CLI shortcut phase")
    args = parser.parse_args()
    if args.mac_window_shortcuts_only and (sys.platform != "darwin" or args.physical):
        parser.error("--mac-window-shortcuts-only requires nonphysical macOS")
    if not 0 < args.timeout <= 120:
        parser.error("--timeout must be greater than zero and at most 120")
    binary = args.binary.resolve(strict=True)
    output = (args.output or binary.parent / "browser-workspace-court").resolve()
    output.mkdir(parents=True, exist_ok=True)
    binary_sha256 = hashlib.sha256(binary.read_bytes()).hexdigest()
    script_sha256 = hashlib.sha256(Path(__file__).read_bytes()).hexdigest()
    counts: dict[str, int] = {}
    proxy_requests: list[str] = []
    proxy_target = "http://preview.minicon.invalid/page"

    h0_run = secrets.token_hex(16)
    h0_path = f"/h0-live/{h0_run}"
    h0_command_path = f"/h0-command/{h0_run}"
    h0_report_path = f"/h0-report/{h0_run}"
    h0_lock = threading.Lock()
    h0_live = {"challenge": secrets.token_hex(16), "report": None, "reports": 0}
    # Only this fixture-owned page uses the bounded same-origin HTTP report.
    # No cookie/storage reads, eval, title transport, or product page IPC.
    h0_html = """<!doctype html><title>Harness live fixture</title>
<input id="unsaved"><script>
(() => {
  const run = __RUN__;
  const nonce = Array.from(crypto.getRandomValues(new Uint8Array(16)), x => x.toString(16).padStart(2, '0')).join('');
  const input = document.getElementById('unsaved');
  input.value = 'unsaved:' + nonce;
  history.replaceState({nonce, step: 1}, '', location.href);
  history.pushState({nonce, step: 2}, '', location.href);
  const started = Date.now();
  let count = 0;
  async function poll() {
    if (++count > 512 || Date.now() - started > 180000) return;
    try {
      const command = await fetch('/h0-command/' + run, {cache: 'no-store', credentials: 'omit'});
      if (!command.ok) return;
      const {challenge} = await command.json();
      const reply = await fetch('/h0-report/' + run, {
        method: 'POST', credentials: 'omit', cache: 'no-store',
        headers: {'Content-Type': 'application/json'},
        body: JSON.stringify({nonce, challenge, draft: input.value, history_length: history.length,
                              history_nonce: history.state && history.state.nonce,
                              history_step: history.state && history.state.step})
      });
      if (!reply.ok) return;
    } catch (_) { return; }
    setTimeout(poll, 200);
  }
  void poll();
})();
</script>""".replace("__RUN__", json.dumps(h0_run))

    class Page(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args: object) -> None:
            pass

        def h0_response(self, body: dict, status: int = 200):
            raw = json.dumps(body).encode()
            self.send_response(status)
            self.send_header("Content-Type", "application/json")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(raw)))
            self.end_headers()
            self.wfile.write(raw)

        def do_POST(self) -> None:  # noqa: N802
            if self.path != h0_report_path or self.headers.get("Transfer-Encoding") is not None:
                self.h0_response({}, 400)
                return
            try:
                size = int(self.headers.get("Content-Length", "0"))
                assert 0 < size <= 2048
                self.connection.settimeout(2)
                data = json.loads(self.rfile.read(size))
                assert set(data) == {"nonce", "challenge", "draft", "history_length", "history_nonce", "history_step"}
                for key in ("nonce", "challenge", "history_nonce"):
                    assert isinstance(data[key], str) and len(data[key]) == 32 and all(c in "0123456789abcdef" for c in data[key])
                assert isinstance(data["draft"], str) and len(data["draft"]) <= 64
                assert type(data["history_length"]) is int and 1 <= data["history_length"] <= 1024
                assert type(data["history_step"]) is int and data["history_step"] in (1, 2)
                with h0_lock:
                    assert h0_live["reports"] < 1024
                    h0_live["reports"] += 1
                    h0_live["report"] = data
            except (AssertionError, OSError, ValueError, TypeError):
                self.h0_response({}, 400)
                return
            self.h0_response({})

        def do_GET(self) -> None:  # noqa: N802
            counts[self.path] = counts.get(self.path, 0) + 1
            if self.path == h0_command_path:
                with h0_lock:
                    challenge = h0_live["challenge"]
                self.h0_response({"challenge": challenge})
                return
            if self.path == h0_path:
                self.send_response(200)
                body = h0_html.encode()
            elif self.path == "/redirect":
                self.send_response(302)
                self.send_header("Location", "/second")
                body = b""
            else:
                self.send_response(200)
                title = "Second page" if self.path == "/second" else "First page"
                body = (f"<!doctype html><title>{title}</title><style>body{{font:18px system-ui;padding:32px}}"
                        f"input{{font:inherit;padding:8px}}</style><h1>{title}</h1>"
                        "<p>A local page inside MiniCon.</p><input placeholder='Click to focus the page'>"
                        "<p><a href='/redirect'>Open the second page</a></p>").encode()
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Cache-Control", "no-store")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    class ProxyPage(http.server.BaseHTTPRequestHandler):
        protocol_version = "HTTP/1.1"

        def log_message(self, *_args: object) -> None:
            pass

        def page_response(self, target: str) -> None:
            # This is a loopback-only responder, never a forwarding proxy.
            assert target.startswith("http://preview.minicon.invalid/"), "proxy fixture received an unexpected target"
            proxy_requests.append(target)
            body = b"<!doctype html><title>Proxy preview</title><h1>Proxy preview</h1>"
            self.wfile.write(("HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\n"
                              "Cache-Control: no-store\r\nConnection: close\r\n"
                              f"Content-Length: {len(body)}\r\n\r\n").encode() + body)
            self.close_connection = True

        def do_GET(self) -> None:  # noqa: N802
            if not self.path.startswith("http://preview.minicon.invalid/"):
                self.send_error(400, "Expected the synthetic proxy target")
                return
            self.page_response(self.path)

        def do_CONNECT(self) -> None:  # noqa: N802
            # Apple's network proxy API uses CONNECT even for an http page.
            if self.path not in ("preview.minicon.invalid", "preview.minicon.invalid:80"):
                self.send_error(400, "Expected the synthetic CONNECT authority")
                return
            self.connection.settimeout(5)
            self.wfile.write(b"HTTP/1.1 200 Connection Established\r\n\r\n")
            self.wfile.flush()
            request = self.rfile.readline(8193)
            words = request.split()
            if len(words) != 3 or words[0] != b"GET" or len(request) > 8192:
                return
            headers = 0
            while headers <= 8192:
                line = self.rfile.readline(8193)
                headers += len(line)
                if line == b"\r\n":
                    break
                if not line:
                    return
            if headers > 8192:
                return
            path = words[1].decode("ascii")
            if path.startswith("/"):
                path = "http://preview.minicon.invalid" + path
            self.page_response(path)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Page)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    recovery_server = None
    recovery_worker = None
    proxy_server = None
    proxy_worker = None
    origin = f"http://127.0.0.1:{server.server_port}"
    windows = sys.platform == "win32"
    scratch = tempfile.TemporaryDirectory(prefix="minicon-browser-")
    scratch_root = Path(scratch.name).resolve(strict=True)
    endpoint = rf"pipe:\\.\pipe\minicon-browser-{os.getpid()}" if windows else f"unix:{scratch_root / 'control.sock'}"
    env = os.environ.copy()
    # Reuse this isolated config only across the process restarts in this run.
    env["MINICON_CONFIG_DIR"] = str(scratch_root / "config")
    env["MINICON_BROWSER_PROFILE_ROOT"] = str(scratch_root / "profile")
    # WebView2's global folder override would force different proxy environments
    # back into one incompatible directory, overriding the native API argument.
    for key in ("WEBVIEW2_USER_DATA_FOLDER", "WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "MINICON_BROWSER_HTTP_PROXY", "MINICON_BROWSER_HTTPS_PROXY",
                "MINICON_BROWSER_NO_PROXY", "MINICON_BROWSER_BIN"):
        env.pop(key, None)
    shell = ["cmd.exe", "/d", "/k"] if windows else ["/bin/sh"]
    child = None
    native = None
    foreground = None
    pointer = None
    mac_window_id = None
    mac_capture_error = None
    screenshots: dict[str, dict] = {}
    checks: dict[str, dict] = {}

    def cli(*arguments: str) -> dict:
        response = subprocess.check_output([str(binary), "cli", "--control", endpoint, *arguments],
                                          text=True, encoding="utf-8", timeout=20)
        return json.loads(response)

    def browser() -> dict:
        return next(tab for tab in cli("list-tabs")["tabs"] if tab["kind"] == "browser")

    def wait(predicate, label: str, timeout: float | None = None):
        timeout = args.timeout if timeout is None else timeout
        deadline = time.monotonic() + timeout
        last = None
        # Stay within the bounded replay cache even when a native page fails;
        # each observation is a fresh public control request.
        while time.monotonic() < deadline:
            assert child is not None and child.poll() is None, "GUI exited before the journey completed"
            last = predicate()
            if last:
                return last
            time.sleep(0.2)
        raise AssertionError(f"timed out: {label}; last={last}; tabs={cli('list-tabs')}")

    def loaded(path: str, title: str):
        def observed():
            row = browser()["browser"]
            return row if row["url"] == origin + path and row["load"] == "ready" and row["title"] == title else None
        return wait(observed, f"ready {path} / {title}")

    def navigate(url: str):
        cli("send-ui-keys", "Ctrl+l")
        cli("send-ui-ime", "commit", url)
        cli("send-ui-keys", "Enter")

    def click_toolbar(index: int):
        geometry = cli("ui-snapshot")["geometry"]
        scale = geometry["scale_permille"] / 1000
        x = geometry["browser_toolbar"]["x"] / scale + index * 34 + 16
        cli("send-ui-mouse", "--action", "click", "--button", "left", "--column", str(round(x)), "--row", "20")

    def click_geometry(name: str, index: int | None = None):
        geometry = cli("ui-snapshot")["geometry"]
        rectangle = geometry[name] if index is None else geometry[name][index]
        click_rectangle(rectangle, geometry, name)

    def click_rectangle(rectangle: dict, geometry: dict, name: str):
        assert rectangle["width"] > 0 and rectangle["height"] > 0, f"{name} has no visible hit target"
        scale = geometry["scale_permille"] / 1000
        x = (rectangle["x"] + rectangle["width"] / 2) / scale
        y = (rectangle["y"] + rectangle["height"] / 2) / scale
        cli("send-ui-mouse", "--action", "click", "--button", "left", "--column", str(round(x)), "--row", str(round(y)))

    def keys(*chords: str):
        # Each complete production chord stays one argv element, including
        # Shift+Tab and Ctrl+Shift+[. This is public injection, not native input.
        assert cli("send-ui-keys", *chords)["sent_keys"] == len(chords)

    def editor(name: str) -> dict:
        return cli("ui-snapshot")["geometry"][name]

    def mac_window_shortcuts_journey():
        """Exact owned PID, single display, logical public CLI input only."""
        assert sys.platform == "darwin" and not args.physical

        def blocked(reason):
            (output / "mac-window-half-blocked.json").write_bytes((json.dumps({
                "status": "BLOCKED", "reason": reason}) + "\n").encode())
            raise RuntimeError("BLOCKED: " + reason)

        def observe():
            probe = subprocess.run(["/usr/bin/swift", "-", str(child.pid)], input='''
import AppKit
import CoreGraphics
import Foundation
let pid = Int32(CommandLine.arguments[1]) ?? -1
func rectangle(_ r: CGRect) -> [Double] {
    return [Double(r.minX), Double(r.minY), Double(r.width), Double(r.height)]
}
var result: [String: Any] = ["status": "BLOCKED", "reason": "single-display-owned-window-unavailable"]
let screens = NSScreen.screens
if screens.count == 1, let screen = screens.first,
   let number = screen.deviceDescription[NSDeviceDescriptionKey("NSScreenNumber")] as? NSNumber,
   number.uint32Value == CGMainDisplayID(),
   let rows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] {
    let owned = rows.filter {
        ($0[kCGWindowOwnerPID as String] as? Int32) == pid &&
        ($0[kCGWindowLayer as String] as? Int) == 0
    }
    if owned.count == 1, let row = owned.first,
       let number = row[kCGWindowNumber as String] as? Int,
       let bounds = row[kCGWindowBounds as String] as? [String: Any],
       let frame = CGRect(dictionaryRepresentation: bounds as CFDictionary) {
        result = ["status": "observed", "pid": pid, "window_id": number,
                  "display_count": 1, "main_display_matches": true,
                  "screen_frame_appkit": rectangle(screen.frame),
                  "visible_frame_appkit": rectangle(screen.visibleFrame),
                  "window_frame_cg": rectangle(frame)]
    }
}
let raw = try JSONSerialization.data(withJSONObject: result, options: [.sortedKeys])
print(String(decoding: raw, as: UTF8.self))
''', text=True, capture_output=True, timeout=30)
            if probe.returncode != 0:
                blocked("owned-window-metadata-helper-unavailable")
            value = json.loads(probe.stdout)
            if value.get("status") != "observed":
                blocked("single-display-unique-owned-window-required")
            assert value["pid"] == child.pid and value["display_count"] == 1 and value["main_display_matches"] is True
            for field in ("screen_frame_appkit", "visible_frame_appkit", "window_frame_cg"):
                r = value[field]
                assert len(r) == 4 and all(type(x) in (int, float) and math.isfinite(x) for x in r)
                assert r[2] > 0 and r[3] > 0
            if value["screen_frame_appkit"][:2] != [0, 0]:
                blocked("single-main-display-origin-unavailable")
            return value

        def close_frame(a, b):
            return len(a) == len(b) == 4 and all(abs(x-y) <= 1 for x, y in zip(a, b))

        def report_document():
            with h0_lock:
                h0_live["challenge"] = challenge = secrets.token_hex(16)
            def received():
                with h0_lock:
                    report = h0_live["report"]
                    return dict(report) if report is not None and report["challenge"] == challenge else None
            return wait(received, "owned shortcut live document")

        def retained_state():
            snap = cli("ui-snapshot")
            address = snap["geometry"]["browser_address_editor"]
            return {"tabs": [{key: row[key] for key in ("id", "kind", "active")} for row in cli("list-tabs")["tabs"]],
                    "active": snap["active"], "composer_text": snap["composer_text"],
                    "address": {key: address[key] for key in ("text", "caret", "selection", "focused", "preedit")},
                    "browser": {key: browser()["browser"][key] for key in ("url", "title", "load")}}

        terminal_target = next(row["id"] for row in cli("list-tabs")["tabs"]
                               if row["kind"] == "terminal")
        keys("Ctrl+Shift+I")
        cli("send-ui-ime", "commit", "owned Mac composer draft")
        open_browser_from_menu()
        with h0_lock:
            h0_live.update(report=None, reports=0)
        navigate(origin + h0_path)
        loaded(h0_path, "Harness live fixture")
        document = report_document()
        assert document["draft"] == "unsaved:" + document["nonce"] and document["history_step"] == 2
        gets = counts[h0_path]
        keys("Ctrl+l", "Ctrl+a")
        cli("send-ui-ime", "commit", "unsubmitted:a中z")
        keys("Home", "Right", "Shift+Right")
        before = retained_state()
        assert before["address"]["focused"] and before["address"]["selection"] is not None
        initial = observe()
        screen = initial["screen_frame_appkit"]
        visible = initial["visible_frame_appkit"]
        # AppKit bottom-left -> CG main-display top-left, in points once.
        left = [visible[0] - screen[0], screen[1] + screen[3] - visible[1] - visible[3], visible[2] / 2, visible[3]]
        right = [left[0] + left[2], left[1], left[2], left[3]]
        if close_frame(initial["window_frame_cg"], left):
            blocked("initial-owned-window-already-left-half")
        observations = {"status": "in_progress", "initial": initial, "samples": [],
                        "input": "public CLI logical Pressed/Released", "physical_keyboard": False,
                        "wk_focus_chain": False, "clipboard": False, "tolerance_points": 1}
        def save_observations():
            (output / "mac-window-half-observations.json").write_bytes((json.dumps(observations, indent=2) + "\n").encode())
        save_observations()
        refusals = []
        for label, arguments in (("ui-invalid-suffix", ["send-ui-keys", "Cmd+Left", "UnknownModifier+x"]),
                                 ("pty-meta-refused", ["send-keys", "--target", terminal_target, "Cmd+Left"])):
            refused = subprocess.run([str(binary), "cli", "--control", endpoint, *arguments],
                                     text=True, capture_output=True, timeout=20)
            # These fixed invalid-key commands contain no private input. Keep
            # bounded raw refusal evidence before the unchanged assertion.
            refusals.append({"phase": label, "arguments": arguments,
                             "exit_code": refused.returncode,
                             "stdout": refused.stdout[:4096], "stderr": refused.stderr[:4096]})
            (output / "mac-window-half-refusals.json").write_bytes(
                (json.dumps(refusals, indent=2) + "\n").encode())
            assert refused.returncode != 0 and "unknown key modifier" in (refused.stdout + refused.stderr), label
            observed = observe()
            assert close_frame(observed["window_frame_cg"], initial["window_frame_cg"]), label + " moved window"
            assert retained_state() == before, label + " changed retained state"
        observations["invalid_suffix_atomic"] = True
        observations["pty_meta_refused"] = True
        for stage, chord, expected in (("left", "Cmd+Left", left), ("right", "Command+Right", right),
                                       ("repeat-right", "Meta+Right", right)):
            keys(chord)
            actual_observation = observe()
            assert actual_observation["window_id"] == initial["window_id"]
            assert actual_observation["screen_frame_appkit"] == screen and actual_observation["visible_frame_appkit"] == visible
            actual = actual_observation["window_frame_cg"]
            observations["samples"].append({"stage": stage, "chord": chord, "actual": actual, "expected": expected})
            save_observations()
            assert close_frame(actual, expected), f"mac-window-half geometry mismatch at {stage}: actual={actual}, expected={expected}"
            assert retained_state() == before, f"{stage}: window shortcut changed drafts or tab state"
            current_document = report_document()
            for field in ("nonce", "draft", "history_length", "history_nonce", "history_step"):
                assert current_document[field] == document[field], f"{stage}: live document changed {field}"
            assert counts[h0_path] == gets, "window shortcut reloaded the native page"
        observations["status"] = "passed"
        observations["drafts_and_live_document_preserved"] = True
        save_observations()
        checks["mac_window_half_public_host_route"] = observations

    def host_controls_journey():
        """Public host checks mirrored from tests/support/browser_windows.rs.

        Run after native navigation using a fresh profile; no provider login,
        clipboard, physical key, or native IME evidence is claimed here.
        """
        keys("Ctrl+Shift+I")
        draft = "terminal draft survives Harness"
        cli("send-ui-ime", "commit", draft)
        assert cli("ui-snapshot")["composer_text"] == draft
        assert cli("ui-snapshot")["geometry"]["harness_settings"] is None, "Harness runtime was not lazy"
        browser_id = open_browser_from_menu()["id"]

        keys("Ctrl+l", "Shift+Tab", "Enter")
        geometry = cli("ui-snapshot")["geometry"]
        assert geometry["new_tab_menu_open"] and geometry["new_menu_highlight"] == 0
        assert len(cli("list-tabs")["tabs"]) == 2, "opening New activated its default terminal action"
        keys("Down")
        assert cli("ui-snapshot")["geometry"]["new_menu_highlight"] == 1
        keys("Enter")
        assert browser()["id"] == browser_id and len(cli("list-tabs")["tabs"]) == 2, "Browser menu activation did not reuse its root exactly once"
        assert not cli("ui-snapshot")["geometry"]["new_tab_menu_open"]
        keys("Ctrl+l", "Tab")
        assert editor("browser_proxy_editor")["focused"]
        keys("Tab", "Space")
        geometry = cli("ui-snapshot")["geometry"]
        assert geometry["browser_proxy_menu_open"] and geometry["proxy_menu_highlight"] == 0
        assert len(geometry["browser_proxy_option_rows"]) == 1
        keys("Down")
        assert cli("ui-snapshot")["geometry"]["proxy_menu_highlight"] == 0
        keys("Escape")
        assert not cli("ui-snapshot")["geometry"]["browser_proxy_menu_open"]
        keys("Shift+Tab")
        assert editor("browser_proxy_editor")["focused"], "menu dismissal did not restore its traversal owner"
        keys("Shift+Tab")
        assert editor("browser_address_editor")["focused"]
        checks["menu_keyboard"] = {"status": "passed", "new_open_without_activation": True,
                                   "browser_single_activation": True, "proxy_clamp": True,
                                   "tab_and_shift_tab_ownership": True}

        for trigger in ("browser_proxy_dropdown", "new_root"):
            keys("Ctrl+l")
            cli("send-ui-ime", "commit", "a中z")
            keys("Home", "Right", "Shift+Right")
            original = editor("browser_address_editor")
            assert original["selection"] == [1, 4]
            cli("send-ui-ime", "preedit", "ni")
            click_geometry(trigger)
            assert cli("ui-snapshot")["geometry"]["menu_return_focus"] == "address"
            assert not editor("browser_address_editor")["focused"]
            keys("Escape")
            restored = editor("browser_address_editor")
            for field in ("text", "caret", "selection"):
                assert restored[field] == original[field], f"{trigger} changed retained {field}"
            assert restored["focused"] and restored["preedit"] == ""
            cli("send-ui-ime", "commit", "Q")
            assert editor("browser_address_editor")["text"] == "aQz"
            checks[f"menu_dismiss_selection:{trigger}"] = {"status": "passed", "utf8_selection": [1, 4], "replacement": "aQz"}

        for name, target, limit in (("browser_address_editor", "browser_address", 8192),
                                    ("browser_proxy_editor", "browser_proxy", 2048),
                                    ("web_proxy_candidate_editor", "web_proxy_input", 2048)):
            if name == "web_proxy_candidate_editor":
                open_web_settings()
            click_geometry(target)
            keys("Ctrl+a")
            cli("send-ui-ime", "commit", "a" * limit)
            assert len(editor(name)["text"].encode("utf-8")) == limit
            keys("Ctrl+a", "z")
            assert editor(name)["text"] == "z", f"{name}: selected keyboard replacement failed at bound"
            keys("Ctrl+a")
            cli("send-ui-ime", "commit", "a" * limit)
            keys("Ctrl+a")
            cli("send-ui-ime", "commit", "短")
            state = editor(name)
            assert state["text"] == "短" and state["caret"] == 3 and state["selection"] is None
            keys("Ctrl+a")
            cli("send-ui-ime", "commit", "a" * limit)
            keys("Home", "Right", "Shift+Right")
            before = editor(name)
            assert before["selection"] == [1, 2]
            cli("send-ui-ime", "commit", "中字")
            rejected = editor(name)
            for field in ("text", "caret", "selection"):
                assert rejected[field] == before[field], f"{name}: oversized commit changed {field}"
            keys("Ctrl+a")
            cli("send-ui-ime", "commit", "a中z")
            keys("Home", "Right", "Shift+Right")
            assert editor(name)["selection"] == [1, 4]
            cli("send-ui-ime", "commit", "XY")
            state = editor(name)
            assert state["text"] == "aXYz" and state["caret"] == 3 and state["selection"] is None
            keys("Ctrl+a")
            cli("send-ui-ime", "commit", "http://127.0.0.1:65535")
            cli("send-ui-ime", "preedit", "ni")
            keys("Enter")
            composing = editor(name)
            assert composing["focused"] and composing["text"] == "http://127.0.0.1:65535" and composing["preedit"] == "ni", f"{name}: composition Enter submitted"
            if name == "browser_address_editor":
                click_geometry("browser_proxy")
            elif name == "browser_proxy_editor":
                keys("Ctrl+l")
            else:
                click_geometry("settings_console_route")
            blurred = editor(name)
            assert not blurred["focused"] and blurred["preedit"] == "" and blurred["text"] == composing["text"]
            if name == "web_proxy_candidate_editor":
                click_geometry("settings_web_route")
            click_geometry(target)
            assert editor(name)["text"] == composing["text"]
            keys("Ctrl+a")
            cli("send-ui-ime", "commit", "xy")
            geometry = cli("ui-snapshot")["geometry"]
            rectangle = geometry[target]
            scale = geometry["scale_permille"] / 1000
            cli("send-ui-mouse", "--action", "click", "--button", "left",
                "--column", str(round((rectangle["x"] + rectangle["width"] - 4) / scale)),
                "--row", str(round((rectangle["y"] + rectangle["height"] / 2) / scale)))
            cli("send-ui-ime", "commit", "a")
            cli("send-ui-ime", "commit", "b")
            assert editor(name)["text"] == "xyab" and editor(name)["selection"] is None
            checks[f"bounded_editor:{name}"] = {"status": "passed", "limit_bytes": limit,
                "selected_key_and_cli_ime_replacement": True, "oversize_preserves_text_caret_selection": True,
                "partial_utf8_replacement": True, "preedit_enter_and_blur": True, "collapsed_click_anchor": True}

        def harness():
            return cli("ui-snapshot")["geometry"]["harness_settings"]

        def open_harness():
            if not cli("ui-snapshot")["settings_open"]:
                click_geometry("settings")
            click_geometry("settings_harness_route")
            assert cli("ui-snapshot")["geometry"]["settings_section"] == "harness"
            return wait(lambda: state if (state := harness()) is not None and not state["busy"] else None,
                        "Harness background initialization", 12)

        def click_harness(row_id: str, column: int):
            # Navigate only published rows/targets; never duplicate row order.
            for _ in range(80):
                geometry = cli("ui-snapshot")["geometry"]
                state = geometry["harness_settings"]
                if state["scroll"] == 0:
                    break
                click_rectangle(state["previous"], geometry, "Harness Previous")
            for _ in range(80):
                geometry = cli("ui-snapshot")["geometry"]
                state = geometry["harness_settings"]
                row = next((row for row in state["rows"] if row["id"] == row_id), None)
                if row is not None:
                    click_rectangle(row["cells"][column], geometry, row_id)
                    return
                click_rectangle(state["next"], geometry, "Harness Next")
            raise AssertionError(f"Harness row {row_id} was not reachable")

        # The Web draft is unsaved, distinct from the terminal and Harness.
        click_geometry("web_proxy_input")
        keys("Ctrl+a")
        cli("send-ui-ime", "commit", "http://127.0.0.1:22123")
        initial = open_harness()
        assert initial["listener"] == "stopped" and not initial["login_active"] and not initial["local_key_exists"]
        assert initial["accounts"] == [] and initial["model"] == ""
        click_harness("model_input", 0)
        cli("send-ui-ime", "commit", "provider/中-draft")
        cli("send-ui-ime", "preedit", "composing")
        keys("Enter")
        assert harness()["status"] is None, "composition Enter applied a Harness model"
        click_geometry("settings_console_route")
        assert cli("ui-snapshot")["geometry"]["settings_section"] == "console"
        click_geometry("settings_web_route")
        assert editor("web_proxy_candidate_editor")["text"] == "http://127.0.0.1:22123"
        retained = open_harness()
        assert retained["model_editor"]["text"] == "provider/中-draft" and retained["model_editor"]["preedit"] == ""
        assert cli("ui-snapshot")["composer_text"] == draft
        checks["settings_routes_independent_drafts"] = {"status": "passed", "routes": ["console", "web", "harness"],
                                                       "model_preedit_not_applied": True, "lazy_runtime": True}

        # Force a real atomic-publish failure, then prove the same draft saves
        # after removing the obstruction. No fake provider/account is created.
        old_port = retained["port"]
        target = Path(env["MINICON_CONFIG_DIR"]) / "harness" / "gateway.json"
        if target.is_file():
            target.unlink()
        target.mkdir()
        try:
            click_harness("port", 0)
            keys("Ctrl+a")
            cli("send-ui-ime", "commit", "30127")
            click_harness("port", 1)
            failed = wait(lambda: state if (state := harness())["status"] is not None and "Storage" in state["status"] else None,
                          "Harness failed port save is visible", 12)
            assert failed["port"] == old_port and failed["port_editor"]["text"] == "30127"
            assert failed["listener"] == "stopped" and failed["actual_port"] is None
        finally:
            target.rmdir()
        click_harness("port", 1)
        saved = wait(lambda: state if (state := harness())["port"] == 30127 else None,
                     "Harness port draft saves after obstruction removal", 12)
        assert saved["port_editor"]["text"] == "30127"
        checks["harness_failed_port_save"] = {"status": "passed", "old_port_preserved": True,
            "editable_draft_retained": True, "same_draft_saved_after_recovery": True, "listener_never_started": True}

        cli("resize-window", "--width", "420", "--height", "300")
        click_harness("key_actions", 0)
        state = wait(lambda: state if (state := harness())["local_key_exists"] else None,
                     "Harness local key creation", 12)
        assert state["scroll"] > 0, "short window did not scroll to connection controls"
        geometry = cli("ui-snapshot")["geometry"]
        state = geometry["harness_settings"]
        scale = geometry["scale_permille"] / 1000
        assert state["rows"], "narrow Harness has no visible readable row"
        for row in state["rows"]:
            assert abs(row["rect"]["height"] / scale - 36) <= 1
            assert row["rect"]["y"] + row["rect"]["height"] <= state["previous"]["y"]
        assert state["listener"] == "stopped" and state["actual_port"] is None
        checks["harness_narrow_readable_scroll"] = {"status": "passed", "logical_size": [420, 300],
            "visible_rows": len(state["rows"]), "scroll": state["scroll"], "row_height_dip": 36,
            "local_key_creation_did_not_start_listener": True}
        keys("Escape")
        assert not cli("ui-snapshot")["settings_open"]
        # Explicit plan cases: actual saved counts 0/1/8, narrow and roomy
        # menus, then resize while the last action remains highlighted.
        saved_count = 0
        for desired in (0, 1, 8):
            cli("resize-window", "--width", "1000", "--height", "700")
            if saved_count < desired:
                assert open_web_settings()["web_proxy_options_count"] == saved_count
                while saved_count < desired:
                    click_geometry("web_proxy_input")
                    keys("Ctrl+a")
                    cli("send-ui-ime", "commit", f"http://127.0.0.1:{35000 + saved_count}")
                    keys("Enter")
                    saved_count += 1
                    assert cli("ui-snapshot")["geometry"]["web_proxy_options_count"] == saved_count
                keys("Escape")
                assert not cli("ui-snapshot")["settings_open"]
            for width, height in ((540, 300), (1000, 700)):
                cli("resize-window", "--width", str(width), "--height", str(height))
                click_geometry("browser_proxy_dropdown")
                opened = cli("ui-snapshot")["geometry"]
                assert opened["browser_proxy_menu_open"]
                assert opened["web_proxy_options_count"] == desired
                rows = opened["browser_proxy_option_rows"]
                assert rows and len(rows) <= desired + 1
                if desired <= 1:
                    assert len(rows) == desired + 1
                scale = opened["scale_permille"] / 1000
                for row in rows:
                    assert abs(row["height"] / scale - 32) <= 1, "proxy menu shrank readable targets"
                assert opened["browser_proxy_menu"]["height"] == sum(row["height"] for row in rows) + round(8 * scale), "proxy menu retained unused choice rows"
                keys(*("Down",) * (desired + 2))
                moved = cli("ui-snapshot")["geometry"]
                assert moved["proxy_menu_highlight"] == desired
                assert desired in moved["browser_proxy_option_indices"], "last saved choice was not revealed"
                if desired == 8 and height == 300:
                    assert moved["proxy_menu_scroll"] > 0
                cli("resize-window", "--width", "760", "--height", "420")
                resized = cli("ui-snapshot")["geometry"]
                assert resized["proxy_menu_highlight"] == desired, "resize changed highlighted proxy action"
                assert desired in resized["browser_proxy_option_indices"]
                resized_scale = resized["scale_permille"] / 1000
                for row in resized["browser_proxy_option_rows"]:
                    assert abs(row["height"] / resized_scale - 32) <= 1
                snapshot_file = f"proxy-menu-count-{desired}-{width}x{height}.json"
                (output / snapshot_file).write_text(json.dumps({"opened": opened, "last_highlighted": moved,
                                                               "resized": resized}, indent=2), encoding="utf-8")
                keys("Escape")
                assert not cli("ui-snapshot")["geometry"]["browser_proxy_menu_open"]
                checks[f"proxy_menu_count:{desired}:{width}x{height}"] = {
                    "status": "passed", "saved_count": saved_count, "logical_size": [width, height],
                    "scale_permille": opened["scale_permille"], "visible_rows": len(rows),
                    "row_height_dip": 32, "highlight": moved["proxy_menu_highlight"],
                    "visible_indices": moved["browser_proxy_option_indices"], "scroll": moved["proxy_menu_scroll"],
                    "resized_logical_size": [760, 420], "resized_scale_permille": resized["scale_permille"],
                    "resized_highlight": resized["proxy_menu_highlight"],
                    "resized_visible_indices": resized["browser_proxy_option_indices"],
                    "resized_scroll": resized["proxy_menu_scroll"], "geometry_file": snapshot_file}
        (output / "host-controls-final.json").write_text(json.dumps({"tabs": cli("list-tabs"), "ui": cli("ui-snapshot")}, indent=2), encoding="utf-8")

    def h0_journey():
        """Fresh same-window destination, public injected UI, same-frame raster.

        This does not observe native physical input, IME, child pixels, cookies,
        provider authorization or an external client. Runtime is not auto-started.
        """
        terminal_id = next(row["id"] for row in cli("list-tabs")["tabs"] if row["kind"] == "terminal")
        cli("resize-window", "--width", "1000", "--height", "700")
        keys("Ctrl+Shift+I")
        terminal_draft = "h0 exact artifact console draft"
        cli("send-ui-ime", "commit", terminal_draft)
        original = cli("ui-snapshot")
        assert original["active"] == terminal_id and original["composer_focused"]
        console_baseline = output / "h0-console-before.png"
        console_baseline.unlink(missing_ok=True)
        cli("screenshot-pane", "--output", str(console_baseline))
        harness_root = Path(env["MINICON_CONFIG_DIR"]) / "harness"
        assert not harness_root.exists(), "fresh H0 fixture already instantiated Harness"

        def state():
            snap = cli("ui-snapshot")
            return snap, snap["geometry"], snap["geometry"]["harness_workspace"]

        def pair(geometry):
            robot, settings = geometry["harness_entry"], geometry["settings"]
            scale = geometry["host_scale_permille"] / 1000
            assert robot["y"] == settings["y"] and robot["x"] + robot["width"] <= settings["x"], "robot must be left of adjacent Settings"
            for rect in (robot, settings):
                assert abs(rect["width"] / scale - 32) <= 1 and abs(rect["height"] / scale - 32) <= 1

        def raster(label, active):
            before, geometry, workspace = state()
            assert before["active"] == active and workspace["active"]
            card = next(card["rect"] for card in workspace["cards"] if card["rect"]["height"] > 0)
            points = [(card["x"] + 2, card["y"] + 2),
                      (card["x"] + 2, card["y"] + card["height"] + 2)]
            path = output / f"h0-{label}-host-frame.png"
            path.unlink(missing_ok=True)
            reply = cli("screenshot-pane", "--output", str(path))
            observed = host_png_samples(path, points)
            assert [observed["width"], observed["height"]] == [reply["width"], reply["height"]]
            assert len({tuple(value) for value in observed["samples_rgb"].values()}) == 2, "H0 card/gap raster must differ from the unchanged Console sidebar"
            after = cli("ui-snapshot")
            assert after["active"] == active and after["geometry"]["harness_workspace"]["active"], "screenshot changed the invoking workspace"
            (output / f"h0-{label}-raster.json").write_text(json.dumps({"before": before, "after": after, "png": observed}, indent=2), encoding="utf-8")
            return {"status": "passed", "scope": "same-frame host raster only", "file": path.name,
                    "sha256": hashlib.sha256(path.read_bytes()).hexdigest(), "width": observed["width"],
                    "height": observed["height"], "card_gap_distinct": True,
                    "sample_points": points, "samples_rgb": observed["samples_rgb"],
                    "active_unchanged": True, "native_child_pixels": False}

        pair(original["geometry"])
        click_geometry("harness_entry")
        entered, geometry, workspace = state()
        assert workspace["active"] and workspace["metadata"] == "not_loaded" and entered["active"] == terminal_id
        assert geometry["harness_settings"] is None and not entered["settings_open"] and not harness_root.exists()
        keys("Ctrl+Shift+I")
        assert cli("send-ui-ime", "commit", "H0_HIDDEN_CONSOLE")["route"] == "harness_workspace"
        keys("Escape")
        assert cli("ui-snapshot")["composer_text"] == terminal_draft
        checks["h0_console_host_raster"] = raster("console", terminal_id)
        click_geometry("harness_entry")
        returned, _, workspace = state()
        assert not workspace["active"] and returned["active"] == terminal_id and returned["composer_focused"]
        assert returned["composer_text"] == terminal_draft and not harness_root.exists()
        checks["h0_console_entry_observe_only"] = {"status": "passed", "robot_left_of_settings": True,
            "metadata": "not_loaded", "selected_robot_returns": True, "composer_draft_and_focus_restored": True,
            "no_runtime_created": True, "input_path": "public CLI injection"}

        browser_id = open_browser_from_menu()["id"]
        navigate(origin + h0_path)
        loaded(h0_path, "Harness live fixture")

        def fresh_document(label):
            with h0_lock:
                h0_live["challenge"] = challenge = secrets.token_hex(16)
            def observed():
                with h0_lock:
                    report = h0_live["report"]
                    return dict(report) if report is not None and report["challenge"] == challenge else None
            report = wait(observed, label)
            assert report["draft"] == "unsaved:" + report["nonce"]
            assert report["history_nonce"] == report["nonce"] and report["history_step"] == 2
            assert report["history_length"] >= 2
            return report

        before_document = fresh_document("fixture-owned live document before H0")
        request_count = counts.get(h0_path, 0)
        assert request_count == 1, "fresh H0 document loaded more than once"
        keys("Ctrl+l", "Ctrl+a")
        address_draft = "https://h0-draft.fixture.invalid/unsubmitted"
        cli("send-ui-ime", "commit", address_draft)
        web_before = cli("ui-snapshot")
        assert web_before["geometry"]["address_focused"]
        click_geometry("harness_entry")
        active, geometry, workspace = state()
        assert workspace["active"] and active["active"] == browser_id and workspace["metadata"] == "not_loaded"
        assert not harness_root.exists() and geometry["harness_settings"] is None
        keys("Ctrl+l")
        assert cli("send-ui-ime", "commit", "H0_HIDDEN_WEB")["route"] == "harness_workspace"
        assert editor("browser_address_editor")["text"] == address_draft
        assert cli("ui-snapshot")["composer_text"] == terminal_draft
        checks["h0_loaded_web_host_raster"] = raster("loaded-web", browser_id)
        # Explicit target retains existing pane semantics while the implicit
        # request above captures the actually visible Dashboard. It must restore
        # the Browser as well as the dashboard/focus state after offscreen work.
        points = checks["h0_loaded_web_host_raster"]["sample_points"]
        pane_path = output / "h0-explicit-console-pane.png"
        pane_path.unlink(missing_ok=True)
        cli("screenshot-pane", "--target", terminal_id, "--output", str(pane_path))
        pane = host_png_samples(pane_path, points)
        baseline = host_png_samples(console_baseline, points)
        assert pane["samples_rgb"] == baseline["samples_rgb"], "explicit target did not capture the Console pane"
        assert pane["samples_rgb"] != checks["h0_loaded_web_host_raster"]["samples_rgb"], "explicit target incorrectly captured Dashboard"
        restored, _, workspace = state()
        assert restored["active"] == browser_id and workspace["active"], "explicit pane capture failed to restore active Browser"
        assert restored["composer_text"] == terminal_draft and editor("browser_address_editor")["text"] == address_draft
        checks["h0_explicit_console_pane_restores_browser"] = {"status": "passed",
            "file": pane_path.name, "sha256": hashlib.sha256(pane_path.read_bytes()).hexdigest(),
            "console_baseline_samples_matched": True, "dashboard_samples_differed": True,
            "active_browser_and_hidden_drafts_preserved": True, "scope": "explicit pane vs visible host frame"}

        # Use published card IDs and row IDs, never duplicate private row order.
        links = []
        for index, section, card_id, row_id in ((0, "Overview", "service", "local_api"),
                                                (1, "Accounts", "accounts", "accounts"),
                                                (2, "Clients", "connection", "model"),
                                                (3, "Usage", "monitoring", "status")):
            _, geometry, workspace = state()
            click_rectangle(workspace["navigation"][index], geometry, section)
            current, geometry, workspace = state()
            assert workspace["section"] == section
            card = next(card for card in workspace["cards"] if card["id"] == card_id)
            assert card["rect"]["height"] > 0, "deep-link card is not visible"
            click_rectangle(card["rect"], geometry, card_id)
            selected, geometry, workspace = state()
            assert workspace["selected"] == card_id
            click_rectangle(workspace["settings"], geometry, "H0 Configure")
            opened = wait(lambda: snap if (snap := cli("ui-snapshot"))["geometry"]["harness_settings"] is not None
                          and not snap["geometry"]["harness_settings"]["busy"] else None,
                          "H0 Settings metadata", 12)
            settings = opened["geometry"]["harness_settings"]
            assert opened["settings_open"] and opened["geometry"]["settings_section"] == "harness"
            assert any(row["id"] == row_id for row in settings["rows"]), f"H0 deep link missed {row_id}"
            if card_id == "connection":
                # The model marker may legitimately clamp at the last full page,
                # but it must never be the default Service position.
                assert settings["scroll"] > 0, "H0 connection deep link stayed at Service"
            else:
                assert settings["rows"][0]["id"] == row_id, f"H0 deep link did not position {row_id}"
            assert settings["listener"] == "stopped" and settings["actual_port"] is None and not settings["login_active"]
            assert settings["accounts"] == [] and not settings["local_key_exists"] and settings["selected_account"] is None
            if index == 0:
                click_geometry("settings_console_route")
                keys("Ctrl+Shift+I")
                assert cli("send-ui-ime", "commit", "H0_HIDDEN_SETTINGS")["route"] == "harness_settings_shell"
            keys("Escape")
            resumed, _, workspace = state()
            assert not resumed["settings_open"] and workspace["active"] and workspace["section"] == section
            assert resumed["active"] == browser_id and resumed["composer_text"] == terminal_draft
            assert editor("browser_address_editor")["text"] == address_draft
            links.append({"section": section, "card": card_id, "visible_settings_row": row_id,
                          "first_visible_settings_row": settings["rows"][0]["id"], "scroll": settings["scroll"],
                          "listener": settings["listener"], "login_active": settings["login_active"]})
        checks["h0_settings_exact_deeplinks_and_return"] = {"status": "passed", "links": links,
            "automatic_start_or_login": False, "hidden_drafts_preserved": True}

        snap, geometry, workspace = state()
        click_rectangle(workspace["back"], geometry, "H0 Back")
        returned = cli("ui-snapshot")
        assert not returned["geometry"]["harness_workspace"]["active"] and returned["active"] == browser_id
        assert returned["geometry"]["address_focused"] and editor("browser_address_editor")["text"] == address_draft
        loaded(h0_path, "Harness live fixture")
        after_document = fresh_document("fixture-owned live document after H0")
        for key in ("nonce", "draft", "history_length", "history_nonce", "history_step"):
            assert before_document[key] == after_document[key], f"H0 replaced live document {key}"
        assert counts.get(h0_path, 0) == request_count, "return from H0 reloaded the ordinary page"
        proof_file = "h0-live-document.json"
        (output / proof_file).write_text(json.dumps({"before": before_document, "after": after_document,
                                                   "page_gets_before": request_count, "page_gets_after": counts.get(h0_path, 0)}, indent=2), encoding="utf-8")
        checks["h0_loaded_web_document_return"] = {"status": "passed", "active_browser_preserved": True,
            "address_draft_and_focus_restored": True, "document_nonce_preserved": True,
            "unsaved_dom_preserved": True, "history_state_preserved": True, "page_gets": request_count,
            "proof_file": proof_file, "cookie_observation": False, "native_child_pixels": False}

        for row in cli("list-tabs")["tabs"]:
            cli("close-tab", "--target", row["id"])
        assert cli("ui-snapshot")["workspace_empty"]
        cli("resize-window", "--width", "420", "--height", "300")
        empty = cli("ui-snapshot")
        pair(empty["geometry"])
        click_geometry("harness_entry")
        empty, geometry, workspace = state()
        assert empty["workspace_empty"] and empty["active"] is None and workspace["active"]
        scale = geometry["host_scale_permille"] / 1000
        def reachable(rect):
            assert rect["width"] > 0 and rect["height"] > 0 and rect["x"] >= 0 and rect["y"] >= 0
            assert rect["x"] + rect["width"] <= round(420 * scale) and rect["y"] + rect["height"] <= round(300 * scale)
        for rect in (workspace["back"], workspace["settings"], *workspace["navigation"]):
            reachable(rect)
        back, configure = workspace["back"], workspace["settings"]
        assert back["x"] + back["width"] <= configure["x"], "H0 Back/Configure overlap"
        visited = []
        for index, section in enumerate(("Overview", "Accounts", "Clients", "Usage", "Activity")):
            _, geometry, workspace = state()
            click_rectangle(workspace["navigation"][index], geometry, section)
            assert state()[2]["section"] == section
            visited.append(section)
        assert cli("send-ui-ime", "commit", "H0_ZERO_MUST_NOT_TYPE")["route"] == "harness_workspace"
        _, geometry, workspace = state()
        click_rectangle(workspace["settings"], geometry, "H0 narrow Configure")
        assert cli("ui-snapshot")["settings_open"]
        keys("Escape")
        empty, geometry, workspace = state()
        assert not empty["settings_open"] and workspace["active"] and workspace["section"] == "Activity"
        click_rectangle(workspace["back"], geometry, "H0 narrow Back")
        final = cli("ui-snapshot")
        assert final["workspace_empty"] and final["active"] is None and not final["geometry"]["harness_workspace"]["active"]
        (output / "h0-zero-narrow.json").write_text(json.dumps({"active": empty, "returned": final}, indent=2), encoding="utf-8")
        checks["h0_zero_narrow_routes_reachable"] = {"status": "passed", "requested_logical_size": [420, 300],
            "scale_permille": geometry["host_scale_permille"], "sections": visited,
            "settings_and_back_reachable": True, "zero_tabs_preserved": True}

    def open_browser_from_menu():
        click_geometry("new_root")
        wait(lambda: cli("ui-snapshot")["geometry"]["new_tab_menu_open"], "New menu opens")
        click_geometry("new_browser_action")
        def observed():
            rows = [row for row in cli("list-tabs")["tabs"] if row["kind"] == "browser"]
            assert len(rows) <= 1, "New Browser created more than one root"
            return rows[0] if len(rows) == 1 and rows[0]["active"] else None
        row = wait(observed, "visible New Browser action focuses its single root")
        assert not cli("ui-snapshot")["geometry"]["new_tab_menu_open"], "New menu stayed open after selection"
        return row

    def open_web_settings():
        click_geometry("settings")
        assert cli("ui-snapshot")["settings_open"], "Settings did not open"
        click_geometry("settings_console_route")
        assert cli("ui-snapshot")["geometry"]["settings_section"] == "console"
        click_geometry("settings_web_route")
        snapshot = cli("ui-snapshot")
        assert snapshot["geometry"]["settings_section"] == "web"
        return snapshot["geometry"]

    def launch(trace):
        nonlocal child
        child = subprocess.Popen([str(binary), "--no-activate", "--control", endpoint, "-e", *shell],
                                 env=env, stdout=subprocess.DEVNULL, stderr=trace, start_new_session=not windows)
        deadline = time.monotonic() + 20
        while True:
            assert child.poll() is None, "GUI exited before control endpoint became ready"
            try:
                cli("list-tabs")
                return
            except subprocess.CalledProcessError:
                assert time.monotonic() < deadline, "control endpoint did not become ready"
                time.sleep(0.1)

    def apply_proxy(value: str):
        click_geometry("browser_proxy")
        assert cli("ui-snapshot")["geometry"]["proxy_focused"], "proxy field did not acquire editing focus"
        cli("send-ui-keys", "Ctrl+a")
        cli("send-ui-keys", "Backspace")
        if value:
            cli("send-ui-ime", "commit", value)
        cli("send-ui-keys", "Enter")

    def capture(name: str):
        nonlocal mac_window_id, mac_capture_error
        (output / f"{name}.json").write_text(json.dumps({"tabs": cli("list-tabs"), "ui": cli("ui-snapshot")}, indent=2), encoding="utf-8")
        if native is not None:
            from PIL import ImageGrab
            rectangle = wintypes.RECT()
            native.GetWindowRect(hwnd, ctypes.byref(rectangle))
            ImageGrab.grab(bbox=(rectangle.left, rectangle.top, rectangle.right, rectangle.bottom), all_screens=True).save(output / f"{name}.png")
        elif sys.platform == "darwin":
            # Capture this fixture's window, including WKWebView pixels. Never
            # fall back to the whole desktop or an interactive permission flow.
            try:
                path = output / f"{name}.png"
                path.unlink(missing_ok=True)
                if mac_capture_error is not None:
                    raise RuntimeError(mac_capture_error)
                if mac_window_id is None:
                    probe = subprocess.run(["/usr/bin/swift", "-", str(child.pid)], input="""
import CoreGraphics
import Foundation
let pid = Int32(CommandLine.arguments[1]) ?? -1
if let rows = CGWindowListCopyWindowInfo([.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID) as? [[String: Any]] {
    for row in rows {
        if let owner = row[kCGWindowOwnerPID as String] as? Int32, owner == pid,
           let layer = row[kCGWindowLayer as String] as? Int, layer == 0,
           let bounds = row[kCGWindowBounds as String] as? [String: Any],
           let width = bounds["Width"] as? Double, width > 0,
           let height = bounds["Height"] as? Double, height > 0,
           let number = row[kCGWindowNumber as String] as? Int {
            print(number); break
        }
    }
}
""", text=True, capture_output=True, timeout=30)
                    if probe.returncode != 0 or not probe.stdout.strip().isdigit():
                        raise RuntimeError("owned-window-id-unavailable")
                    mac_window_id = int(probe.stdout.strip())
                captured = subprocess.run(["/usr/sbin/screencapture", "-o", "-x", "-t", "png", f"-l{mac_window_id}", str(path)],
                                          capture_output=True, timeout=10)
                if captured.returncode != 0:
                    raise RuntimeError("owned-window-capture-unavailable")
                raw = path.read_bytes()
                if len(raw) < 24 or raw[:16] != b"\x89PNG\r\n\x1a\n\0\0\0\rIHDR" or not all(
                        int.from_bytes(raw[start:start + 4], "big") > 0 for start in (16, 20)):
                    raise RuntimeError("owned-window-png-invalid")
                screenshots[name] = {"status": "captured", "file": path.name}
            except (OSError, RuntimeError, subprocess.TimeoutExpired) as error:
                reason = str(error) if isinstance(error, RuntimeError) else type(error).__name__
                mac_capture_error = reason
                screenshots[name] = {"status": "BLOCKED", "reason": reason}
            (output / "screenshots.json").write_text(json.dumps(screenshots, indent=2) + "\n", encoding="utf-8")

    previous_sigterm = None
    if args.mac_window_shortcuts_only:
        def owned_stop(_signal, _frame):
            raise InterruptedError("owned public court cancelled")
        previous_sigterm = signal.signal(signal.SIGTERM, owned_stop)
    try:
        if args.mac_window_shortcuts_only:
            with (output / "host.log").open("wb") as trace:
                launch(trace)
                mac_window_shortcuts_journey()
                cli("close-window")
                assert child.wait(timeout=10) == 0
            assert hashlib.sha256(binary.read_bytes()).hexdigest() == binary_sha256
            receipt = {"status": "passed", "platform": sys.platform,
                       "binary_sha256": binary_sha256, "script_sha256": script_sha256,
                       "scope": "single-display public CLI ConApp window route; not WK focus/hardware",
                       "host_controls_checks": checks}
            (output / "receipt.json").write_bytes((json.dumps(receipt, indent=2) + "\n").encode())
            print(json.dumps(receipt))
            return 0
        with (output / "host.log").open("wb") as trace:
            launch(trace)
            terminal_id = next(row["id"] for row in cli("list-tabs")["tabs"] if row["kind"] == "terminal")
            terminal_draft = "preview draft stays editable"
            cli("send-ui-keys", "Ctrl+Shift+I")
            cli("send-ui-ime", "commit", terminal_draft)
            assert cli("ui-snapshot")["composer_text"] == terminal_draft
            first_id = open_browser_from_menu()["id"]
            assert open_browser_from_menu()["id"] == first_id
            navigate(origin + "/first")
            loaded("/first", "First page")
            if args.physical:
                assert windows, "the physical court requires interactive Windows"
                native = ctypes.WinDLL("user32", use_last_error=True)
                native.GetForegroundWindow.restype = wintypes.HWND
                native.SetForegroundWindow.argtypes = [wintypes.HWND]
                native.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
                native.GetWindowRect.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.RECT)]
                native.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
                foreground = native.GetForegroundWindow()
                pointer = wintypes.POINT()
                native.GetCursorPos(ctypes.byref(pointer))
                handles = []
                callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

                @callback_type
                def visit(handle, _data):
                    pid = wintypes.DWORD()
                    native.GetWindowThreadProcessId(handle, ctypes.byref(pid))
                    if pid.value == child.pid:
                        handles.append(handle)
                    return True

                native.EnumWindows(visit, 0)
                assert handles, "native parent window was not found"
                hwnd = handles[0]
                assert native.SetForegroundWindow(hwnd), "could not activate this court's GUI"
                page = cli("ui-snapshot")["geometry"]["browser_page"]
                point = wintypes.POINT(page["x"] + 70, page["y"] + 100)
                assert native.ClientToScreen(hwnd, ctypes.byref(point))
                native.SetCursorPos(point.x, point.y)
                native.mouse_event(2, 0, 0, 0, 0)
                native.mouse_event(4, 0, 0, 0, 0)
                time.sleep(0.2)
                native.keybd_event(0x11, 0, 0, 0)
                native.keybd_event(0x4C, 0, 0, 0)
                native.keybd_event(0x4C, 0, 2, 0)
                native.keybd_event(0x11, 0, 2, 0)
                wait(lambda: cli("ui-snapshot")["geometry"].get("address_focused"), "physical page Ctrl+L reaches the address editor", 5)
                cli("send-ui-keys", "Escape")
                time.sleep(0.2)
                native.keybd_event(0x11, 0, 0, 0)
                native.keybd_event(0x10, 0, 0, 0)
                native.keybd_event(0xDB, 0, 0, 0)
                native.keybd_event(0xDB, 0, 2, 0)
                native.keybd_event(0x10, 0, 2, 0)
                native.keybd_event(0x11, 0, 2, 0)
                wait(lambda: not browser()["active"],
                     "physical page Ctrl+Shift+[ selects the terminal", 5)
                cli("send-ui-keys", "Ctrl+Shift+O")
                loaded("/first", "First page")
            capture("first")
            navigate(origin + "/redirect")
            assert loaded("/second", "Second page")["can_back"]
            capture("second")
            click_toolbar(0)
            assert loaded("/first", "First page")["can_forward"]
            click_toolbar(1)
            loaded("/second", "Second page")
            count = counts.get("/second", 0)
            click_toolbar(2)
            wait(lambda: counts.get("/second", 0) > count, "reload reaches the page server")
            loaded("/second", "Second page")
            cli("resize-window", "--width", "1100", "--height", "740")
            for chord in ("Ctrl+Shift+B", "Ctrl+Shift+H", "Ctrl+Shift+H", "Ctrl+Shift+B"):
                cli("send-ui-keys", chord)
                geometry = cli("ui-snapshot")["geometry"]
                assert geometry["browser_page"]["y"] == geometry["browser_toolbar"]["height"]
            cli("send-ui-keys", "Ctrl+Shift+[")
            terminal = cli("ui-snapshot")
            assert terminal["active"] == terminal_id
            assert terminal["composer_text"] == terminal_draft, "returning from preview lost the terminal draft"
            cli("send-ui-keys", "Ctrl+Shift+O")
            loaded("/second", "Second page")
            capture("resized")
            with socket.socket() as refused:
                refused.bind(("127.0.0.1", 0))
                recovery_port = refused.getsockname()[1]
                failed_url = f"http://127.0.0.1:{recovery_port}/unavailable"
            navigate(failed_url)
            wait(lambda: browser()["browser"]["load"] == "unavailable", "failed connection is explicit")
            assert browser()["browser"]["url"] == failed_url
            assert browser()["browser"]["reason"].startswith("browser-navigation-")
            capture("failure")
            click_geometry("browser_edit_address")
            wait(lambda: cli("ui-snapshot")["geometry"]["address_focused"], "failure Edit address action focuses the editor")
            assert browser()["browser"]["url"] == failed_url
            cli("send-ui-keys", "Escape")
            recovery_server = http.server.ThreadingHTTPServer(("127.0.0.1", recovery_port), Page)
            recovery_worker = threading.Thread(target=recovery_server.serve_forever, daemon=True)
            recovery_worker.start()
            count = counts.get("/unavailable", 0)
            click_geometry("browser_retry")
            wait(lambda: counts.get("/unavailable", 0) > count, "Retry reaches the restarted local service")
            wait(lambda: browser()["browser"]["load"] == "ready" and browser()["browser"]["url"] == failed_url,
                 "Retry restores the attempted URL in the same panel")
            recovered = browser()
            assert recovered["id"] == first_id
            assert recovered["browser"]["reason"] is None, "successful Retry retained the previous failure"
            capture("recovered")
            navigate(origin + "/first")
            loaded("/first", "First page")
            proxy_server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), ProxyPage)
            proxy_worker = threading.Thread(target=proxy_server.serve_forever, daemon=True)
            proxy_worker.start()
            apply_proxy(f"http://127.0.0.1:{proxy_server.server_port}")
            navigate(proxy_target)
            def proxy_loaded():
                state = browser()["browser"]
                return state["url"] == proxy_target and state["load"] == "ready" and state["title"] == "Proxy preview"
            wait(proxy_loaded, "visible proxy field applies to native page requests")
            assert proxy_target in proxy_requests, "proxy setting did not reach the loopback proxy"
            assert browser()["id"] == first_id, "applying a proxy must keep the Browser tab"
            capture("proxied")
            direct_path = "/direct-after-proxy"
            navigate(origin + direct_path)
            wait(lambda: browser()["browser"]["url"] == origin + direct_path, "direct recovery URL is retained before clearing proxy")
            direct_count = counts.get(direct_path, 0)
            apply_proxy("")
            loaded(direct_path, "First page")
            assert counts.get(direct_path, 0) > direct_count, "clearing proxy did not reload the local origin directly"
            blocked_path = "/blocked-by-invalid-proxy"
            apply_proxy("http://127.0.0.1:not-a-port")
            wait(lambda: browser()["browser"]["reason"] == "browser-proxy-invalid", "invalid proxy is explicit")
            navigate(origin + blocked_path)
            wait(lambda: browser()["browser"]["url"] == origin + blocked_path
                 and browser()["browser"]["reason"] == "browser-proxy-invalid", "invalid proxy blocks subsequent navigation")
            click_geometry("browser_retry")
            deadline = time.monotonic() + 0.5
            while time.monotonic() < deadline:
                assert counts.get(blocked_path, 0) == 0, "invalid proxy leaked a direct origin request"
                time.sleep(0.01)
            apply_proxy("")
            loaded(blocked_path, "First page")
            capture("proxy-cleared")
            cli("select-tab", "--target", str(terminal_id))
            assert cli("ui-snapshot")["composer_text"] == terminal_draft, "failure/recovery lost the terminal draft"
            assert open_browser_from_menu()["id"] == first_id
            cli("close-tab", "--target", str(first_id))
            cli("send-ui-keys", "Ctrl+Shift+O")
            assert browser()["id"] != first_id
            assert open_web_settings()["web_proxy_options_count"] == 0, "fixture settings must start empty"
            click_geometry("web_proxy_input")
            candidate = f"http://127.0.0.1:{proxy_server.server_port}"
            cli("send-ui-ime", "commit", candidate)
            click_geometry("web_proxy_add")
            assert cli("ui-snapshot")["geometry"]["web_proxy_options_count"] == 1
            cli("send-ui-keys", "Escape")
            assert not cli("ui-snapshot")["settings_open"]
            click_geometry("browser_proxy_dropdown")
            click_geometry("browser_proxy_option_rows", 1)
            proxy_count = len(proxy_requests)
            navigate(proxy_target)
            wait(proxy_loaded, "new candidate is immediately selectable")
            assert len(proxy_requests) > proxy_count, "new candidate did not produce a proxy request"
            cli("close-window")
            child.wait(timeout=10)
            launch(trace)
            assert open_web_settings()["web_proxy_options_count"] == 1, "candidate was not persisted across restart"
            cli("send-ui-keys", "Escape")
            open_browser_from_menu()
            click_geometry("browser_proxy_dropdown")
            geometry = cli("ui-snapshot")["geometry"]
            assert geometry["browser_proxy_menu_open"]
            assert len(geometry["browser_proxy_option_rows"]) == 2
            proxy_count = len(proxy_requests)
            click_geometry("browser_proxy_option_rows", 1)
            navigate(proxy_target)
            wait(proxy_loaded, "persisted proxy candidate controls native requests")
            assert len(proxy_requests) > proxy_count, "candidate selection did not produce a new proxy request"
            assert open_web_settings()["web_proxy_options_count"] == 1
            click_geometry("web_proxy_remove", 0)
            assert cli("ui-snapshot")["geometry"]["web_proxy_options_count"] == 0
            cli("send-ui-keys", "Escape")
            cli("close-window")
            child.wait(timeout=10)
            launch(trace)
            assert open_web_settings()["web_proxy_options_count"] == 0, "deleted candidate returned after restart"
            cli("send-ui-keys", "Escape")
            open_browser_from_menu()
            click_geometry("browser_proxy_dropdown")
            assert len(cli("ui-snapshot")["geometry"]["browser_proxy_option_rows"]) == 1, "deleted candidate remains selectable"
            cli("send-ui-keys", "Escape")
            cli("close-window")
            child.wait(timeout=10)
            # Keep native navigation/proxy persistence evidence independent of
            # host editing and failed-save setup in this fresh private profile.
            env["MINICON_CONFIG_DIR"] = str(scratch_root / "host-controls-config")
            env["MINICON_BROWSER_PROFILE_ROOT"] = str(scratch_root / "host-controls-profile")
            launch(trace)
            host_controls_journey()
            cli("close-window")
            child.wait(timeout=10)
            # H0 gets its own fresh profile; prior host/runtime/proxy mutations
            # cannot supply its lazy-entry or live-document evidence.
            env["MINICON_CONFIG_DIR"] = str(scratch_root / "h0-config")
            env["MINICON_BROWSER_PROFILE_ROOT"] = str(scratch_root / "h0-profile")
            launch(trace)
            h0_journey()
            cli("close-window")
            child.wait(timeout=10)
            if sys.platform == "darwin":
                env["MINICON_CONFIG_DIR"] = str(scratch_root / "mac-shortcuts-config")
                env["MINICON_BROWSER_PROFILE_ROOT"] = str(scratch_root / "mac-shortcuts-profile")
                launch(trace)
                mac_window_shortcuts_journey()
                cli("close-window")
                assert child.wait(timeout=10) == 0
        assert hashlib.sha256(binary.read_bytes()).hexdigest() == binary_sha256, "binary changed during exact-artifact court"
        receipt = {"binary_sha256": binary_sha256, "platform": sys.platform,
                   "script_sha256": script_sha256,
                   "product_source_authority": "bound by enclosing exact-artifact build receipt; not inferred from local HEAD",
                   "physical_page_ctrl_l": args.physical, "physical_page_tab_switch": args.physical,
                   "visible_browser_entry": True, "single_browser_root_reused": True,
                   "service_restart_retry": True, "terminal_draft_preserved": True,
                   "visible_proxy_applied": True, "proxy_clear_restores_direct": True,
                   "invalid_proxy_blocks_direct": True, "proxy_candidates_persisted": True,
                   "proxy_candidate_selected": True, "proxy_candidate_deletion_persisted": True,
                   "host_controls_checks": checks,
                   "host_controls_input": {"path": "public CLI send-ui-keys / send-ui-ime / send-ui-mouse",
                                           "physical_keyboard": False, "native_ime": False, "clipboard": False},
                   "proxy_requests": proxy_requests,
                   "requests": counts, "status": "passed", "screenshots": screenshots}
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(receipt))
        return 0
    finally:
        if sys.exc_info()[0] is not None:
            try:
                (output / "failure-final.json").write_text(json.dumps({
                    "tabs": cli("list-tabs"), "ui": cli("ui-snapshot"),
                    "host_controls_checks": checks, "binary_sha256": binary_sha256,
                    "script_sha256": script_sha256,
                    "proxy_requests": proxy_requests, "requests": counts,
                }, indent=2) + "\n", encoding="utf-8")
            except (OSError, subprocess.SubprocessError):
                pass
        if child is not None and child.poll() is None:
            if not windows:
                os.killpg(child.pid, signal.SIGKILL)
            else:
                child.kill()
            child.wait(timeout=10)
        if args.mac_window_shortcuts_only:
            (output / "mac-owned-cleanup.json").write_bytes((json.dumps({
                "child_reaped": child is not None and child.poll() is not None,
                "exit_code": child.returncode if child is not None else None,
                "descendant_absence_independently_observed": False}) + "\n").encode())
        if previous_sigterm is not None:
            signal.signal(signal.SIGTERM, previous_sigterm)
        if native is not None:
            native.SetForegroundWindow(foreground)
            native.SetCursorPos(pointer.x, pointer.y)
        server.shutdown()
        server.server_close()
        worker.join(timeout=2)
        if recovery_server is not None:
            recovery_server.shutdown()
            recovery_server.server_close()
            recovery_worker.join(timeout=2)
        if proxy_server is not None:
            proxy_server.shutdown()
            proxy_server.server_close()
            proxy_worker.join(timeout=2)
        scratch.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
