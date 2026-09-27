# plan-v0.2.3

## Scope decision

Owner (2026-09-27): 0.2.2 shipped (`79f64a5`). Following `plan-v0.2.2.md`'s
own rule ("if still blocked then, roll forward again rather than let it
block a release"), 0.2.3 carries forward 0.2.2's two structural `BLOCKED`
leaves (`{HB}` native-tls live-call proof, `{UI}` C3 box-drawing glyph gap)
and adds the active, already-in-progress harness context-engineering work
(`plan-harness-context-engineering.md`'s `{PALACE}` and `{LOOP}`) as this
version's own new scope — `{ROLLOUT}` (the tree-DAG leaf) is already closed
(`85602f1`, on `main`).

Non-goal: 0.3.x's `harness-manage`/GUI workbench — still explicitly out of
scope per `prd/PRD_02_31_v0_2_horizon.md`'s 2026-09-26 decision. This is a
patch/feature version continuing 0.2.x's own horizon, not a 0.3.x design
start.

## Tree DAG

```text
0.2.3: harness context engineering ({PALACE}, {LOOP}) + roll forward 0.2.2's
structurally-blocked leaves
├── {PALACE} mermaid memory palace                              [v] @host=none
│      invariant: one Mermaid flowchart per session, capturing shared
│        prerequisites, exact-artifact flow and kill paths the tree cannot
│        show well; node ids share the tree's `{id}` namespace
│      closed 2026-09-27: see `plan-harness-context-engineering.md`'s
│        `{PALACE}` leaf for full evidence -- `render_palace` in
│        `src/harness.rs`, persisted into `SessionState.palace`, 3 new
│        fixture tests, full harness unit suite (20/20) green
│      dependency: ->CTX (closed, `{ROLLOUT}` in `85602f1`)
│      non-goal: rendering the diagram as an image; text stays Mermaid
│        source, same as this repo's own plan-writing convention
│      #decision this shipped additive-only, as planned: it renders only the
│        tree's own hierarchy for now -- a palace with no jump target is
│        documentation, per the context-engineering plan's rollout order
├── {LOOP} micro-workflow state machine                         [_] @host=none
│      invariant: every turn is exactly one of five states -- categorize,
│        draft, decide-pick, execute, decide-continue -- with the two
│        "decide" states as bounded code-side rules, never model whim,
│        mirroring `exec`'s existing allow-list posture
│      evidence needed: a fixture test per transition (5 minimum) plus one
│        black-box test proving a real multi-step task actually jumps (not
│        just linearly completes) and terminates on the bounded rule
│      dependency: ->PALACE (a jump target may cross a shared prerequisite
│        recorded only in the palace, not the tree alone) -- strict order,
│        not parallel with {PALACE}
│      safe failure: an unrecognized/missing state on resume is the same
│        bounded CLI error as a corrupt tree, never a silent restart
│      #risk highest-risk leaf of the three; may not close inside 0.2.3 --
│        if so, re-BLOCK explicitly rather than claim partial completion,
│        same posture as {HB}/{UI} below
├── {HB} native-tls live-call proof                              [-] @host=macOS/Windows #risk
│      carried from plan-v0.2.2.md, unresolved there. invariant: the
│        native-tls arm of `network-http` actually makes a live HTTPS call
│        on Windows and/or macOS, not just compiles/links
│      evidence: six-cell-qualify.sh's native-tls cell moves BLOCKED→PASS on
│        a real Windows or macOS runner/court
│      safe failure: BLOCKED (as in 0.2.1/0.2.2) if no such host becomes
│        available during 0.2.3 -- never claim closed on cross-compile
│        evidence alone
│      dependency: a Windows or macOS host/court this session does not have
│      non-goal: touching the proven Unix Rustls/WebPKI arm
├── {UI-C3} box-drawing glyph gap (Consolas, 1px at 12px)         [-] @host=Windows-display #risk
│      carried from plan-v0.2.2.md / plan-carried-debt.md's C3, unresolved
│        there. invariant: the rendered glyph closes the 1px gap on a real
│        Windows display with Consolas installed
│      safe failure: BLOCKED here specifically -- this container has no
│        Consolas (`fc-list` confirms, per plan-v0.2.2's own note) and no
│        screen a human can inspect even with Xvfb; needs a real Windows
│        display host regardless of Xvfb availability
│      dependency: a Windows display host with Consolas this session does
│        not have
│      non-goal: any other UI feature; this is the one named glyph bug
└── {RELTOOL} release-chain tooling hardening                    [_] @host=none
       owner-requested 2026-09-27, from the Mac-side signer/publisher's own
       post-mortem on the real v0.2.2 dispatch (candidate 36327096656,
       defender-ci-scan 36327312549, reputation 36327475537, release
       36327567587) run concurrently with this same Linux session's
       unrelated docs/plan pushes to `main`. Six concrete findings, in
       impact order (highest first); each is independently closeable, no
       forced sequencing between them:
       1. `company-signing.yml`/`macos-signing.yml`'s preflight still binds
          on raw `origin/main == source_sha` instead of the
          `scripts/product-source-hash.sh` comparison
          `defender-ci-scan.yml`/`release.yml` already use. A docs/plan-only
          push landing on `main` mid-chain fails these two signing
          dispatches outright (measured twice, live, during the v0.2.2
          run) even though the same push would not fail candidate/
          defender-ci-scan/reputation/release. Fix: port the
          product-source-hash comparison into both signing workflows'
          preflight, matching the comment already in
          `defender-ci-scan.yml` ("Product source tree only, not raw HEAD
          equality").
       2. Every preflight `[[ cond ]]` assertion in these workflows fails
          silent under `set -euo pipefail` -- the run just says "Process
          completed with exit code 1" with no indication of which
          condition tripped. Diagnosing the v0.2.2 failures required
          replaying every check by hand in a local shell. Fix: give each
          assertion an explicit `|| { echo "<reason>" >&2; exit 1; }`,
          matching the style already used for the "release-eligible
          signing requires an unpublished version" check later in the same
          scripts.
       3. No lock/lease signals a release chain is in flight on `main`.
          Coordination during v0.2.2 was pure ad hoc mux chat ("please
          stop pushing for 15 minutes"), and it was still violated once by
          a different concurrent session pushing docs/plan commits,
          costing a full minicon-com.yml rebuild and two re-dispatches.
          Fix: a lightweight release-in-progress marker (a checked-in
          `release-lock.json` with holder/source_sha/expiry, or a draft
          GitHub Deployment) that dispatch scripts check before pushing to
          `main` and clear on completion/timeout -- replacing verbal
          coordination with something every session can check
          mechanically.
       4. `target-six/builds/` grew to 150 GB on the Mac signer's host
          (duplicate hash-keyed cache directories plus a whole stale
          `target-six-0.2.1/` sibling), and
          `scripts/cleanup-build-state.py --scope all` freed almost
          nothing (only `__pycache__`) -- `six-cell-qualify.sh` hit
          `ENOSPC` and had to be rerun after ~70 GB of manual cleanup. Fix:
          give `target-six/builds/<hash>` cache directories and
          `target-six-<version>` siblings an actual staleness/TTL policy
          inside `cleanup-build-state.py`'s `six-cell`/`all` scopes, not
          just routine leftovers.
       5. `six-cell-qualify.sh`'s default `BUILD_JOBS=5` fully saturates a
          14-core host; under that contention,
          `host_process_rss_stays_within_named_budget` (in
          `tests/minicon_control.rs`) intermittently failed at 17.70 MiB
          against its 16 MiB extra-tab ceiling, while isolated
          `cargo test --release` reruns held steady at ~9 MiB (3/3), and
          `MINICON_BUILD_JOBS=2` made the full six-cell run clean
          (FAIL=0 PASS=27). This is measurement noise, not a product
          regression, but it cost a full extra six-cell round to prove.
          Fix: either default `BUILD_JOBS` to something core-count-aware,
          or move RSS-budget tests out of the concurrent build-fanout
          window (run them serially, after the fan-out settles).
       6. Corrected 2026-09-27 (the first write-up of this item named the
          wrong script): `scripts/ci-release.sh` covers only
          candidate/reputation/release -- it has no signing subcommand at
          all, so `company-signing.yml`/`macos-signing.yml` still get
          dispatched by hand every time, `qualification_only` default
          (`true`) included; a caller who forgets `-f
          qualification_only=false` gets an instant, silent preflight
          failure (see #2) instead of a release-eligible signature (hit
          live during the v0.2.2 signing dispatch). Separately,
          `ci-release.sh`'s three subcommands each hardcode
          `minicon_ci_dispatch_and_wait <workflow>.yml main ...` -- there is
          no way to pass a pinned `candidate-src-<v>` branch instead of
          `main`, so the script cannot be used at all once `main` has moved
          past the Candidate SHA (exactly the situation `{RELTOOL}`#3's
          lock is meant to prevent, and what actually happened twice during
          v0.2.2). Fix: add a `candidate`/`macos-signing` subcommand (with
          an explicit `--release`/`--qualification-only` flag, mirroring
          the workflow's own boolean) and an optional ref override (e.g. a
          fourth positional arg or `MINICON_CI_REF`) to all subcommands.
       evidence needed: for #1/#2, a modified preflight script plus a
         fixture dispatch (or a documented dry run) proving a docs-only
         push no longer breaks signing and a failing assertion prints its
         reason; for #3, the lock is checked by at least one real dispatch
         path; for #4/#5, before/after disk and six-cell timing numbers;
         for #6, `ci-release.sh --help` documents the new flag
       dependency: none of the six block on {PALACE}/{LOOP}/{HB}/{UI-C3};
         pure release-infrastructure work, independent of this version's
         product leaves
       non-goal: redesigning the release chain's stage order or adding new
         gates; this is hardening the existing five-stage chain
         (minicon-com -> signing -> candidate -> defender-ci-scan ->
         reputation -> release), not changing its shape

0.2.3 close-out rule (mirrors `plan-v0.2.1.md`'s and `plan-v0.2.2.md`'s own scope
reasoning): if {HB} or {UI-C3} are still BLOCKED at 0.2.3's close (no
capable host materialized), that is not a planning failure -- re-affirm
BLOCKED, carry both forward to 0.2.4, and ship 0.2.3 on {PALACE} (+{LOOP} if
it closes cleanly; if {LOOP} alone is still open, it rolls forward too,
since {PALACE}→{LOOP} is a strict dependency chain, not two independent
leaves). {RELTOOL}'s six sub-items are each independently closeable and
should ship as far as they get -- a partial {RELTOOL} (e.g. #1/#2/#6 closed,
#3/#4/#5 carried) is still real progress, not a BLOCKED leaf; only carry
forward the sub-items that didn't get done. A release is not held hostage
by leaves this environment structurally cannot prove.
```

## Memory palace

```mermaid
flowchart TD
    A[0.2.2 shipped, ROLLOUT closed, HB/UI-C3 still BLOCKED] --> B{host available this round?}
    B -->|no host needed| PALACE[PALACE: design + implement + test]
    PALACE --> LOOP[LOOP: 5-state machine + 2 bounded gates]
    B -->|needs macOS/Windows| HB[native-tls live-call proof]
    B -->|needs Windows display + Consolas| UI[UI-C3: box-drawing glyph gap]
    B -->|no host needed| RELTOOL[RELTOOL: six release-tooling fixes]
    LOOP --> GATE[0.2.3 GATE: fmt + clippy + build.sh test + six-cell-qualify.sh]
    RELTOOL --> GATE
    HB -->|host found| GATE
    HB -->|no host| REBLOCK1[re-affirm BLOCKED, carry to 0.2.4]
    UI -->|host found| GATE
    UI -->|no host| REBLOCK2[re-affirm BLOCKED, carry to 0.2.4]
    GATE --> SHIP[0.2.3 release: PALACE closed (+LOOP if clean),
                   RELTOOL's closed sub-items, HB/UI-C3 re-BLOCKED items
                   explicit in release history]
    REBLOCK1 --> SHIP
    REBLOCK2 --> SHIP
```

`PALACE` and `LOOP` are drawn as a strict chain, not parallel branches,
matching `plan-harness-context-engineering.md`'s own rollout order --
`{LOOP}` cannot start meaningfully before `{PALACE}` exists, since a jump
target may be a palace edge.
