# Goal 0.2.21 — Windows browser content panel, then a UX proposal

Hand-off brief for an agent session (owner request 2026-10-23). Owner
decision: Windows is brought up **before** `{UX-BASELINE}`; this overrides the
ordering in [plan-v0.2.21.md](plan-v0.2.21.md), which must be updated to say so.

Goal: bring up the minicon 0.2.21 browser content panel (`minicon-web`) on
Windows, then propose UI/UX improvements.

## Read first

`AGENTS.md`, [plan-v0.2.21.md](plan-v0.2.21.md),
[plan-browser-tabs.md](plan-browser-tabs.md),
`crates/minicon-web/src/native_page.rs` (the macOS adapter, used as the
reference), and [PRD 25](../prd/PRD_02_25_con_workspace.md) for the
Tab / content panel terminology.

## Current state

- A browser tab is a peer of a terminal tab, and each tab has exactly one
  content panel (1:1).
- **macOS:** the minicon-web feature `native-page-host` builds one WKWebView
  as a child of the main window's NSView, using wry `build_as_child`.
- **Linux/X11:** a separate `minicon-web-host` helper, reparented into the
  main window.
- **Windows: not implemented.**
  - The root `Cargo.toml` Windows target does not enable `native-page-host`
    on minicon-web.
  - `PixelWindow::embed_parent()` returns no parent window on Windows.

## Tasks (in order)

1. **Implement the Windows adapter.**
   - Make `embed_parent()` in `native_pixel_window` return the main window's
     HWND.
   - Add a Win32 parent (`RawWindowHandle::Win32`) to `native_page.rs` and
     build the WebView2 child with wry `build_as_child`.
   - Enable `native-page-host` for the Windows target.

   The result must meet these requirements:
   - The child sits exactly in `Layout::content_column`.
   - It follows window resize, sidebar and history-panel toggles, and DPI
     changes.
   - Switching to a terminal tab hides the child and gives focus back to the
     main window; otherwise the host's Ctrl+Shift shortcuts are swallowed.
   - A second Ctrl+Shift+O focuses the existing browser tab instead of
     creating another.
   - Ctrl+click on an http(s) link navigates in the built-in panel and never
     opens the OS browser.
   - The proxy hook (`browser.http_proxy` and its siblings) takes effect.
   - A missing WebView2 runtime is reported as a typed Unavailable, not a
     crash.

2. **Prove it on this machine from canonical `./scripts/build.sh dev`
   output.** Run each journey and record evidence:
   - open the browser tab;
   - navigate a real URL;
   - switch to a terminal tab and back;
   - resize the window and maximize it;
   - close the tab and reopen it.

   Capture evidence with `minicon cli` `screenshot-pane` and `ui-snapshot`.
   WebView2 renders in its own child HWND, so a pane screenshot may not
   contain the page pixels. Check whether it does and record what you find;
   do not assume.

3. **Gates.** All of these must pass:
   - `cargo fmt`
   - `cargo clippy --all-targets -- -D warnings`
   - `./scripts/build.sh test`
   - a build with `--no-default-features`

   Every new behavior needs a test. Prove each test works: break the code it
   guards and watch the test fail before trusting it.

4. **Update the plan.** In [plan-v0.2.21.md](plan-v0.2.21.md):
   - update `{WIN-PANEL}` and `{ARTIFACT}` with status and evidence;
   - record that Windows comes before `{UX-BASELINE}` by owner decision
     (2026-10-23).

   Record only what was measured. Windows ARM64 is BLOCKED unless it has its
   own runtime evidence (for example a `dev-loop-crosscheck` run on the
   `win-aarch64` cell).

5. **UX proposal.** From actual use on Windows and macOS, write a proposal for
   the browser workspace. Cover:
   - address bar, status and back navigation;
   - tab hierarchy;
   - focus;
   - the home page;
   - loading and error states.

   Present the proposal and wait for owner confirmation. Do not start a large
   redesign before that.

## Constraints

- Work directly on `main`; no worktrees or side branches. Stage only reviewed
  paths. End each commit with the attribution line the session requires.
- Do not publish. Do not run `scripts/release.sh`, and do not create a tag or
  a Candidate.
- G4 and G6 are carried debt and outside this goal.
- In public documents, write no real hostnames, emails, credentials or
  expanded home paths. Use repo-relative paths.
- Talk with the owner in Chinese. Code and documents stay in English.
