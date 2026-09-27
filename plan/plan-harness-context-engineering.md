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
├── {PALACE} mermaid memory palace                              [v] @host=none
│      invariant: one Mermaid flowchart per session, capturing what the tree
│        can't — shared prerequisites between subgoals, which file a
│        decision's evidence lives in, kill paths (a subgoal marked
│        abandoned, not deleted, so the model doesn't re-attempt it blind)
│      closed 2026-09-27: `render_palace` (src/harness.rs) renders one
│        Mermaid node per `TreeNode` in the tree's own `{id}` namespace plus
│        one edge per tree parent/child relationship; recomputed and
│        persisted into `SessionState.palace` on every `save_session` call,
│        never hand-edited, never read back into `tree` — derived-only, so
│        it cannot drift out of sync with the tree it renders. Shared-
│        prerequisite/kill-path edges beyond the tree's own hierarchy are
│        deliberately not added yet: `{LOOP}` needs a decision state that
│        can mark one before there is any such fact to render.
│      evidence: `palace_node_ids_match_the_trees_id_tagged_nodes_one_to_one`
│        (fixture, asserts no orphan node either direction plus the exact
│        parent-->child edges), `a_session_round_trips_...` extended to
│        assert `save_session` always recomputes `palace` from the bounded
│        tree rather than trusting a caller-supplied one, and
│        `a_pre_palace_session_with_no_palace_key_still_deserializes` (an
│        older session file with no `palace` key must not error). `cargo
│        fmt`, `cargo clippy --all-targets -- -D warnings`, full harness
│        unit suite (20/20) all pass.
│      dependency: ->CTX (same node-id namespace, so a tree leaf and its
│        palace node are the same identity, never two names for one fact) —
│        closed, satisfied by `{ROLLOUT}` (`85602f1`)
│      non-goal: rendering the diagram as an image; text stays Mermaid
│        source, same as this repo's own plan-writing convention
├── {LOOP} micro-workflow state machine                         [v] @host=none
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
│      landed 2026-09-27 (in progress): `run_loop` (src/harness.rs) drives
│        the five states as a fixed loop around the existing backend
│        closure -- `{Draft}`/`{Execute}` both call it, since the backend
│        codec's own turn loop already performs tool-calling end to end in
│        one round trip (`{WIRE}`'s "dumb transport" contract is unchanged,
│        never reimplemented here); every transition is logged into a
│        `trace: Vec<TreeNode>` that `turn_node` now places ahead of the
│        turn's final-answer child. `decide-pick` rejects only on the
│        literal `LOOP_REDRAFT_MARKER`, bounded by
│        `LOOP_MAX_PLAN_REVISIONS` (force-accept past it); `decide-continue`
│        jumps back to `categorize` unless the literal `LOOP_STOP_PHRASE`
│        appears or `LOOP_MAX_ITERATIONS` is reached (force-end past it) --
│        both are fixed-string/counter checks in MiniCon's own code, never
│        the model's free judgement alone.
│      evidence: five fixture tests (`run_loop_ends_on_the_explicit_stop_
│        phrase_after_all_five_states`, `..._jumps_...then_ends_on_the_
│        iteration_ceiling`, `..._bounds_redrafts_then_force_accepts`,
│        `..._jump_carries_the_prior_answer_into_the_next_call`,
│        `turn_node_places_the_loop_trace_ahead_of_the_final_answer`), all
│        using a fake backend closure so the state machine is proven without
│        a network. `cargo fmt`, `cargo clippy --all-targets -- -D
│        warnings`, full harness unit suite (25/25, was 20/20) pass.
│      closed 2026-09-27: also fixed a real gap the black-box test below
│        surfaced -- the jump message originally carried only the prior
│        draft forward, dropping the ORIGINAL task text; since the backend
│        closure is stateless across `run_loop`'s own calls (unlike
│        `compose_resumed_task`'s cross-invocation session fold, a different
│        seam), iteration 2+ would have had no idea what the task even was.
│        Fixed to carry both the original task and the prior result every
│        iteration.
│      evidence: `tests/minicon_harness.rs`'s
│        `deepseek_backend_loop_jumps_at_least_once_before_the_stop_phrase`
│        -- a real live DeepSeek call, task text that increments a
│        `counter.txt` through the (shared, persistent-across-iterations)
│        `file` tool and only emits `TASK COMPLETE` once the counter reaches
│        2, forcing a real jump before the real stop phrase ends it. Ran
│        live in this environment (a real `MINICON_DEEPSEEK_API_KEY` is
│        set) and passed in 7.69s -- not `BLOCKED`. `cargo fmt`, `cargo
│        clippy --all-targets -- -D warnings`, full harness unit suite
│        (26/26) and `six-cell-qualify.sh` (0 FAIL, rest expected `BLOCKED`
│        for hosts this session lacks) all pass.
│      safe failure: an unrecognized/missing state on resume is the same
│        bounded CLI error as a corrupt tree, not a silent restart at
│        categorize (unchanged by this leaf -- resume still replays `tree`
│        as plain Markdown context, `{LOOP}`'s states are not themselves
│        persisted as a resumable position yet)
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

1. **`{ROLLOUT}` — closed (`85602f1`).** Landed the tree format alone, still
   linear (no jump), proving CTX round-trips before LOOP exists. Extends
   HS's session file with a `tree` field; `turns` stays for backward-compat
   read of existing session files (a v0.2.x session predates this feature).
2. **`{PALACE}` — closed 2026-09-27.** Additive, as planned: it only
   documents the tree's own hierarchy for now, since there was no cross-
   subgoal fact yet worth a shared-prerequisite/kill-path edge; shipped
   without LOOP's jump logic, exactly as this section anticipated.
3. `{LOOP}` last, highest risk, not yet started. Needs `{ROLLOUT}` and
   `{PALACE}` both in place since a jump target is a
   tree node possibly reached through a palace edge.

Each step lands with its own named evidence (fixture tests first, one
black-box test per step) before the next starts, per AGENTS.md's "work
independent leaves in parallel only when file ownership and evidence are
independent" — these three are *not* independent (strict dependency chain
above), so they run in this order, not in parallel.
