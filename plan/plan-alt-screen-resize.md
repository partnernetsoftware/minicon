# Plan: alt-screen resize limitation (Windows console)

Status: **open — platform limitation, not fixable in our code with the standard API.**
Owner: none yet. Found during the v0.2.5 "resize scrambles a hosted TUI" investigation.

## What is broken

A hosted full-screen TUI that uses the **alternate screen buffer** (`ESC[?1049h`)
cannot be resized on this Windows build (Server 2022, build 20348). After a
window resize the child keeps drawing at its old size while the host grid has
already changed, so the frame is drawn in the wrong place (scattered borders).

## Evidence

A P/Invoke probe attached to the running agent's console and called the API
directly, with the alternate screen active (target 118x33, view was 88x24):

- `SetConsoleWindowInfo` to a smaller window returns TRUE but has **no effect**
  (the window stays 88x24).
- `SetConsoleWindowInfo` to a larger window returns FALSE with
  `ERROR_INVALID_PARAMETER` (87).
- Only `SetConsoleScreenBufferSize` succeeds: the buffer grows to 118x33, the
  window does not, so the child ends at **118x24** — columns follow the buffer,
  rows stay frozen. Rows are exactly what a TUI needs.

Same result on the in-box **ConPTY** backend, so this is the console host, not
`crates/minicon-platform/src/adapters/windows/console_agent.rs`.

Control: the same child **without** the alternate screen resizes correctly
(88x24 to 118x33) on both backends. The alternate screen is the only variable.

Real `grok` (Grok Build TUI) reproduces it: after resize the frame stays about
88 wide while the grid is 118x33.

## Workaround (verified)

Run the TUI **without the alternate screen**. `grok --no-alt-screen` resizes
correctly under MiniCon — the whole UI reflows to the new size. grok also
exposes `[terminal] alt_screen` in its config.

## Options not taken

- Loading a newer console host from beside the executable (MiniCon already looks
  for `conpty.dll` next to the binary, with `OpenConsole.exe` beside it): **not
  verified** — no Microsoft-signed pair is present on this machine. Windows
  Terminal ships `OpenConsole.exe` but no `conpty.dll`.
- Routing full-screen TUIs to ConPTY does not help: the limitation is inherited.

## Related change that did land

The console-agent now gives the child a **fixed viewport** (screen buffer equals
the window, anchored at the origin) instead of a 500-row buffer it slid the
window through; the geometry a child sees is owned in one place. That is correct
for programs that do not use the alternate screen and is not the cause here.
