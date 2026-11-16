# cosmocc lane — frozen history (retired with v0.2.27)

These files built and qualified the six-OS polyglot `minicon.com`: a cosmocc
fat-APE trampoline (`loader.c`) with a ZipOS cells overlay (`pack.sh`), its
Authenticode pad/version resources, its installer swap guarantees, and its G3
courts (reaper / lifecycle / cosmocc-swap) plus the APE size court.

The v0.2.27 owner direction (2026-11-16, [plan](../../../plan/plan-v0.2.27.md))
retired this lane: the shipped single-file artifacts are the per-cell Rust
launchers `minicon-<cell>.com` (see `loader/com-rs` and
`loader/pack-rust-all.sh`), the working courts are
`loader/rust-lane-courts.sh`, and the launcher identity digests live in the
`rust_coms` build-receipt section. Nothing in the live release chain invokes
anything in this directory.

The files stay verbatim for archaeology and for any future complete-product
decision to revive a six-OS single file; `git log --follow` on any of them
reaches their operational history through v0.2.26.
