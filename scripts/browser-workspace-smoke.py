#!/usr/bin/env python3
"""Public CLI/native page journey for exact desktop artifacts. Owner: PRD 25/27."""
from __future__ import annotations

import argparse
import ctypes
from ctypes import wintypes
import hashlib
import http.server
import json
import os
from pathlib import Path
import signal
import socket
import subprocess
import sys
import tempfile
import threading
import time


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--output", type=Path)
    parser.add_argument("--timeout", type=float, default=45, help="navigation observation deadline in seconds (0 < timeout <= 120)")
    parser.add_argument("--physical", action="store_true", help="Windows interactive focus/shortcut and page capture court")
    args = parser.parse_args()
    if not 0 < args.timeout <= 120:
        parser.error("--timeout must be greater than zero and at most 120")
    binary = args.binary.resolve(strict=True)
    output = (args.output or binary.parent / "browser-workspace-court").resolve()
    output.mkdir(parents=True, exist_ok=True)
    counts: dict[str, int] = {}
    proxy_requests: list[str] = []
    proxy_target = "http://preview.minicon.invalid/page"

    class Page(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args: object) -> None:
            pass

        def do_GET(self) -> None:  # noqa: N802
            counts[self.path] = counts.get(self.path, 0) + 1
            if self.path == "/redirect":
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
    endpoint = rf"pipe:\\.\pipe\minicon-browser-{os.getpid()}" if windows else f"unix:{Path(scratch.name).resolve(strict=True) / 'control.sock'}"
    env = os.environ.copy()
    # Reuse this isolated config only across the process restarts in this run.
    env["MINICON_CONFIG_DIR"] = str(Path(scratch.name) / "config")
    env["MINICON_BROWSER_PROFILE_ROOT"] = str(Path(scratch.name) / "profile")
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
        assert rectangle["width"] > 0 and rectangle["height"] > 0, f"{name} has no visible hit target"
        scale = geometry["scale_permille"] / 1000
        x = (rectangle["x"] + rectangle["width"] / 2) / scale
        y = (rectangle["y"] + rectangle["height"] / 2) / scale
        cli("send-ui-mouse", "--action", "click", "--button", "left", "--column", str(round(x)), "--row", str(round(y)))

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

    try:
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
        receipt = {"binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "platform": sys.platform,
                   "physical_page_ctrl_l": args.physical, "physical_page_tab_switch": args.physical,
                   "visible_browser_entry": True, "single_browser_root_reused": True,
                   "service_restart_retry": True, "terminal_draft_preserved": True,
                   "visible_proxy_applied": True, "proxy_clear_restores_direct": True,
                   "invalid_proxy_blocks_direct": True, "proxy_candidates_persisted": True,
                   "proxy_candidate_selected": True, "proxy_candidate_deletion_persisted": True,
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
