# Windows memory research inputs

Owned by the Windows RSS branch. Do not mix macOS Hello/pixel-host files here.

- `plan/research-windows-memory.md` — questions, non-goals, named receipts
- `target/windows-memory/` — cross-linked Windows artifacts (gitignored)
- `run-release-rss.sh` — UTM win-aarch64 RSS against a **release** PE, without
  changing `scripts/windows-utm-runner.sh` (that runner's `rss` mode is debug).
  Null `ExitCode` throws wrapper failure; raw code is recorded. Wrapper shape
  is accepted; do not re-court it.
- `live-owners.md` — split live PTY / tab+vt100 / GDI vs heap residency
- `probe-gdi-objects.ps1` — WS, private bytes, GDI/USER, named PTY threads
- `sample-close-owners.ps1` / `run-close-owners.sh` — four-cycle close sample
  (not an RSS wrapper court; does not cut PTY capacity)
- `probe-ws-regions.ps1` / `sample-idle-regions.ps1` / `run-idle-regions.sh` —
  walk internally closed; PMC−walk is unexplained remainder, not
  proven drift. `cow_protect` vs `privatized_image/mapped` (`Shared=0`).
  Keep unknown alloc-base and truncation. LastError only on failed
  calls. IME stays on.
- `sample-size-compare.ps1` / `run-size-compare.sh` — two client sizes,
  new process each, then same-process shrink. No screenshot.
- `sample-init-phases.ps1` / `run-init-phases.sh` — spawn/hwnd/control/
  first_frame; per-module shared0. IME on. TIF total ≠ all privatized_image.
- `init-hooks/init_trace.rs` + `run-init-hooks.sh` / `sample-init-hooks.ps1`
  — research PE (copy of pin 745f52b under `target/windows-memory/`,
  not production pin / not `target/font-platform-fix`). In-process
  CreateWindow stages, foreground HWND/focus. Skip-Focus delay is not
  idle savings (accepted). Next: removable post-activate loads
  (`SHGetFolderPathW`, font `select_primary`, Gdiplus import). IME on.
  Chinese IME compose is BLOCKED unless a zh layout is present.
