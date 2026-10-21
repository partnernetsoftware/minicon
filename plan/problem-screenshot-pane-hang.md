# Problem: `screenshot-pane` writes its PNG but the command does not return

Status: [-] open — a reproduction and root cause are not yet established.
The 0.2.18 control owner now bounds an accepted screenshot to 30 seconds and
releases its queue slot on timeout, with failing-first unit tests for an
unrendered frame and a stalled PNG worker. This prevents an indefinite server
queue hold; it does not prove the original PNG-written hang is resolved.
Written 2026-10-11 from an investigation session on this Windows host
(HEAD `9752fdd`); recovery added 2026-10-21.

## Symptom

On Windows, running

```
minicon cli --control <pipe> screenshot-pane --output <path>
```

was observed to **write the PNG file but never return**; it had to be ended by an
external `timeout` (exit 124). One earlier call of the same kind returned normally,
so the failure looked intermittent.

## Reproduction

- **Not reproducible against a healthy GUI.** ~250+ requests across sequential,
  new-tab-then-shot, `send-ui-keys Ctrl+Shift+T`-then-shot, background-target shot,
  10-way concurrent, heavy-output, 80-call mixed-op, and sustained-PTY-flood runs
  all returned in <1 s with rc=0. **0 hangs.**
- No sequence or precondition has yet been found that triggers it reliably.

## Raw observations (facts, not conclusions)

1. On a hung call the command needed `timeout`; a PNG file was present at the
   output path (possibly left by an earlier successful call to the same path — not
   established either way).
2. The server-side wait is `GUI_RESPONSE_TIMEOUT = 125s` (`src/control.rs:25`; the
   wait is at `:1347`). After it elapses the client reports
   `terminal GUI did not respond before timeout`.
3. `ui-snapshot` exposes `pending_control_screenshots` (`src/control_dispatch.rs:300`),
   usable as a read-only probe for a stuck request.
4. Platform layer: when the window is minimized or its client area is not drawable,
   `paint()` returns early without calling `application.render()`
   (`crates/minicon-platform/.../native_pixel_window.rs`; `paint_should_skip` defined
   2161-2163, call sites 1877-1879). `render()` is where the screenshot queue is
   consumed and replied to (`src/main.rs:3213/3221`). **Whether this relates to the
   observed symptom is unverified** — that path does not write a file.
5. Environment factors, all unverified as causes:
   - Another session was actively editing the working tree at the time
     (`src/main.rs`, `src/host_paint.rs`, `src/host_ui.rs`, `src/ui.rs`,
     `src/workspace.rs`, all uncommitted).
   - Build/cleanup steps run `taskkill /F /IM minicon.exe`, which kills any running
     GUI; the sandbox also kills background GUI processes after a while (a GUI was
     observed to disappear mid-session, after which connecting reported
     "the system cannot find the file" for the pipe).

## Code coordinates (for navigation, not a claim)

- Screenshot lifecycle: `src/control_pending.rs` — `enqueue_screenshot:139`,
  `prepare_screenshot:161`, `take_screenshot:169`, `start_screenshot:173`,
  `reap_finished_screenshot:186`, `cancel_for_tab:196`.
- Dispatch / queue: `src/control_dispatch.rs:535` (ScreenshotPane), `:630`
  `drain_control` (reap at `:631`).
- Render consumes + replies: `src/main.rs:3113` prepare, `:3221` take, `:3231`
  start, `:3242` completion callback; forced redraw for a pending screenshot at
  `:2672` and `:3345`.
- Client wait / timeout: `src/control.rs:1282` `run_cli_exchange`, `:1336-1348`
  `serve_one`, `:25` timeout constant.
- PNG worker (`sync_channel(1)`; the completion callback sends the reply):
  `src/agent_interface.rs:262-353`.

## Open questions

1. How is the hang triggered reliably? What sequence / precondition?
2. Is the stall server-side (no reply sent) or client-side (reply not returned)?
   Who actually wrote the file — the hung call, or an earlier one to the same path?
3. Is it related to "window minimized / not drawable → `render()` skipped"?
4. Is it related to the concurrent editing session, the `taskkill` of the GUI, or
   the sandbox killing the background process?

## Cautions for whoever picks this up

- The working tree may still contain another session's uncommitted changes; any
  build mixes in that code, and build/cleanup `taskkill`s any running GUI. Reconcile
  before treating an observed failure as a product defect.
- Keep the symptom fixed until a reproduction exists: a "fix" chosen here must be
  falsifiable by a test that would fail without it.
