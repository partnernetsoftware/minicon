# Mouse delivery court

Same-court comparison of mouse delivery across the three Windows backends,
for the 0.2.20 default decision. Verdict and evidence are owned by
[PRD 24](../../../prd/PRD_02_24_con_terminal.md#pty-backend-selection)
(backend selection) and the native semantics by
[PRD 34](../../../prd/PRD_02_34_windows_console_backend.md).

- `fixture.ps1` — child: sets VT input + `ENABLE_MOUSE_INPUT` +
  `ENABLE_EXTENDED_FLAGS` (what mouse-aware TUIs set), requests SGR mouse
  reporting (`?1000h ?1006h`), and prints every raw console input record it
  reads with `ReadConsoleInputW` — mouse records and key characters alike.
- `run-court.ps1 -Backend <agent|conpty|rust-console>` — drives a debug
  MiniCon over a control pipe: `send-mouse` press/release at cell (10,5),
  `send-wheel` one notch at (20,6), then `q` to end; captures the pane.
  rust-console needs `--features experimental-rust-console`.

A `ReadFile` text-mode reader receives no mouse bytes on any backend
(conhost delivers mouse as records, not VT text, in that mode), so the court
reads records; that parity is itself part of the finding.
