# GDI/native 21 MiB live owners

Pin `745f52b2`, PE `d2d08ce7…`, accepted complete court idle
**22,507,520 B**. Child shells excluded. No production patch in this file.

## Accepted idle arithmetic (not a six-cell claim)

Named pieces that exist in the idle process:

- one client DIB `HostState.pixels`: 960×600×4 = 2.25 MiB (scale 1.0)
- one `BoundedOutputPipe`: 1.00 MiB
- TLS GDI faces: HFONT+DC per actually created family; cmap empty until
  non-BMP lookup; glyph outline `Vec` is per-call, not retained
- one vt100 parser (4000-line cap, idle almost empty) + one ConPTY

Those named pieces are **~3.3 MiB** at scale 1. Remainder of 21.46 MiB is
**~18 MiB unnamed** until guest object counts land. Extra-tab 1.54 MiB
matches one more pipe plus a small parser/ConPTY, not a second DIB.

Four-cycle **+9.13 MiB** is not the DIB and not TLS fonts (they survive
close). `close_active_session` drops the session and calls
`shutdown_session_detached` (bounded reaper; Full → extra thread).

## Top 5 and how to falsify each

### 1. Unnamed remainder (~14–18 MiB)

Largest gap. Includes PE image, CRT, ntdll heaps, USER32/GDI32 working
set, IME, window class. Not macOS vmmap.

Intervention: while `minicon-release-rss.exe` is idle one-tab, sample
`WorkingSet64`, `VirtualMemorySize64`, `HandleCount`,
`GetGuiResources(GR_GDIOBJECTS)` and `GR_USEROBJECTS`. Compare greeting
(no tab) vs one-tab if a later court can stop before spawn. Pass if
remainder shrinks when GDI/USER counts stay flat (then it is heap/WS);
fail if GDI objects track the 21 MiB.

### 2. Single DIB `HostState.pixels` (2.25–5 MiB)

`native_pixel_window.rs` resizes one `Vec<u32>` to
`physical_width * physical_height`. Window is `LogicalSize::new(960, 600)`
in MiniCon; `--cols/--rows` size the PTY grid, not this buffer.

Intervention: research-only smaller `PixelWindowOptions` (not this
commit). Idle RSS should fall by ~4 bytes × Δpixels. If idle barely
moves, DIB is not the 21 MiB owner.

### 3. TLS `PixelFace` / cmap (not four-cycle)

`CreateFontW` never fails; the 12-name list is a mapper wish list.
`GetFontData` cmap is lazy, capped at 4 MiB/face. `Drop` deletes HFONT/DC.
`RASTER_FACES` is `thread_local`, so extra-tab close does not free it.

Intervention: `GetGuiResources` after first paint vs after four cycles.
If GDI count is flat across cycles, fonts are idle-only. Research-only
later: omit Segoe UI Emoji from the wish list and re-court.

### 4. Per-tab `BoundedOutputPipe` (1 MiB)

`READ_BUF * 128` = 1 MiB. `drop(session)` should release it.

Intervention: extra-tab delta already 1.54 MiB on the accepted court —
about one pipe plus ~0.5 MiB. A research-only smaller ring would drop
that delta ~1 MiB if this owner is real. Product ring cut is a **non-goal**.

### 5. vt100 4000 + ConPTY detach (four-cycle primary)

Load grew only +0.56 MiB (22,507,520 → 23,085,056). Four extra-tab
open/close grew **+9.13 MiB**. Detached PTY teardown can block; the
reaper queue `Full` path spawns another thread and may retain native
objects.

Intervention: sample handle/GDI/thread counts at idle, after load, after
each cycle. If handles climb ~linearly with cycles, stop detaching or
join the reaper in a research build. If counts are flat while WS climbs,
it is allocator/WS retention (macOS-style relief already rejected as a
product change — measure first).

## Probe (guest, no product binary change)

`probe-gdi-objects.ps1` — pass a PID, print WS/GDI/USER/handles.
Run against the live `minicon-release-rss` PID during an existing court,
or a short `--control` GUI. Do not wrap that GUI with `*>` or
`Start-Process -Wait`.
