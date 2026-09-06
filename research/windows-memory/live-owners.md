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

External hwnd polling cannot prove IME is undelayable. In-process
hooks (research PE, not production pin):

## CreateWindow-stage hooks (research PE)

Identity: `target/windows-memory/research-pe/aarch64-pc-windows-msvc/release/minicon.exe`
SHA-256 `f71f095ffcf7991be100f6b9338808577a6fc9d5142537959caa76b8b48021d9`
(752,128 B). Source copy `target/windows-memory/research-src/agenterm`
= pin `745f52b` + `init_trace` hooks. IME=true. No screenshot.
`run_pixel_window_entry` is **not** process entry.
Log: `target/windows-memory/init-hooks-b7651f9da8d3dfd97311700a6168e59e64556c30-20260906T114058Z.log`

Earliest TIF 0→1 (baseline, window.focus() after opened):
**`WM_IME_SETCONTEXT` reentered from `SetForegroundWindow`**
(seq=15, t=33.3 ms, WS 12,877,824 → 17,428,480). Not StretchDIBits
(TIF already 1 at StretchDIBits_paint_before). Not ShowWindow (TIF
already 1 at before_ShowWindow). `paint_during_create=0`.

Supported comparison (visible ShowWindow, IME=true, **do not delay
first frame**):

| variant | TIF at first present | first_recorded_present WS |
|---|---|---:|
| baseline Focus | 1 at WM_IME_SETCONTEXT during SetForegroundWindow | 22,142,976 |
| skip Focus | **0** through Show + BeginPaint + render + StretchDIBits | 17,637,376 |

Skip-focus keeps the window shown and `apply_ime_allowed(true)`.
TIF/CoreMessaging/CoreUI stay 0. That is a defer-until-real-focus
candidate, not idle-by-skipping-present. **That 4.29 MiB Δ is a
non-activated start comparison** (both groups used `--no-activate`
and `AGENTERM_NO_ACTIVATE=1`; skip also omitted `NativeCommand::Focus`).
It is not public foreground idle. IME=true is not Chinese input.

## Activate-after-present + normal-start control (research PE)

Identity: `target/windows-memory/research-pe/aarch64-pc-windows-msvc/release/minicon.exe`
SHA-256 `c941be928c4ac38050aa6a4d345387962ee12cbd2185a2f9cd902a1c5d73580d`
(755,712 B). Same pin copy `745f52b` + hooks. IME=true. First frame
kept. No screenshot. No control write to PTY.
Log: `target/windows-memory/init-hooks-7a1453b9362739bb7f2255d6eb7685dad4ce9f1d-20260906T115253Z.log`

Foreground HWND/focus recorded on every sample (`hwnd`/`fg`/`focus`/
`fg_ours`/`focus_ours`).

| variant | flags | first_present WS | tif | fg_ours | notes |
|---|---|---:|---|---|---|
| noact_opened_focus | `--no-activate` + opened `Focus` | 22,151,168 | 1 | **1** | opened Focus **takes foreground** despite no-activate |
| noact_skip_then_activate | `--no-activate`, skip Focus, then in-process `SetForegroundWindow` after first present | 17,694,720 | **0** | 0 | then real activate |
| normal_activate | no `--no-activate`, no `AGENTERM_NO_ACTIVATE` | 19,668,992 | 1 | **0** | `focus_ours=1`, `GetForegroundWindow=0`, coremsg/coreui stay 0 |

Skip group after first present (`before_activate_after_present`
17,698,816, tif=0, fg_ours=0):

- TIF 0→1 again at `WM_IME_SETCONTEXT` during activate (WS 22,020,096,
  coremsg=1, coreui=1, fg_ours=1)
- `after_activate_after_present` fg_ok=1 attached=1 WS **22,138,880**
  tif=1 — recovers the ~4.2 MiB (22,138,880 − 17,694,720 = 4,444,160)
- English `SendInput` VK_A/B/C, `pty_control_write=0`, then **3**
  `WM_KEYDOWN` + **3** `WM_CHAR` on our HWND (WS after first keydown
  22,577,152; later present ~22,917,120). That is keyboard delivery,
  not a PTY control write.
- **Chinese IME compose/commit: BLOCKED.** Probe:
  `n_layouts=1 current_langid=0x409 has_zh=0` →
  `chinese_ime_BLOCKED_no_zh_keyboard_layout`. No
  `WM_IME_COMPOSITION` / `WM_IME_CHAR`. Do not claim input
  qualification.

`normal_activate` is the requested no-flag control. It still did not
win the foreground HWND in this UTM job (`fg_ours=0` for the whole
run). TIF loaded; CoreMessaging/CoreUI did not. Do not publish
19.67 MiB as public idle.

Do **not** treat skip-Focus / no-activate as ordinary idle savings:
real activate after the first frame brings TIF+CoreMessaging+CoreUI
back. Do not delay first frame.

### Read-only: should `opened` Focus respect no-activate?

Yes. `PixelWindowOptions.no_activate` only picks `SW_SHOWNOACTIVATE`.
`ConTerminal::opened` always calls `window.focus()` →
`NativeCommand::Focus` → `SetForegroundWindow`+`SetFocus`. This run
proves that path sets `fg_ours=1` under `--no-activate`.

**Minimal patch scope (not shipped this round, not an idle cut):**

1. `src/main.rs` only. Store `no_activate` on `ConApp` (parsed flag
   plus `AGENTERM_NO_ACTIVATE`, already merged in `main`).
2. Remove `window.focus()` from `ConTerminal::opened`. Call it from
   `ConApp::opened` gated on `!self.no_activate`.
3. Leave `NativeCommand::Focus` unchanged. Tab switch, a11y
   `NODE_FRAME`, and empty-greeting new-terminal still need Focus.
   A platform-wide Focus no-op is too wide for startup-only
   `--no-activate`.

Product no-activate fix is **in tree** at `56207cb` (`src/main.rs` only).
Do not re-patch it here. Do not use research skip-Focus as a substitute.

Exact production PE verify (HEAD `20e501b` contains `56207cb`, pin
`745f52b2`, no research skip env):
`target/windows-memory/exact-pe/.../minicon.exe`
SHA-256 `083bcc802099be5d385460c151f043cef21af49ef6a8a7b1faf182269bfb3897`
(746,496 B).
Log: `target/windows-memory/no-activate-behavior-20e501b8c90c5545b0aa0d5d036f27dce9def4cb-20260906T120633Z.log`

- `--no-activate` after first frame: hwnd `0x2009e`, fg `0x100d2`,
  **fg_ours=0**. PASS: did not steal foreground. WS 17,723,392.
- Explicit `SetForegroundWindow` (AttachThreadInput): fg_ok=1
  fg_ours=1 WS 22,515,712. Not an idle cut (already closed).
- English `SendInput` VK_A/B/C, `pty_control_write=0`.
  `capture-pane` shows `C:\Windows\System32>abc`. PASS.
- Chinese IME: `n_layouts=1 current_langid=0x409 has_zh=0` →
  **BLOCKED**. No input qualification.

**Stop the delay-until-focus idle direction.** TIF+CoreMessaging+CoreUI
after real activate are IME-on cost, not a cut.

## Post-activate ~21 MiB: call candidates (not another field court)

Activated idle is ~22.1 MiB research / **21.46 MiB** public release.
Named and **not** the next cut: IME stack (TIF 1,069,056 + CoreMessaging
950,272 + CoreUI 626,688 + MSCTF 839,680), DIB/unnamed-mapped that
already tracked 960×600 in size-compare, PTY ring 1 MiB (non-goal).
Do not re-court those.

Concrete MiniCon/platform **calls** that still run on a normal
foreground start, with a one-shot intervention each:

### 1. `load_config` → `SHGetFolderPathW` (shell32 / windows.storage)

- `src/main.rs` `load_config()` / `config_path()` always calls
  `agenterm_platform::runtime::user_config_directory()`.
- Windows leaf:
  `crates/agenterm-platform/.../windows/runtime.rs`
  `user_config_directory` → `SHGetFolderPathW(CSIDL_APPDATA)`.
- Idle modules: `windows.storage.dll` **741,376**, `shell32.dll`
  **532,480**, plus `shcore`/`shlwapi`/`kernel.appcore`.
- **Intervention (research copy of platform runtime, not pin):** skip
  `load_config` or replace `SHGetFolderPathW` with
  `GetEnvironmentVariableW("APPDATA")`. Same activated one-tab, IME on.
- **Pass:** `windows.storage` and/or `shell32` resident drop, first-frame
  still paints, `minicon.json` still found when `%APPDATA%\minicon.json`
  exists.
- **Fail:** those DLLs still present at the same WS (CreateWindow/IME
  already pulled them). Then this call is not an owner.

### 2. `font::cell_metrics` → `select_primary` `CreateFontW` walk

- `ConTerminal::recompute_metrics` in `opened` calls
  `font::cell_metrics` before first paint.
- Leaf: `windows/font.rs` `select_primary` walks `RASTER_FAMILIES`
  (NSimSun … **Segoe UI Emoji**, 12 names). `CreateFontW` never fails;
  mapper may fault font files into WS even if `PixelFace` is dropped.
- **Intervention:** research-only count `PixelFace::create` until
  `select_primary` returns; second build with Emoji/unused tail omitted
  (keep a CJK-capable face — do not drop 中文 raster).
- **Pass:** create-count > 1 and WS falls while `中` still glyphs and
  GDI object count stays flat.
- **Fail:** create-count = 1 (early NSimSun/Consolas win) or CJK paint
  breaks. Then this is not an idle cut.

### 3. Static `gdiplus.dll` import from screenshot encode

- `Cargo.toml` enables `screenshot` on every GUI binary.
- Leaf: `windows/ui_screenshot.rs` `use Gdiplus::{GdiplusStartup, …}`.
  `GdiplusStartup` runs only on `screenshot-pane`, but the **import
  table** still loads `gdiplus.dll` before `main`
  (`tests/minicon_load_portability.rs` lists it as OS-provided).
- Idle resident is only **143,360** (shared0=24,576) — small, but a
  real unnecessary load.
- **Intervention:** resolve Gdiplus via `GetProcAddress` on the encode
  path (same pattern as ConPTY), or drop `screenshot` from the idle
  GUI feature set.
- **Pass:** PE no longer lists `gdiplus.dll`; idle WS drops ~that
  resident; `screenshot-pane` still writes a PNG when invoked.
- **Fail:** WS unchanged (pages were never private). Still a load-time
  hygiene win, not a 21 MiB owner.

### Explicit non-candidates (do not run)

- Delay TIF / skip `NativeCommand::Focus` / `MINICON_INIT_SKIP_FOCUS`.
- PTY `PTY_QUEUE_BYTES` product cut.
- Repeat size-compare, wrapper, 8-page remainder, hwnd-poll.
- `accessibility_publish::start` on Windows without `a11y-tree` (no-op).
- Clipboard (`set_text`/`get_text` only on copy/paste).
- Disable IME / `ImmAssociateContextEx`.

Next research PE (after this list, not skip-Focus): implement candidate 1
on the existing `target/windows-memory/research-src` copy, one activated
control vs one `SHGetFolderPathW`-free build, same IME=true first frame.

Probe self-cost: warmup_a 10,813,440 → warmup_b 10,866,688 (**+53,248**);
entry equals warmup_b.

| stage | WS | Δ from previous | tif | msctf | coremsg | coreui | imm32 |
|---|---:|---:|---|---|---|---|---|
| entry | 10,866,688 | — | 0 | 0 | 0 | 0 | 1 |
| after LoadCursorW | 10,895,360 | +24,576 | 0 | 0 | 0 | 0 | 1 |
| after RegisterClassW | 10,924,032 | +20,480 | 0 | 0 | 0 | 0 | 1 |
| before CreateWindowExW | 10,928,128 | — | 0 | 0 | 0 | 0 | 1 |
| reenter WM_NCCREATE (depth=1) | 11,264,000 | +335,872 inside create | 0 | 0 | 0 | 0 | 1 |
| after CreateWindowExW | 11,882,496 | **+954,368** vs before create | 0 | **1** | 0 | 0 | 1 |
| after apply_ime_allowed | 11,907,072 | +20,480 | 0 | 1 | 0 | 0 | 1 |
| after ApplicationOpened | 12,877,824 | +966,656 | 0 | 1 | 0 | 0 | 1 |
| first_present StretchDIBits | 22,151,168 | **+9,273,344** | **1** | 1 | **1** | **1** | 1 |

No `WM_PAINT` during `CreateWindowExW` (`paint_during_create=0`);
window is created hidden. Nested **WM_NCCREATE** is recorded — do
not call the whole +954,368 “just CreateWindow”. **MSCTF** appears
inside CreateWindowExW. **TIF / CoreMessaging / CoreUIComponents**
load on `SetForegroundWindow` → `WM_IME_SETCONTEXT`, not on
`ImmAssociateContextEx` (+20 KiB only). That delay path is
**closed as an idle cut** (activate returns the ~4.2 MiB).

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
