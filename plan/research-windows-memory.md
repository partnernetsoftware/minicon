# Windows host RSS: name live allocations from the real GDI host

Owner: MiniCon Windows memory branch (not the macOS main line).
Product criterion: **host process RSS**, idle one-tab intent **10 MiB**.
Non-goals: PTY 1 MiB ring, raising the 384 MiB ceiling, copying macOS
numbers, repeating rejected macOS experiments (allocator relief, menu
separator removal, delayed winit inputContext).

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

## Wrapper PASS paused (null is not Wait-Process)

cdx-wjhk 2026-09-06 re-block: a reader still seeing
`if ($null -eq $code) { $code = 0 }` at line 126 is looking at a stale
buffer. **Wait-Process / WaitForExit is not “ExitCode=null solved.”**
Null after Handle + WaitForExit + Refresh is **wrapper failure**; raw
ExitCode must be recorded; never coerce null to 0.

Current `research/windows-memory/run-release-rss.sh` at HEAD `a8679ae`
has **no** `$code = 0` fallback anywhere in this clone:

| file line | what it does |
|---|---|
| 116 | `$null` → `ExitCode_raw=NULL` text; does not write job.exit 0 |
| 123 | `$null = $p.Handle` (open handle before wait) |
| 126 | `Save-PidState timeout $p` inside the WaitForExit timeout throw |
| 135 | `if ($null -eq $p.ExitCode) { throw "wrapper failure: …" }` |
| 140 | catch sets `$exitCode = 1` |

**Pause wrapper PASS.** Do not start another wrapper RSS court.
Body `MINICON_HOST_RSS_RECEIPT` idle **21.45–21.48 MiB** may still be
recorded independently of wrapper status.

## Code reading (pin `745f52b2`, GDI host)

- `native_pixel_window.rs`: one `Vec<u32>` resized to
  `physical_width * physical_height`. At 960×600×1.0 that is 2.25 MiB; at
  150% DPI about 5 MiB. Not 22 MiB by itself.
- `font.rs`: GDI `CreateFontW` / `GetGlyphOutlineW`. `GetFontData` cmap is
  lazy, capped at 4 MiB/face, not `Box::leak` of a TTC.
- MiniCon `cfg(windows)` selects `native-pixel-window`. Do not attribute
  winit/softbuffer RSS to this cell.

## Open measurements

1. Court availability: `win-aarch64-desktop` QGA can lease and
   `interactive-ready`. First push **BLOCKED** on missing
   `target\release\` parent. Retry pushed release PE into existing
   `target\debug\`. Hung job (keep-until-timeout) last live step:
   **`minicon-release-rss.exe` running (~33.6 MiB WS)**; harness image
   absent; `job-*.log` sharing-violation; **no `job-*.exit`**. Agent still
   inside `& harness *> log` (GUI child likely holding redirected handles),
   not a finished RSS FAIL. Detail:
   `research/windows-memory/hang-last-step.md`.
2. Exact **release** RSS on current HEAD (`ad991b0` family / pin `8d8de88`),
   same RSS black box. Debug 22.47 MiB is not that number.
3. If release idle is still ≫ 10 MiB, name surviving GDI/USER/DIB/IME
   working-set from the guest (not macOS vmmap).

## Current-source release court (not the hang WS)

2026-09-06 win-aarch64 UTM, Start-Process harness (no `*>`), 180 s bound.
Hang evidence in `hang-last-step.md` is unchanged. 33,620 K is not this row.

| field | value |
|---|---|
| source | `3419d468c561225e54fa607af85c0479d239c7f6` |
| pin | `745f52b2e169d5b41b51377a82cf8a93a9b00c8b` |
| PE SHA-256 | `d2d08ce7600dfcc73b6b1002f73aef38bc9c4c28545198d403bafb96b4f47ecd` |
| idle | **21.45 MiB** (22,495,232 B) |
| after 2000-line load | 22.00 MiB |
| extra-tab delta | 1.56 MiB |
| four-cycle growth | 9.41 MiB |
| cargo test | `ok` in 1.40 s |
| log | `target/windows-memory/rss-win-aarch64-release-3419d468…-20260906T101551Z.log` |

Job wrapper EXIT 1 on the first Start-Process run was `$p.ExitCode` null
(unknown), **not** coerced to 0 and **not** an RSS FAIL. Later host
`WIN_RELEASE_RSS_EXIT=0` rows are **not** accepted as wrapper proof until
`.exitmeta` records a non-null raw ExitCode. Body `MINICON_HOST_RSS_RECEIPT`
stays independently valid.

Named live allocations from pin `745f52b2` + MiniCon `src/main.rs`
(code reading, **not** a guest GDI-object dump yet; no production patch):

| owner | lifetime | size bound | four-cycle? |
|---|---|---|---|
| `HostState.pixels` one `Vec<u32>` DIB, StretchDIBits | process window | 960×600×4 ≈ 2.25 MiB; 150% DPI ≈ 5 MiB | no (one buffer, resize in place) |
| `PixelFace` `CreateFontW` + DC; cmap `GetFontData` ≤4 MiB/face | `thread_local` `RASTER_FACES` | ≤4 MiB/face, Drop deletes HFONT/DC | **no** — not per-tab; survives close |
| Segoe UI Emoji via GDI mapper (`seguiemj.ttf`), not `Box::leak` TTC | same TLS | mapper, not 183 MiB Color Emoji | no |
| per-tab `BoundedOutputPipe` | session | `READ_BUF*128` = **1 MiB** | should drop on `sessions.remove` |
| per-tab `vt100::Parser` scrollback 4000 | session | empty ~small; filled lines dominate | should drop; **if not, cycle growth** |
| ConPTY / `PtyMaster`+`PtyChild` | session; `shutdown_pty` detaches off GUI thread | unnamed guest WS | candidate if detach leaks |

Idle **21 MiB** is mostly process + one DIB + TLS faces + one session
(~1 MiB pipe). Extra-tab **~1.5–1.8 MiB** matches one more pipe+parser,
not a second DIB or a new font set.

Four-cycle **~9–10.5 MiB** is **not** explained by TLS fonts or the
single DIB. `close_active_session` does `sessions.remove` + `drop(session)`
+ `shutdown_pty`. Remaining suspects: ConPTY/GDI objects not released on
session close, working-set not returned after free, or parser/pipe not
actually dropped. Next measurement is on-guest object/handle counts
around those owners — still no production patch.

Confirmed body-only retest (harness-PID wait, no `-Wait`), same source/pin/PE:

| field | value |
|---|---|
| idle | **21.45 MiB** (22,491,136 B) |
| after 2000-line load | 22.07 MiB |
| extra-tab delta | 1.73 MiB |
| four-cycle growth | 9.93 MiB |
| cargo test | `ok` in 1.41 s |
| `WIN_RELEASE_RSS_EXIT` | recorded 0 — **wrapper PASS paused**; not re-claimed |
| log | `target/windows-memory/rss-retest-now.host.log` |

33,620 K hang WS is not this row. Body idle **21.45–21.48 MiB** is
recordable. Wrapper status is paused. Still above 10 MiB. No production
pin/source patch yet.

Next: on-guest object/handle counts for the named owners above
(DIB / TLS faces / per-tab PTY+vt100 / ConPTY detach). No production
patch until that list is measured. No more RSS-wrapper retests.

Logs and artifacts: `research/windows-memory/`, `target/windows-memory/`.
Source or shared-pin edits wait on an explicit patch list.
