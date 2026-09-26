# Plan v0.2.1 — mux + harness hardening, no new-role features

Owning PRD module: `prd/PRD_02_31_v0_2_horizon.md` (scope, non-goals, the
2026-09-26 `harness`/`harness-manage` role split, the 0.3.x GUI/manager
deferral — read it first; this file only sequences delivery). Product
boundary: `prd/PRD_02_23_minicon.md`. v0.2.0 is already released
(`plan/archive/plan-v0.2.0.md`); this plan is the next code-bearing round on
the same two subcommands, not a new capability.

## Release handoff (2026-09-26)

The owner authorized this agent to take over and publish 0.2.1. The earlier
cloud-session approval blocker below is historical and no longer blocks this
authorized release. On the local Mac, the inherited staged harness fix omits
unsupported macOS memory limits while retaining other configured limits.
The full build.sh test gate and script selftests passed with HTTP proxy
variables unset for loopback fixtures. All six target builds and both local macOS test/throughput lanes passed.
The first local receipt failed source stability because the temporary output
directory was not ignored and handoff docs changed during the run; the frozen
source is being requalified. No unavailable court is counted as passed. Signing and
Promotion follow the registered skills and retain both required signatures.

## Why this plan exists, and what it deliberately does not contain

The 2026-09-26 discussion (mux's moltbaby-evidenced tmux-verb-compat
rationale, the `harness`/`harness-manage` worker/manager split, the GUI
workbench renderer decision) produced real scope decisions, but **none of
them add work to 0.2.x** — they were mostly about *excluding* things:
`harness-manage` and the GUI workbench were both placed in an unassigned,
likely-0.3.x horizon, explicitly `BLOCKED` on design work
(context/workflow concepts, three-platform WebView evidence) that has not
started. So 0.2.1's own content does not need adjusting because of that
discussion — it needed the exclusion **recorded**, which is done in
`prd/PRD_02_31_v0_2_horizon.md` as of commits `cfcf0d0`/prior. What 0.2.1 picks
up is the hardening debt v0.2.0 itself already named as owed.

## Tree DAG

```text
v0.2.1 — harden mux + harness; no new role (owner decision 2026-09-26)
├── HS harness statefulness {hs} [v] closed 2026-09-26
│   ├── outcome: `--session ID` / `--continue ID`, mutually exclusive,
│   │     replace "one task, exit" with a bounded multi-turn session,
│   │     scoped only to a single `harness` worker (not `harness-manage`
│   │     orchestration)
│   ├── #decision this is the harness {h} node's own worker capability, not
│   │     the `harness-manage` {hm} question -- a worker that remembers its
│   │     last turn is not a manager that dispatches across tabs
│   ├── dependency: PRD_02_31 "Run shape" (now closed there too) and the
│   │     existing DeepSeek/opencode-go wire codecs -- resolved by composing
│   │     prior turns into the plain task string, so neither
│   │     `harness_wire.rs` nor `harness_opencode.rs` needed touching at
│   │     all (simpler than this leaf's own forecast of a shared-hot-file
│   │     pass over both)
│   ├── design questions, resolved
│   │   ├── session state: a bounded on-disk file under
│   │   │     `--root/.minicon-harness-sessions/<id>.json` (reusing
│   │   │     `FileTool`'s path-escape/symlink protection), because the
│   │   │     process still exits each run either way and a resume needs
│   │   │     something to resume from
│   │   ├── turn/tool-call bound: stays a hard cap per invocation, not
│   │   │     accumulated across resumes
│   │   └── streaming output: explicitly deferred, no groundwork laid --
│   │         still blocks the GUI workbench leaf, see PRD_02_31
│   └── safe failure: `--continue` against a session that does not exist or
│         is corrupt is a bounded CLI error, never a silent fresh start;
│         `--session` against an existing id is refused, not overwritten.
│         Evidence: 5 new unit tests (37 total in `harness.rs`), `cargo
│         fmt`, `cargo clippy --all-targets -- -D warnings` all pass; see
│         `prd/PRD_02_31_v0_2_horizon.md` "Run shape" for full detail
├── HB harness — close the BLOCKED items v0.2.0 already named {hb} [-]
│   ├── Windows/macOS `native-tls` arm: compiled and proven in at least one
│   │     court each, not just the feature graph compiling (PRD_02_31
│   │     "Owed, and BLOCKED rather than skipped", item 1) ->hs (shares the
│   │     wire transport being touched for statefulness -- do both in one
│   │     pass over `harness_wire.rs`/`harness_opencode.rs`, not two)
│   │     `BLOCKED` 2026-09-26: this cloud container is Linux-only and
│   │     `scripts/six-cell-qualify.sh` itself requires an Apple Silicon
│   │     macOS host for the cross-compile+runtime proof -- cannot be closed
│   │     from here; needs a macOS/Windows court, not silently skipped
│   ├── [v] closed 2026-09-26 contained-process spawn debt: `exec` now
│   │     spawns via `agenterm_platform::contained_process::
│   │     ContainedHeadlessCommand` with hard `ContainedProcessLimits`
│   │     (memory/file-size/open-files/active-processes/cpu-seconds), not
│   │     `std::process::Command`; `terminate_and_wait` on timeout reaps the
│   │     whole native containment group. See PRD_02_31's harness dependency
│   │     line for full evidence and the one honest gap (no test yet proves
│   │     descendant-reaping under load)
│   └── #decision these are debt closure, not new scope; each gets its own
│         named evidence the same way H4/H5 did, per AGENTS.md's "every test
│         must be provable" rule
├── MH mux — close what M's own non-goals left open {mh} [v] verified 2026-09-26
│   ├── re-checked the mux verb/flag table against
│   │     mgttt/moltbaby's mux skill documentation (real, running
│   │     tmux-based agent-bus). Most usage already covered; tmux's own
│   │     documented footguns (window names containing `.` breaking
│   │     `send-keys` target parsing, bare `send-keys` being unreliable)
│   │     are tmux-inherent, not this table's gap
│   ├── one real gap found and recorded, not folded in silently: no
│   │     `list-panes` verb at all, needed by moltbaby's `super-query`
│   │     (`tmux list-panes -F '#{pane_id} #{pane_width} #{pane_height}
│   │     #{pane_active} #{pane_dead}'`). Three of five fields map cleanly
│   │     onto data MiniCon already has; `pane_width`/`pane_height` need a
│   │     new per-tab round trip design this round does not make -- left
│   │     `BLOCKED` for a future round rather than shipped partial. See
│   │     `prd/PRD_02_31_v0_2_horizon.md`'s "mux hardening against
│   │     moltbaby-shaped real usage" for full detail
│   ├── #decision this is verification/hardening against the moltbaby
│   │     evidence node, not a scope change -- the real gap found becomes
│   │     its own named, `BLOCKED` decision, not folded in silently
│   └── dependency: `prd/PRD_02_26_con_control_cli.md` (protocol surface)
├── UI continued UI/UX and platform-foundation stabilization {ui} [_] `BLOCKED`
│   ├── outcome: no new features -- close existing rough edges in
│   │     `terminal.rs`/`theme.rs`/`host_ui.rs`/`host_paint.rs`/
│   │     `raster_surface.rs` and the consumed `agenterm-platform`/
│   │     `agenterm-ui-core` pins, per the 2026-09-26 "0.2.x stays
│   │     hardening" decision
│   ├── candidates identified from `plan/plan-carried-debt.md`: C3
│   │     (box-drawing glyphs from cell geometry, Consolas 1 px gap at
│   │     12 px) and C4 (idle one-tab host RSS toward 10 MiB, paused since
│   │     2026-09-06) -- both are exactly this leaf's shape
│   ├── #decision scope for this leaf is picked from whatever open bugs/
│   │     rough edges exist when this plan starts implementation, not
│   │     pre-enumerated here -- keeps this plan from going stale before
│   │     work begins
│   └── `BLOCKED` 2026-09-26: this cloud container has no display/GPU --
│         the existing blackbox/control test suites already fail here on
│         unmodified `main` for exactly that reason (confirmed by stashing
│         this round's own changes and re-running them). C3 is a rendering
│         change verifiable only by looking at rendered glyphs; C4 is a
│         memory measurement. Neither can be honestly verified from this
│         session -- recorded `BLOCKED` on a display-capable host rather
│         than changed blind or skipped silently
└── GATE {g} shared release gate, run once all leaves above are closed or
    │     honestly `BLOCKED` [-]
    ├── [v] `cargo fmt` + `cargo clippy --all-targets -- -D warnings`, clean
    │     on every leaf's commits (AGENTS.md: six-cell gates on these,
    │     `cargo test` does not)
    ├── [-] `./scripts/build.sh test`: the non-GUI suites (`harness` unit
    │     tests, `minicon_alignment`) pass; `minicon_blackbox`/
    │     `minicon_control` fail in this container for the same
    │     no-display/GPU reason as `UI` above, confirmed pre-existing on
    │     unmodified `main`, not a regression from this plan's changes
    │     `BLOCKED` on a display-capable host
    ├── [_] `./scripts/six-cell-qualify.sh`: requires an Apple Silicon
    │     macOS host per the script itself; cannot run from this Linux
    │     cloud container `BLOCKED`, same constraint as `HB`'s native-tls arm
    └── [v] PRD upsert: `HS`/`HB`(contained-process-spawn half)/`MH` closed
          with named evidence in `prd/PRD_02_31_v0_2_horizon.md`; `HB`'s
          native-tls half and `UI` recorded `BLOCKED` there too rather than
          silently dropped. This plan stays open (not archived) until a
          display-capable, Apple-Silicon-macOS-having session can close
          the three `BLOCKED` items and actually run the release/signing
          phase of the active `/goal` directive
    └── [_] release/signing phase (`签章/发布`) itself: `BLOCKED`,
          2026-09-26. Read `run-reputation-and-release`'s top-level page
          (candidate → company-signing/macos-signing → defender-ci-scan →
          reputation → release chain, all real GitHub Actions dispatches,
          no local Mac/UTM needed for the CI-native Defender path) and
          confirmed v0.2.0 is already a published GitHub release while
          Cargo.toml/Cargo.lock/release-policy.json still read `0.2.0` --
          the first real step is bumping them to `0.2.1` and committing.
          That bump was refused by this session's own platform-level auto
          mode classifier (reason: "Production Deploy") before any file
          changed; the tool description is explicit that this denial
          covers the *outcome*, not just the one command, and instructs
          stopping and asking the owner rather than working around it
          through another tool, encoding or later turn. This is a
          different kind of `BLOCKED` from the Apple-Silicon/display gaps
          above: it is not a missing host, it is this session being
          disallowed from taking the first production-release step
          un-supervised. Needs the owner's explicit go-ahead (or to run
          this phase from a session/permission mode where release actions
          are pre-authorized) before it can proceed

Explicitly NOT in this plan (parked in PRD_02_31, unassigned horizon,
BLOCKED on design):
├── harness-manage {hm} -- manager/orchestration role; needs a context
│     concept and a workflow concept designed first (owner's own caveat)
└── harness GUI workbench -- needs agenterm-platform's webview adapter to
      actually be consumed, which is 0.3.x-or-later work in AgenTerm's own
      three-platform WebView evidence, not MiniCon's to build first
```

## Mermaid flowchart memory palace

What the tree above cannot show well: the statefulness leaf (`HS`) and the
BLOCKED-item closure leaf (`HB`) both touch the same two files
(`harness_wire.rs`, `harness_opencode.rs`), so they share a hot-file
constraint even though they're separate leaves; and the whole plan sits
between a released v0.2.0 and a not-yet-started, not-yet-scoped 0.3.x
horizon that this plan must not reach into.

```mermaid
flowchart LR
    subgraph DONE["v0.2.0 — released"]
        R020["v0.2.0 candidate<br/>mux + harness, DeepSeek + opencode-go<br/>both [v] with named evidence"]
    end

    subgraph V021["v0.2.1 — this plan, hardening only"]
        HS["HS harness statefulness<br/>multi-turn, one worker, in-process or<br/>bounded resume file"]
        HB["HB close BLOCKED items<br/>Win/macOS native-tls proof ·<br/>contained-process-spawn debt"]
        MH["MH mux hardening<br/>re-check non-goals vs moltbaby-shaped<br/>real usage"]
        UI["UI terminal/platform<br/>foundation stabilization<br/>(no new features)"]
        HOTFILES{{"hot files:<br/>harness_wire.rs<br/>harness_opencode.rs"}}
        HS -.shares.-> HOTFILES
        HB -.shares.-> HOTFILES
    end

    GATE{"shared gate<br/>fmt+clippy → build.sh test →<br/>six-cell-qualify.sh"}
    HS --> GATE
    HB --> GATE
    MH --> GATE
    UI --> GATE
    GATE --> UPSERT["upsert PRD_02_31<br/>[_]/[-] → [v] with evidence"]
    UPSERT --> R021["v0.2.1 candidate"]

    R020 --> V021

    subgraph PARKED["0.3.x horizon — NOT this plan, design not started"]
        HM["harness-manage {hm}<br/>needs context concept +<br/>workflow concept"]
        GUI["harness GUI workbench<br/>needs agenterm-platform webview<br/>consumed, 3-platform evidence"]
    end
    R021 -. "authority boundary: needs its own<br/>PRD_02_3x + owner go-ahead,<br/>not an automatic next step" .-> PARKED
```

## Decision: 0.2.1 vs. jumping to 0.2.2 planning

**Ship this plan as v0.2.1; do not skip to 0.2.2 planning yet.** Reasoning:

1. v0.2.0 named real BLOCKED/owed debt (TLS proof, contained-process
   spawn) that has not been paid down. Deferring it to a hypothetical 0.2.2
   without doing it in 0.2.1 would mean two versions in a row shipping with
   the same recorded gaps, which is what `PRD_02_31`'s "Owed, and BLOCKED
   rather than skipped" discipline exists to prevent.
2. Today's harness statefulness need (real, owner-stated, needed even
   before any manager/GUI work) has no home yet. It belongs with the other
   harness hardening in the same release, not spread across two.
3. Planning a 0.2.2 now, before 0.2.1's own scope is even implemented,
   would be planning against a moving baseline — AGENTS.md's own planning
   method says identify shared prerequisites and hot files *before*
   parallel work, not stack a second plan on top of an unstarted one.
4. `harness-manage` and the GUI workbench do not belong in 0.2.2 either --
   they are parked in an unassigned 0.3.x horizon pending design work this
   plan does not do. There is currently no reason to expect a 0.2.2 at all;
   if 0.2.1 closes its own `[_]`/`[-]` items cleanly, the next real horizon
   is 0.3.x's `harness-manage` + GUI design, not a 0.2.2 patch.

**Next action:** implement this plan's four leaves (`HS`, `HB`, `MH`, `UI`),
upsert `prd/PRD_02_31_v0_2_horizon.md`, archive this file, then open the design
work for `harness-manage`/GUI as a new `PRD_02_3x` module -- not a 0.2.2 plan.
