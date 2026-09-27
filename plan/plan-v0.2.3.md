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
├── {PALACE} mermaid memory palace                              [_] @host=none
│      invariant: one Mermaid flowchart per session, capturing shared
│        prerequisites, exact-artifact flow and kill paths the tree cannot
│        show well; node ids share the tree's `{id}` namespace
│      evidence needed: a fixture test that a rendered flowchart's node ids
│        match the tree's `{id}`-tagged nodes 1:1 (no orphan references
│        either direction)
│      dependency: ->CTX (closed, `{ROLLOUT}` in `85602f1`)
│      non-goal: rendering the diagram as an image; text stays Mermaid
│        source, same as this repo's own plan-writing convention
│      #decision this is additive-only until there is more than one subgoal
│        worth cross-referencing -- a palace with no jump target is
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
└── {UI-C3} box-drawing glyph gap (Consolas, 1px at 12px)         [-] @host=Windows-display #risk
       carried from plan-v0.2.2.md / plan-carried-debt.md's C3, unresolved
       there. invariant: the rendered glyph closes the 1px gap on a real
       Windows display with Consolas installed
       safe failure: BLOCKED here specifically -- this container has no
         Consolas (`fc-list` confirms, per plan-v0.2.2's own note) and no
         screen a human can inspect even with Xvfb; needs a real Windows
         display host regardless of Xvfb availability
       dependency: a Windows display host with Consolas this session does
         not have
       non-goal: any other UI feature; this is the one named glyph bug

0.2.3 close-out rule (mirrors plan-v0.2.1.md/plan-v0.2.2.md's own scope
reasoning): if {HB} or {UI-C3} are still BLOCKED at 0.2.3's close (no
capable host materialized), that is not a planning failure -- re-affirm
BLOCKED, carry both forward to 0.2.4, and ship 0.2.3 on {PALACE} (+{LOOP} if
it closes cleanly; if {LOOP} alone is still open, it rolls forward too,
since {PALACE}→{LOOP} is a strict dependency chain, not two independent
leaves). A release is not held hostage by leaves this environment
structurally cannot prove.
```

## Memory palace

```mermaid
flowchart TD
    A[0.2.2 shipped, ROLLOUT closed, HB/UI-C3 still BLOCKED] --> B{host available this round?}
    B -->|no host needed| PALACE[PALACE: design + implement + test]
    PALACE --> LOOP[LOOP: 5-state machine + 2 bounded gates]
    B -->|needs macOS/Windows| HB[native-tls live-call proof]
    B -->|needs Windows display + Consolas| UI[UI-C3: box-drawing glyph gap]
    LOOP --> GATE[0.2.3 GATE: fmt + clippy + build.sh test + six-cell-qualify.sh]
    HB -->|host found| GATE
    HB -->|no host| REBLOCK1[re-affirm BLOCKED, carry to 0.2.4]
    UI -->|host found| GATE
    UI -->|no host| REBLOCK2[re-affirm BLOCKED, carry to 0.2.4]
    GATE --> SHIP[0.2.3 release: PALACE closed (+LOOP if clean),
                   HB/UI-C3 re-BLOCKED items explicit in release history]
    REBLOCK1 --> SHIP
    REBLOCK2 --> SHIP
```

`PALACE` and `LOOP` are drawn as a strict chain, not parallel branches,
matching `plan-harness-context-engineering.md`'s own rollout order --
`{LOOP}` cannot start meaningfully before `{PALACE}` exists, since a jump
target may be a palace edge.
