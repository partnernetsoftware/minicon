# Windows host RSS: name live allocations from the real GDI host

Owner: MiniCon Windows memory branch (not the macOS main line).
Product criterion: **host process RSS**, idle one-tab intent **10 MiB**.
Non-goals: PTY 1 MiB ring as a product cut, raising the 384 MiB ceiling,
copying macOS numbers, repeating rejected macOS experiments (allocator
relief, menu separator removal, delayed winit inputContext). Main PRD
Windows receipt is filled by cdx-wjhk, not this branch.

```text
Windows idle host RSS must be named, then cut toward 10 MiB
├── Court: win-aarch64 UTM exact artifact, GDI native-pixel-window
│   ├── invariant: working-set of MiniCon.exe, child shells excluded
│   ├── evidence: host_process_rss_stays_within_named_budget receipt
│   ├── failure: missing guest / QGA / transfer timeout is BLOCKED
│   └── non-goal: win-x86_64 until aarch64 is named at current source
├── Code: Win32 pixel host + GDI faces, not whole-file TTC leak
│   ├── HostState.pixels: one client-area XRGB Vec (StretchDIBits)
│   ├── PixelFace: CreateFontW + optional cmap ≤ 4 MiB, lazy per family
│   └── non-goal: portable-pixel-window / winit on Windows
└── Repair: only after a named live allocation and coordinated patch scope
```

## Already named (do not recycle as current-source release)

2026-09-06 win-aarch64 UTM **debug** GUI,
`target-six/logs/rss-win-aarch64-utm.log`, source identity
`7d7a116573c168903adff4215e1b57f023eb1c523e0d0aa2ea7500cec9e3364c`:

| sample | RSS |
|---|---:|
| idle one-tab | 22.47 MiB (23,564,288 B) |
| after 2000-line load | 22.92 MiB |
| max extra-tab delta | 3.64 MiB |
| four-cycle growth | 12.42 MiB |

That is **debug**, not exact release, and not this HEAD. It only proves the
Windows court can run the RSS black box and that GDI does not reproduce the
macOS 183 MiB Color Emoji heap leak.

Hang evidence (keep): `research/windows-memory/hang-last-step.md`.
33,620 K tasklist WS is hang-time only, not a public idle court.

## Wrapper (accepted — latest; do not pause or re-run)

cdx-wjhk 2026-09-06 **this letter overrides late old null mail**.
Those null re-blocks were against the **pre-fix** snapshot. After
Handle + WaitForExit + Refresh and null→throw:

- `target/windows-memory/rss-exitcode-handle.host.log` is a **real
  EXIT 0** (idle **22,495,232 B** / 21.45 MiB). Recorded in main PRD
  at `e6cd7b0`.
- Later `.exitmeta` confirmed `ExitCode_raw=0` (`Handle=2452`):
  `target/windows-memory/rss-win-aarch64-release-e6cd7b0fbab8c3f22de8b5aef54e306d71958bf6-20260906T102522Z.log`
  (idle 22,507,520 B / 21.46 MiB).

Wrapper is **accepted**. Do not pause. Do not re-court.

Still **not** wrapper evidence (coerce-era snapshot only):
`target/windows-memory/rss-retest-now.host.log` /
`…-20260906T101827Z.log`. First Start-Process
`…-20260906T101551Z.log` EXIT 1 was null-throw, not RSS FAIL.
Hang 33,620 K is not a public idle row.

| field | accepted Handle EXIT0 (`rss-exitcode-handle.host.log`) |
|---|---|
| source / pin / PE | `3419d46` / `745f52b2` / `d2d08ce7…` |
| idle | **21.45 MiB (22,495,232 B)** |
| after 2000-line load | 22.13 MiB (23,203,840 B) |
| extra-tab delta | 1.67 MiB (1,748,992 B) |
| four-cycle growth | 9.86 MiB (10,342,400 B) |

Main PRD Windows receipt is cdx-wjhk’s at `e6cd7b0`. Still above
10 MiB. No production pin/source patch yet. macOS WritingToolsUI
dlopen ~22.6 MiB is **not** this GDI remainder.

`PrivateMemorySize64` is **commit**. Do not subtract it from WS.
`PSAPI_WORKING_SET_BLOCK.Shared` is **sharable**, not already-shared;
`ShareCount` is the process count (max 7). Official:
https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-psapi_working_set_block

Classification **walk is internally closed**. PMC WS − walk
(**32,768**) is an **unexplained remainder**, not proven drift.
Do not open a full court for it.

5486 × 4096 = 22,470,656 (cdx-verified ShareCount/class totals).
`cow_protect` = QWS Protection 5/7. Privatized resident =
`MEM_IMAGE`/`MEM_MAPPED` ∧ `Shared=0` (Type stays IMAGE/MAPPED
after COW). Do not read `cow_private=0` as no COW.

IME-on / 中文输入维持 only — **do not disable IME**. No wrapper
re-run. 32 KiB unexplained remainder: no extra court.

**Retract** `MEM_PRIVATE + privatized_image = not_sharable`
(off by 4096). Do not assume `MEM_PRIVATE` is all `Shared=0`.
type×Shared cross rides the next size-compare sample; no extra
counting court.

Size compare accepted: largest R/W unnamed mapped tracks area
(614,400 vs 2,379,776); shrink returns most; not DIB / not a 2 MiB
leak. `privatized_image=1,069,056` **is not all TIF** (TIF
shared0=24,576; 1.02 MiB is Shared=0 summed over 33 modules).

Init-phase external polling cannot prove IME undelayable.
Research PE `c941be928c4ac38050aa6a4d345387962ee12cbd2185a2f9cd902a1c5d73580d`
(755,712 B; prior hook PE `f71f095f…` 752,128 B).
`run_pixel_window_entry` ≠ process entry. TIF 0→1 at
**WM_IME_SETCONTEXT during SetForegroundWindow**, not StretchDIBits
or ShowWindow. Skip-Focus Δ 4,444,160 B is **accepted as non-activated
start**, not foreground idle. **Stop that delay-optimization direction.**
Chinese IME compose/commit stays **BLOCKED** (langid 0x409). Do not use
`MINICON_INIT_SKIP_FOCUS` as a stand-in for the product no-activate fix
(`56207cb`, `src/main.rs` only; pin unchanged).
Exact PE `083bcc80…` (746,496 B) on win-aarch64: `--no-activate`
fg_ours=0 after first frame; explicit activate then English SendInput
shows `abc` in `capture-pane`; Chinese IME still BLOCKED (0x409).

Next: name **removable init/loads inside the post-activate ~21 MiB**.
Call candidates and interventions:
`research/windows-memory/live-owners.md` (section after activate).
Do not re-run wrapper / 8-page / size-compare / hwnd-poll / skip-Focus.

## Top 5 live owners and verifiable interventions

Working file: `research/windows-memory/live-owners.md`.
No production patch until an intervention moves RSS on this court.

| # | owner | idle bound | four-cycle? | verify without product patch |
|---|---|---|---|---|
| 1 | unnamed remainder (PE+CRT+USER/GDI heap, not DIB/pipe) | **~14–17 MiB** after items 2–5 | maybe WS not returned | guest `Get-Process` WS + `GetGuiResources` GDI/USER/handle at idle |
| 2 | `HostState.pixels` one `Vec<u32>` StretchDIBits | 960×600×4 = **2.25 MiB**; 150% DPI ~5 MiB | no (one buffer) | compare idle WS at current 960×600 vs a research-only smaller window later |
| 3 | TLS `RASTER_FACES` `CreateFontW`+DC; cmap ≤4 MiB/face | typically small unless cmap filled; 12-name wish list, mapper | **no** — process TLS | `GetGuiResources(GR_GDIOBJECTS)` idle vs after first glyph; skip emoji later |
| 4 | per-tab `BoundedOutputPipe` | **1 MiB** (`8192*128`) | should drop on `sessions.remove` | extra-tab delta 1.54 MiB already ≈ pipe + leftover; research `PTY_QUEUE_BYTES` cut |
| 5 | vt100 `SCROLLBACK=4000` + ConPTY `shutdown_session_detached` | idle small; load only +0.56 MiB | **yes — primary cycle suspect** | handle/GDI counts after 4 cycles; reaper-queue Full spawns extra threads |

After-close split (sampled, not inferred from +10 MiB RSS):

| class | after explicit close-tab |
|---|---|
| live PTY | `minicon-reader`/`waiter`/`agenterm-conpty-output` return to 1; reaper 1; overflow 0 |
| tab/vt100 | `list-tabs` returns to 1 |
| GDI | **11** at idle, two-tab, and after close |
| commit vs resident | `PrivateMemorySize64` is commit; do not subtract from WS |
| residue | +6 handles, +3 USER — not 10 MiB |

Load +11 MiB is **private commit**, not a named malloc stack.
Do not cut PTY capacity. Do not re-open close/wrapper. macOS
WritingToolsUI is not this remainder.

Logs: `research/windows-memory/live-owners.md`.
Driver: `research/windows-memory/run-close-owners.sh`.

Do not edit `src/`, Cargo, or the public memory PRD from this branch.

Logs and artifacts: `research/windows-memory/`, `target/windows-memory/`.
