# Plan: built-in wry browser + system tray (0.2.x overnight design)

Status: design + spike outline only (2026-10-23 overnight). **Not a release
claim.** No Draft / Latest / signed package / public ship from this leaf.

Points into: [PRD_02_31 renderer choice](../prd/PRD_02_31_v0_2_horizon.md)
(reuse agenterm-platform webview / prefer-direct-wry; **do not** link `wry`/`tao`
into core `minicon`; separate subcommand/process), and the existing passive
probe in `crates/minicon-platform` `webview` (WebView2 / WKWebView / WebKitGTK
discovery only — no create/navigate yet).

## User outcomes (commissioner 2026-10-23)

```text
built-in browser surface
├── [ ] OAuth login flow (must honor HTTP proxy settings)
├── [ ] large-screen dashboard
├── [ ] file preview
├── [ ] wasm / rich GUI pages
├── [ ] system tray entry (show/hide, quit, open dashboard)
└── [ ] terminal → browser: "open in browser" and Ctrl+click link
         MUST open this built-in wry host, NEVER the OS default browser
```

HTTP proxy **UI** placement is deferred (commissioner: later). Design must
still leave a **settings hook** so OAuth and page loads read proxy from one
named config path when the UI lands.

## Architecture (decided shape)

```text
minicon (core GUI / PTY)          separate process, not linked into core
  │ OSC-8 / Ctrl+click /          ┌─────────────────────────────────────┐
  │ "open browser" action  ──────►│ minicon-browser (direct-wry host)   │
  │ (never xdg-open/ShellExecute  │  tray + one or more WebView windows │
  │  for http(s) from terminal)   │  custom origin + bridge-v1 caps     │
  └───────────────────────────────│  proxy: from config hook (no UI yet)│
                                  └─────────────────────────────────────┘
                                            │
                                            ▼
                                  system WebView runtime
                                  (WebView2 / WKWebView / WebKitGTK)
```

### Hard rules (from PRD + overnight addenda)

1. **Separate process / subcommand.** Candidate name: `minicon browser` or a
   sibling binary `minicon-browser`. Core `minicon` stays free of wry/tao
   linkage so a missing WebView runtime cannot take down the terminal
   ([PRD_02_31 size boundary](../prd/PRD_02_31_v0_2_horizon.md)).
2. **Prefer direct-wry**, evidence already in
   `~/repos/agenterm/research/agenterm-webview/` (Windows receipt: direct-WRY
   ~520 KiB vs Tauri ~8.4 MiB). Consume shared adapter / bridge-v1 when the
   agenterm-platform pin is ready; until then a MiniCon-local spike may
   vendor the same patterns, not re-litigate Tauri/Electron/CDP (all
   rejected in PRD_02_31).
3. **Terminal link open is product-owned.** Today `host_open` on Linux is
   `xdg-open` (system handler). For http(s) targets originating from the
   terminal (OSC-8 hyperlink Ctrl+click, explicit "open browser", future
   composer link actions), MiniCon must spawn/activate `minicon-browser`
   with the URL — **not** call `host_open` / OS default. Non-http schemes
   (mailto, local paths policy TBD) stay out of this leaf's first cut.
4. **Proxy hook (no UI yet).** Single config key, e.g.
   `browser.http_proxy` / `browser.https_proxy` / `browser.no_proxy` under the
   existing minicon config root (or env override `MINICON_BROWSER_HTTP_PROXY`).
   OAuth and ordinary navigations both read it. UI control surface is a
   follow-up; the host must already accept the value so OAuth is not blocked
   behind a settings screen.
5. **Tray.** Show/hide last browser window, Open dashboard, Quit browser
   host. Tray lives in the browser process so core minicon can stay tray-free
   on hosts that never open the browser.
6. **Security baseline** (from agenterm-webview): packaged/custom origin for
   first-party pages (dashboard, file preview shell, wasm host page);
   deny-by-default navigation/CSP for unknown origins; message cap / origin
   binding for any bridge; OAuth may need a tightly allowlisted redirect
   origin — record the allowlist in config, not in code comments only.

## Leaf split (implementation order when unblocked)

```text
[_ ] B0 spike: empty direct-wry window + tray on Linux X11/Wayland
       evidence: process starts, tray icon visible, quit works; no OAuth
[_ ] B1 URL handoff: core minicon Ctrl+click / open-browser → IPC/argv to
       browser process; prove http URL never hits xdg-open in that path
[_ ] B2 proxy hook: config/env → wry/WebKit network session; curl-equivalent
       check through corporate proxy fixture
[_ ] B3 OAuth vertical: one provider login page + redirect allowlist +
       token hand-back to core via bridge (no token in tray menu text)
[_ ] B4 dashboard + file preview + wasm host pages (assets/, include_bytes!)
[_ ] B5 upsert PRD_02_3x when streaming/workbench blockers clear
```

Non-goals for this overnight leaf: shipping a signed browser package,
replacing the terminal GUI with HTML, bundling Chromium/Electron, or claiming
three-platform runtime evidence before B0 receipts exist.

## Spike notes (overnight)

- Code already has **passive** `probe_system_webview()` on Linux (WebKitGTK
  sonames). Spike should fail soft with a clear status if probe is
  `Unavailable`, matching agenterm-webview's launcher pattern.
- Wire point in core: any future OSC-8 / hyperlink click handler and any
  "Open in browser" command must call a new `browser::open_url(&str)` owned
  by MiniCon that targets the wry host; keep `host_open` for true
  system-handler cases (update archive download folder, etc.).
- Do not start renderer feature work that blocks Ctrl+B / Linux qualify
  fixes; those remain higher priority for landable commits.

## Overnight decision log

| Decision | Choice |
| --- | --- |
| Renderer | direct-wry via separate process (PRD_02_31) |
| Terminal Ctrl+click / open browser | built-in wry host only for http(s) |
| System default browser | not used for terminal-originated http(s) |
| Proxy UI | deferred; config/env hook required in design |
| Tray | browser process |
| This doc | design only — no release |

