# Windows memory research inputs

Owned by the Windows RSS branch. Do not mix macOS Hello/pixel-host files here.

- `plan/research-windows-memory.md` — questions, non-goals, named receipts
- `target/windows-memory/` — cross-linked Windows artifacts (gitignored)
- `run-release-rss.sh` — UTM win-aarch64 RSS against a **release** PE, without
  changing `scripts/windows-utm-runner.sh` (that runner's `rss` mode is debug).
  Null `ExitCode` is wrapper failure (`throw`), recorded as `ExitCode_raw=NULL`;
  it is never coerced to 0. Wrapper PASS is paused.
