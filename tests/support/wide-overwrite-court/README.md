# Wide-overwrite misalignment court

The distilled reproduction and boundary-attribution assets for the owner's
2026-10-21 file-tool misalignment report. The record and verdict are owned by
[PRD 24.01](../../../prd/PRD_02_24_01_tui_compatibility.md) ("Open
compatibility contracts", attribution of 2026-10-22); this directory only
holds the runnable pieces.

- `fixture.ps1` — child-side probe: sets UTF-8 code pages + VT mode, then
  writes ambiguous/wide-character runs and the wide-lead overwrite pattern.
- `run-court.ps1 -Backend <agent|conpty|rust-console>` — drives a debug
  MiniCon GUI over a control pipe, waits for the sentinel, captures
  `capture-pane` (parser state) and `screenshot-pane` (pixels). The
  rust-console backend needs a build with `--features
  experimental-rust-console`.
- `wire-probe/` — standalone consumer of `minicon-platform::pty` that dumps
  the raw ConPTY wire bytes (`cargo run -- fixture.ps1 out.bin`), plus
  `--bin parse`, which feeds controlled byte sequences straight to the owned
  vt100 parser. Note its mandatory `run_if_console_agent` hook: any binary
  that opens platform PTYs and skips it re-executes itself endlessly when the
  hosting environment carries `AGENTERM_FORCE_CONSOLE_AGENT=1` (which a
  MiniCon tab's children inherit today — see carried debt S5).

The parser-side invariant this court established is pinned in
`third_party/vt100/src/row.rs`
(`overwriting_a_wide_lead_blanks_the_orphan_and_keeps_columns`).
