# tinygui named receipts

Host: macOS 26.5.1 aarch64. Court: 960×600 logical window, no activate, no
timer. Settle 5–8 s then `ps` RSS + `sample`. One cell; not six-cell.

Build 2026-09-07 from this tree (`./lab/tinygui/build.sh`):

| rung | SHA-256 | bytes |
|---|---|---|
| tinygui-objc | `d610ffcb9adc4ddaed2e6661ace69f2d6110610776c1acc1582614029475be4a` | 50,976 |
| tinygui-cocoa | `0856d833dd7627ab07423e4a5715fa58e131e7fc73c2e622c1d6fd2ecbb31f28` | 286,672 |
| tinygui-winit | `269659e1820bd503aa8887290cd26d607601222105dc12daafe7126c8f187084` | 409,568 |

Idle after settle:

| order | rung | RSS | CPU% | `sample` |
|---|---|---|---|---|
| 1 | objc | 77.31 MiB (79,168 KiB) | 0.0 | `mach_msg2_trap` |
| 2 | cocoa | 71.17 MiB (72,880 KiB) | 0.0 | `mach_msg2_trap` |
| 3 | winit | 115.42 MiB (118,192 KiB) | 0.0 | `mach_msg2_trap` |
| reverse 1 | winit | 115.44 MiB (118,208 KiB) | 0.0 | `mach_msg2_trap` |
| reverse 2 | cocoa | 75.98 MiB (77,808 KiB) | 0.0 | `mach_msg2_trap` |
| reverse 3 | objc | 77.20 MiB (79,056 KiB) | 0.0 | `mach_msg2_trap` |

Independent cocoa `footprint -p`: **16 MB** physical footprint (RSS still
~71 MiB). Dirty is mostly `MALLOC_SMALL` (9.6 MiB) plus `__DATA_DIRTY`.

Same-host MiniCon `target/release/minicon` (PID 80803, already settled): RSS
70.88 MiB (72,576 KiB), CPU 5.2%. That is not a tinygui cell and not a
qualification court.

## What this decides

- Empty AppKit on this OS already sits at **~71–77 MiB RSS** while **CPU is
  idle in `mach_msg`**. The 10 MiB **RSS** intent is below this host floor.
  Private footprint of the cocoa rung is **16 MiB**.
- Rust objc2 vs clang AppKit is within a few MiB RSS; it is not the MiniCon
  gap.
- Empty winit+softbuffer is **~115 MiB RSS**, ~40 MiB above cocoa, still 0%
  CPU. MiniCon's portable unix host is this stack; an empty winit window is
  not cheaper than current MiniCon RSS on this settled run.
- MiniCon CPU (5.2% on the live window) is MiniCon-owned (blink + present).
  tinygui proves the host can sleep.

## What this does not decide

- Linux or Windows floors (BLOCKED / not executed here).
- Whether WritingTools or Retina backing can be opted out with a public API.
- A MiniCon product patch. Do not copy these numbers into a six-cell cell.
