# Plan: built-in wry browser as first-class tabs (0.2.x overnight design)

Status: design + spike outline only (2026-10-23 overnight; **commissioner re-nail 2026-10-23**: no tray; browser = terminal tab peer; panel composition later). **Not a release claim.** No Draft / Latest / signed package / public ship from this leaf.

Renamed from `plan-wry-tray-browser.md` (tray-era title/content retired).

Points into: [PRD_02_31 renderer choice](../prd/PRD_02_31_v0_2_horizon.md)
(reuse agenterm-platform webview / prefer-direct-wry; **do not** link `wry`/`tao`
into core `minicon`; separate subcommand/process OK for isolation, but **UX is
same-window tabs** IPC'd back to the main UI), and the existing passive
probe in `crates/minicon-platform` `webview` (WebView2 / WKWebView / WebKitGTK
discovery only — no create/navigate yet).

## User outcomes (commissioner 2026-10-23, re-nailed)

```text
built-in browser surface (first-class tab, same bar as terminal tabs)
├── [ ] OAuth login flow (must honor HTTP proxy settings)
│         including AI provider OAuth (default: DeepSeek)
├── [ ] large-screen dashboard
├── [ ] file preview
├── [ ] wasm / rich GUI pages
└── [ ] terminal → browser: "open in browser" and Ctrl+click link
         MUST open / focus a built-in wry **browser tab**, NEVER the OS
         default browser, NEVER a tray+external-window product surface

composer / input AI assist (future)
├── [ ] provider registry: OAuth and/or API key per vendor
├── [ ] default provider: DeepSeek via OAuth
├── [ ] login / consent opens a **browser tab** in the same tab strip
│         (never OS default browser; never tray-driven popup)
└── [ ] API key entry stays in MiniCon settings (no browser required);
         OAuth tokens land via bridge from the wry tab

panel composition (direction only — not this leaf)
├── [ ] **now (default model):** 1 tab ↔ 1 panel (tab and panel are 1:1)
└── [ ] **future:** 1 tab → many panels (split / side-by-side inside one tab)
```

**废 tray（硬规则）：** 不设系统托盘入口；show / hide / quit **不靠托盘**。
窗口生命周期跟主 UI / tab 栏走，不是 tray 宿主进程外挂窗。

HTTP proxy **UI** placement is deferred (commissioner: later). Design must
still leave a **settings hook** so OAuth and page loads read proxy from one
named config path when the UI lands.

## Architecture (decided shape)

Product surface is **tabs in the main MiniCon window**, not tray + floating
WebView windows. Process isolation may still use a sibling binary / subcommand
for wry; the main process owns the tab chrome and bridges content back.

```text
minicon (main UI — tab strip owns lifecycle)
  │
  │  terminal tabs  │  browser tabs   ← same bar, open/close/switch/title
  │       │                │
  │       ▼                ▼
  │     PTY host     wry surface (embed or child-process bridge)
  │
  │ OSC-8 / Ctrl+click / "open browser"
  │   → open or focus a browser tab (never xdg-open / ShellExecute for http(s))
  │
  │ [now]  each tab hosts exactly one panel (1:1)
  │ [later] one tab may host multiple panels (split / side-by-side)
  │
  └─ optional: minicon-browser helper process (direct-wry, not linked into core)
       IPC / embed bridge back into the tab shell — UX is tab, not tray+外窗
                    │
                    ▼
             system WebView runtime
             (WebView2 / WKWebView / WebKitGTK)
```

### Hard rules (from PRD + commissioner 2026-10-23 re-nail)

1. **No system tray.** Do not add a tray entry for show/hide/quit/open
   dashboard. Retire any tray checklist / architecture boxes from the
   tray-era draft. Lifecycle is tab + main window.
2. **Browser tab = terminal tab (first-class peer).** Same tab bar and
   lifecycle (open, close, switch, title). Not an bolted-on independent
   window driven by tray.
3. **Ctrl+click / open link / OAuth login** still open **built-in wry**
   (never the OS default browser). Landing point is **open or focus a
   browser tab**, symmetric with opening a terminal tab.
4. **Panel direction (staged; this leaf does not ship a panel layout engine).**
   - **Now (default):** one tab ↔ one panel (1:1).
   - **Future:** one tab may hold multiple panels (split / side-by-side) so
     the capability surface can grow without a second windowing model.
5. **Separate process / subcommand OK for isolation.** Candidate name:
   `minicon browser` or sibling `minicon-browser`. Core `minicon` stays free
   of wry/tao linkage so a missing WebView runtime cannot take down the
   terminal ([PRD_02_31 size boundary](../prd/PRD_02_31_v0_2_horizon.md)).
   **If "independent process" vs "same-window tab" appears to conflict:
   recommend main process owns the tab shell; wry may nest/bridge from a
   child process. Product face is tab, not tray+外窗.**
6. **Prefer direct-wry**, evidence already in
   `~/repos/agenterm/research/agenterm-webview/` (Windows receipt: direct-WRY
   ~520 KiB vs Tauri ~8.4 MiB). Consume shared adapter / bridge-v1 when the
   agenterm-platform pin is ready; until then a MiniCon-local spike may
   vendor the same patterns, not re-litigate Tauri/Electron/CDP (all
   rejected in PRD_02_31).
7. **Terminal link open is product-owned.** Today `host_open` on Linux is
   `xdg-open` (system handler). For http(s) targets originating from the
   terminal (OSC-8 hyperlink Ctrl+click, explicit "open browser", future
   composer link actions), MiniCon must open/focus a browser tab via the
   wry host — **not** call `host_open` / OS default. Non-http schemes
   (mailto, local paths policy TBD) stay out of this leaf's first cut.
8. **Proxy hook (no UI yet).** Single config key, e.g.
   `browser.http_proxy` / `browser.https_proxy` / `browser.no_proxy` under the
   existing minicon config root (or env override `MINICON_BROWSER_HTTP_PROXY`).
   OAuth and ordinary navigations both read it. UI control surface is a
   follow-up; the host must already accept the value so OAuth is not blocked
   behind a settings screen.
9. **Security baseline** (from agenterm-webview): packaged/custom origin for
   first-party pages (dashboard, file preview shell, wasm host page);
   deny-by-default navigation/CSP for unknown origins; message cap / origin
   binding for any bridge; OAuth may need a tightly allowlisted redirect
   origin — record the allowlist in config, not in code comments only.

## Leaf split (implementation order when unblocked)

```text
[_ ] B0 spike: empty direct-wry surface hosted as a browser tab in the main
       tab strip (Linux X11/Wayland); evidence: tab open/close/switch/title;
       no tray; no OAuth
[_ ] B1 URL handoff: core minicon Ctrl+click / open-browser → open or focus
       a browser tab (IPC/argv to wry helper if used); prove http URL never
       hits xdg-open in that path
[_ ] B2 proxy hook: config/env → wry/WebKit network session; curl-equivalent
       check through corporate proxy fixture
[_ ] B3 OAuth vertical: one provider login page + redirect allowlist +
       token hand-back to core via bridge (no token in UI chrome labels)
[_ ] B4 dashboard + file preview + wasm host pages (assets/, include_bytes!)
[_ ] B5 upsert PRD_02_3x when streaming/workbench blockers clear
[_ ] Bx later (not this leaf): panel layout engine — still 1:1 tab↔panel by
       default; then multi-panel inside one tab
```

Non-goals for this overnight leaf: system tray of any kind, shipping a signed
browser package, replacing the terminal GUI with HTML, bundling
Chromium/Electron, implementing the multi-panel layout engine, or claiming
three-platform runtime evidence before B0 receipts exist.

## Spike notes (overnight)

- Code already has **passive** `probe_system_webview()` on Linux (WebKitGTK
  sonames). Spike should fail soft with a clear status if probe is
  `Unavailable`, matching agenterm-webview's launcher pattern.
- Wire point in core: any future OSC-8 / hyperlink click handler and any
  "Open in browser" command must call a new `browser::open_url(&str)` owned
  by MiniCon that opens/focuses a browser tab on the wry host; keep
  `host_open` for true system-handler cases (update archive download folder,
  etc.).
- Do not start renderer feature work that blocks Ctrl+B / Linux qualify
  fixes; those remain higher priority for landable commits.

## AI provider auth (composer assist) — commissioner 2026-10-23

The composer / input box is expected to grow AI assist. Auth is part of the
browser leaf because **OAuth must run inside the built-in wry host**, not the
system browser (same rule as terminal Ctrl+click).

```text
composer "Connect AI" / first-use assist
  → pick provider (default DeepSeek)
  → if OAuth: browser tab → provider authorize URL
       → redirect allowlist → bridge returns code/token to core
  → if API key: MiniCon settings field only (no wry tab)
  → both paths honor browser.http_proxy hook when set
```

### Defaults and hooks

| Item | Choice |
| --- | --- |
| Default provider | **DeepSeek OAuth** |
| Other providers | registry entries (OAuth and/or API key); not overnight scope |
| OAuth UI surface | **browser tab** in the main tab strip (same lifecycle as terminal tabs) |
| API key UI surface | MiniCon settings (later); stored under config, never in ephemeral chrome |
| Proxy | same `browser.http_proxy` / env hook; UI later |
| Token hand-back | bridge-v1 style message, origin-bound, size-capped |

### Leaf addendum

```text
[_ ] B3a DeepSeek OAuth vertical on wry browser tab (authorize → redirect → bridge)
[_ ] B3b provider registry stub + API-key config schema (no UI chrome yet)
[_ ] B3c composer entry point calls B3a by default when assist is enabled
```

Non-goal overnight: shipping a working DeepSeek login, storing production
secrets, or enabling assist in the composer UI. Design + hooks only.

## Overnight decision log

| Decision | Choice |
| --- | --- |
| Renderer | direct-wry; helper process OK; core does not link wry/tao (PRD_02_31) |
| Product surface | **browser tabs peer to terminal tabs** (same bar / lifecycle) |
| System tray | **none** — show/hide/quit not tray-driven |
| Panel model now | **1 tab ↔ 1 panel** |
| Panel model future | **1 tab → many panels** (layout engine later; not this leaf) |
| Terminal Ctrl+click / open browser | built-in wry **browser tab** only for http(s) |
| System default browser | not used for terminal-originated http(s) |
| Proxy UI | deferred; config/env hook required in design |
| AI assist default auth | DeepSeek OAuth in wry browser tab |
| AI API key | settings/config path (no wry); schema hook only overnight |
| This doc | design only — no release |

