# Plan: built-in wry browser as first-class tabs (0.2.x overnight design)

Status: partial horizon, updated 2026-10-24. The native browser tab,
address/navigation workspace and canonical platform baseline shipped in
[0.2.21](../prd/archive/v0.2.21-release-history.md). The owner's explicit
publication instruction released the replacement's UX gate. Product behavior
is owned by [PRD 25](../prd/PRD_02_25_con_workspace.md), adapter ownership by
[PRD 28](../prd/PRD_02_28_shared_core.md), and version-specific sequencing is
preserved in the [archived plan](archive/plan-v0.2.21.md). Proxy/OAuth coverage,
provider integration, dashboard, preview, wasm and 1:N composition retain their
separate open leaves. This horizon does not authorize a future release;
release authority and exact-artifact qualification remain with PRD 27.

**Naming (PRD_02_25 Terminology):** **Tab** = work unit; **content panel** (alias workspace panel) = layout cell inside a tab (now 1:1, later 1:N); **chrome panel** = host sent-history / settings / panel toggle. Bare "panel" in this plan means **content panel** unless marked chrome. Do not rename chrome evidence ids.

Points into: [PRD_02_31 renderer choice](../prd/PRD_02_31_v0_2_horizon.md)
(reuse the shared WebView security shape / prefer-direct-wry; `{ARCH}` permits
GUI-only platform linkage inside `minicon-web`, while **UX is still same-window
tabs**), and the existing passive
probe in `crates/minicon-platform` `webview` (WebView2 / WKWebView / WebKitGTK
discovery only — no create/navigate yet).

## User outcomes (commissioner 2026-10-23, re-nailed)

```text
built-in browser surface (first-class tab, same bar as terminal tabs)
├── [-] OAuth login flow (core + Linux live path; macOS/Windows OAuth vertical unqualified)
│         mechanism only; provider inference grants → PRD 31.01
├── [_] large-screen dashboard
├── [_] file preview
├── [_] wasm / rich GUI pages
└── [v] terminal → browser: "open in browser" and Ctrl+click link
         MUST open / focus a built-in wry **browser tab**, NEVER the OS
         default browser, NEVER a tray+external-window product surface

composer / input AI assist (future)
├── [_] provider registry: OAuth and/or API key per vendor
├── [-] DeepSeek authorize/code-exchange hook exists; actual inference OAuth grant is unqualified → PRD 31.01
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

Browser proxy UI is released in 0.2.22 under
[PRD 25.01](../prd/PRD_02_25_01_browser_preferences.md). Provider account/auth
policy is now owned by [PRD 31.01](../prd/PRD_02_31_01_provider_gateway.md);
the browser bridge here remains a presentation/transport mechanism. Browser
navigation or a synthetic callback is not a model-inference OAuth receipt.

## Architecture (decided shape)

Product surface is **tabs in the main MiniCon window**. The main process owns
tab chrome; selected native adapters and the bounded Linux helper are owned by
[PRD 28](../prd/PRD_02_28_shared_core.md), with canonical artifact composition
owned by [PRD 27](../prd/PRD_02_27_con_delivery.md).

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
7. **Terminal link open is product-owned.** For http(s) targets originating
   from the terminal (OSC-8 hyperlink Ctrl+click, explicit "open browser",
   future composer link actions), MiniCon opens/focuses the browser content
   panel through `minicon-web`. That path never calls `host_open` / OS default.
   Non-http schemes
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

**Crate boundary (implementation inventory owned by PRD 28):** `crates/minicon-web/Cargo.toml`
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
| `open_url` / load URL | Open or navigate http(s) in the built-in surface; never `host_open` / xdg-open / ShellExecute for terminal-originated http(s) | `PageHost::open_url` / `ProcessPageHost`; `page_host_request` → `--load <url>`; `spawn_page_host`; `BrowserTab::show_http` + `PageLoad` | B1 handoff [v]; Bw native baseline [v] |
| `http_url_at` | Recognize http(s) tokens for Ctrl+click; non-http → no open | `minicon_web::http_url_at` (re-exported from `src/browser.rs`) | [v] |
| Proxy hook | Read `browser.http_proxy` / `https_proxy` / `no_proxy` + `MINICON_BROWSER_*` (env wins); pass them to the page host | `resolve_proxy`; scoped receipts are in the [archived plan](archive/plan-v0.2.21.md); macOS and Windows ARM64 proxy runtime evidence remains open | B2 [-] |
| Host/artifact contract | Canonical dev/release output can navigate without manual feature flags or `MINICON_BROWSER_BIN`; reject system opener names; missing runtime → typed Unavailable | Canonical native artifact scope is recorded in the [0.2.21 release history](../prd/archive/v0.2.21-release-history.md) | Bw [v] |
| `PageHost` trait | Process-host API; native adapter inventory is owned by PRD 28 | `PageHost` / `ProcessPageHost` in `crates/minicon-web/src/lib.rs` | [v] |
| OAuth bridge | Provider authorize URL in a browser **tab**; redirect allowlist; origin-bound, size-capped token hand-back | Core mechanism and Linux adapter evidence exist; macOS/Windows provider OAuth vertical remains unqualified | B3 [-] |
| Dashboard / file preview / wasm pages | First-party packaged origins under `assets/` / `include_bytes!`; deny-by-default foreign navigation | later `PageHost` callers | B4 [_] |
| Runtime probe | Soft-fail when system WebView absent | host `--probe` → `host_reply`; also `minicon_platform::webview::probe_system_webview` (passive) | [v] |

### Hard non-goals for the crate

- System tray entry or tray-driven show/hide/quit.
- Linking a WebView adapter into `minicon-core`, `minicon-console` or headless paths; GUI-binary linkage is decided only by `{ARCH}`.
- Renaming chrome evidence ids (`history_panel`, `panel_toggle`, …).
- Claiming Draft / Latest / signed package / public ship from this leaf.

## Leaf split (implementation order when unblocked)

```text
[v] B0 product surface: browser tab is a root peer of a terminal tab;
       Ctrl+Shift+O reuses its stable id and one native content panel.
       Evidence: canonical native browser journeys in the release history above.
       Failure stays local to the browser panel; no tray or OS browser.
[v] B0-UX accepted workspace: address/navigation, shared chrome, state and
       terminal switch/return form the published 0.2.21 baseline owned by PRD 25.
       The owner's final publication instruction released the replacement's
       UX gate; earlier prototype rejection remains in the archived plan.
[v] B1 URL handoff: Ctrl+click on an http(s) token opens or focuses the
       one browser content panel and stores the canonical URL. A non-http
       token does not change the panel. That path does not call
       host_open. Non-http is ignored.
       Evidence: `http_handoff_reuses_one_browser_content_panel`
[v] Bw native page load: the content panel navigates through minicon-web
       from canonical dev/release output without manual feature flags or
       MINICON_BROWSER_BIN. Missing runtime is typed Unavailable and preserves
       terminal access. Adapter/API ownership is in PRD 28; released artifact
       evidence is in the 0.2.21 history above.
[-] B2 proxy hook: configuration and native Windows behavior are owned by
       PRD 25, and shared adapter mechanisms by PRD 28. Scoped Linux and
       Windows x86_64 live proxy/direct-bypass receipts remain in the archived
       0.2.21 plan. macOS and Windows ARM64 proxy modes still require their own
       runtime evidence; the native page baseline does not close them.
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
[v] Bp shipped native platform baseline: canonical artifacts provide a
       same-window page on the six shipped cells. Evidence and exact artifact
       scope belong to the 0.2.21 release history, not this planning inventory.
```

Non-goals for this horizon: system tray, replacing the terminal GUI with HTML,
bundling Chromium/Electron, or declaring later page types and 1:N composition
delivered from the shipped 1:1 browser baseline. macOS physical page-focus
shortcut interception, Windows ARM64 proxy modes and real Windows DPI
transitions retain the qualification limits owned by PRD 25. Basic navigation
and screenshots do not close those event-path or platform-specific courts.

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
       the byte count. Scoped adapter evidence is preserved in the archived
       0.2.21 plan; it does not certify a production-provider account.
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
| This doc | sequencing for the remaining browser horizon; the native baseline shipped in 0.2.21, with evidence owned by its release history |
