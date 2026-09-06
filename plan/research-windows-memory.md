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

## Wrapper script (accepted shape)

cdx-wjhk 2026-09-06 read-only confirmed
`research/windows-memory/run-release-rss.sh`: Handle + WaitForExit +
Refresh, null **throws** wrapper failure, raw ExitCode recorded
(`ExitCode_raw=NULL` text, never coerced to 0). That shape is accepted.
No further wrapper courts.

## Contaminated EXIT0 retest — body only, not wrapper

Do **not** keep or cite this row as wrapper confirmation. It is the old
“Confirmed wrapper EXIT0 retest” / “Confirmed body-only retest” table.

| field | value |
|---|---|
| when | 2026-09-06T10:18:27Z, source `3419d46` |
| idle | 21.45 MiB (22,491,136 B) — **body only** |
| after 2000-line load | 22.07 MiB (23,138,304 B) |
| extra-tab delta | 1.73 MiB (1,814,528 B) |
| four-cycle growth | 9.93 MiB (10,416,128 B) |
| `WIN_RELEASE_RSS_EXIT` | 0 **polluted**: null→0 coerce era, no `.exitmeta` |
| log | `target/windows-memory/rss-retest-now.host.log` |
| same bytes | `target/windows-memory/rss-win-aarch64-release-3419d468c561225e54fa607af85c0479d239c7f6-20260906T101827Z.log` |

Same pollution class (EXIT=0, no `.exitmeta`, do not cite as wrapper):

- `target/windows-memory/rss-waitprocess.host.log` (idle 22,519,808 B)
- `target/windows-memory/rss-exitcode-handle.host.log` (idle **22,495,232 B**
  = 21.45 MiB; body used in earlier notes; Handle opened but no
  `.exitmeta`)

First Start-Process run
`…-20260906T101551Z.log` / `rss-retest-3419d46.host.log` is EXIT 1 with
body 22,495,232 B: wrapper failure on null, **not** RSS FAIL, **not**
coerced. Hang 33,620 K is not any of these rows.

## Accepted complete court (body + wrapper)

cdx-wjhk accepted latest body **and** wrapper closure. Exact log (has
`.exitmeta` `ExitCode_raw=0`):

`target/windows-memory/rss-win-aarch64-release-e6cd7b0fbab8c3f22de8b5aef54e306d71958bf6-20260906T102522Z.log`

Host alias: `target/windows-memory/rss-exitmeta.host.log`.
Sidecar: same path + `.exitmeta`.

| field | value |
|---|---|
| source | `e6cd7b0fbab8c3f22de8b5aef54e306d71958bf6` |
| pin | `745f52b2e169d5b41b51377a82cf8a93a9b00c8b` |
| PE SHA-256 | `d2d08ce7600dfcc73b6b1002f73aef38bc9c4c28545198d403bafb96b4f47ecd` |
| idle | **21.46 MiB (22,507,520 B)** |
| after 2000-line load | 22.02 MiB (23,085,056 B) |
| extra-tab delta | 1.54 MiB (1,609,728 B) |
| four-cycle growth | 9.13 MiB (9,576,448 B) |
| cargo test | `ok` in 1.40 s |
| exitmeta | `HasExited=True`, `ExitCode_raw=0`, `Handle=2452`, pid 10004 |
| `WIN_RELEASE_RSS_EXIT` | 0 |

Same PE’s body spread is 21.45–21.46 MiB. Public 21.45 MiB (22,495,232 B)
is the earlier body receipt, not this complete log. Main PRD upsert is
cdx-wjhk’s. Still above 10 MiB. No production pin/source patch yet.

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
| heap vs WS | idle private **6.25 MiB**, WS **21.45**; load +**11 MiB private**; cycle after settled load **~0** |
| residue | +6 handles, +3 USER — not 10 MiB |

RSS-court four-cycle +9 MiB is **WS catching up to already-filled
parser/heap**, not leftover tab owners. Do not cut PTY capacity.
macOS `finishLaunching` +22.59 MiB is not this remainder.

Logs: `research/windows-memory/live-owners.md`.
Driver: `research/windows-memory/run-close-owners.sh`.

Do not edit `src/`, Cargo, or the public memory PRD from this branch.

Logs and artifacts: `research/windows-memory/`, `target/windows-memory/`.
