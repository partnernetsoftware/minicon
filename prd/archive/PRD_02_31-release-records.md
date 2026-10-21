# PRD 02.31 release-record history (archived 2026-10-22)

Frozen verbatim from `prd/PRD_02_31_v0_2_horizon.md` after v0.2.19 shipped:
the 0.2.0 version-meaning record and the resolved 0.2.1 release-handoff diary.
The unresolved v0.2.1 public-Release discrepancy stays in the live module.

### 0.2.0 — what the version number stands for, and what it does not

The version is bumped in `Cargo.toml` and `release-policy.json` at the SHA
whose evidence reads:

- `./scripts/build.sh test` green under a display server: 517 tests, zero
  failures, across the host unit suite, alignment, the mux black boxes, the
  harness codecs and the 28 GUI black boxes. Those 28 fail as a block when no X
  server is present, which is an environment report and not a product signal;
  they must be run under `xvfb-run -a -s "-screen 0 1280x900x24"`, the same
  invocation `scripts/linux-runtime-qualify.sh` uses.
- `./scripts/six-cell-qualify.sh` with FAIL 0 / PASS 23 / BLOCKED 18 on the
  repinned `agenterm`: `common` fmt, build-fanout and both Windows-cell
  boundary checks, plus all-target-link, throughput-link and artifact
  inspection for `win-x86_64`, `win-aarch64`, `lnx-x86_64` and `lnx-aarch64`,
  and — with `MINICON_APPLE_SDK_ROOT` set to a macOS 15.5 SDK — `clippy`,
  `test-link` and artifact inspection for `osx-aarch64` and `osx-x86_64`. This
  is the only evidence that the repin's Windows console-agent code, the
  `native-tls` arm of `network-http` and the macOS process adapter compile and
  link at all.

Two whole classes of evidence are BLOCKED at this SHA, and the version number
claims neither:

- **No runtime evidence.** Six-cell on a non-macOS host links but never
  executes: all four runtime courts are BLOCKED on unconfigured runners, and
  the Apple cells' `test` and `throughput` stages are BLOCKED because a Linux
  kernel answers `Exec format error` to a Mach-O binary. Nothing here has run a
  MiniCon binary on Windows or macOS. Cross-compilation is not execution, and
  the eighteen BLOCKED stages say so by name rather than by omission.
- **No public release.** The sealed Candidate to Promotion chain is owned by
  the `run-reputation-and-release` skill, with `sign-macos-artifacts` and
  `sign-windows-artifacts` for the signing courts. None of the three is
  registered under `~/.claude/skills/` in the container this version was built
  in, and `AGENTS.md` forbids reconstructing such a procedure from the
  `.github/workflows/` files or hand-rolling a local equivalent. So 0.2.0 is a
  development-complete version, not a released one; it becomes releasable when
  those skills are registered, and public Promotion still needs explicit human
  version and publish authority on top of that.

  **Resolved 2026-09-26.** The owner ran the three skills locally (they were
  registered in that container), and 0.2.0 published as `v0.2.0` (release.yml
  run `36230348611`, Candidate run `36223506557`, source SHA
  `c0b5ed7a2289cbe18c25f92dcae3769101424f7a`) — this repo's own
  `six-grid-cloud-build`/`defender-ci-scan.yml` CI path, not `utm-court`. Two
  real defects surfaced and were fixed on the way, not glossed over: (1)
  `scripts/product-source-hash.sh` needed `MSYS_NO_PATHCONV=1` for the
  `windows-2025` runner's bash step (see its own history); (2) `minicon.com`
  outgrew the 9 MiB Candidate ceiling from `mux`+`harness`, and the owner
  raised it to 11 MiB by explicit dated decision (see "Artifact budget" in
  `PRD_02_27_con_delivery.md`) rather than trimming 0.2.0's scope. This repo
  now also vendors redacted copies of the three skills under
  [`.claude/skills/`](../../.claude/skills/) so a future cloud/Linux-only agent
  does not get stuck the same way — see `AGENTS.md`'s "Skills: look them up
  BEFORE acting". **Still not resolved:** the "No runtime evidence" gap above
  is unchanged — 0.2.0 shipped on cross-compile + static-signing + static-scan
  evidence only, no real execution of the Windows/macOS bytes.

### 0.2.1 local release handoff (2026-09-26)

The owner explicitly authorized publication of 0.2.1 after the previous
agent lost its network connection. macOS contained exec now omits the unsupported
`memory_bytes` limit: the platform adapter rejects that limit before spawning,
so requesting it made every exec fail. Other configured limits remain unchanged;
this is not a claim of a macOS 512 MiB memory ceiling. The complete
`scripts/build.sh test` gate and `scripts/selftest.sh` passed on the local
macOS host. Loopback tests require this host's inherited HTTP proxy variables
to be unset; the initial proxied attempt failed transport tests. Frozen local qualification finished 27 PASS / 0 FAIL / 14 BLOCKED.
The exact Candidate/signing/Defender/Promotion chain and public-download checks
passed; v0.2.1 is published. See `archive/v0.2.1-release-history.md` for run
identities. Unavailable local runtime courts remain explicitly BLOCKED.
