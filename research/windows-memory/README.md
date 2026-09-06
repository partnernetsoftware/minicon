# Windows memory research inputs

Owned by the Windows RSS branch. Do not mix macOS Hello/pixel-host files here.

- `plan/research-windows-memory.md` — questions, non-goals, named receipts
- `target/windows-memory/` — cross-linked Windows artifacts (gitignored)
- `run-release-rss.sh` — UTM win-aarch64 RSS against a **release** PE, without
  changing `scripts/windows-utm-runner.sh` (that runner's `rss` mode is debug).
  Null `ExitCode` throws wrapper failure; raw code is recorded. Wrapper shape
  is accepted; do not re-court it.
- `live-owners.md` — top-5 GDI/native idle owners and falsifiable probes
- `probe-gdi-objects.ps1` — guest WS/GDI/USER/handle sample for one PID
