# plan-v0.2.10

> Finalised 2026-10-14 after v0.2.9 published and was verified from the
> owner's view (Release live, assets re-downloaded, hash-matched). The chain's
> incident record, including the Defender-court outage and the mid-chain
> main-move trap, lives in `prd/archive/v0.2.9-release-history.md`.

Read `AGENTS.md` first for the dev loop, the gate and the release boundary.

## The product outcome this version exists for

**Ordinary output stops re-rasterising the whole window.** The user-reported
"flashing" (`总是感觉闪屏`) is the most-felt open defect in the repository, and
0.2.9 did not fix it — deliberately. What 0.2.9 did do is write the spec
(`plan/plan-scroll-damage.md`), which turned an unfixed bug into a *sequenced*
one: it named the primitive, and it proved the fix is three leaves, not one.

> **A line of output at the bottom of a full screen invalidates the rows it
> changed, not the window.**

## Why this is now tractable — and what it is NOT

Two facts the 0.2.9 spec established, both still true:

1. The changed band for a scroll is exactly `[scroll_top, scroll_bottom]`, and
   the one thing `mark_full` was silently covering is the scrollbar thumb,
   which is drawn outside the row band. Hence the primitive: a row range plus a
   `scrollback_changed` flag — the same shape as the existing
   `viewport_changed`.
2. **The parser narrowing alone changes nothing a user sees.** Both shipped
   hosts upgrade every frame to full. So this version's outcome is delivered
   by the *last* leaf below, not the first.

## Tree DAG

```text
0.2.10: ordinary output stops re-rasterising the whole window
├── Product track
│   ├── {GATE-FORK} the vendored fork's damage oracle becomes runnable   [_]
│   │      problem: `third_party/vt100` is a `[patch.crates-io]` path with an
│   │        empty `[workspace]` table (deliberately standalone), so its
│   │        `assert_damage_covers` oracle is executed by no gate; changing
│   │        the damage contract without it would be an unproven claim
│   │      shape: a script the existing gate calls —
│   │        `cargo test --locked --manifest-path third_party/vt100/Cargo.toml`
│   │        — NOT a workspace member (that would change what `--workspace`
│   │        builds for every future change and pull the fork's dev-deps into
│   │        the root lock). The fork's own manifest already declares
│   │        independence; the fork carries its own `Cargo.lock`
│   │      evidence: **measured on the Windows host 2026-10-14** — the fork
│   │        builds and runs standalone from a cold external target dir, its
│   │        lib suite reporting **11 passed / 0 failed**; the only failure is
│   │        the doctest step, which cannot spawn `rustc` inside the agent
│   │        sandbox (os error 231, pipe exhaustion) — an environment limit,
│   │        not a fork defect, so the gate script should scope to `--lib` and
│   │        say so. The path needs no new machinery, only an entry in the
│   │        gate that refuses to be skipped
│   │      this resolves owner question 2 from 0.2.9 with the smaller change
│   ├── {SCROLL-DAMAGE} the parser primitive                            [_]
│   │      problem / invariant / safe failure: as already specified in
│   │        `plan/plan-scroll-damage.md` — do not re-derive it
│   │      shape: `scroll_up`/`scroll_down` mark `[scroll_top, scroll_bottom]`
│   │        instead of `mark_full`; `GridDamage`/`ScreenDamage` gain
│   │        `scrollback_changed` (accessor/merge/is-empty like
│   │        `viewport_changed`); `src/terminal.rs` marks the scrollbar bounds
│   │        when the flag is set; the alternate screen and `viewport_changed`
│   │        paths are not narrowed
│   │      evidence: a cfg-free unit test on the damage a scroll returns, plus
│   │        the fork oracle extended for the scrollbar cases
│   ├── {AMPLIFIER-T} the transient host stops forcing a full frame     [_]
│   │      problem: `!host_retains_pixels` marks both the host UI and the
│   │        session full on every frame (`src/main.rs` transient-host path)
│   │      invariant: a transient host still presents a complete, correct
│   │        frame; only the *invalidation* narrows to what changed
│   │      evidence: the Linux `xvfb-run` journey — before/after frames under
│   │        a real (virtual) display, partial present observable
│   │      dependency: `{SCROLL-DAMAGE}` (otherwise there is nothing partial
│   │        to present)
│   ├── {AMPLIFIER-W} the Windows `con` path stops forcing a full frame [_]
│   │      problem: any non-empty, non-full candidate becomes a whole frame
│   │        (`src/main.rs` Windows path)
│   │      evidence: a real Windows display journey, before/after; this is the
│   │        host the flashing was reported on
│   │      dependency: `{SCROLL-DAMAGE}`
│   └── Blocked, named — not this version
│       ├── {FIND} search what a terminal printed   blocked on C1
│       └── {SIGV} verify the signature             blocked on a size decision
└── Consolidation track — behaviour-preserving, never gates the above
    ├── {SPLIT} one module at a time, gate after each                   [_]
    │      first unit: the 338-line `mod native_endpoint_tests` sitting in
    │      the middle of live code in `src/control.rs`
    ├── {SEAM} / {ENCAP} / {NAME}                                       [_]
    │      as scoped in `plan/archive/plan-v0.2.9.md`; unchanged
    └── {FEAT} / {DOC-OWNER}                                            [_]
           still gated on an owner decision — a removal that deletes a route
           another product will want is not consolidation
```

## Owner decisions this plan asks for

1. **Do the amplifiers belong in 0.2.10?** They are what actually delivers the
   outcome; without them the version ships a faster parser and the same
   flashing. If they are deferred, the honest version name is "the parser
   stops over-reporting damage", not "the flashing is fixed".
2. **`{FEAT}` — which of the 54 unconsumed platform features are future route
   (`a11y-*`, `app-facts`, `desktop-host`, `network-dns`,
   `virtualization-probe`) and which are residue?** Unchanged from
   `plan/archive/plan-v0.2.9.md` open decision 3.
3. **`{DOC-OWNER}` — who owns the sentence** in the four READMEs? Unchanged
   from `plan/archive/plan-v0.2.9.md` open decision 4.

## Outside the version number, but with a date

- **R1 — closed 2026-10-14, before the deadline.** All floating `-latest`
  runner labels are pinned (11 sites: `ubuntu-latest` ×9, `windows-latest` ×1,
  `macos-latest` ×1, across six workflows) so GitHub's 2026-10-19 migration of
  `ubuntu-latest` to Ubuntu 26 changes nothing here; `defender-ci-scan.yml` had
  already moved to `windows-2022` during the 0.2.9 outage. Record:
  `plan/plan-repo-hygiene.md` "Fourth pass". Left as an owner question, not
  touched: the three org-owned `ubuntu-slim` labels
  (`minicon-com.yml`, `company-signing.yml`, `six-grid-runtime.yml`), which ran
  green the same day.
- GitHub #5 (the notes scratchpad) — deferred twice now; give it a leaf or a
  dated owner refusal.

## Memory palace

```mermaid
flowchart TB
    FLASH["user-reported flashing"] --> SPEC["0.2.9 wrote the spec:<br/>plan-scroll-damage.md"]
    SPEC --> GATE["{GATE-FORK} run the fork's<br/>damage oracle at all"]
    GATE --> SC["{SCROLL-DAMAGE} rows +<br/>scrollback_changed"]
    SC --> T["{AMPLIFIER-T} transient host"]
    SC --> W["{AMPLIFIER-W} Windows con"]
    T --> OUT["outcome: ordinary output<br/>invalidates rows, not the window"]
    W --> OUT
    R1["R1 runs-on re-pin<br/>deadline 2026-10-19"] -. outside the version .-> OUT
```

## GATE status

Nothing implemented; no gate run for this version. The local gate and the
`CARGO_TARGET_DIR` rule are unchanged from `plan/archive/plan-v0.2.9.md`.
