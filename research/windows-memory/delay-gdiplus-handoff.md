# Pin handoff: delay-load Gdiplus on screenshot encode

Not applied to production pin `745f52b2`. Research copy:

`target/windows-memory/research-src/agenterm/crates/agenterm-platform/src/adapters/windows/ui_screenshot.rs`

Same pattern as `pty.rs` ConPTY: `LoadLibraryW("gdiplus.dll")` + `GetProcAddress` for `GdiplusStartup`, `GdiplusShutdown`, `GdipCreateBitmapFromScan0`, `GdipDisposeImage`, `GdipSaveImageToFile`. Screenshot encode still uses GDI+; idle GUI no longer maps `gdiplus.dll` at process load.

Evidence (activated, IME on, first frame, `fg_ours=1`):

- Production IAT (`083bcc80`): has `gdiplus.dll`
- Research IAT (`f6e662ac…`): no `gdiplus.dll` (`research/windows-memory/pe_imports.py --forbid gdiplus.dll`)
- Guest WS keep-config 22,376,448 vs prior keep 22,556,672 (Δ 180,224)
- `gdiplus=0` on every idle sample

When the pin lands: invert `gdiplus_is_still_a_static_import_on_pin_745f52b2` in `tests/minicon_load_portability.rs`. Do not treat this as a 10 MiB idle cut.
