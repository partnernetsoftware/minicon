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

Product definition, invariants and non-goals are owned in
`prd/PRD_02_23_minicon.md` ("Product boundary" / "Governing invariants"),
narrowed for the 0.2.x `mux`/`harness` subcommands in
`prd/PRD_02_31_v0_2_horizon.md`. Read those before changing scope — do not
restate or re-derive the boundary here; this file only points at it.

Local UTM/Lima court calling conventions (never route AgenTerm through
MiniCon's scripts; a missing court is `BLOCKED`, never a skipped pass) are
detailed in `prd/PRD_02_27_con_delivery.md`'s "Delivery ownership" section.

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

## Where the bytes come from, and what a test costs

The build/test/sign/scan division of labour (this Mac vs. GitHub-hosted
runners vs. `utm-court`), the rejected-and-why "whole build in CI" proposal,
measured per-cell timings, and the Windows-vs-Unix platform quirks that cost a
round each to learn are owned in `prd/PRD_02_27_con_delivery.md` under "Where
the bytes come from" and "Where a test runs, and what that costs". Read them
before redesigning any part of the pipeline or transport — they record why the
current shape exists, not just what it is.

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
