# Backend cost court

`run-court.ps1 -Backend <agent|conpty|rust-console>` measures, for one
backend on one host: process count and total working set of the GUI plus all
descendants with three tabs, a 20-round input-echo round trip, and orphans
left after a hard kill of the GUI. The rust-console cell needs a debug build
with `--features experimental-rust-console`. The court clears an inherited
`AGENTERM_FORCE_CONSOLE_AGENT` first, so running it from inside a MiniCon tab
still measures the backend it names. Results and verdict are owned by
[PRD 24](../../../prd/PRD_02_24_con_terminal.md#pty-backend-selection).
