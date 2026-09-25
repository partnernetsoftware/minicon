# Carried debt (from v0.1.24, still open after v0.1.26)

Not a release plan of its own — a holding pen so open leaves survive an
archived plan doc instead of being silently dropped
(`plan/archive/plan-v0.1.24.md`). Pull an item out into the next real plan
doc when it is picked up; delete its line here once it ships or is decided
`(OWNERS)`-closed.

```text
[carried] open since v0.1.18/v0.1.21, not a regression, no release blocked on them
├── C1 `capture-pane --scrollback N`: decide the semantics (cross-screen
│      stitching, viewport restore), then implement. Needs the instrument gap
│      noted in v0.1.24's B3 decision first: `max_scrollback` in the snapshot
│      reports parser capacity, not how much scrollback actually exists.
├── C3 box-drawing glyphs from cell geometry (Consolas, 1 px gap at 12 px)
├── C4 idle one-tab host RSS toward 10 MiB (paused since 2026-09-06)
├── D2 select cells from the change: wire the per-cell case, not just
│      documents-only. Needs its own negative-control evidence (a change that
│      should need `win-*` but doesn't touch an obviously Windows-named path)
│      before it ships — an autonomous heuristic guess is not acceptable here.
└── E1/E2 shared seam with AgenTerm                              (OWNERS)
       ├── E1 click streak D1-D4: four behaviour divergences
       └── E2 composer rules: survey before anything moves; A1-A3 landed in
              MiniCon first (v0.1.24/v0.1.26), only a second consumer
              justifies sharing them
```
