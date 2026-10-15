# plan-v0.2.12 — user notes scratchpad (#5) + {AMPLIFIER-T} debt

## Product thinking first

MiniCon's promise is `one file · no install · nothing left behind`. Every
feature so far serves the *terminal* (run, read, scroll, resend). The one
thing a terminal cannot do is hold the *user's own intent* — the hostname
you'll need in ten minutes, the TODO for the pipeline you're watching.
That thought today lands in a different window or dies. #5's product
thesis: a scratchpad inside the terminal, never sent to the PTY, is the
smallest feature that makes MiniCon a place you *stay* rather than a
surface you *use*.

The information architecture is already reserved: the right-side panel
(#3) was designed with history (top) · notes (middle) · loop/goal
(reserved, bottom). 0.2.12 fills the middle slot.

The second thread is trust. 0.2.10→0.2.11 was the first public crash —
owners forgive one, but the product's credibility now depends on the
render pipeline's remaining debt being paid visibly. {AMPLIFIER-T} is
that debt: the transient host still forces full frames, which is the
same amplifier class 0.2.10 removed on Windows. Paying it on the *other*
host completes the flashing story on both sides of the product.

## Scope

1. **{NOTES} — GitHub #5 phase 1** (session-scoped skeleton, the issue's
   decided defaults):
   - Notes section in the right-side panel, below sent-history, own
     collapsible section label; does not touch the loop/goal reserved
     sub-area.
   - One editor, target = current context. Phase 1 ships the Global
     note + `This tab / Global` toggle with only Global active (per the
     issue's global-first decision); per-tab follows after the
     interaction is proven.
   - **Never written to the PTY.** Clicking a note edits it; clicking
     history fills the composer. The two must never be confusable.
   - Session-only — must be described as a scratchpad, not memory; no
     persistence, no charter change, nothing irreversible.
   - The hard part is focus/IME: the product has exactly one text input
     surface today (the composer, with the "exactly one focus owner"
     a11y invariant and IME routing bound to it, PRD_02_25 §Host UI).
     The notes editor is the second editable field: focus arbitration
     (composer must not receive keys while notes is focused; Escape
     blurs notes back to the composer), and the IME publish path
     extended to route to the notes field. This is the engineering
     risk, not the drawing.
   - Observability per AGENTS.md: `ui-snapshot` exposes the notes
     section (open/closed, context, text, geometry); a black-box
     journey types into the notes editor through the real input path
     and reads it back; a second journey proves PTY isolation (note
     text never appears in the terminal stream).
   - Control surface: **no** `--control` read or write path in phase 1
     (the issue's privacy question stays unanswered — do not open the
     cross-tab disclosure surface).

2. **{AMPLIFIER-T}** (carried from 0.2.10, design note in
   plan/plan-v0.2.10.md): on a Transient-retention host the frame
   contract rejects partial commits (`TransientRequiresFull`) and a
   fresh buffer needs every pixel — so "narrow the invalidation" there
   means retaining a second canvas and presenting a diff band, which is
   real machinery. Two honest options at implementation time:
   (a) build the retained-canvas path with its own journey, or
   (b) if measurement shows the transient host's forced-full presents
   are not a user-visible amplifier on any shipped platform, close the
   leaf as measured-not-felt with the numbers recorded. Decide by
   measurement, not preference.

3. **{NOTES-FEEDBACK}** — after landing, watch how the owner actually
   uses notes for a few days (global-only assumption, scratchpad vs
   memory expectations). Feeds the phase-2 gate (persistence charter,
   durable tab identity, privacy decisions — all owner-authority
   questions, not this release).

## Non-goals

- No persistence, no charter wording change, no per-tab notes, no
  templates/sendable snippets, no search/tags/formatting.
- No control-CLI notes surface of any kind.
- No loop/goal work — the reserved area stays reserved.
- No renderer changes beyond {AMPLIFIER-T} if it ships; 0.2.11's fix
  stands as-is.

## Risks and their fences

| Risk | Fence |
|---|---|
| Second editor breaks the one-focus-owner invariant (IME, AT-SPI, IMM32) | The invariant is asserted in existing journeys; notes journeys must cover CN IME input on Windows at minimum |
| Notes text leaks into PTY | Dedicated isolation journey: type a note, assert the byte never appears in the child's stream |
| Panel gets crowded (history + notes) | Notes section is collapsed by default; the region's IA was designed for this in #3 |
| {AMPLIFIER-T} grows beyond a small release | The measured-not-felt exit (b) is a legitimate close; do not build machinery without a felt defect |

## GATE status

Planned 2026-10-15. Not started. Owner's go signal = start of work.
