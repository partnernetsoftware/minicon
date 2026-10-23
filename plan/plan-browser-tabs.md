# Plan: built-in wry browser as first-class tabs (0.2.x overnight design)

Status: partial (2026-10-23). The shell, core hooks and Linux/X11 adapter exist; run `39925651159` is valid Linux evidence only. Platform WebView ownership is selected in [PRD 28](../prd/PRD_02_28_shared_core.md); local canonical macOS arm64 and Windows x86_64 journeys are recorded in [plan-v0.2.21.md](plan-v0.2.21.md). Owner decision 2026-10-23 puts Windows adapter bring-up before the UX proposal. The proposal is now written and awaits owner confirmation; the current engineering shell is not accepted UX. Automated/second-ISA macOS, Windows ARM64 and canonical cross-platform packaging remain open. **Not a release claim.** Browser remains a terminal-tab peer; no tray, system opener or floating helper. No Draft / Latest / signed package / public ship from this leaf.

**Naming (PRD_02_25 Terminology):** **Tab** = work unit; **content panel** (alias workspace panel) = layout cell inside a tab (now 1:1, later 1:N); **chrome panel** = host sent-history / settings / panel toggle. Bare "panel" in this plan means **content panel** unless marked chrome. Do not rename chrome evidence ids.

Renamed from `plan-wry-tray-browser.md` (tray-era title/content retired).

Points into: [PRD_02_31 renderer choice](../prd/PRD_02_31_v0_2_horizon.md)
(reuse the shared WebView security shape / prefer-direct-wry; `{ARCH}` permits
GUI-only platform linkage inside `minicon-web`, while **UX is still same-window
tabs**), and the existing passive
probe in `crates/minicon-platform` `webview` (WebView2 / WKWebView / WebKitGTK
discovery only — no create/navigate yet).

## User outcomes (commissioner 2026-10-23, re-nailed)

```text
built-in browser surface (first-class tab, same bar as terminal tabs)
├── [-] OAuth login flow (core + Linux live path; macOS/Windows surface open)
│         including AI provider OAuth (default: DeepSeek)
├── [_] large-screen dashboard
├── [_] file preview
├── [_] wasm / rich GUI pages
└── [-] terminal → browser: "open in browser" and Ctrl+click link
         MUST open / focus a built-in wry **browser tab**, NEVER the OS
         default browser, NEVER a tray+external-window product surface

composer / input AI assist (future)
├── [_] provider registry: OAuth and/or API key per vendor
├── [v] default provider hook: DeepSeek via OAuth
├── [-] login / consent opens a **browser tab** in the same tab strip
│         (never OS default browser; never tray-driven popup)
└── [_] API key entry stays in MiniCon settings (no browser required);
         OAuth tokens land via bridge from the wry tab

content-panel composition (direction only — not this leaf)
├── [v] **now (default model):** 1 tab ↔ 1 content panel (1:1)
└── [_] **future:** 1 tab → many content panels (split / side-by-side inside one tab)
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
minicon (main UI — tab strip owns lifecycle; GUI-only linkage accepted by {ARCH})
  │
  │  terminal tabs  │  browser tabs   ← same bar, open/close/switch/title
  │       │                │
  │       ▼                ▼
  │     PTY host     browser content panel (product face; PRD_02_25)
  │                        │
  │                        ▼
  │              crates/minicon-web  ← unified browser function interface
  │              (library API + optional host binary / subcommand)
  │                        │
  │                        ▼
  │              wry / system WebView (isolated here only)
  │              (WebView2 / WKWebView / WebKitGTK)
  │
  │ OSC-8 / Ctrl+click / "open browser"
  │   → open or focus a browser tab via minicon-web (never xdg-open /
  │     ShellExecute for http(s); never tray+外窗)
  │
  │ [now]  each tab hosts exactly one content panel (1:1)
  │ [later] one tab may host multiple content panels (split / side-by-side)
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
4. **Content-panel direction (staged; this leaf does not ship a layout engine).**
   - **Now (default):** one tab ↔ one content panel (1:1).
   - **Future:** one tab may hold multiple content panels (split / side-by-side) so
     the capability surface can grow without a second windowing model.
   - Host **chrome panels** (sent-history / settings) are out of this rule —
     they stay chrome (PRD_02_25).
5. **`minicon-web` owns isolation.** Path `crates/minicon-web` remains the
   only browser API/implementation boundary. `minicon-core`, `minicon-console`
   and headless paths stay WebView-free; the GUI binary may feature-link only
   that adapter, as selected by `{ARCH}`
   ([PRD_02_31](../prd/PRD_02_31_v0_2_horizon.md)).
   The host binary is **`minicon-web-host`** (crate bin); `MINICON_BROWSER_BIN`
   remains an accepted stand-in path. Product face stays **tab /
   content panel**, not tray+外窗. Main process owns the tab shell;
   wry nests or bridges only through minicon-web.
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


## minicon-web — boundary and target interface (2026-10-23)

Commissioner proposed; machine-room accepted: encapsulate the browser surface
as **`minicon-web`** so wry/WebView can change underneath one stable function
interface. **Product names stay PRD_02_25** (Tab / content panel / chrome
panel; c48a6a0). This crate is **not** tray, not chrome history/settings, and
not a second windowing model.

| Layer | Owns | Does not own |
| --- | --- | --- |
| `src/` workspace + `src/browser.rs` | Tab strip, `TabKind::Browser`, Ctrl+click wiring. `browser.rs` re-exports `minicon-web` | wry create/navigate; tray |
| **`crates/minicon-web`** | `PageHost`, panel state, navigation, proxy hook and the selected same-window adapter | Tab tree; chrome `history_panel` / `settings_panel`; OS browser for http(s); WebView calls outside this crate |
| `minicon-platform` `webview` feature | Passive `probe_system_webview()` only | create / navigate / tray |

**Crate status (align with `0:grk` WIP):** `crates/minicon-web/Cargo.toml`
declares lib + bin `minicon-web-host`. Public API in `src/lib.rs`:
`BrowserPanel` / `PageLoad` / `BrowserTab` / `BrowserProxy*` /
`resolve_proxy` / `http_url_at` / `page_host_request` / `spawn_page_host` /
`PageHost` / `ProcessPageHost` / `host_reply`. `src/browser.rs` is a
re-export adapter. Host argv: `--probe` | `--load <http-url>` | `--smoke`
(exit 69 = Unavailable when `wry-host` is off; never OS browser). Feature
`wry-host` links wry/tao into the Linux helper; GUI-only native feature
ownership is mapped in [PRD 28](../prd/PRD_02_28_shared_core.md). The crate is a
workspace member and a dependency of the MiniCon GUI composition. Do not invent a parallel API in
`src/` that bypasses minicon-web.

### Target function interface (library)

Shapes already exercised in `src/browser.rs` are the product-side contract the
crate must satisfy or absorb; names may move into `minicon_web::` without
changing meaning.

| API / capability | Intent | Tip code today | Stage |
| --- | --- | --- | --- |
| `open_url` / load URL | Open or navigate http(s) in the built-in surface; never `host_open` / xdg-open / ShellExecute for terminal-originated http(s) | `PageHost::load` / `ProcessPageHost`; `page_host_request` → `--load <url>`; `spawn_page_host`; `BrowserTab::show_http` + `PageLoad` | B1 shell [v]; Bw cross-platform load [-] |
| `http_url_at` | Recognize http(s) tokens for Ctrl+click; non-http → no open | `minicon_web::http_url_at` (re-exported from `src/browser.rs`) | [v] |
| Proxy hook | Read `browser.http_proxy` / `https_proxy` / `no_proxy` + `MINICON_BROWSER_*` (env wins); pass them to the page host | `resolve_proxy`; Linux and Windows x86_64 live receipts are in plan-v0.2.21; macOS native proxy receipt remains open | B2 [-] |
| Host/artifact contract | Canonical dev/release output can navigate without manual feature flags or `MINICON_BROWSER_BIN`; reject system opener names; missing runtime → typed Unavailable | Local canonical macOS arm64 and Windows x86_64 dev receipts are in plan-v0.2.21; release/cross-platform inventory remains open | Bw [-] |
| `PageHost` trait | Swappable surface: process host today; in-process WebView later without new product traits | `PageHost` / `ProcessPageHost` in `crates/minicon-web/src/lib.rs` | [v] |
| OAuth bridge | Provider authorize URL in a browser **tab**; redirect allowlist; origin-bound, size-capped token hand-back | Core mechanism and Linux adapter evidence exist; macOS/Windows page-surface evidence is open | B3 [-] |
| Dashboard / file preview / wasm pages | First-party packaged origins under `assets/` / `include_bytes!`; deny-by-default foreign navigation | later `PageHost` callers | B4 [_] |
| Runtime probe | Soft-fail when system WebView absent | host `--probe` → `host_reply`; also `minicon_platform::webview::probe_system_webview` (passive) | [v] |

### Hard non-goals for the crate

- System tray entry or tray-driven show/hide/quit.
- Linking a WebView adapter into `minicon-core`, `minicon-console` or headless paths; GUI-binary linkage is decided only by `{ARCH}`.
- Renaming chrome evidence ids (`history_panel`, `panel_toggle`, …).
- Claiming Draft / Latest / signed package / public ship from this leaf.

## Leaf split (implementation order when unblocked)

```text
[-] B0 product surface: browser tab is a root peer of a terminal tab (Ctrl+Shift+O;
       list-tabs kind=browser). One content panel. No tray. A second
       Ctrl+Shift+O must focus that tab, not insert another (0.2.21 {PANEL}:
       second add_browser_root is None; content_column_is_the_terminal_rectangle.
       Run 39925651159 proves exact X child resize on Linux; direct macOS
       arm64 Computer Use proves the child WKWebView in the same column,
       hide/restore, focus release and fullscreen resize. Automated/second-ISA
       macOS and Windows ARM64 surface evidence remain open; Windows x86_64
       canonical journeys are recorded in plan-v0.2.21)
[-] B0-UX accepted workspace: Windows adapter bring-up precedes the proposal
       by owner decision 2026-10-23. The proposal in plan-v0.2.21 awaits owner
       confirmation before a large redesign. Acceptance covers open, real URL
       navigation, tab switch/return, resize/fullscreen and close/reopen, with
       legible hierarchy, focus and state. A visible native child alone is not
       evidence. Owner: plan-v0.2.21 {UX-BASELINE}.
[v] B1 URL handoff: Ctrl+click on an http(s) token opens or focuses the
       one browser content panel and stores the canonical URL. A non-http
       token does not change the panel. That path does not call
       host_open. Non-http is ignored.
       Evidence: `http_handoff_reuses_one_browser_content_panel`
[-] Bw page load: the content panel asks **minicon-web** (or today's
       `MINICON_BROWSER_BIN` stand-in) `--load URL`. Core crates/headless do
       not link wry/tao. A missing host or system opener is Unavailable, not
       `host_open`. The same child carries the proxy hook into the WebView. Evidence:
       `page_host_request_is_the_helper_not_the_system_browser`,
       `spawn_page_host_passes_the_url_to_the_helper`
[-] B2 proxy hook: browser.http_proxy / browser.https_proxy /
       browser.no_proxy plus MINICON_BROWSER_* (env wins). Shown on the
       shell and installed on the page-host process at `--load`.
       `webkit_proxy_env` maps that hook onto `http_proxy` / `https_proxy` /
       `no_proxy` for a `wry-host` build. Absent fields are removed
       (`webkit_proxy_clear`, including `HTTP_PROXY` / `ALL_PROXY`) so a parent
       proxy cannot leak. Exit 69 stays
       Unavailable.
       Evidence: `spawn_page_host_installs_the_proxy_hook_on_the_child`,
       `exit_69_is_unavailable_and_proxy_maps_onto_webkit_names`
       Native Windows settings are owned in PRD 25, with local proxy and
       direct-bypass receipts in plan-v0.2.21.
[-] B3 OAuth vertical: authorize URL is https and loads with `--load` on
       the one content panel. Redirect host must be allowlisted. No OS
       browser. DeepSeek uses this hook.
       Evidence: `deepseek_oauth_authorize_loads_in_the_content_panel_only`
       Tip `be99a86` also exchanges the code. The page host GETs an http
       loopback redirect itself (`deliver_loopback_get`) because wry's
       ignore list is empty. An existing bearer file is forced to `0600`
       and a symlink is refused. The bridge requires `state`, sends an S256
       challenge, and keeps listening after a wrong request. Bearer storage
       goes through minicon-platform's protected private-directory and atomic
       file seam; Unix 0600/no-follow evidence is local. Run 39919403513 proves
       the Windows ACL on both ISAs; run 39925651159 proves real WebKit
       proxy/direct-loopback navigation on both Linux ISAs.
[_] B4 dashboard + file preview + wasm host pages (assets/, include_bytes!)
[v] B5 upsert PRD_02_3x owners with the current scope and platform evidence boundaries
[_] Bx later (not this leaf): content-panel layout engine — still 1:1
       tab↔content panel by default; then multi-content-panel inside one tab
[-] Bp portable platform closure: choose the same-window host boundary;
       canonical dev/release artifacts; macOS WKWebView visible journey;
       Windows WebView2 implementation or an explicit narrowed platform claim.
       Owner: plan-v0.2.21 {ARCH}/{ARTIFACT}/{MAC-PANEL}/{WIN-PANEL}.
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
[-] B3a DeepSeek OAuth starts as an https authorize URL on the content panel
       (`deepseek_oauth_authorize`, Ctrl+Shift+D). Redirect host is allowlisted.
       The page host receives the resolved browser proxy hook. The redirect is
       `127.0.0.1` only (`OAuthBridge`). The code is exchanged at an https
       token URL through that same hook. The access token is the harness
       bearer when `MINICON_DEEPSEEK_API_KEY` is unset. The notice reports
       the byte count. Native adapter evidence is in 0.2.21 `{WEBKIT}`.
[_] B3b provider registry stub + API-key config schema (no UI chrome yet)
[_] B3c composer entry point calls B3a by default when assist is enabled
```

Non-goal overnight: a production-provider credential round trip, storing
production secrets, or enabling assist in the composer UI. The synthetic
native court qualifies the adapter path without claiming a provider account.

## Overnight decision log

| Decision | Choice |
| --- | --- |
| Renderer | **minicon-web** boundary; direct-wry platform adapter; GUI-only linkage accepted by `{ARCH}` while core/headless remain WebView-free (PRD_02_31) |
| Product surface | **browser tabs peer to terminal tabs** (same bar / lifecycle) |
| System tray | **none** — show/hide/quit not tray-driven |
| Content-panel model now | **1 tab ↔ 1 content panel** |
| Content-panel model future | **1 tab → many content panels** (layout engine later; not this leaf) |
| Chrome panels | sent-history / settings — host chrome; not content panels (PRD_02_25) |
| Terminal Ctrl+click / open browser | built-in wry **browser tab** only for http(s) |
| System default browser | not used for terminal-originated http(s) |
| Proxy UI | deferred; config/env hook required in design |
| AI assist default auth | DeepSeek OAuth in wry browser tab |
| AI API key | settings/config path (no wry); schema hook only overnight |
| This doc | partial Linux implementation plus portable-boundary decision — no release |
