# Carried debt

Not a release plan of its own — a holding pen so open leaves survive an
archived plan doc instead of being silently dropped. Pull an item out into
the next real plan doc when it is picked up; delete its line here once it
ships or is decided `(OWNERS)`-closed. Closed items are deleted, not kept as
narrative: the full investigation record up to 2026-10-21 (including the
closed G1 dev-loop/CI-permission thread and the closed G2 idle-prompt Ctrl+C
thread) is frozen in
[the 2026-10 snapshot](archive/plan-carried-debt-2026-10-snapshot.md), and
durable mechanisms were upserted to their owners
([dev court](../prd/PRD_02_32_dev_loop_crosscheck.md) owns
`dev-loop-crosscheck.yml`; the agenterm `console_agent` Backspace-erasure
mechanism lives with its code and test).

```text
[carried] open leaves, no release blocked on them
├── C1 `capture-pane --scrollback N`: semantics still undecided
│      state: the instrument gap is closed — the vendored vt100 fork gained
│        `scrollback_depth()` (live depth, not capacity) and
│        `ConTerminal::scrollback_bounds` reports it; evidence
│        `scrollback_bounds_reports_the_real_depth_not_the_capacity`
│        (src/terminal.rs) and `scroll_shortcut_pages_minicon_history`
│        (tests/minicon_control.rs).
│      open: the semantics decision (cross-screen stitching, viewport
│        restore), and the alternate screen still yields all scroll
│        affordances to the app (`scrollback_bounds` returns (0,0)); on
│        Windows that branch is effectively unreachable under ConPTY.
├── C3 box-drawing glyphs from cell geometry (Consolas, 1 px gap at 12 px)
│      BLOCKED: needs a real Windows display host with Consolas and a human
│        eye; claimed by plan/archive/plan-v0.2.2.md `{UI}`, do not pick up
│        separately.
├── C4 idle one-tab host RSS toward 10 MiB
│      state: measured 12.01 MiB release / 21.46 MiB debug on an x86_64
│        Xvfb container (see prd/PRD_02_27_con_delivery.md "Runtime host
│        memory"). open: a matching six-cell lnx-aarch64 receipt. Claimed
│        by plan/archive/plan-v0.2.2.md `{UI}`, do not pick up separately.
├── D2 per-cell test selection from the change
│      state: `select_cells()` in scripts/round.sh is binary (all cells or
│        none). open: a per-path→per-cell heuristic, which must not ship
│        without real negative-control evidence (a change that needs win-*
│        without touching a Windows-named path, run through the six cells);
│        a wrong guess is a silent skip. Needs a session with standing `gh`
│        dispatch access.
├── E1/E2 shared seam with AgenTerm                              (OWNERS)
│      E1 click streak D1-D4 behaviour divergences; E2 composer rules —
│        survey before anything moves; only a second consumer justifies
│        sharing.
├── F1 composer image paste: thumbnail-chip UI only
│      state: paste path, PNG re-encode and the single decision point
│        (src/clipboard_paste.rs) shipped with real Windows/Linux GUI
│        evidence; survey and decided shape live in
│        prd/PRD_02_25_con_workspace.md ("Screenshot/image paste").
│      open: the chip rendering — a display-only lookup over the temp-path
│        pattern (option (a) in the snapshot's scoping note) is the agreed
│        first cut; a mixed-segment ComposerState redesign needs human
│        visual review.
├── F2 true headless render — screenshot/UI events with no window at all
│      owned writeup in prd/PRD_02_26_con_control_cli.md "Transport and
│        bounds" (search F2); not started.
├── G3 Windows new-tab layout garbled (reported 2026-10-05, v0.2.3)
│      state: not reproduced on this dev box at 0.2.4/agenterm 29003fc8 —
│        both new-tab paths rendered correctly in pane and pixels. open:
│        needs a sharper trigger (DPI/monitor change, first resize after
│        spawn, interleaved load); suspect remains agenterm's Windows
│        ConPTY width sync, do not guess-fix without a repro.
│      side finding tracked separately: plan/problem-screenshot-pane-hang.md.
├── S1 control SendUiIme re-decides text-input ownership without notes
│      found by the 2026-10-21 main.rs seam inventory: the real funnel routes
│        rename → notes → composer → terminal (src/main.rs event), but
│        src/control_dispatch.rs SendUiIme's commit pre-check and its reply
│        `route` label skip notes — with notes focused, an injected commit is
│        refused on an exited tab and the reply misreports "terminal". The
│        `route` values are pinned by control contract tests, so the fix is a
│        protocol decision owned by PRD_02_26; extract one
│        text-input-owner decision when it is made.
├── S2 composer text geometry triplicated
│      painter (src/host_paint.rs), IME anchor and click hit-test
│        (src/main.rs) each re-derive visible rows / sliding window / caret
│        cell with drifted caret-reserve, clamp, scale source and rounding.
│        Needs one composer-geometry owner returning rows/window/caret; a
│        wrong unification moves the caret, so it needs visual evidence.
├── S3 tab-activation effects diverge by origin
│      select_relative, tree click, control SelectTab and screenshot restore
│        apply different subsets of {geometry, reveal, composer unfocus,
│        title refresh}; control SelectTab applies no geometry and a stale
│        inset can survive a sidebar change. Needs one activation plan owner.
├── S4 paste-review handoff spelled three ways
│      "injected input skips review" exists as `!injected_ui_keys_live` once
│        and literal `false` twice; control SendMouse skips the
│        ensure_pty_input_open gate the keys path applies. One
│        paste-review-policy owner plus one handoff; PRD_02_24 owns review.
├── G4 minicon_control journeys flake on the owner dev box (2026-10-21)
│      state: `gui_control_surface_isolated_multitab_black_box` /
│        `scroll_shortcut_pages_minicon_history` fail ~1 in 3 local runs with
│        wait-text timeouts; one run had the fixture host die with
│        0xC0000374 (STATUS_HEAP_CORRUPTION) before cleanup. Reproduced
│        identically on the published 0.2.18 source (60afc8c, 1 of 3 runs),
│        so it predates the 0.2.19 changes; the same suites pass natively on
│        all six CI cells (dev-loop-crosscheck 39624936183, win-x86_64
│        included). open: the heap-corruption one-off deserves a trigger
│        hunt (suspect: parallel suite load on a busy host); do not treat a
│        local wait-text timeout alone as a product regression.
│      consequence evidence (2026-10-22): a crashed fixture host leaks its
│        console-agent children — the owner's process audit found exactly two
│        orphaned cmd.exe (created 2026-10-21 16:40, parent PID dead) matching
│        that day's gate-run crash window; fixture crashes cost real orphans,
│        raising the trigger hunt's priority.
├── G5 minicon_throughput budget misses on macos-15-intel runners (2026-10-21)
│      state: `sustained_long_output_keeps_control_and_sibling_responsive`
│        measured 36.4s for 32MiB against the 30s budget on the shared
│        macos-15-intel runner (dev-loop-crosscheck 39624936183); every
│        other suite on that cell and all five other cells passed. The
│        budget was calibrated elsewhere; per the roadmap's review rule, do
│        not invent a cross-machine threshold from one host — decide whether
│        the osx-x86_64 cell gets its own measured budget or the claim names
│        the cell as unqualified for the throughput bound.
└── R1 CI action/runner migration debt (observed 2026-10-10)
      warnings, not failures yet: pinned actions on Node 20 shims
        (checkout/upload/download-artifact/setup-python) and
        `ubuntu-latest` → Ubuntu 26 from 2026-10-19. Fix is a bounded
        re-pin pass preferring explicit runner tags; no blind `latest`.
```
