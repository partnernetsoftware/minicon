# Shared core and reuse boundary (`minicon-core`)

Owner of the question "what can another product take from MiniCon, and what
has to stay". Parent: [MiniCon product requirements](../PRD.md).

Legend: `[x]` shipped, `[~]` partial, `[ ]` planned.

## Why this module exists

MiniCon and `agenterm` are separate products that share a terminal. The
README's staged plan says a `terminal-core` (pty / vt100 / raster / font / ime)
should sink into this repository and `agenterm` should depend on it. That end
state is right — MiniCon *is* the terminal, and the larger product adds
workspace, fleet and server on top — but the order matters, and taking it in
one step would be a mistake.

## The constraint that decides the shape

`agenterm` cannot depend on a crate that itself depends on `agenterm`'s own
crates. Cargo would then resolve those crates twice — once as a workspace path
member and once through a git dependency — producing two distinct instances of
the same types. That is not a style preference; it does not link.

So the reusable half is exactly **what needs nothing from `agenterm`**, and
that is measurable rather than arguable. Measured on 2026-08-23:

| module | non-`std` dependencies | reusable |
|---|---|---|
| `composer` | `unicode-width` | yes |
| `json` | `itoa` | yes |
| `session_store` | `workspace::TabId` | via `workspace` |
| `control_pending` | `control`, `json`, `workspace` | no — `control` is host-bound |
| `workspace` | `agenterm_ui_core::compute_tree_depths_by` | one helper away |
| `ui` | `agenterm_ui_core`, `agenterm_platform::numeric` | two helpers away |
| `palette` | `agenterm_platform::numeric`, `vt100::Color` | two helpers away |
| `perf`, `raster_surface`, `a11y`, `font`, `terminal_paint`, `agent_interface` | platform / vt100 | no |

`agenterm-ui-core` is already "Host-neutral AgenTerm interaction geometry" and
is OS-neutral, with architecture-specific SIMD behind scalar fallbacks. It is
not a thing to duplicate; it is a thing to draw the line against.

## Stage 1 — shipped

- [x] `crates/minicon-core` contains `composer` and `json`: single-line editing
  with a caret and wide-character measurement, and a bounded JSON codec. Both
  are ordinary data and arithmetic.
- [x] the crate has **no platform, OS, window or terminal-backend dependency,
  and no `cfg` on architecture or operating system anywhere**. That is what
  makes it consumable without dragging a platform layer along.
- [x] both halves of that rule are enforced by tests rather than by intent. One
  reads the manifest, because the property is about the dependency graph and no
  compiler error announces a new dependency; the other scans the modules,
  because a manifest can stay clean while the code stops behaving identically
  everywhere. The manifest test was negative-controlled: adding `vt100` turns it
  red.
- [x] the dependency test is bounded to the production section. A
  dev-dependency cannot reach a consumer, and failing on one would forbid the
  `serde_json` oracle that keeps the codec honest — the codec exists *because*
  `serde_json` is too large for this product's budget, so comparing against it
  is the test that matters.

Found by the split, and worth recording because it generalizes: `to_vec` was
`#[cfg(test)]`. Inside a binary that works, because a test build compiles it.
Across a crate boundary it does not — a consumer's test build does not enable
the dependency's `test` cfg — so a test-only helper simply disappears. Every
`cfg(test)` item crossing a new boundary has to be re-examined.

## Stage 2 — planned

- [ ] move the leaf helpers MiniCon needs out of `agenterm-ui-core` into this
  crate: tree-depth computation, scrollbar geometry, and the numeric rounding
  shims. They are small, pure and have no reverse dependency on anything above
  them.
- [ ] `workspace`, `session_store`, `ui` and `palette` then follow, because the
  single helper each was waiting on is here.
- [ ] `agenterm` depends on `minicon-core` for those helpers. This is the
  dependency inversion the README describes, proved on leaves rather than on
  the terminal engine.

The value is not line count. It is that the inversion gets tested on code where
being wrong is cheap, before it is attempted on `pty` / `font` / `ime`, where
every `cfg` and every `unsafe` in the product lives.

## Stage 3 — not scheduled

- [ ] `terminal-core`: pty, vt100 wiring, raster, font, ime.

Deliberately unscheduled rather than merely future. Those adapters live in
`agenterm-platform`, a crate serving process, ipc, webview, accessibility,
screenshot and virtualization as well as the terminal. Extracting only the
terminal-relevant adapters means splitting that crate along a seam that does
not currently exist, and the cost is not the move — it is owning two platform
layers while the split is in flight. Stage 2 has to have paid off first.

## Non-goals

- Not a general-purpose utility crate. Something belongs here because two
  products need the same *rule*, not because it happens to be generic.
- Not a reason to make MiniCon's own code more abstract than its use demands.
  The boundary exists to keep a dependency out, not to invite indirection.
- **No mandatory dependency inversion.** Keep extracting host-neutral rules
  into `minicon-core` and keep consuming `agenterm-platform` /
  `agenterm-ui-core` at the git pin in `Cargo.toml`. AgenTerm depending on
  `minicon-core` is a later optional payoff (Stage 2), not a prerequisite for
  MiniCon shipping, for continued reuse, or for platform-feature work. Do not
  split `agenterm-platform` solely to invert the arrow.
