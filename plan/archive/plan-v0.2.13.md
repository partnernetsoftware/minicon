# plan-v0.2.13 — the features release (#5 notes scratchpad)

Owner's direction (inherited from 2026-10-15): 0.2.12 was the quality
release (structure, damage API, module split, parity invariant, tiered
tests — all shipped, plus the post-release platform-boundary guard
`ad0f071`). Functional needs were explicitly deferred to "0.2.13 and
later" — this is that release.

## Why notes now

The notes scratchpad (#5) is the only named feature deferred twice
(plan-v0.2.11 kept it out of the rename-fix release; plan-v0.2.12's
"Explicitly deferred" section names it verbatim). More important than
schedule debt: 0.2.12 built exactly the rails it needs —

- `TextInputOutcome` (Changed/Unchanged-with-reason) is the damage ledger
  every editable surface must ride;
- `ConApp::event` is the single routing authority, and
  `injected_ui_input_rides_the_physical_event_pipeline` pins that any new
  focus owner wired into one path cannot be forgotten in the other;
- `rename.rs` is the template: a second editable field that talks to the
  rails is now a known shape, not new architecture.

The 0.2.11/rename defect class was "a new dispatch site forgets state,
damage or routing while tests stay green." Notes is the first feature
built *under* those fences — which is also the test of the fences.

## Scope

1. **Notes panel section (the #5 scratchpad).**
   A Global-only notes panel section: free text, one per workspace,
   PTY-isolated (notes never reach the shell). Design rails already fixed
   by the previous draft (5dfe4b7): panel section, Global-only, PTY
   isolation, focus/IME arbitration.

2. **Focus + IME arbitration for the second editable field.**
   Composer, rename editor and notes cannot all own IME at once; the
   arbitration follows the existing rename pattern (focus owner decided
   in `ConApp::event`, IME anchor update per owner). The parity invariant
   test must gain the notes arm — that is the guard 0.2.12 was built to
   provide.

3. **Persistence.**
   Notes survive restart via the existing config/session store
   (`session_store.rs` / `minicon.json`), atomic write through
   `filesystem_publish` — no new persistence mechanism.

4. **{NOTES-FEEDBACK} leaf (carried): notes into harness context.**
   If a session is agent-harness-backed, an explicit Send-to-harness
   affordance feeds the note as context (never automatic — the agent must
   not silently read the user's scratchpad). Exact wire verb follows the
   harness-context-engineering plan's shape; if that plan's open design
   questions are not settled in time, this leaf defers again **with an
   owner-visible note**, not silently.

5. **Test tiering adoption (process, not code).**
   Development loop runs `scripts/test-tier.sh l0/l1`; per-feature check
   adds the touched l2 suite; `l3` runs before the release chain only.
   Release = the standard 8-step chain (`scripts/release.sh 0.2.13`).

## Explicitly out of scope

- No new control verbs beyond what notes testability forces (and any
  forced verb rides the six-site registration discipline).
- No renderer behavior changes; no alt-screen scrollback semantics (C1
  remains carried debt with its vt100-side premise now solved — a
  separate release's work).
- macOS/Linux platform work: none beyond what CI cells already prove.

## Risks and their fences

| Risk | Fence |
|---|---|
| Notes becomes a third monolith in main.rs | New `notes.rs` module from day one, rename.rs shape; alignment's file-shape guard extended if needed |
| IME arbitration regresses composer/rename | Parity invariant gains the notes arm; rename + composer journeys re-run per commit; mutation-verify the new arm |
| Persistence corrupts config | Atomic write via `filesystem_publish`; corrupt-notes is a bounded error like the harness session rule (no silent fresh start) |
| NOTES-FEEDBACK scope creep | Explicit Send-only affordance; automatic feeding is a hard no; deferral-with-note is the sanctioned exit |

## GATE status

Drafted 2026-10-16. Owner's go signal = start of work (this document's
commit on main).
