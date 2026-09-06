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
  QWS walk internally closed; PMC WS + UTC before/after QWS/classify;
  unnamed mapped listed by allocation-base with last_error/why.
  IME stays on (Chinese input preserved).
