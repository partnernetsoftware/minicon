#!/usr/bin/env python3
"""Exercise the real WebKit proxy + OAuth loopback navigation path."""

from __future__ import annotations

import http.server
import os
import subprocess
import sys
import threading
import urllib.parse


class QuietHandler(http.server.BaseHTTPRequestHandler):
    def log_message(self, _format: str, *_args: object) -> None:
        return


def serve(server: http.server.ThreadingHTTPServer) -> threading.Thread:
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    return thread


def main() -> int:
    if len(sys.argv) != 2:
        print("usage: scripts/minicon-web-live-smoke.py HOST", file=sys.stderr)
        return 64
    host = os.path.abspath(sys.argv[1])
    if not os.path.isfile(host):
        print(f"missing minicon-web-host: {host}", file=sys.stderr)
        return 66
    if not os.environ.get("DISPLAY"):
        print("DISPLAY is required for the live WebKit court", file=sys.stderr)
        return 69

    proxy_seen = threading.Event()
    callback_seen = threading.Event()
    proxy_paths: list[str] = []

    class Callback(QuietHandler):
        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
            if self.path != "/minicon/oauth?code=live-code&state=live-state":
                self.send_error(400)
                return
            callback_seen.set()
            body = b"MiniCon received the live OAuth redirect."
            self.send_response(200)
            self.send_header("Content-Type", "text/plain; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    callback = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Callback)
    callback_url = (
        f"http://127.0.0.1:{callback.server_port}"
        "/minicon/oauth?code=live-code&state=live-state"
    )

    class Proxy(QuietHandler):
        def do_GET(self) -> None:  # noqa: N802 - BaseHTTPRequestHandler API
            proxy_paths.append(self.path)
            parsed = urllib.parse.urlsplit(self.path)
            if parsed.hostname != "minicon.invalid" or parsed.path != "/start":
                self.send_error(502)
                return
            proxy_seen.set()
            body = (
                "<!doctype html><meta http-equiv=\"refresh\" "
                f"content=\"0;url={callback_url}\">"
            ).encode("utf-8")
            self.send_response(200)
            self.send_header("Content-Type", "text/html; charset=utf-8")
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

    proxy = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Proxy)
    threads = [serve(callback), serve(proxy)]
    env = os.environ.copy()
    endpoint = f"http://127.0.0.1:{proxy.server_port}"
    env["MINICON_BROWSER_HTTP_PROXY"] = endpoint
    env["MINICON_BROWSER_HTTPS_PROXY"] = endpoint
    env.pop("MINICON_BROWSER_NO_PROXY", None)
    try:
        result = subprocess.run(
            [
                host,
                "--oauth-smoke",
                "http://minicon.invalid/start",
                callback_url,
            ],
            env=env,
            capture_output=True,
            text=True,
            timeout=45,
            check=False,
        )
    except subprocess.TimeoutExpired:
        print("live WebKit OAuth smoke timed out", file=sys.stderr)
        return 1
    finally:
        proxy.shutdown()
        callback.shutdown()
        for thread in threads:
            thread.join(timeout=2)

    if result.returncode != 0:
        print(result.stdout, end="", file=sys.stderr)
        print(result.stderr, end="", file=sys.stderr)
        print(f"host exited {result.returncode}", file=sys.stderr)
        return 1
    if not proxy_seen.is_set():
        print("initial WebKit navigation did not use the configured proxy", file=sys.stderr)
        return 1
    if not callback_seen.is_set():
        print("loopback OAuth redirect was not delivered directly", file=sys.stderr)
        return 1
    if any("/minicon/oauth" in path for path in proxy_paths):
        print("loopback OAuth redirect leaked through the proxy", file=sys.stderr)
        return 1
    print("live WebKit OAuth proxy/loopback smoke passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
