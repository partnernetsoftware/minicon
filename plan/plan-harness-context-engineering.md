# plan-harness-context-engineering

## Scope decision

Owner (2026-09-27): `harness`'s only two tools (`file`, `exec`) stay as they
are — no third tool. What changes is how a single bounded task's *context* is
built and how the turn loop *decides*, not the tool surface. Owner's own
naming for the two techniques to use: `markdown-tree-dag` (a tree DAG as the
task's working memory) and `mermaid-flowchart-memory-palace` (a flowchart for
the relationships the tree can't show well), composed with a small,
fixed-shape micro-workflow: **categorize task & route model → model drafts a
plan → decision picks a plan → execute the plan → decision: end or jump**.

Non-goal: a general agent framework, a third tool, a new backend wire format.
This plan only changes what text `harness_wire.rs`/`harness_opencode.rs` send
as the task/context and how `run_harness`'s own loop decides to keep going —
both codecs stay dumb transports per the existing rule ("neither backend codec
needs to know sessions exist").

## Tree DAG

```text
harness context engineering: replace flat session-text folding with a
structured working-memory the turn loop can reason over
├── {CTX} tree-DAG working memory                              [_] @host=none
│      invariant: the task's current understanding (subgoals, open
│        questions, decisions already made, their dependencies) is one
│        Markdown indented tree, not prose — this is the thing the model
│        reads and writes each turn, replacing the flat "prior turns folded
│        as text ahead of the new task" scheme HS already ships
│      evidence needed: a fixture test asserting a multi-turn run's saved
│        tree round-trips (parse → mutate one node → serialize) without
│        losing sibling order or a node's evidence/failure annotations
│      safe failure: an unparseable tree on `--continue` is a bounded CLI
│        error, same posture as HS's corrupt-session-file rule — never a
│        silent fresh start
│      dependency: extends the existing `--root/.minicon-harness-sessions/
│        <id>.json` file (HS, closed) rather than a new store
│      non-goal: rendering the tree as anything but Markdown; no TUI tree
│        widget
├── {PALACE} mermaid memory palace                              [_] @host=none
│      invariant: one Mermaid flowchart per session, capturing what the tree
│        can't — shared prerequisites between subgoals, which file a
│        decision's evidence lives in, kill paths (a subgoal marked
│        abandoned, not deleted, so the model doesn't re-attempt it blind)
│      evidence needed: a fixture test that a rendered flowchart's node ids
│        match the tree's `{id}`-tagged nodes 1:1 (no orphan references
│        either direction)
│      dependency: ->CTX (same node-id namespace, so a tree leaf and its
│        palace node are the same identity, never two names for one fact)
│      non-goal: rendering the diagram as an image; text stays Mermaid
│        source, same as this repo's own plan-writing convention
├── {LOOP} micro-workflow state machine                         [_] @host=none
│      invariant: every turn is exactly one of five states — categorize
│        (task type + which backend/model fits it), draft (model proposes
│        one plan step), decide-pick (accept/reject/request another draft),
│        execute (run the picked step through `file`/`exec`, unchanged
│        tools), decide-continue (end vs. jump to another {CTX} node) — and
│        the state transition itself is logged as a tree node, so a session
│        replay shows *why* it moved on, not just what it did
│      #decision the "decide" states are a bounded rule evaluated in
│        MiniCon's own code (turn/tool-call ceiling, explicit stop phrase,
│        max plan-revision count), never the model's own free judgement
│        alone — mirrors `exec`'s existing "bounded, provable" posture
│      ->CTX (state transitions are tree mutations) ->PALACE (a jump target
│        is a palace edge, not a bare tree edge, when it crosses a shared
│        prerequisite)
│      evidence needed: a fixture test per transition (5 minimum) plus one
│        black-box test proving a real multi-step task actually jumps
│        (not just linearly completes) and terminates on the bounded rule,
│        not a live-model whim
│      safe failure: an unrecognized/missing state on resume is the same
│        bounded CLI error as a corrupt tree, not a silent restart at
│        categorize
├── {WIRE} codec neutrality preserved                            [v] @host=none
│      invariant: `harness_wire`/`harness_opencode` still only see one
│        composed string in, one reply string out — CTX/PALACE/LOOP compose
│        that string before the transport call and parse the reply after,
│        the same seam HS already proved works for session folding
│      already true today; this leaf is the regression to guard, not new
│        work — a fixture test asserting the composed prompt sent to a
│        fixture backend contains the tree's Markdown verbatim (not a
│        reformatted summary) is the guard
└── {ROLLOUT} first shippable slice                              [_] @host=none
       #decision smallest slice that is falsifiable end to end: CTX alone,
         still one linear turn per invocation (no LOOP jumping yet), tree
         replaces the flat text fold from HS, session file gains a `tree`
         field alongside HS's existing `turns`. Ship this before touching
         LOOP's jump logic, so a tree-format bug is caught without also
         debugging a new state machine.
       ->CTX
```

## Memory palace

```mermaid
flowchart TD
    subgraph invocation["one minicon harness invocation"]
        CAT[categorize: task type + backend/model route]
        DRAFT[draft: model proposes one plan step]
        PICK{decide: pick step}
        EXEC[execute: file/exec tool, unchanged]
        CONT{decide: end or jump}
    end

    SESSION[("session file\n--root/.minicon-harness-sessions/id.json\nturns[] + tree{}")]
    TREE[[tree-DAG working memory\nMarkdown, {id}-tagged nodes]]
    PALACE_D[[mermaid palace\nshared prereqs, kill paths, edges]]

    CAT --> DRAFT --> PICK
    PICK -- reject/redraft --> DRAFT
    PICK -- accept --> EXEC
    EXEC --> CONT
    CONT -- jump --> CAT
    CONT -- end --> DONE[["print result to stdout, exit"]]

    TREE <-. read/write each turn .-> CAT
    TREE <-. read/write each turn .-> DRAFT
    TREE <-. read/write each turn .-> EXEC
    PALACE_D -. cross-checked on jump only .-> CONT
    TREE -- persisted --> SESSION
    PALACE_D -- persisted --> SESSION

    classDef bounded fill:#fee,stroke:#c33;
    class PICK,CONT bounded;
```

`PICK` and `CONT` are the two bounded-rule gates (`{LOOP}`'s `#decision`
node): they are code, not model whim, same posture as `exec`'s allow-list.
Every other box is model-facing text, composed and parsed at the `WIRE` seam
without either backend codec knowing any of this exists.

## Rollout order

1. `{ROLLOUT}` first: land the tree format alone, still linear (no jump),
   proving CTX round-trips before LOOP exists. Extends HS's session file
   with a `tree` field; `turns` stays for backward-compat read of existing
   session files (a v0.2.x session predates this feature).
2. `{PALACE}` second, additive: only meaningful once there's more than one
   subgoal worth cross-referencing; can ship without LOOP's jump logic since
   a palace with no jump target is just documentation.
3. `{LOOP}` last, highest risk: the state machine and its two bounded gates.
   Needs `{ROLLOUT}` and `{PALACE}` both in place since a jump target is a
   tree node possibly reached through a palace edge.

Each step lands with its own named evidence (fixture tests first, one
black-box test per step) before the next starts, per AGENTS.md's "work
independent leaves in parallel only when file ownership and evidence are
independent" — these three are *not* independent (strict dependency chain
above), so they run in this order, not in parallel.
