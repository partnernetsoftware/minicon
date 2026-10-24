#!/usr/bin/env python3
"""Native Windows no_proxy court through the public CLI and a real Ctrl+click.

Requires the interactive desktop. Owns and closes one temporary MiniCon window;
restores the foreground window and pointer afterwards. Behavior owner: PRD 25.
"""

from __future__ import annotations

import ctypes
from ctypes import wintypes
import http.server
import json
import os
from pathlib import Path
import subprocess
import sys
import threading
import time


def main() -> int:
    if sys.platform != "win32" or len(sys.argv) != 2:
        print("usage (interactive Windows): minicon-web-windows-bypass-smoke.py MINICON", file=sys.stderr)
        return 64
    binary = Path(sys.argv[1]).resolve(strict=True)
    output = binary.parent / "browser-court" / f"bypass-{os.getpid()}"
    output.mkdir(parents=True, exist_ok=True)
    direct_seen = threading.Event()
    proxy_seen = threading.Event()

    class Handler(http.server.BaseHTTPRequestHandler):
        def log_message(self, *_args: object) -> None:
            pass

        def do_GET(self) -> None:  # noqa: N802
            if self.path == "/bypass-receipt":
                direct_seen.set()
                body = b"MiniCon native proxy bypass receipt"
                self.send_response(200)
            else:
                if "bypass.example.test" in self.path:
                    proxy_seen.set()
                body = b"Proxy refused the request"
                self.send_response(502)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    direct = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    proxy = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Handler)
    threads = []
    for server in (direct, proxy):
        worker = threading.Thread(target=server.serve_forever, daemon=True)
        worker.start()
        threads.append(worker)
    env = os.environ.copy()
    env["MINICON_CONFIG_DIR"] = str(output / "config")
    env["MINICON_BROWSER_HTTP_PROXY"] = f"http://127.0.0.1:{proxy.server_port}"
    env.pop("MINICON_BROWSER_HTTPS_PROXY", None)
    env["MINICON_BROWSER_NO_PROXY"] = "bypass.example.test"
    env["MINICON_BROWSER_PROFILE_ROOT"] = str(output / "profile")
    env.pop("WEBVIEW2_USER_DATA_FOLDER", None)
    env.pop("WEBVIEW2_BROWSER_EXECUTABLE_FOLDER", None)
    env["WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS"] = (
        '--host-resolver-rules="MAP bypass.example.test 127.0.0.1"'
    )
    endpoint = rf"pipe:\\.\pipe\minicon-bypass-{os.getpid()}"
    user = ctypes.WinDLL("user32", use_last_error=True)
    user.GetForegroundWindow.restype = wintypes.HWND
    user.SetForegroundWindow.argtypes = [wintypes.HWND]
    user.ClientToScreen.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.POINT)]
    user.GetWindowThreadProcessId.argtypes = [wintypes.HWND, ctypes.POINTER(wintypes.DWORD)]
    foreground = user.GetForegroundWindow()
    pointer = wintypes.POINT()
    user.GetCursorPos(ctypes.byref(pointer))
    child = None

    def cli(*args: str) -> str:
        return subprocess.check_output(
            [str(binary), "cli", "--control", endpoint, *args],
            text=True, encoding="utf-8", timeout=20,
        )

    try:
        with (output / "host.log").open("wb") as trace:
            child = subprocess.Popen(
                [str(binary), "--no-activate", "--control", endpoint, "-e", "cmd.exe", "/d", "/k"],
                env=env, stdout=subprocess.DEVNULL, stderr=trace,
            )
            deadline = time.monotonic() + 15
            while True:
                try:
                    cli("list-tabs")
                    break
                except subprocess.CalledProcessError:
                    assert child.poll() is None, "GUI exited before becoming ready"
                    assert time.monotonic() < deadline, "GUI control startup timed out"
                    time.sleep(0.05)
            url = f"http://bypass.example.test:{direct.server_port}/bypass-receipt"
            cli("send-text", "--target", "@1", f"echo {url}")
            cli("send-keys", "--target", "@1", "Enter")
            cli("wait-text", "--target", "@1", "--timeout-ms", "10000", url)
            snapshot = json.loads(cli("ui-snapshot"))
            geometry = snapshot["geometry"]
            rows = cli("capture-pane", "--target", "@1").splitlines()
            row, text = next((i, line) for i, line in enumerate(rows) if url in line)
            column = text.index(url) + 8
            point = wintypes.POINT(
                geometry["terminal_viewport"]["x"] + column * geometry["grid"]["cell_w"],
                geometry["terminal_viewport"]["y"] + row * geometry["grid"]["cell_h"]
                + geometry["grid"]["cell_h"] // 2,
            )
            found = []
            callback_type = ctypes.WINFUNCTYPE(wintypes.BOOL, wintypes.HWND, wintypes.LPARAM)

            @callback_type
            def visit(hwnd: int, _data: int) -> bool:
                pid = wintypes.DWORD()
                user.GetWindowThreadProcessId(hwnd, ctypes.byref(pid))
                if pid.value == child.pid:
                    found.append(hwnd)
                return True

            user.EnumWindows(visit, 0)
            assert found, "GUI's native parent HWND was not found"
            hwnd = found[0]
            assert user.SetForegroundWindow(hwnd), "could not activate the isolated test window"
            assert user.ClientToScreen(hwnd, ctypes.byref(point))
            user.SetCursorPos(point.x, point.y)
            user.keybd_event(0x11, 0, 0, 0)
            try:
                time.sleep(0.2)
                user.mouse_event(0x0002, 0, 0, 0, 0)
                user.mouse_event(0x0004, 0, 0, 0, 0)
            finally:
                user.keybd_event(0x11, 0, 0x0002, 0)
            tabs = json.loads(cli("list-tabs"))
            (output / "tabs.json").write_text(json.dumps(tabs, indent=2), encoding="utf-8")
            (output / "ui.json").write_text(cli("ui-snapshot"), encoding="utf-8")
            cli("screenshot-pane", "--target", "@1", "--output", str(output / "shell.png"))
            assert any(tab["kind"] == "browser" and tab["active"] for tab in tabs["tabs"]), tabs
            assert direct_seen.wait(12), "the native page did not reach the direct local server"
            assert not proxy_seen.is_set(), "the bypass URL leaked into the proxy"
            print("native WebView2 no_proxy direct-load smoke passed")
            return 0
    finally:
        if child is not None:
            child.terminate()
            child.wait(timeout=10)
        user.SetForegroundWindow(foreground)
        user.SetCursorPos(pointer.x, pointer.y)
        for server in (direct, proxy):
            server.shutdown()
            server.server_close()
        for worker in threads:
            worker.join(timeout=2)


if __name__ == "__main__":
    raise SystemExit(main())
