# plan-v0.2.11 — GDI band-present out-of-bounds fix

## Problem

v0.2.10 crashed (`0xc0000005`, faulting module GDI32.dll offset 0x226c)
shortly after a busy TUI (codex via mycodex.bat) started. v0.2.9 did not:
the crashing call path did not exist before 0.2.10.

## Root cause

`9892242` offset the DIB bit pointer to the band origin **including the
column component** (`source.top * stride + source.left`) while keeping
`biWidth` at the full frame stride. GDI reads `biWidth` pixels per row from
the offset start, so the final row ran `source.left` u32s past the end of
the `width*height`-sized pixel buffer. A bottom-band present (bottom ==
frame height) with left > 0 — which a busy TUI emits constantly — read
out of bounds on every partial present; when the read crossed a page
boundary GDI32 faulted with an access violation.

Why the 0.2.10 gate missed it: the band math was inline at the call site
and nothing asserted the read window against the buffer extent. The
blackbox journeys present bands whose left is 0 often enough that the
unclamped form survived the suite by luck of allocation slack.

## Fix

`band_present_read_window(source, frame_width, frame_height) -> (offset,
len)`: the single derivation of the read window. The band present now
offsets the pointer by whole rows only (`source.top * stride`) and asks
for a full-stride band (biWidth = stride, biHeight = exposed rows). The
paint DC clips output to the update region, so columns outside the band
cost clipped blits, not pixels. `offset + len` ends exactly at the buffer
end for a bottom band, and strictly inside it for interior bands.

## Evidence

- `band_present_read_window_stays_inside_the_frame_buffer` (new, in
  native_pixel_window tests): pins the crash case (720x400 frame, band
  40..680 x 300..400) — the shipped 0.2.10 offset form is asserted
  provably out of bounds; the fixed form ends exactly at the buffer end.
- `debug_assert!` at the present call site now guards the contract in
  debug builds.
- Full gate: `scripts/build.sh test` 1185 passed / 0 failed (one known
  flake under full-suite host starvation, `repeated_ctrl_wheel_zoom…`,
  passes standalone and on rerun; one known sandbox HTTP_PROXY artifact
  removed via env).

## Non-goals

- No change to the scroll-damage model, the guard removal, or the
  amplifier work — 0.2.10's flashing fix stands; this is strictly the
  out-of-bounds read.
- {AMPLIFIER-T} remains deferred as in 0.2.10.

## GATE status

Local gate green 2026-10-15. Release chain via `scripts/release.sh 0.2.11`
per the standardized process.
