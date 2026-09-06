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

Walk is **internally closed**. A separately-read WS is **not** that
product. Corrected (cdx-wjhk; they own the main-PRD arithmetic fix):

- 18,186,240 + 4,300,800 = **22,487,040** = 5490 × 4096
- old WS 22,515,712 − walk = **28,672 (7 pages)**
- image 16,343,040 + mapped 2,908,160 + private_type 3,235,840 =
  **22,487,040**; WS 22,519,808 − walk = **32,768 (8 pages)**

cdx-wjhk: 5486 × 4096 = **22,470,656**; classification and
ShareCount totals of that round are correct. PMC WS − walk =
**32,768** is an **unexplained remainder**, not proven drift.
Do not open a full court for those 8 pages.

`cow_private` was only incremented on the `MEM_PRIVATE` branch
after a module-range `continue`, so it **did not** count
image/mapped privatized pages. `cow_private=0` does **not** mean
no COW. `cow_protect` is QWS Protection 5/7 family (COW
**protection**). Privatized resident is `MEM_IMAGE`/`MEM_MAPPED`
**and** `Shared=0`. After COW, `VirtualQueryEx` Type stays IMAGE
or MAPPED:
https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualqueryex

Latest log (IME **on**, Chinese preserved, **no IME opt-out**):
`target/windows-memory/idle-regions-edcedfabedcaeec3773142d260010bc3c4b687a6-20260906T110648Z.log`

| instant | UTC | PMC WS |
|---|---|---:|
| before QWS | 2026-09-06T11:07:38.411Z | 22,532,096 |
| after QWS | 2026-09-06T11:07:38.411Z | 22,532,096 |
| after classify | 2026-09-06T11:07:38.427Z | 22,532,096 |

| walk (internally closed) | bytes |
|---|---:|
| 5493 × 4096 | **22,499,328** |
| image+mapped+private_type | 16,343,040+2,916,352+3,239,936 = **22,499,328** |
| PMC − walk (all three) | **32,768 unexplained remainder** |
| sharable / `ShareCount>=2` / `==1` | 18,194,432 / 17,412,096 / 782,336 |
| not sharable | 4,304,896 |
| `cow_protect` (prot 5/7) | 4,096 |
| `privatized_image` (`IMAGE`∧`Shared=0`) | **1,069,056** |
| `privatized_mapped` | 0 |
| unknown / vq_fail | 2,744,320 / 0 |

**Retract** `MEM_PRIVATE + privatized_image = not_sharable`.
3,239,936 + 1,069,056 = **4,308,992**, which is **4,096 (1 page)**
above `not_sharable` 4,304,896. `MEM_PRIVATE` is **not** all
`Shared=0`. Walk and each partition still sum. type×Shared cross
table is on the size-compare probe; no extra counting court.

`count_meta`: qws_retry=0 enum_ok=1 enum_needed=33 enum_slots=512
enum_truncated=0 getmod_fail=0 module_keys=33 mapped_keys=8.
**33 modules were collected in full**; the log only **displayed** 25
(`module_list_truncated`). `enum_err=299` after enum_ok=1 is
**not** a failure (`LastError` after success is non-authoritative).
Do not re-run for that.

Unnamed `MEM_MAPPED` by **allocation-base** (all `GetMappedFileNameW`
**ERROR_FILE_INVALID 1006**; kept unknown; ShareCount retained):

| alloc | resident | protect | ShareCount>=2 |
|---|---:|---|---:|
| `0x1ace0000000` | 2,306,048 | 0x4 R/W | 2,154,496 |
| `0x1acdbd10000` | 110,592 | 0x2 R | 110,592 |
| `0x1acdb760000` | 102,400 | 0x2 R | 102,400 |
| `0x1acdb810000` | 45,056 | 0x2 R | 45,056 |
| `0x1acdb800000` | 45,056 | 0x2 R | 45,056 |
| (11 smaller 1006 mappings) | rest of 2,736,128 | 0x2/0x4 | — |

IME DLLs below are the **IME-on / 中文输入维持** baseline, not a
disable experiment.

Module rows from **this** snapshot’s `VirtualPage` in
`EnumProcessModulesEx` ranges (not VA commit):

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
| 2,736,128 | **unknown-mapped** (1006, see alloc-base table) |

Do not re-open wrapper. Do not disable IME. 32 KiB PMC−walk stays
**unexplained remainder** (no extra court).

## Size causal compare (frozen PE, new process ×2, then same-process shrink)

`target/windows-memory/size-compare-91752f59296e6e2e4b2d718c2a97df69f83746bc-20260906T111300Z.log`

IME on. No screenshot. First frame = `perf-stats`
present_success/host_direct/frames ≥ 1. DPI 96. Logical inner size
via `resize-window`.

| sample | client | pixel area | 4×area | largest R/W unnamed mapped | ShareCount≥2 | private_type | privatized_image |
|---|---|---:|---:|---:|---:|---:|---:|
| new 480×300 | 480×300 | 144,000 | 576,000 | **614,400** | 598,016 | 3,121,152 | 1,069,056 |
| new 960×600 | 960×600 | 576,000 | 2,304,000 | **2,379,776** | 2,347,008 | 3,612,672 | 1,069,056 |
| same proc before | 960×600 | 576,000 | 2,304,000 | 2,379,776 | 2,347,008 | 3,600,384 | 1,069,056 |
| same proc after | 480×300 | 144,000 | 576,000 | 675,840 | 598,016 | 3,403,776 | 1,069,056 |

Largest R/W `MEM_MAPPED` **does** follow pixel area (ratio 3.87 vs 4).
Do **not** name it DIB: it is still 1006-unnamed and `ShareCount>=2`.
`private_type` does **not** scale with 4× pixels (+0.49 MiB only).
`privatized_image` 1,069,056 is **identical** at both sizes (IME-on
`TextInputFramework` resident matches that number).

Same-process 960→480: mapped 2,379,776 → 675,840 (most of the
size-correlated bytes return). Leftover vs fresh-small: mapped
+61,440, private_type +282,624. Not a 2 MiB leak.

No MiniCon production patch this round. If a later pin looks at
GDI `StretchDIBits` backing in
`native_pixel_window.rs` (no `CreateDIBSection` in that file),
scope is that adapter’s resize/present, not the PTY ring and not
IME-off.

**`privatized_image=1,069,056` matching TIF total resident is not
evidence that this 1.02 MiB is all IME.** Per-module `Shared=0` at
init: TIF `shared0=24,576` of `1,069,056` total; the 1.02 MiB is
the **sum of Shared=0 across all 33 modules** (shell32 106,496,
ntdll 98,304, KERNELBASE 73,728, …).

## Init-phase causal (IME on, no screenshot)

`target/windows-memory/init-phases-25635c4f39397fa5151d4ef1376c1ba348924bbd-20260906T112057Z.log`

One process. First QWS already has `hwnd` and all 33 modules
(TIF/MSCTF/CoreMessaging/imm32 present). Spawn→first_frame PMC WS
22,458,368 → 22,515,712 (**+57,344**). TIF shared0 stays 24,576.

| phase | Get-Process WS | walk | TIF tot / shared0 |
|---|---:|---:|---|
| spawn (hwnd already) | 22,458,368 | 22,446,080 | 1,069,056 / 24,576 |
| hwnd | 22,478,848 | 22,446,080 | same |
| control | 22,511,616 | (same stack) | same |
| first_frame | 22,515,712 | 22,478,848 | same |

No delayable IME load after first observable window: the IME
image set is already resident. Keeping Chinese input (`IME=true`)
does not leave a post-hwnd deferral in this timeline. Splitting
`CreateWindow` vs IME DLL map needs in-process timing, not another
QWS field court. `cross type=private shared1=4096` is the page that
broke `MEM_PRIVATE + privatized_image = not_sharable`.

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
