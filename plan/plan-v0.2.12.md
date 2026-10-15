# plan-v0.2.12 — structure and robustness (quality release; features deferred)

Owner's direction (2026-10-15): this release is themed on **product code
structure, module structure, abstraction and reuse, encapsulation and
coupling, robustness and performance** — incremental refinement across all
of those axes. Functional needs (the #5 notes scratchpad, new surfaces of
any kind) defer to 0.2.13 and later.

## Why quality now

0.2.10 shipped the first public crash; 0.2.11 fixed it; and within a day a
third defect surfaced (the rename editor advanced invisibly under the
damage model). All three share one root shape: **state, damage ownership
and event routing live in different places, and a new dispatch site can
forget one of them while every existing test stays green.** The fix for
rename (e986344) was also the first structural payment: injected UI input
now rides `ConApp::event` as the single routing authority, and host UI
became black-box reachable (`send-ui-mouse`). 0.2.12 makes that shape the
rule instead of a one-off rescue.

## Scope (all quality-axis, no new features)

1. **Damage ownership as an API, not a convention.**
   The rename bug was a dispatch arm that returned without marking damage;
   a bare `request_redraw` re-presented the last frame. Today
   `mark_tree_dirty` / `mark_composer_dirty` / `mark_host_ui_full` are
   conventions scattered across dispatch sites. Consolidate: one
   input-surface → damage-ownership boundary so an arm that consumes an
   event cannot forget its damage. Include a guard (unit test or debug
   assertion) that fails a dispatch branch which swallows Keyboard/Ime
   events without a damage mark — the exact trap class that produced 0.2.11
   and the rename defect.

2. **Split main.rs along its seams.**
   main.rs is the last monolith: rename state machine, composer routing,
   snapshot assembly, pixel helpers and the window loop all share one file.
   control_dispatch.rs and host_paint.rs proved the pattern works (moved
   verbatim, no behavior change). Candidates in dependency order:
   rename editor (state + key/IME handling, ~100 lines, already
   self-describing), snapshot builders, pointer-gesture handling. Each move
   is a separate commit, each commit is verbatim-plus-imports only.

3. **Injected-input parity as a standing invariant.**
   e986344 unified send-ui-keys / send-ui-ime / send-ui-mouse onto the real
   event path. Lock it: a contract test that walks the dispatch arms of
   `event` and the control injection paths and requires them to share one
   routing entry point, so a future focus owner (notes editor, settings
   search) cannot be wired into one path and forgotten in the other.

4. **Journey-suite robustness.**
   `controlled_resize_storm` failed once inside a full parallel-suite run
   and passed in isolation — timing-budget flakiness under load. Re-audit
   the frame/timeout budgets of the load-sensitive journeys so a green
   suite does not depend on machine mood. (Not the assertion itself: the
   budgets.)

5. **{AMPLIFIER-T}, measurement-first** (carried; demoted from feature to
   robustness probe): measure whether Transient-retention forced-full
   presents are felt on any shipped platform. If measured-not-felt, close
   the leaf with numbers recorded; only build the retained-canvas machinery
   if measurement says otherwise.

## Explicitly deferred (to 0.2.13+)

- **#5 notes scratchpad** — unchanged from the previous draft of this plan
  (kept in git history, commit 5dfe4b7): panel section, Global-only,
  PTY isolation, focus/IME arbitration. It is the second editable field and
  deserves a release whose theme is features, not one stacked on a
  refactor.
- **{NOTES-FEEDBACK}** — follows the notes release.
- No new control verbs beyond what testability already forced
  (send-ui-mouse landed with the rename fix); no renderer behavior changes;
  0.2.11's band-present fix stands as-is.

## Risks and their fences

| Risk | Fence |
|---|---|
| "Refactor" drifts into behavior change | Every move is verbatim-plus-imports; behavior changes (damage API, routing guard) are separate commits with their own tests; clippy `-D warnings` + full suite per commit |
| Damage API over-abstracts | The API is exactly the three mark helpers plus the dispatch-site pairing; anything more is rejected |
| Module split churns conflicts with in-flight fixes | Split commits land early in the cycle; the release chain (six-grid → signatures → Candidate → Defender → reputation → publish) starts only after the tree is quiet |
| Budget re-audit masks a real regression | Only budgets move; assertions and thresholds stay, and any test that needed a budget change gets its reason recorded in a comment |

## GATE status

Planned 2026-10-15. Started: e986344 (rename fix + single routing
authority + send-ui-mouse testability) is scope item 3's first payment.
Owner's go signal = start of work.
