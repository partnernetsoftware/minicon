# GDI/native 21 MiB live owners

Pin `745f52b2`, PE `d2d08ce7…`, accepted complete court idle
**22,507,520 B**. Child shells excluded. No production patch in this file.

Do **not** attribute four-cycle +9–10 MiB to “tabs” from RSS
alone. Split after `close-tab`:

1. **Still-live PTY threads/handles** — per tab: detached
   `minicon-reader` + `minicon-waiter`; process-wide
   `agenterm-pty-reaper` (overflow thread only if the 64-slot queue
   is Full). `Drop` calls `pty_output.close()` then
   `shutdown_session_detached`. Reader should EOF/unblock; waiter
   should see process exit. Leftover named threads or extra handles
   after `list-tabs` is back to 1 are closed-tab residue and **may
   be repaired** if the sample proves they survive. Product PTY ring
   capacity is **not** a cut target.
2. **tab / vt100 owners** — `SessionStore<ConTerminal>` + workspace
   node + `vt100::Parser` (SCROLLBACK 4000). `list-tabs` length is
   the observable owner count. A node with `child_alive=false` that
   still sits after explicit `close-tab` is leftover product state.
   Parser bytes are not the 1 MiB pipe.
3. **GDI objects** — process `thread_local` `RASTER_FACES`, one
   client DIB. Extra-tab close must **not** free them; they are idle
   owners. `GetGuiResources` should be flat across cycles if GDI is
   not the +10 MiB.
4. **Commit vs resident** — `PrivateMemorySize64` / `PrivateUsage`
   is **commit**, not RSS. **Do not** write
   `WS = PrivateMemorySize64 + (WS − private)`. Subtract only
   counters from the same resident-page walk
   (`QueryWorkingSet` / `QueryWorkingSetEx` + `VirtualQueryEx`).
   A load-time private-commit climb is **not** a named malloc
   stack (parser/heap) until that walk says so.

Probe: `probe-gdi-objects.ps1` (WS, private, handles, GDI/USER,
named PTY thread counts). Driver:
`sample-close-owners.ps1` / `run-close-owners.sh`.

## Guest samples 2026-09-06 (win-aarch64, same PE)

Not an RSS wrapper court. `list-tabs` is the tab-owner count.
PTY thread names from `GetThreadDescription`.

### No 2000-line load

`target/windows-memory/close-owners-db2c43b77ab4c1b9d58924f0fd88d85115009ebc-20260906T103924Z.log`

| phase | tabs | WS | private | handles | GDI | USER | threads | reader | waiter | conpty-out |
|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|---:|
| idle | 1 | 22,491,136 | 6,553,600 | 207 | 11 | 14 | 13 | 1 | 1 | 1 |
| two-tab | 2 | ~22.9 MiB | ~8.1 MiB | ~220 | 11 | 16–18 | 16 | 2 | 2 | 2 |
| after 4 closes | 1 | 22,581,248 | 6,594,560 | 213 | 11 | 17 | 13 | 1 | 1 | 1 |

After close: reader/waiter/conpty-output return to **1**. Tab count **1**.
GDI **flat 11**. WS +88 KiB. `PrivateMemorySize64` +40 KiB is
**commit**, not a WS term. Handles +6, USER +3.
**No leftover live PTY threads or vt100 tab owners** (this round’s
large close residue is excluded). Extra-tab **commit** +1.53 MiB
returns on close. Product ring not cut. Do not re-open this court.

### With 2000-line load, then four extra-tab cycles

`target/windows-memory/close-owners-db2c43b77ab4c1b9d58924f0fd88d85115009ebc-20260906T104114Z.log`

| phase | tabs | WS | private | handles | GDI | reader/waiter |
|---|---:|---:|---:|---:|---:|---|
| idle | 1 | 22,548,480 | 6,574,080 | 207 | 11 | 1/1 |
| after_load | 1 | **34,353,152** | **18,083,840** | 209 | 11 | 1/1 |
| two-tab | 2 | ~34.7 MiB | ~19.6 MiB | ~222 | 11 | 2/2 |
| after 4 closes | 1 | 34,398,208 | 18,079,744 | 213 | 11 | 1/1 |

Load itself is **+11.0 MiB `PrivateMemorySize64` commit**. That is
load-related **commit**, **not** a malloc-stack naming of parser/heap.
GDI still 11, PTY threads still 1. Four-cycle after that load: WS
**+45 KiB**, commit flat. Live PTY/tab/GDI do not explain RSS-court
+9 MiB. RSS-court after_load +0.56 MiB is a **timing** note only
(1.4 s sample), not parser/heap evidence.

Closed-tab leftover that is real and small: +6 handles, +3 USER.
Not a 10 MiB repair. No production patch. Do not re-open close or
wrapper courts.

## Idle resident pages (same QueryWorkingSet snapshot)

Official:
https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-psapi_working_set_block

| field | meaning |
|---|---|
| `Shared` | page is **sharable**, not “already shared by other processes” |
| `ShareCount` | how many processes share it (max 7) |
| `Protection` 5 / 7 family | copy-on-write |
| `VirtualPage` | page VA used for module-range / `VirtualQueryEx` |

Verified earlier walk: 5490 × 4096 = 22,487,040 and
18,186,240 + 4,300,800 = 22,487,040 (Get-Process WS 22,515,712 is
**not** that product; that gap is snapshot drift).

Latest same-snapshot log:
`target/windows-memory/idle-regions-9ed910ea3219a07e57595c2b86b6cf4e6d2e97da-20260906T105829Z.log`

| same-walk counter | bytes | do not call it |
|---|---:|---|
| ws_before = ws_after | 22,503,424 | — |
| walk 5486 × 4096 | **22,470,656** | — |
| drift | 32,768 (8 pages) | ignore as RSS split |
| `Shared=1` sharable | 18,182,144 | **not** “Win32/GDI/IME” |
| `ShareCount>=2` | 17,399,808 | actually shared |
| `ShareCount==1` | 782,336 | sharable, this process only |
| `Shared=0` not sharable | 4,288,512 | **not** “DIB+pipe” |
| COW (prot 5/7) | 4,096 | kept |
| `cow_private` | 0 | kept |
| unknown (unnamed mapped) | **2,732,032** | kept, not named |
| `EnumProcessModules` image | 16,343,040 | sum of module rows |
| mapped (incl. unknown) | 2,904,064 | — |
| `MEM_PRIVATE` type | 3,223,552 | not DIB+pipe until proven |

sharable + not_sharable = walk. Module rows from **this** snapshot’s
`VirtualPage` in `EnumProcessModulesEx` ranges (not VA commit):

| resident | module |
|---:|---|
| 3,735,552 | `ntdll.dll` |
| 1,069,056 | `TextInputFramework.dll` |
| 1,052,672 | `KERNELBASE.dll` |
| 950,272 | `CoreMessaging.dll` |
| 839,680 | `MSCTF.dll` |
| 741,376 | `windows.storage.dll` |
| 704,512 | `combase.dll` |
| 626,688 | `CoreUIComponents.dll` |
| 593,920 | `minicon-release-rss.exe` |
| 532,480 | `shell32.dll` |
| 487,424 | `user32.dll` |
| 327,680 | `gdi32full.dll` |
| 2,732,032 | **unknown-mapped** |

Do not re-open wrapper. Next: identify unknown-mapped; IME four are
named modules, not the whole sharable bucket.

## Accepted idle arithmetic (not a six-cell claim)

Named pieces that exist in the idle process:

- one client DIB `HostState.pixels`: 960×600×4 = 2.25 MiB (scale 1.0)
- one `BoundedOutputPipe`: 1.00 MiB
- TLS GDI faces: HFONT+DC per actually created family; cmap empty until
  non-BMP lookup; glyph outline `Vec` is per-call, not retained
- one vt100 parser (4000-line cap, idle almost empty) + one ConPTY

DIB 2.25 MiB and pipe 1 MiB are **code-size candidates** inside
`MEM_PRIVATE` type (~3.2 MiB). They are **not** the 4.29 MiB
not-sharable bucket. Extra-tab RSS ~1.5 MiB is a live-session cost,
not a close leak.

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
