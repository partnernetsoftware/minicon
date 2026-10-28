# v0.2.26 design target — make @0 the tab-agent watch floor

Status: `[_]` selected next design sequence after public 0.2.25. This file
selects a product direction for the next version; it does not claim that
0.2.26 source, native evidence or publication exists. The owning behavior is in
[PRD 39.01](../prd/PRD_02_39_01_home_canvas.md), with @0 identity in
[PRD 39](../prd/PRD_02_39_home_dashboard.md) and operation facts in
[PRD 36](../prd/PRD_02_36_harness_workspace.md).

## Product decision

Make @0 the distinctive, scene-first **watch floor** for agents working inside
real MiniCon tabs and for the health of this MiniCon instance. A person should
spot the exact @ID needing attention, inspect what the host truly observed,
and move to that tab or its owning detail view in one deliberate action.
The office canvas represents tab stations even before typed agent events
exist; characters appear only after an accepted event source binds an actual
run to an @ID. Model API traffic or a process name alone never proves an
agent's stage. The robot view owns detailed Harness operation/monitoring and
Settings owns configuration. Console/Web fundamentals remain maintained.

The owner-described AIGC dashboard reference suggested a prominent central
stage and supporting side/summary regions. The previously named local image
was not present in this workspace at design time; PRD 39.01's wireframe is
based on that description and the current @0 implementation. If the image
becomes available later, use it to review visual hierarchy, not to import
AIGC-specific workflow or fictional job cards.

## Outcome tree DAG

```text
Outcome: 0.2.26 @0 is a trustworthy visual watch floor for tab-contained agents and local health
├── [_] V26 — open next source version only after 0.2.25 public boundary is recorded
│   invariant: released 0.2.25 artifacts remain immutable and new work has a distinct source identity
│   evidence: version/policy/lockfile consistency and 0.2.25 release-history receipt
│   safe failure: no version bump or publish inference from a design document
│   dependency: exact 0.2.25 public Promotion; non-goal: overlapping release Candidates
├── [_] U26 — review tab-fleet scenes and alert routes with native captures
│   invariant: first glance identifies tab, concern, measured system scope and exact inspect route
│   evidence: wide/4:3/narrow wireframes, English/Chinese copy and five-second multi-tab walkthrough notes
│   safe failure: ambiguity remains an explicit design issue before coding a target
│   dependency: PRD 39.01 states and the existing static shell; non-goal: final logo or unmeasured slogan claim
├── [_] C26 — build the scene-first Home composition and navigation
│   invariant: Home is read-only, scene targets carry stable @IDs, and real tab/browser drafts survive round trips
│   evidence: public GUI pointer/keyboard journey, native captures and no-side-effect traces
│   safe failure: empty office retains a working primary route and explanation
│   dependency: PRD 39 @0 host navigation; non-goal: another tab kind or Settings form on Home
├── [_] T26 — represent the tab fleet and exact drill-down
│   invariant: station/list/inspector select the same stable @ID through rename, reorder, exit and overflow
│   evidence: 0/1/many/256 mixed-tab public journeys, stale-target refusal and native click/focus receipts
│   safe failure: missing agent observer stays Unknown; closed tab never retargets another row
│   dependency: PRD 25 tab tree/session lifecycle and PRD 39.01 HC-06; non-goal: reading hidden PTY output
├── [_] A26 — prioritize attention and measured system health
│   invariant: every alert names exact tab/component, source, freshness, severity and one safe action
│   evidence: exited/stale/failed/host-unavailable mixed journeys, dedupe/resolve and measurement provenance
│   safe failure: collector outage becomes Unknown rather than false healthy/zero or an alarm storm
│   dependency: PRD 25 lifecycle, PRD 36 projection and qualified host telemetry; non-goal: unmeasured whole-OS health claim
├── [-] Z26 — converge zero-tab workspace on @0
│   invariant: closing the last real tab opens the same @0 Home, with New Terminal/New Web routes
│   evidence: development close-path/unit checks and Home action/snapshot geometry; native last-tab/action/window-close journey still due
│   safe failure: an unavailable route leaves Home and explicit recovery visible
│   dependency: PRD 39 Home identity and C26 routes; non-goal: treating @0 as a closable tab
├── [_] W26 — make +W a real New Web action
│   invariant: +W, Home New Web and shortcut create distinct Web tab IDs and independent page state
│   evidence: two-Web-tab native navigation, independent URL/history/proxy/context, auth detour, switch/close/reopen and public snapshots
│   safe failure: allocation failure preserves existing pages and shows a notice
│   dependency: replace unique browser_tab/page/address/auth assumptions with per-tab host/context lifecycle under PRD 25/28.01
│   non-goal: a second button that merely reselects or reuses one mutable page host; split-screen browsing
├── [v] F26 — make manual update feedback settle
│   invariant: each worker outcome wakes the host and install has its own visible phase
│   evidence: loopback check wake and scoped install success/failure journeys (24 passed)
│   safe failure: worker loss becomes retryable failure with the running binary untouched
│   dependency: PRD 25 update worker; non-goal: automatic update or silent restart
├── [_] S26 — join only truthful local status
│   invariant: tab/process, model request and typed agent-run claims stay separate with source/freshness
│   evidence: request-without-tab, typed exact-tab run, partial/stale/offline public scenarios
│   safe failure: Unknown/Last known replaces inferred agent work or unmeasured system health
│   dependency: PRD 25 lifecycle, PRD 36 read-only projection and accepted observer seam; non-goal: provider polling
├── [_] B26 — decide the inside-tab agent observer seam
│   invariant: a registered agent run binds to window/session incarnation, stable @ID, source and ordered run identity
│   evidence: one real CLI/TUI agent adapter feasibility trace, disconnect/restart/closed-tab negative controls
│   safe failure: unregistered agents remain visible as tabs with Unknown work state
│   dependency: PRD 26 control boundary, PRD 25 lifecycle and PRD 36 run view; non-goal: parsing PTY prose or controlling arbitrary agents
├── [_] R26 — qualify visual responsiveness, accessibility and idle cost
│   invariant: essential meaning stays visible across scale/language/window states without continuous idle repaint
│   evidence: 1x/1.5x/2x native visual courts, focus order, reduced-motion and idle repaint measurements
│   safe failure: decorative detail collapses before targets or state text do
│   dependency: shared host palette/font/input; non-goal: animation used as an information source
├── [_] E26 — preserve a gated path to real office characters
│   invariant: a character requires exact @ID, run identity and ordered lifecycle events from an accepted source
│   evidence: contract review and, only if available, one real run/cancel/final native journey
│   safe failure: release still shows truthful tab stations with Unknown agent state
│   dependency: PRD 36/40 app seam or separately accepted tab-bound observer; non-goal: inferring arbitrary CLI agent state
└── [_] Q26 — qualify and decide exact-source release
    invariant: every claim matches the source, platform and observation that proved it
    evidence: selected smoke/scoped suites, fmt/clippy, six-cell, native visual/behavior courts and release receipts
    safe failure: missing evidence blocks only the corresponding claim or Promotion gate
    dependency: PRD 27/33 release chain and explicit owner authority; non-goal: treating a mock as a public artifact
```

## Memory palace

```mermaid
flowchart LR
    P25["Public 0.2.25 exact boundary"] --> V26["0.2.26 source opened"]
    V26 --> UX["U26: scene/copy/native review"]
    UX --> HOME["C26: @0 watch-floor composition"]
    HOME --> FLEET["T26: stable @ID stations/list/inspector"]
    FLEET --> ATT["A26: exact attention + scoped system status"]
    HOME --> Z26["Zero real tabs use @0"]
    WEB["PRD 25/28.01 per-tab Web host + context lifetime"] --> W26["+W / Home / shortcut create distinct Web tab"]
    V26 --> F26["Update worker wakes on every outcome"]
    WORK["PRD 25 real @1+ tab/child lifecycle"] --> FLEET
    VIEW["PRD 36 read-only gateway/request view"] --> S26["S26: distinct evidence rungs"]
    TELE["Qualified host-process telemetry"] --> ATT
    S26 --> ATT & FLEET
    HOME --> R26["Responsive/focus/reduced-motion court"]
    APP["PRD 40 worker or accepted tab-bound observer"] --> H2{Exact @ID + run lifecycle?}
    FLEET --> B26["B26: agent observer seam decision"]
    B26 --> H2
    H2 -->|yes| ACTOR["One observed run: actor at its tab"]
    H2 -->|no| STILL["Tab station; agent state Unknown"]
    ACTOR & STILL --> HOME
    HOME & FLEET & ATT & B26 & R26 & Z26 & W26 & F26 --> Q26{Exact-source release gates}
    Q26 -->|pass + authority| PUB["Public 0.2.26"]
    Q26 -->|missing native/real proof| BLOCK["BLOCKED claim; no invented evidence"]
```

## Sequence

1. **Anchor the public 0.2.25 boundary.** Use its recorded source/artifact
   receipt and carry forward only accepted unresolved PRD requirements. Do not quietly
   move 0.2.25's connection or Web/auth P0 obligations into this visual slice.
2. **Review the product, then freeze the interaction map.** Compare the
   current @0 screenshot with PRD 39.01 at wide, 4:3 and narrow sizes. Walk
   six situations: zero tabs; one working terminal; many identically named
   CLI-agent tabs; an exited tab; a stale tab observer plus failed account;
   an active gateway request with no exact tab/run correlation. Record the
   first concern noticed, exact @ID/owner, source time, primary route and
   recovery. Keep accepted behavior in PRD 39.01.
3. **Implement composition and tab drill-down.** Build the tab office stage,
   summary, attention rail, system card and bounded All tabs list in the native
   host. Each station/inspector targets a stable @ID; rename, reorder, close
   and 256-tab overflow cannot retarget another row. Use code-native shapes
   and typography. Public CLI/snapshot tests prove exact selection and state
   preservation; captures prove hierarchy.
   Reuse the already implemented zero-tab-to-@0 transition and Home New
   Terminal/Web hit targets. Qualify them natively before treating Z26 as
   complete. W26 must first replace the current single `browser_tab` and
   `native_browser_page`/`browser_page`/address/auth assumptions with per-tab
   host and context lifetime, including isolated authorization detours. Only
   then can +W, Home New Web and the shortcut promise a new independent page;
   button copy alone is not acceptance. The native two-Web-tab navigation,
   switching, auth, close and reopen court is an explicit gate.
4. **Join facts and attention through owners.** Present PRD 25 tab/child
   lifecycle, PRD 36 model API/request projection and qualified MiniCon-host
   telemetry as separate evidence rungs. Instrument an output timestamp or
   CPU/RSS only if its source, cost and platform scope can be proven. Deduplicate
   attention by source/key; resolve it when the owner recovers. A model request
   without exact correlation cannot illuminate a specific tab's agent or
   spawn a character. Home entry adds no provider polling or process probe.
5. **Qualify native presentation.** Claim each OS/scale/language only after its
   named native court. Check short-height scroll, focus, hit targets, text
   contrast, reduced motion, browser parking and idle CPU/repaint behavior.
   Use captures to settle visual hierarchy and public journeys to settle
   behavior. Integrate shared `src/main.rs`, `src/host_paint.rs`, `src/ui.rs`,
   `src/snapshot.rs` changes serially against the public 0.2.25 baseline.
6. **Decide the inside-tab observer seam.** Assign its protocol/lifecycle owner
   outside the Home painter. A real CLI/TUI agent adapter feasibility trace
   must bind the event source to window/session incarnation and @ID and prove
   stale/closed-tab refusal. A generic process name or PTY text is not enough.
   This decision is part of the 0.2.26 design handoff even if the adapter
   implementation lands later.
7. **Gate real actors separately.** If the independent worker or a separately
   accepted inside-tab observer publishes exact @ID/run identity, ordered
   lifecycle and source freshness in time, run PRD 39.01's event/observer-loss
   court before enabling characters. Otherwise ship still tab stations with
   Unknown agent state and keep HC-05 open.
8. **Run exact-source release gates.** Reconcile PRD status and evidence before
   archiving this plan. Promotion needs the owner-selected 0.2.26 decision and
   PRD 27 Candidate/Defender/reputation chain; this design request is not
   pre-authorization to publish the next version.

## Cut, deferrals and handoff

The intended 0.2.26 cut is HC-01 through HC-04 plus HC-06/07: scene-first
@0 tab fleet, exact drill-down, prioritized attention, scoped system status
and responsive native UX, plus the B26 observer-seam decision. HC-05 characters
are a separately gated future capability. Arbitrary inside-tab agent stage
observation implementation, H2 task launch/Stop,
cross-machine coordination,
computer use, dynamic ujs/WASM scheduling, plugin management, provider auto
rotation and installation remain with their existing owners and do not become
Home canvas prerequisites. A truly unavailable owner is shown as unavailable;
it is never filled with a demo-looking worker.

The handoff to implementation should include a native current-state capture,
the selected wide/narrow composition, a state/action matrix and exact public
journeys from PRD 39.01. Before making visual claims, confirm the actual
local reference image if it is reintroduced; its absence does not block the
truthfulness and navigation requirements above.
