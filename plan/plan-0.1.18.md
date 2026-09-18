# Plan 0.1.18 — unblock Windows mouse, give the greeting page an exit

Status: **planning.** Two items block a user right now; everything else is
carried debt. Ordered so the two blockers land first and the risky refactor does
not sit in front of them.

## P0-1 — Mouse does not work in Windows TUIs

**What is known** (do not re-derive):

- macOS works, confirmed twice: by the owner using grok, and by probe —
  `send-mouse` makes the child receive `ESC[<0;11;6M` / `…m`. So MiniCon's mouse
  *mode detection, SGR encoding and routing are sound*.
- Therefore the fault is in a Windows-only layer: **ConPTY**, or the native
  window layer (macOS uses winit).
- ConPTY can break it in **either direction**, and they need different fixes:
  - **outbound**: the child's `ESC[?1000h/?1006h` never reaches MiniCon, so
    MiniCon never believes the child wants the mouse and forwards nothing;
  - **inbound**: MiniCon forwards SGR reports but they never reach the child.

**First step is diagnosis, not code.** A PowerShell probe is ready
(`diagnose-terminal-io` skill, `references/probes.md`). Distinguishing trick,
no new debug surface needed: after an injected click, read the snapshot's
`selection` —

| probe sees mouse bytes | `selection` after click | meaning |
| --- | --- | --- |
| yes | empty | works; look elsewhere (real mouse events in the native layer) |
| no | empty | MiniCon *did* forward → **inbound** ConPTY break |
| no | non-empty | MiniCon never saw the DECSET → **outbound** ConPTY break |

Only after that verdict do we choose a fix — and if it proves to be a ConPTY
limitation MiniCon cannot route around, say so plainly and document it rather
than shipping a workaround that pretends otherwise.

## P0-2 — `[Quit MiniCon]` on the greeting page

With no tabs, `ConApp::event` returns early for an empty workspace, so
`CloseRequested` never reaches the handler in `ConTerminal::event`: **the window
cannot be closed and the only way out is killing the process.** The greeting page
must offer an explicit exit.

- Add the button beside `[New Terminal]` in `ui::Layout` (`empty_new_terminal`
  gains a sibling), with hit-testing and paint.
- Invariant tests in the existing style: the two buttons never overlap, both
  stay inside the window, and both remain hittable across the scale/size sweep.
- **Open decision for the owner:** also restore plain window close when empty?
  The "prevent a misclick" argument is weakest precisely here — with no sessions
  open there is nothing to lose. Recommendation: fix both (working `X` *and* an
  explicit button). Alternative, if deliberate: keep `X` inert so quitting is
  always an explicit act.

## P1 — Carried from 0.1.17

- `capture-pane --scrollback N`. Deferred because the semantics were not settled:
  "the last N lines including what scrolled off" needs cross-screen stitching via
  `set_scrollback`, and the viewport must be restored so the user sees no jump.
  Decide the semantics first, then implement.
- Black-box test fixing the Send behaviour: the unit test pins the *bytes*, but
  nothing yet pins that the paste and the Enter arrive in **two separate
  `read()`s** — the part that actually regressed. Needs a probe child, so gate it
  on `#[cfg(unix)]` plus a python3 check.

## P2 — Carried from 0.1.16

- **Theme A, code clarity**: split `src/main.rs` (~8.9k lines) into
  `cli.rs` / `app.rs` / `terminal.rs`; extract `paint_host_ui` and
  `build_ui_snapshot`. Pure refactor, now covered by the invariant suite. Kept
  behind the blockers on purpose: it is the largest-risk, lowest-user-value item.
- **Theme B, interactive desktop court**: the session-0 Windows court presents no
  frames, so real mouse and pixel tests are impossible there. An interactive
  logged-in court would make P0-1's *real* mouse path testable, not just the
  injected one.
- **Theme C, doc debt**: archive the completed plans. (The README signing range
  and the old-Windows diagnostic path were already fixed in 0.1.15/0.1.17.)
