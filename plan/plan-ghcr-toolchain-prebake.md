# GHCR toolchain pre-bake — scoping before building anything

Owner's ask: pre-bake a container image with the cross-compile toolchains
(`cargo-xwin`, `cargo-zigbuild`, `cosmocc`, osxcross, `llvm-rc`) so builds skip
the ~9-10 minute toolchain-install step, reusable by AgenTerm-like multi-arch
projects. `docker` is confirmed available locally.

```text
where does the 9-10 min toolchain install actually happen?
├── minicon-com.yml (macos-15, CI)                              [✓] already cached
│      zig/llvm/cargo-xwin/cargo-zigbuild/cosmocc installed once per
│      Cargo.lock/rust-toolchain.toml hash via actions/cache (line ~38-51);
│      a cache hit already skips the install. No GHCR win here — the cache
│      key already does the job an image would do.
├── six-grid-cloud-build.yml (per-cell CI)                      [✓] not needed
│      each of the six cells builds NATIVELY on its own runner
│      (win-x86_64 on windows-2025, lnx-aarch64 on ubuntu-24.04-arm, ...).
│      No cargo-xwin/cargo-zigbuild anywhere in this workflow — GHCR here is
│      only the publish target for the assembled six-grid runtime, unrelated
│      to toolchain pre-bake. #assumption verified by reading the matrix.
└── scripts/six-cell-qualify.sh (LOCAL, on the release Mac)     {bottleneck} #risk
       cross-builds all six cells from one macOS host: 2 native
       (aarch64/x86_64-apple-darwin) + win-x86_64/win-aarch64 via
       `cargo xwin build` + lnx-x86_64/lnx-aarch64 via `cargo zigbuild`.
       No CI cache backs this path — every fresh local environment (or
       container reset) re-pays the cargo-xwin clang-cl fetch and
       cargo-zigbuild zig fetch cold. This is the actual 9-10 min cost the
       owner is feeling. ->bottleneck
```

```text
can a GHCR image fix the local bottleneck? ->bottleneck
├── the two osx-* cells need a REAL macOS toolchain (Xcode/SDK), which a
│      Linux container on Docker Desktop's VM cannot produce — those two
│      cells must stay on bare macOS. #risk
├── the win-* and lnx-* cells only need cargo-xwin's clang-cl cache and
│      cargo-zigbuild's zig install, which are host-arch-independent blobs
│      cargo already caches under `~/.cargo` and `~/Library/Caches/cargo-xwin`
│      — i.e. this is a HOST CACHE problem, not a toolchain-image problem.
│      A GHCR image adds a second cache mechanism on top of one cargo
│      already has, for zero extra coverage on the two cells that actually
│      dominate cold-start time (win-aarch64/win-x86_64 clang-cl fetch).
└── verdict [✓] don't build a "run six-cell-qualify.sh inside a container"
       image — mixing a Linux container with two required bare-macOS cells
       doesn't fit the actual pipeline shape, and the win/lnx half is
       already a plain directory-cache problem solvable without Docker.
```

```text
where a GHCR pre-bake DOES fit ->fits {fits}
├── the CI-only ubuntu-24.04/ubuntu-24.04-arm jobs that install apt package
│      lists per run (scripts/setup-linux-runners.sh COMMON_PACKAGES,
│      scripts/linux-x11-package-smoke.sh's xvfb/libx11/... list) — these
│      are today's genuine "reinstall the same packages every run" cost on
│      Linux CI, and Docker on Linux runners works natively (no VM tax).
└── a shared base image for a THIRD project (the "agenterm等类似多架构软件"
       reuse case) that also cross-builds FROM Linux with cargo-xwin/
       cargo-zigbuild and has no bare-macOS cell to route around — that
       project's pipeline shape, not this one's, is the real fit.
```

## Decision

Do not build a Docker/GHCR experiment for `six-cell-qualify.sh`'s local
Mac bottleneck — the two macOS-native cells make a container a wrong fit,
and the win/lnx half is a cargo host-cache gap, not a missing image. If the
local cold-start cost still needs fixing, the next real step is warming
`~/.cargo/registry`, `~/Library/Caches/cargo-xwin` and cosmocc/zig once and
keeping them, the same shape `minicon-com.yml`'s `actions/cache` already
proves works — not a container.

A GHCR pre-bake experiment is worth actually building only for: (a) the
apt package lists on `ubuntu-24.04`/`ubuntu-24.04-arm` CI jobs, where Docker
has no macOS-VM tax, or (b) a future Linux-hosted cross-compile pipeline in
a *different* multi-arch project that has no bare-macOS cell to satisfy.
Neither is this repository's current pain point, so this is intentionally
left `BLOCKED` pending one of those two triggers rather than built on
spec — per AGENTS.md, unavailable evidence for "this fixes our slow build"
is `BLOCKED`, not asserted from a plausible-sounding design.
