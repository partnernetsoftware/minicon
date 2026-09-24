# MiniCon agent guide

Start at `PRD.md`. It is the compact product index and memory palace; follow
its links to the owning `prd/PRD_*.md` module for detailed behavior and
evidence. Machine truth lives in `alignment-contract.json`, public evidence
identities in `evidence-registry.json`, and release selection in
`release-policy.json`.

## Working language

Think, reason, and discuss with the user in Chinese (中文). Code, identifiers,
commit messages, and file/doc content stay in their existing language (mostly
English); only the assistant's own thinking and conversational replies default
to Chinese unless the user asks otherwise.

## Planning method: tree DAG + memory palace

Use both views for material product planning; neither replaces the other.

1. Write a Markdown tree DAG first. Begin with one user outcome, split it into
   capability owners, and give every delivery leaf its invariant, observable
   evidence, safe failure, dependency and explicit non-goal.
2. Draw one Mermaid flowchart memory palace for relationships that the tree
   cannot show well: shared prerequisites, exact-artifact flow, parallel
   courts, authority boundaries, decision gates and kill paths.
3. Keep one owner per fact. `PRD.md` is the compact index; an owning PRD module
   holds current product truth; `plan/` holds sequencing; `plan/archive/` and
   `prd/archive/` preserve superseded decisions and release history.
4. Upsert accepted scope and status into the owning PRD before archiving a
   completed plan. Link instead of copying. `[x]` requires named evidence;
   unavailable evidence is `BLOCKED`, never silently skipped.
5. Before implementation, identify shared prerequisites and hot files. Work
   independent leaves in parallel only when their file ownership and evidence
   are independent; integrate and run final gates serially.

## Product boundary

MiniCon is a one-file local terminal. Preserve these exclusions: no server,
persistent workspace, Fleet, mux, MCP, script runtime, plugin host or Agent
permission policy. Its `--control` endpoint's lifetime is bound to the
**process**, not to the window: it is bound before any window exists and dies
with the process. That amendment (0.1.19) admits a GUI that can be detached and
reattached; it does not admit a server, a daemon, a listening network
interface, a persistent workspace, session sharing or discovery, or background
auto-start. A MiniCon without a window is the same single process, minus its
window.

Preserve these invariants:

- Child exit retains its tab, final screen and exit status until explicit close.
- Closing a parent promotes direct children; parent cycles are rejected.
- Closing the final tab leaves the window open on its greeting state.
- Composer and terminal input have one focus owner; Enter inserts a newline and
  Ctrl+O sends the complete draft.
- Native callbacks never unwind across FFI; bounded failures remain local.
- One platform's evidence never becomes a six-cell claim.

Local UTM and optional Lima courts are not MiniCon product code. Lifecycle,
guest adapters and image recipes live in sibling `utm-court`
(`partnernetsoftware/utm-court`). MiniCon calls those CLIs from
`scripts/*-utm-runner.sh` and `scripts/lima-court.sh`. AgenTerm has its own
caller and must not be routed through MiniCon scripts. Missing court is a
locator failure or `BLOCKED`, never a skipped PASS. Caller map:
`~/repos/utm-court/CALLERS.md`. Sequencing: `plan/archive/plan-utm-court-extract.md`.

## Skills: look them up BEFORE acting

Signing, the release chain and the VM courts all have registered skills. Invoke
the skill first — do not reconstruct the procedure from memory or from reading a
workflow file, and do not hand-roll a local equivalent.

| Task | Skill |
| --- | --- |
| macOS codesign / notarize / staple / Gatekeeper / `.app` packaging | `sign-macos-artifacts` |
| Windows Authenticode / Azure Artifact Signing / APE `.com` | `sign-windows-artifacts` |
| sealed Candidate → Defender court → reputation → release publish | `run-reputation-and-release` |

Skills are registered under `~/.claude/skills/`, which is also where to read
them. **A skill's top-level page is only its index — the operational detail
lives in its `references/`. Read those before you run anything.** Skipping them
means re-deriving what is already written down, wrongly. Real instance
(2026-09-17): a local `codesign` was run against the login keychain and hung
forever on an authorization prompt, while
`~/.claude/skills/sign-macos-artifacts/references/apple-signing-setup.md` §4
already specified a temporary keychain plus `set-key-partition-list` for exactly
this reason — and §6 already flagged that a bare Mach-O may not be staplable.

When a task teaches something durable, write it back into the owning skill (and
its `references/`), not only into private notes. A skill that is not registered
under `~/.claude/skills/` is invisible to every future session — register it.

## Development and delivery

From the repository root:

```bash
./scripts/build.sh dev
./scripts/build.sh release
./scripts/build.sh test
./scripts/selftest.sh          # the checks that guard the scripts, not the product
./scripts/six-cell-qualify.sh
```

Run `six-cell` **before** pushing, not after CI fails. It is the only thing that
cross-compiles, and 0.1.20 lost three build rounds to changes that a macOS build
accepts and Linux does not — a `Cell` imported under a macOS `cfg`, among
others.

Work directly on `main`. Do not create worktrees or side branches for MiniCon
work — the owner has ruled that out after it cost real time. The one exception
is the throwaway `candidate-src-<v>` branch a release dispatch needs when `main`
has moved past the Candidate SHA; delete it immediately after.

When you change the pinned `agenterm` platform crate, verify with **MiniCon's**
build, not with a `cargo check` inside agenterm. That crate's default feature
set omits features MiniCon enables (`pty` among them), so whole modules — the
Windows console agent, for one — are not compiled there at all and a green check
proves nothing about them. Measured: two "clean" agenterm checks in a row while
the code did not compile under MiniCon's features.

Before you commit or run six-cell, run `cargo fmt` and
`cargo clippy --all-targets -- -D warnings`. six-cell gates on fmt and clippy
while `cargo test` does not, so a change that passes every test can still fail
the release gate and cost a whole round. Painting helpers with many parameters
carry `#[allow(clippy::too_many_arguments)]` (precedent in `terminal_paint.rs`).

Do not pick a product binary by modification time. Canonical local outputs are
documented in `README.md`. Do not commit generated binaries; local outputs stay
under ignored `target*/`, `dist/` or research output directories.

Release is exact-source Candidate followed by no-rebuild Promotion. Signing is
a `release-policy.json` choice, not a fallback inferred from credentials.
Public Promotion always requires explicit human version and publish authority.

## Where the bytes come from

Owner decision, 2026-09-24. This is the division of labour; anything in a plan
or a PRD that contradicts it is out of date, not an alternative.

| stage | where | why |
| --- | --- | --- |
| six-cell cross-compile, every iteration | **this Mac** | APFS clone plus incremental rebuilds a cell in about 4 s; a CI job starts cold |
| test execution | **GitHub hosted runners** | they have a real logged-in desktop; `minicon_control` runs 9/9 in 8.5 s |
| fallback tests | `utm-court` | Defender scans, legacy images, offline, long debugging |
| signing and stamping | **CI** | the only stage that must be there, because the keys are there |
| cold-build verification | a GHCR image, weekly or before a release | proves the build does not depend on this machine's local state |

Moving the whole build into CI was considered and rejected on 2026-09-24. The
reasoning, so it is not re-litigated from scratch:

- A GHCR image covers four cells, not six. Containers run on Linux runners
  only, so the two macOS cells fall back to `macos-14`/`macos-13` -- and
  `macos-13` is the queue that cost two rounds over an hour each.
- The loop would lose incremental builds. GitHub's per-repository cache quota
  does not hold six target trees, so restores evict and jobs go cold. That
  trades a 4-second rebuild for a network round trip, which is the shape of
  "CI as a debugger" this repository forbids.
- Splitting the six cells across several images changes what the Candidate's
  byte set attests. That is a deliberate piece of work, not a side effect.

What the proposal was actually after -- proof that the build does not depend on
one machine's accumulated state -- is a low-frequency cold-build job. That
answers the question at the rate it is asked, instead of taxing every
iteration.

That 2026-09-24 rejection covered moving the *whole* build into CI as the
routine loop, and still stands for that. A separate, narrower question --
whether a Linux CI runner could ever produce the two macOS cells at all,
as a low-frequency fallback rather than the routine path -- is still open
and tracked as `[ ]` in `prd/PRD_02_27_con_delivery.md` under "BLOCKED --
non-Mac macOS cross-compile from a Linux CI/cloud host", with what has and
has not been verified so far. Do not read that entry as reversing this
section's decision, and do not upgrade it past `[ ]` without a real
Mach-O produced end to end and named evidence, per this file's own
`[x]`-requires-evidence rule.

## Where a test runs, and what that costs

Measured 2026-09-23; the numbers are why, not decoration.

**GitHub first, the local court as the fallback.** Cross-compile here, upload
the test executables, let the runners execute them. CI builds nothing outside
a release.

| cell | host | measured |
| --- | --- | --- |
| lnx-x86_64, lnx-aarch64 | GitHub `ubuntu-24.04`, `ubuntu-24.04-arm` | 5 s per job |
| win-x86_64, win-aarch64 | GitHub `windows-2025`, `windows-11-arm` | 9-10 s per job |
| osx-aarch64 | this Mac, natively | under 1 s |
| osx-x86_64 | this Mac, under Rosetta | 2 s. GitHub `macos-13` sat queued for 7 minutes; keep it out of the loop |
| Defender scan, legacy images, offline, long debugging | utm-court | 20-26 s per round |

A hosted Windows runner **has a real logged-in desktop** (`runneradmin`,
session 2, Active, 1024x768) and MiniCon's GUI journeys run on it:
`minicon_control` 9/9 in 8.5 s, `minicon_blackbox` 29 passed / 1 failed /
1 ignored in 164 s. The local court used to be assumed necessary for those;
it is not, and every routine round that stays off the Mac keeps it cool.

`scripts/round.sh` is the one entry point: pre-flight, build, route, one
receipt with per-stage timings. A cell whose backend cannot answer is
BLOCKED, never a silent pass.

Transport facts that cost a round each to learn:

- A **draft** release is not reachable from a job ("release not found"), and a
  job that fetches a draft asset by id gets HTTP 403 "Resource not accessible
  by integration". That is the design, not a misconfiguration: a draft is
  visible only to an identity with push access. No `GITHUB_TOKEN` permission
  set fixes it, and neither does a read-only fine-grained PAT -- seeing a draft
  requires Contents: **write**, which is exactly what a Candidate must not
  have. Do not spend a round retrying the download with another permission
  combination. Use a tagged prerelease and delete it afterwards; when the bytes
  must not be publicly usable, upload them encrypted and let the job decrypt
  with a repository secret, so the read-only token stays sufficient.
- The releases-by-tag endpoint can answer with an empty asset list while the
  release's own assets endpoint reports the file as uploaded. Fetch by release
  id inside a job.
- `cargo xwin` only reaches the network when a proxy is configured; with the
  CRT/SDK cache present it builds offline in seconds. Do not export a proxy
  for builds -- a 503 through it failed two cells.
- Bound every wait on something outside this machine. An unbounded
  `gh run watch` on a queued `macos-13` held two rounds open for over an hour.
- In the court, `MINICON_WINDOWS_ONE="<suite> <test>"` runs a single test
  (~20 s per round), `MINICON_WINDOWS_UTM_DISPOSABLE=0` keeps the guest warm,
  and an in-memory pause resumes to desktop-ready in 5-9 s against ~100 s
  cold. Release the court when the sequence ends; it is shared with AgenTerm.

Windows behaves differently from Unix in ways that are the platform's, not
bugs to "fix" in the product:

- A program that cannot start is not a spawn error: ConPTY creates the console
  host first, so it arrives as a child that exited (code 251).
- ConPTY forwards viewport changes, not bytes, so a PTY byte count can never
  cover what a program wrote (1 MiB gave 6,628 bytes; 32 MiB gave 7,014).
- Deleting a running image only marks it delete-pending, so the path still
  resolves; deny execute instead when a spawn must fail.

## Change and documentation rules

- Put cross-platform mechanisms in the shared platform crates and product
  meaning in MiniCon-owned code. Do not make MiniCon depend on AgenTerm product
  state or evidence.
- Exercise observable behavior through public CLI/GUI black boxes.
- Every test must be provable: change the code it guards and watch the test
  fail before you trust it. A test that passes with its guard removed is
  documentation, not evidence.
- Run the test gate through `./scripts/build.sh test`, not bare `cargo test`. It
  denies `dead_code`, `unused_variables` and `unused_must_use`, because a test
  that loses its `#[test]` attribute compiles as dead code and silently runs
  nothing, and an unused `Result` hides a setup that failed. Add a test and
  confirm the gate's test count went up.
- Keep `README.md` brief and human-facing; keep operational agent rules here.
- Preserve user changes in a dirty tree and stage only exact reviewed paths.
- In public documents use repo-relative paths for this clone and `~/...` for
  locations below a user home. Never write real emails, hostnames, IPs,
  credentials, cloud identifiers or expanded home paths.
