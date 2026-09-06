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

## Wrapper closure (stop repeating)

exitmeta round on source `e6cd7b0` / pin `745f52b2` / PE `d2d08ce7…`:
`ExitCode_raw=0`, `HasExited=True`, `Handle=2452`, harness pid 10004.
`WIN_RELEASE_RSS_EXIT=0` now has a raw code, not a null→0 coerce.
Idle **21.46 MiB**. No further wrapper courts.

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

GDI live-allocation reading (no patch yet): one `Vec<u32>` frame
(`native_pixel_window.rs`, ~2–5 MiB at 960×600×DPI); `PixelFace` is
`CreateFontW` + optional cmap ≤4 MiB/face, not a leaked TTC; 12-family list
includes Segoe UI Emoji but via GDI mapper. Four-cycle growth ~10 MiB on
Windows is a tab/session lifetime candidate, separate from macOS screenshot
Vecs. No source patch until that growth is named on-guest.

Confirmed body-only retest (harness-PID wait, no `-Wait`), same source/pin/PE:

| field | value |
|---|---|
| idle | **21.45 MiB** (22,491,136 B) |
| after 2000-line load | 22.07 MiB |
| extra-tab delta | 1.73 MiB |
| four-cycle growth | 9.93 MiB |
| cargo test | `ok` in 1.41 s |
| `WIN_RELEASE_RSS_EXIT` | 0 |
| log | `target/windows-memory/rss-retest-now.host.log` |

33,620 K hang WS is not this row. Still above 10 MiB. No production pin/source patch yet.

Next: name the **21 MiB idle** and **~10 MiB four-cycle growth** on-guest
(GDI objects / DIB / cmap / per-tab PTY+vt100). No production patch until
that list exists. No more RSS-wrapper retests.

Logs and artifacts: `research/windows-memory/`, `target/windows-memory/`.
Source or shared-pin edits wait on an explicit patch list.
