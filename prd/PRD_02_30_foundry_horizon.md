# PRD 02.30 — `foundry` software-production-line horizon

Status: **portfolio horizon, not MiniCon scope**. This records a cross-product
direction the owner has decided on (初心: 抽象与复用 — abstraction and reuse), so
the blueprint is not lost. It is **not** near-term MiniCon work, does **not**
authorize creating a repository yet, and does **not** change any MiniCon
workflow, receipt, or contract. MiniCon remains the reference consumer whose
real release chain is the only proven source of the parts inventoried below.

The canonical home for this blueprint is the private
`partnernetsoftware/dev-lifecycle-playbooks` repository (methodology layer). This
file is a MiniCon-side mirror so the decision is discoverable from the product
that seeded it.

## Why this exists

MiniCon, AgenTerm, and more software queued behind them each re-implement the
same build / test / sign / release machinery. That repetition is the waste the
owner wants removed. The goal is a **software production line** (软件生产线 /
工坊) — a reusable line that mass-produces trustworthy releases — named
**`foundry`** (a chip-foundry metaphor: designs come in,规模化 signed products go
out). It pairs, by metaphor, with the existing `utm-court` (裁决场, the
test-environment engine): **court judges, foundry casts**.

## What `foundry` is — and is not

`foundry` is **not** a new build system or a new CI runner. Those already exist
as *capabilities*. `foundry` is the **assembly line**: the correct assembly
order, the quality gates, the safety red lines, and an AI-friendly driver that
turns those capabilities into a "confidently mass-produce" line.

```text
foundry ≈  GitHub-CI executors (build / sign / release runners)      [capability, exists]
         + utm-court test-environment engine (health / AV courts)     [capability, exists]
         + assembly craft: order + gates + red lines                  [foundry's own asset]
         + AI-friendly driver: one-command runs, structured receipts  [foundry's own asset]
```

The craft is the reusable body. Machines can be swapped per product; the order
and the gates carry over. Everything below marks each part as **[generic]**
already product-neutral, **[parameterize]** generic once a value is lifted out,
or **[minicon]** product-specific and must not be hoisted from one sample.

## Product outcome tree

```text
foundry software production line
├── user problem
│   └── every new product re-implements build/test/sign/release; effort wasted
├── outcome
│   └── one reusable line produces trustworthy releases for many products
├── the four layers
│   ├── github-ci executors                                   [generic capability]
│   ├── utm-court test-environment engine                     [generic capability]
│   ├── assembly craft (order + gates + red lines)            [the reusable asset]
│   └── AI-friendly driver (one-command, structured receipts) [the reusable asset]
├── layer boundaries (do not merge)
│   ├── dev-lifecycle-playbooks  = why (methodology, paper)
│   ├── foundry                  = how, executable (this)
│   ├── company-dev-hub/skills   = how an agent invokes foundry
│   └── utm-court / github-ci    = the machines foundry calls
├── entry gate (do not build the repo until all hold)
│   ├── ≥ 2 real consumers exist (MiniCon + a genuinely different 2nd, e.g. Android client)
│   ├── the release chain has been stable across several versions
│   └── each hoisted part has a named parameter, not a copied constant
└── non-goals
    ├── no repository before the entry gate holds
    ├── no abstraction straightened from a single sample (MiniCon alone)
    └── no weakening of any existing MiniCon gate or red line to fit the line
```

## Parts inventory — the MiniCon release chain, marked for reuse

The order below is the real assembly order; each step's genericness is marked.

1. **six-cell qualify** (`{win,lnx,osx}×{x86_64,aarch64}` fmt/clippy/test/build)
   — **[parameterize]** the cell matrix and per-product ceilings; the harness is
   generic.
2. **one-pack build** (all artifacts from one source state) — **[parameterize]**
   artifact list; **[minicon]** the `minicon.com` APE representation.
3. **dual signing**, Windows Trusted Signing (OIDC) + macOS Developer ID +
   notarization, both `mode=required` — **[generic]** the flow; **[parameterize]**
   signing identities and profiles (must stay out of receipts/logs).
4. **candidate seal** — freeze exact bytes, bind `GITHUB_SHA == source_sha`.
   **[generic]** and load-bearing: never rebuild/re-sign to promote.
5. **court health / AV scan** — native-ISA Defender court (not emulated),
   produces a receipt. **[generic]** via utm-court; **[parameterize]** which
   courts a product requires.
6. **weld receipt → qualification** — `reputation_court.py qualify` converts the
   court receipt into the qualification kind reputation consumes. **[generic]**
   gate; a raw receipt must never be fed to reputation.
7. **reputation** — **[generic]**; **[parameterize]** `mode` (e.g. `defender`).
8. **release** — dry-run then publish; publish *promotes* the sealed candidate,
   never rebuilds. **[generic]**.

### Red lines the line must enforce (all [generic])

- Never bulk-kill the product process during a run (shares the user session's
  kill-on-close job object).
- Never relax/skip the AV scan or edit assertions to force green.
- Never rebuild/re-sign to promote — promotion moves the sealed candidate only.
- Never run exploratory VMs in the release court instance during a release.
- Never commit to `main` during a release (dispatch `--ref` must resolve to the
  candidate's `source_sha`); use a throwaway branch at the SHA if `main` moved.
- Keep signing identifiers out of receipts, logs, and docs.

## New-product onboarding checklist (what the line asks of a consumer)

- Declare the cell matrix and per-cell size/latency/memory ceilings.
- Declare the artifact list and any product-specific representation.
- Provide signing identities/profiles by reference (never inline).
- Declare which courts are required and their native-ISA targets.
- Provide a machine-readable release policy (signing mode, reputation mode).
- Nothing product-specific may leak into the shared craft; if it must, it is a
  parameter, not a fork.

## Second consumer, measured — AgenTerm 0.1.17, 2026-09-19/20

The inventory above was written from one consumer. AgenTerm then ran the same
assembly order for its own 0.1.17 and exercised every step, which is the
condition this file set for deciding what graduates. What follows is what that
attempt cost and what it proved; it is evidence, not a plan to copy AgenTerm.

**Fifteen-plus Candidate rounds, and not one of them failed on a product
defect.** Every failure was a gate or a test whose own assumption had gone
stale or had never held on Windows: a leaked child handle exhausting the qjs
door's 32-slot cap, CRLF reaching a source parser, a cold self-relaunch
exceeding its budget, guest memory exhausted by retained gate output, a
PowerShell ledger that had not been reachable since 2026-09-03, an artifact
budget last validated before seventeen days of work landed.

Three properties of that failure mode are what the line has to answer:

- **Failure is a queue, not a bug.** The first failing gate hides every gate
  behind it, so seventeen days of accumulated drift discharges one item per
  round. AgenTerm's round is ~40 minutes.
- **A gate that is never reached is indistinguishable from a passing gate.**
  The PowerShell ledger threw at its first unaccounted file for seventeen days;
  when it finally ran to completion it reported three more problems at once,
  including that the script could not finish inside its own step budget.
- **Where you discover decides the cost.** The same `agenterm-cu.exe` size that
  took a 40-minute CI round to learn was produced on this Mac by
  `cargo xwin build --release --target x86_64-pc-windows-msvc` in **62
  seconds**, within 2.6% of the figure derived from the ARM artifact.

### What that decides

| inventory step | evidence from the 2nd consumer | verdict |
|---|---|---|
| 1 six-cell qualify | AgenTerm already owns the harness (`agenterm/scripts/qjs/build-all.qjs` and its `six-cell-qualify.qjs`, same `cargo-xwin` + `cargo-zigbuild` drivers, "Building is host-only; running is not"), yet its `candidate.yml` compiles in all six cells: `cargo build|test` appears **17 times** there against **0** in MiniCon's, whose bytes come from one upstream cross-build job | **[generic]** — the harness carried over unchanged; what did not carry is the topology that uses it, one build feeding six execute-only cells |
| 3 dual signing | not exercised: `signing.windows=off`, `macos=unsigned-preview` | stays **[parameterize]**, undecided |
| 4 candidate seal | exact-SHA binding held; `reputation.yml:63` and `release.yml:59` both assert controller `GITHUB_SHA == source_sha` | **[generic]**, confirmed twice |
| 5 court / AV scan | rehearsed end to end on a synthetic candidate before the real one existed, and the rehearsal is what found the blocker: `interactive-ready` timed out claiming its nonce, while a guest-agent scan of the same bytes on the same VM returned exit 0 with an unchanged post-scan hash | **[generic]** via `utm-court`; **new parameter**: which guest path drives the scan, recorded in the receipt |
| 6 weld receipt → qualification | two independent implementations, one rule set. `agenterm-reputation-court.py` (263 lines) and `reputation_court.py` (233) share two function names and nothing else | **[generic]** as a *contract*, not as code |
| 7–8 reputation, release | dispatch input shapes differ between products (`release.yml` takes no `source_sha` and no `version` in AgenTerm; MiniCon's does) | **[generic]** flow, **[parameterize]** inputs — and never from memory: read the workflow file |

### Corrections this evidence forces on the red lines above

- **"use a throwaway branch at the SHA if `main` moved" is wrong as a general
  rule.** The owner forbids branches and worktrees outright — they slow the
  work and introduce their own failures — and AgenTerm's `AGENTS.md` encodes
  that. The line's rule is the invariant, not the workaround: **keep `main`
  frozen at the Candidate SHA for the whole promotion**, and if a fix must
  land, re-cut the Candidate. A branch is one product's permitted means, never
  the line's instruction.
- **Add a red line the first consumer never needed:** a budget that guards a
  compile is a runaway guard, not a contract. Raising it is correct when the
  gate *is* the build and nothing is left to prebuild out of the window; it is
  wrong when the number states a behaviour. Both cases occurred in one night,
  and telling them apart is the judgement the line must encode.

### The code half, for contrast

Both products consume the same `agenterm-platform`. MiniCon enables **13**
features; `agenterm-cu` enables **42**, and ships a 7,302,144-byte control CLI
against a PRD-stated 2 MiB ceiling that was set when the same binary measured
1,420,800 bytes. The crate is not the problem and neither is the ceiling: a
shared platform layer only stays shareable while each consumer takes a
declared slice of it. That is the same rule [`PRD_02_28`](PRD_02_28_shared_core.md)
states from the other direction, and it is the reason this file's boundary is
mechanisms rather than product state.

## Plan — ordered by measured cost, not by ambition

Nothing here authorizes a `foundry` repository. Each item is a MiniCon-side
step that pays off on its own and leaves the line better specified.

- [ ] **Write down "build once, execute six times" as the line's first rule.**
  Not "build locally": both products compile in CI, and they must, because the
  sealed Candidate's provenance is that CI built it from the exact SHA in a
  clean checkout. The rule is about *how many times* the bytes are produced.
  MiniCon cross-builds all six targets in **one** job (`minicon-com.yml`'s
  "pack once (macos-15)" running `rebuild-payloads.sh` with `cargo zigbuild` +
  `cargo xwin`), then six runtime cells download that one artifact and run it
  under `Guard runner identity (no compile)` — each cell first proving its
  `RUNNER_OS`/`RUNNER_ARCH` really is the platform it claims and that the
  SHA-256 matches, before a final job requires "six unique PASS same digest".
  AgenTerm's `candidate.yml` instead compiles in all six cells, so its cells
  attest six independently produced binaries rather than one, and each pays the
  build again — the windows-x86_64 cell alone takes about 40 minutes.
  The developer machine's role is separate and smaller: discovery before
  pushing, never a source of shipped bytes.
- [ ] **Publish the local-gate invocation as part of the line's contract.** A
  gate runnable only in CI is a gate nobody runs. The exact shape matters and
  was learned twice the hard way: `--max-operations 1000000000` (CI's own
  value) and a wall-clock budget sized for a debug build, or the gate reports
  `budget exhausted` and reads as broken.
- [ ] **State the reachability requirement.** A consumer's gate set must be
  provably reachable end to end at least once per release, because an
  unreachable gate reads exactly like a passing one. This is the cheapest rule
  on this page and it is the one whose absence cost the most.
- [ ] **Extract the Defender-court rule set as a contract document**, not as
  code: lease → transport-ready → push exact manifest-selected assets →
  `MpCmdRun -DisableRemediation` → compare pre/post SHA-256 → typed receipt →
  release the lease, with the guest path named in the receipt. Two
  implementations already satisfy it; a third consumer should implement the
  contract, not inherit either script.
- [ ] **Keep `utm-court` as the only shared executable.** It is already
  product-neutral, already consumed by both, and gained Windows
  `interactive-exec` from MiniCon's work. Growing it is cheaper and safer than
  starting a second shared binary.
- [ ] **Defer the driver.** Two consumers have now exercised the assembly
  order; neither has exercised a shared driver. Deciding its shape before a
  third consumer would repeat the mistake this page was written to avoid.

## Deferred decisions

- The driver's interface shape (single binary vs. script set vs. workflow) —
  still deferred; see the plan item above for why the 2nd consumer does not
  settle it.
- Whether `company-dev-hub/skills` call `foundry` directly or through playbooks.
- Numeric per-product ceilings, fixed per consumer at onboarding.
- Dual signing's genericness: AgenTerm 0.1.17 ran with Windows signing `off`
  and macOS `unsigned-preview`, so step 3 is still evidenced by one consumer.
