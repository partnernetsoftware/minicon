# plan-v0.2.6

## Scope decision

**PENDING — owner to confirm.** This document opens 0.2.6 as a planning home.
The set below is a *candidate* scope assembled from the owner's GitHub issues
opened 2026-10-12 and from carried debt; it is **not** yet an owner-ratified
scope. Per `AGENTS.md`, accepted scope is upserted into the owning PRD before
this plan is archived.

Candidate threads, in the order the owner raised them:

1. Two owner issues (2026-10-12):
   - **#3** sent-history region — a host-UI region showing the active
     terminal's recent sends, click-to-refill, with blank room reserved for
     future loop/goal controls.
   - **#5** user notes — a per-context scratchpad (global-first), never sent to
     the PTY; phase 1 session-scoped only, persistence gated.
   **#4** ("Check for updates") was moved to **0.2.7** by the owner
   (2026-10-12) — see `plan/plan-v0.2.7.md`.
2. **Repo hygiene (三理)** — tracked in `plan/plan-repo-hygiene.md` (batch B,
   not yet written). Batches B–E only; no feature-level refactors.
3. Carried debt pulled in **only if the owner accepts** — see "Carried, not
   pulled in" below. One is time-sensitive:
   **{R1} the `ubuntu-latest` → Ubuntu 26 migration begins 2026-10-19**, when
   the Node-20 action shims are removed. That leaf does not need to ride a
   version at all; flag it for a bounded pass regardless of 0.2.6's final scope.

### Placement decision for {SENTHIST} / {NOTES} (owner delegated, 2026-10-12)

The owner asked me to decide and review the result later. Decision: both live in
**one right-side, collapsible interaction panel**.

- It survives the sidebar rail collapsing (a sidebar-bottom home would vanish
  at 44 dip), and it fits a document-like notes editor (a thin band above the
  composer cannot).
- Precedent: iTerm2's right-side Toolbelt hosts per-session Notes.
- Toggle from a header control or a composer affordance; open/closed is
  session-scoped view state, like `sidebar_collapsed` (`src/main.rs:600`).
- Two hard constraints: (1) the panel takes a **real column** and the terminal
  is re-inset — never an overlay covering content (the product has no centered
  modals; the settings panel is anchored, not floating); (2) this is the
  **second** collapsible region (sidebar rail + this) and is the cap — do not
  add a third chrome state.
- Rejected: a band above the composer (cramps notes; second-best for history
  alone, kept as the fallback if the panel reads badly in use) and a
  sidebar-bottom home (dies on collapse, competes with the tab tree).

## Tree DAG

```text
0.2.6: owner-issues #3/#5 + hygiene; scope PENDING owner confirmation
├── {SENTHIST} sent-history region in the host UI                    [-] @host=none
│      landed (2026-10-12): the right-side collapsible panel, the newest
│        sent lines as clickable rows (click refills the composer via the
│        recall path), the reserved loop/goal area, and `ui-snapshot`
│        `history_panel`; toggled by Ctrl+Shift+H. Follow-ups: a header
│        toggle control, per-row destination label, scrolling past capacity.
│      source: GitHub #3 (owner, 2026-10-12); owns `prd/PRD_02_25_con_workspace.md`
│      placement: the right-side collapsible interaction panel (§Scope decision)
│      invariant: the region is a VIEW over the one existing sent-history
│        store (`ComposerState.history`, 64 entries, `crates/minicon-core/src/
│        composer.rs:52`) — never a second store, or the region and Up/Down
│        recall disagree about "what I sent"; it shows the newest ~20, one row
│        per entry with the destination tab, newest first; a reserved blank
│        sub-area below it belongs to future loop/goal work (placeholder only)
│      evidence: `ui-snapshot` publishes the panel (rows + hit rects +
│        reserved rect + open/closed); a black-box journey clicks a row through
│        the real pointer path and reads the composer back (== clicked entry,
│        caret at end, draft preserved); the panel's geometry joins
│        LAYOUT_SWEEP at every size/scale, expanded and collapsed; a screenshot
│        pair proves the terminal re-insets (no dead strip, no overlap)
│      safe failure: clicking never sends; a click during recall is just a new
│        jump; sending still ends recall
│      dependency: none
│      non-goal: pinning/search/filter/edit/delete of entries; changing the
│        64 bound or duplicate suppression; persistence across restarts
│      open: per-active-terminal view vs app-global (the composer is one
│        app-wide widget today, `src/main.rs:569`); default open/closed
├── {NOTES} user notes — per-context scratchpad, never sent to PTY      [_] @host=none
│      source: GitHub #5 (owner, 2026-10-12); owns `prd/PRD_02_25_con_workspace.md`
│      placement: the same right-side panel, as a second section
│      phase 1 (session-scoped, no charter change): context = {active tab} ∪
│        {Global}; global-first; its own editor with focus/IME plumbing (this
│        is the real cost — the product has exactly one text input surface
│        today and an "exactly one focus owner" a11y contract); `ui-snapshot`
│        exposes it
│      hard boundary: a note is NEVER written to the PTY; clicking a note edits
│        the note, it does not fill the composer
│      #risk phase 1 must be described as a scratchpad, not memory — it does
│        not survive a restart
│      blocked (phase 2, persistence): owner charter decision on "user data
│        file ≠ product residue"; a durable tab-id key across restarts; a
│        privacy/control-surface decision (`--control` read/write = new
│        disclosure surface). Do not start phase 2 until all three land
│      non-goal: templates/sendable snippets (a different model — separate
│        idea); per-tab persistence; search/tags/formatting
├── {HYGIENE} 三理 (repo hygiene)                                   [-] @host=none
│      owner-ordered 2026-10-12; owned in `plan/plan-repo-hygiene.md`
│      batch A DONE (commit 905bc84): archived 6 plans, repointed links,
│        normalized non-goal markers to `[no]`, fixed a `[✓]` and a home path
│      remaining: B ledger + delivery-tree + orphan/problem registration + issue
│        registration; C `.gitignore` real gaps (research outputs only — do NOT
│        ignore vendored deps); D dead-code pass (`.rs`, needs test+clippy);
│        E local/remote hygiene (cargo clean, disabled workflow, stale branches)
│      dependency: owner answers for C/E (see §Open)
│      non-goal: feature-level refactors (main.rs split, 90-feature convergence,
│        pinned-crate drift) — recorded in the ledger, not done here
└── {VERSION} 0.2.6                                                 [_] @host=none
       `Cargo.toml` version 0.2.6; `release-policy.json` is bumped by the
       release step, not here

Moved to 0.2.7: {UPDATE} GitHub #4 "Check for updates" — see `plan/plan-v0.2.7.md`.

Carried, not pulled in (unchanged unless the owner says otherwise):
- C1 `capture-pane --scrollback N` — BLOCKED on a vendored-vt100 API addition
  (cross-repo), see `plan/plan-carried-debt.md`
- C3 box-drawing glyphs — needs a real Windows display host with Consolas
- C4 idle RSS toward 10 MiB — needs a six-cell lnx-aarch64 receipt
- D2 select-cells-from-change — needs gh dispatch rights + negative-control
  evidence; a wrong guess is a silent cell skip, forbidden here
- E1/E2 AgenTerm shared seam `(OWNERS)`
- F1 composer image-paste chip UI — self-contained, could be pulled in if wanted
- F2 true headless render — not owner-scoped
- G3 Windows new-tab layout garbled — not reproduced (2026-10-10 attempt); needs
  a sharper trigger, do not claim fixed on a non-repro
- {SCRSHOT-HANG} `screenshot-pane` writes its PNG then does not return —
  `plan/problem-screenshot-pane-hang.md`, no repro yet; a candidate pull-in
  because it now blocks other verification work
- {ALTSCREEN} Windows alt-screen resize — platform limitation,
  `plan/plan-alt-screen-resize.md`
- {LOOP} harness context engineering — `plan/plan-harness-context-engineering.md`
- NewAgent dialog templates — `plan/plan-newagent-config.md`
```

## Dependencies and shared prerequisites

- `{SENTHIST}` and `{NOTES}` share one panel and both touch `src/ui.rs` layout,
  `src/host_paint.rs`, and `ui-snapshot`. Sequence them: panel geometry +
  collapse first, then history, then the notes section — and keep the notes
  area distinct from the loop/goal placeholder.
- `{HYGIENE}`'s batch D is the only code-touching hygiene batch; it needs
  `cargo fmt` + `clippy -D warnings` + `./scripts/build.sh test`.

## GATE status

Not started. Version-gate (six-cell, exact-source Candidate, no-rebuild
Promotion, dual signing) is run per the `run-reputation-and-release` skill when
a Candidate is cut; not pre-claimed here.

## Memory palace

```mermaid
flowchart TD
    OWNER[owner issues 2026-10-12] --> SENT[#3 sent-history region]
    OWNER --> NOTE[#5 user notes]
    OWNER --> HYG[hygiene 三理]
    OWNER -.->|moved out| UPD[#4 check for updates -> 0.2.7]
    SENT --> PANEL[right-side collapsible panel]
    NOTE --> PANEL
    PANEL --> RESV[reserved: loop/goal]
    SENT --> STORE[(one sent-history store)]
    STORE --> RECALL[Up/Down recall]
    NOTE -.->|phase 2 gated| PERSIST[persistence: charter + tab-id + privacy]
    HYG --> GATE
    SENT --> GATE
    NOTE --> GATE
    GATE[GATE per cell: fmt + clippy + build.sh test] --> SHIP[0.2.6 Candidate -> Promotion]
```

## Open decisions (owner)

0.2.6 scope itself: which of the candidate threads are in, and is the version
cut for all of them or a subset?
1. `{SENTHIST}` per-terminal vs app-global; default open/closed. (Placement is
   decided — right-side collapsible panel; review it in use and revert to the
   composer-band fallback if it reads badly.)
2. `{NOTES}` confirm notes (never sent) vs templates/sendable snippets (a
   different model); whether to expose it on `--control` at all.
3. `{HYGIENE}` C/E confirms: `external-results.json` (864K) keep? remote stale
   branches delete? `cargo clean` now? `research/` ignore granularity?
4. Pull-ins: `{F1}` chip UI and/or `{SCRSHOT-HANG}` into 0.2.6?
5. `{R1}` (Ubuntu 26 migration, starts 2026-10-19): fix now in a bounded pass,
   independent of the version?
