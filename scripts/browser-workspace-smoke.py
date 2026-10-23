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
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Page)
    worker = threading.Thread(target=server.serve_forever, daemon=True)
    worker.start()
    origin = f"http://127.0.0.1:{server.server_port}"
    windows = sys.platform == "win32"
    scratch = tempfile.TemporaryDirectory(prefix="minicon-browser-")
    endpoint = rf"pipe:\\.\pipe\minicon-browser-{os.getpid()}" if windows else f"unix:{Path(scratch.name) / 'control.sock'}"
    env = os.environ.copy()
    env["MINICON_CONFIG_DIR"] = str(output / "config")
    env["WEBVIEW2_USER_DATA_FOLDER"] = str(output / "profile")
    for key in ("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", "MINICON_BROWSER_HTTP_PROXY", "MINICON_BROWSER_HTTPS_PROXY",
                "MINICON_BROWSER_NO_PROXY", "MINICON_BROWSER_BIN"):
        env.pop(key, None)
    shell = ["cmd.exe", "/d", "/k"] if windows else ["/bin/sh"]
    child = None
    native = None
    foreground = None
    pointer = None

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
        while time.monotonic() < deadline:
            assert child is not None and child.poll() is None, "GUI exited before the journey completed"
            last = predicate()
            if last:
                return last
            time.sleep(0.05)
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

    def capture(name: str):
        (output / f"{name}.json").write_text(json.dumps({"tabs": cli("list-tabs"), "ui": cli("ui-snapshot")}, indent=2), encoding="utf-8")
        if native is not None:
            from PIL import ImageGrab
            rectangle = wintypes.RECT()
            native.GetWindowRect(hwnd, ctypes.byref(rectangle))
            ImageGrab.grab(bbox=(rectangle.left, rectangle.top, rectangle.right, rectangle.bottom), all_screens=True).save(output / f"{name}.png")

    try:
        with (output / "host.log").open("wb") as trace:
            child = subprocess.Popen([str(binary), "--no-activate", "--control", endpoint, "-e", *shell],
                                     env=env, stdout=subprocess.DEVNULL, stderr=trace, start_new_session=not windows)
            deadline = time.monotonic() + 20
            while True:
                try:
                    cli("list-tabs")
                    break
                except subprocess.CalledProcessError:
                    assert time.monotonic() < deadline, "control endpoint did not become ready"
                    time.sleep(0.1)
            cli("send-ui-keys", "Ctrl+Shift+O")
            first_id = browser()["id"]
            cli("send-ui-keys", "Ctrl+Shift+O")
            assert browser()["id"] == first_id
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
            cli("send-ui-keys", "Ctrl+Shift+O")
            loaded("/second", "Second page")
            capture("resized")
            with socket.socket() as refused:
                refused.bind(("127.0.0.1", 0))
                failed_url = f"http://127.0.0.1:{refused.getsockname()[1]}/unavailable"
            navigate(failed_url)
            wait(lambda: browser()["browser"]["load"] == "unavailable", "failed connection is explicit")
            assert browser()["browser"]["url"] == failed_url
            assert browser()["browser"]["reason"].startswith("browser-navigation-")
            capture("failure")
            navigate(origin + "/first")
            loaded("/first", "First page")
            cli("close-tab", "--target", str(first_id))
            cli("send-ui-keys", "Ctrl+Shift+O")
            assert browser()["id"] != first_id
            cli("close-window")
            child.wait(timeout=10)
        receipt = {"binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(), "platform": sys.platform,
                   "physical_page_ctrl_l": args.physical, "physical_page_tab_switch": args.physical,
                   "requests": counts, "status": "passed"}
        (output / "receipt.json").write_text(json.dumps(receipt, indent=2) + "\n", encoding="utf-8")
        print(json.dumps(receipt))
        return 0
    finally:
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
        scratch.cleanup()


if __name__ == "__main__":
    raise SystemExit(main())
