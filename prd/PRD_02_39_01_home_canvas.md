# @0 visual home canvas

Parent: [@0 destination and tab boundary](PRD_02_39_home_dashboard.md).
Operational facts and exact Settings links: [PRD 36](PRD_02_36_harness_workspace.md).
Optional worker/event authority: [PRD 40](PRD_02_40_plugins_apps.md).
Delivery sequence: [0.2.26 design plan](../plan/plan-v0.2.26.md).

Status: `[_]` 0.2.26 design target, refined for the owner's 2026-10-28
direction: @0 is the command overview for agents working **inside MiniCon
tabs** and for the local system, with a large visual scene. The public 0.2.25
@0 implementation is a static brand/office card with host-chrome navigation;
it supplies a truthful starting point, not this canvas's acceptance.

The current 0.2.26 development tree already routes closing the last real tab
to @0 and paints **New Terminal / New Web** actions there, with shared
geometry in public snapshots. These are development implementations pending
their named native journeys. New Web must not be described as independent
per-tab browsing until PRD 25/28.01's W26 host/context lifecycle replaces the
current single-browser-tab/page/address/auth assumption and proves two
separate Web tab IDs, navigation, authorization isolation and return state.
The tab-office design below uses only observed Web facts in the accepted host.

## Outcome tree DAG

```text
Outcome: @0 lets one person watch many tab-contained agents and local health, then act on the exact concern
├── [_] HC-01 — scene-first visual hierarchy
│   invariant: tab fleet, attention and system health have distinct ranks; the central scene remains dominant
│   evidence: native 16:9, 4:3 and short-window captures; five-second tab/alert reading with English/Chinese labels
│   safe failure: unavailable data leaves the composition intact with a clear empty explanation
│   dependency: PRD 39 fixed destination and shared host theme/font metrics
│   non-goal: a second Harness Settings page or an always-running wallpaper animation
├── [_] HC-02 — truthful tab, agent and system projection
│   invariant: tab/process facts, typed agent work and system telemetry remain separate, each with scope/source/freshness
│   evidence: public snapshots and mixed active/exited/unreported/stale/error native journeys
│   safe failure: lost owner data removes claims and preserves explicit routes to inspection/retry
│   dependency: PRD 25 tab/session state, host telemetry owner and PRD 36 typed run/request projection
│   non-goal: detecting agents from title/PTY text, polling providers or exposing private output
├── [_] HC-03 — focused, reversible navigation
│   invariant: every clickable region has one target and opening @0 does not mutate a service, account, task or tab
│   evidence: pointer, keyboard, focus and no-activation journeys across Home, real tab, robot and Settings
│   safe failure: unavailable destination remains explained and Home remains usable
│   dependency: PRD 39 @0 routing and PRD 36 typed deep links
│   non-goal: task Start/Stop, account editing or direct PTY input on Home
├── [_] HC-04 — responsive and accessible canvas
│   invariant: identity, primary action and status retain order and full targets at every supported window size
│   evidence: 1x/1.5x/2x native captures, narrow/short scroll and tab-order/readout courts
│   safe failure: condensed one-column view replaces decorative scene detail before essential controls clip
│   dependency: host font scaling, language/theme choices and native input focus
│   non-goal: color-only status or animation-only meaning
├── [_] HC-06 — tab fleet and exact drill-down
│   invariant: every station/card addresses a stable @ID and selects that tab or its evidenced inspector
│   evidence: 0/1/many/256 mixed terminal-Web tabs, reorder/rename/exit and exact selection journeys
│   safe failure: unobserved agent state stays unknown; overflow uses a bounded list, never clipped desks
│   dependency: PRD 25 tab tree and public stable IDs; non-goal: reading hidden tab text or controlling arbitrary child processes
├── [_] HC-07 — attention queue and scoped system status
│   invariant: each item names its cause, affected @ID or component, observed time and one safe repair route
│   evidence: mixed exited tab, stale observer, failed provider and host pressure; sorting/dedupe/recovery journeys
│   safe failure: collector loss says Unknown, never healthy; a missing reading is not an alarm storm
│   dependency: tab lifecycle, PRD 36 attention projection and measured host telemetry
│   non-goal: unmeasured OS-wide health claims, automatic remediation or duplicate provider billing UI
└── [_] HC-05 — office characters only after typed runs
    invariant: one character maps to one observed run ID, stage and source time; request counts never become people
    evidence: real event start/tool/wait/cancel/final traces plus stale/restart/no-run states
    safe failure: empty desks and static labels when events are absent, stale or app unavailable
    dependency: PRD 36 H2/PRD 40 worker events or an accepted tab-bound typed agent-observer seam
    non-goal: 0.2.26 release gate, autonomous launch, remote agents or simulated productivity
```

## Memory palace

```mermaid
flowchart LR
    TAB["@1+ real tabs / current workspace"] -->|select @0| HOME["PRD 39 fixed Home"]
    HOME --> HERO["Identity + fleet summary"]
    HOME --> SCENE["Dominant tab-office scene / stable @IDs"]
    HOME --> ALERTS["Prioritized attention + system rail"]
    SCENE -->|select @ID| TAB
    SCENE -->|inspect @ID| DETAIL["Tab evidence inspector"]
    DETAIL -->|open linked run| ROBOT["PRD 36 robot operation view"]
    ALERTS -->|affected @ID or component| DETAIL
    ALERTS -->|exact account/client| ROBOT
    ROBOT -->|change configuration| SETTINGS["Settings owner"]
    TABSTATE["PRD 25 tab/process lifecycle"] -->|stable ID + observed exit| SCENE
    GATEWAY["PRD 36 account / API / request projection"] -->|read-only scoped facts| ALERTS
    GATEWAY -->|request signal; no agent claim| SCENE
    HOST["Measured MiniCon host/process telemetry"] -->|source/time + unavailable| ALERTS
    EVENTS["Typed worker or accepted tab-bound agent events"] --> BIND{Exact @ID + run identity + fresh lifecycle?}
    BIND -->|yes| ACTOR["One observed run → one character at its tab"]
    BIND -->|no, stale, or absent| EMPTY["Tab station with unknown agent state"]
    ACTOR & EMPTY --> SCENE
    PREF["Theme / language / reduced motion"] --> HERO & SCENE & ALERTS
    LOSS["Owner unavailable"] --> UNKNOWN["Unknown + source + route; preserve navigation"]
    UNKNOWN --> ALERTS & DETAIL
```

## Product role and first-glance questions

@0 is the **window's watch floor** for work spread across real @1+ tabs. In
five seconds it should answer: (1) which tab needs me, (2) what was actually
observed there, (3) whether this MiniCon instance and its model gateway are
healthy, and (4) where one click takes me to inspect or repair the exact
entity. A Console tab running Codex, Cursor, Grok, pi or another CLI/TUI agent
is still a terminal tab. MiniCon can observe its own tab and child-process
lifecycle; it cannot infer the agent's task stage, intent, success or whether
the external service is online from a title, PTY pixels, output silence or a
model request alone. An explicit tab-bound event contract is required for
those claims. A Web tab remains a Web tab unless a separately accepted adapter
supplies equally scoped evidence.

The visual scene represents the **tab fleet**, with one stable station per
represented @ID. A station is useful before agent telemetry exists: it can
show the tab label, kind, observed child state and an exact route. A character
appears only for a typed agent run joined to that tab. The right rail ranks
things needing attention and measured system status; a compact bottom strip
shows recent observed changes. The robot view remains the detailed operation
surface for account, client, request and owned worker activity. Settings
remains the configuration editor. Home does not acquire their authority.

### Wide-window wireframe, content area excluding the existing tab tree

```text
┌──────────────────────────────────────────────────────────────────────────────┐
│ MiniCon @0 · watch floor      N real tabs · M need attention · observed at time │
│ Focus last tab        Open Harness       New Terminal / Web      system status  │
├──────────────────────────────────────────────────────┬───────────────────────┤
│                                                      │ Needs attention       │
│     TAB OFFICE / dominant stage                       │ @7 child exited       │
│     @3 terminal · child running · agent unknown       │ @3 observer stale     │
│     @7 terminal · child exited · inspect              │ gateway error         │
│     @9 Web · page state owner                         │ exact inspect route   │
│                                                      ├───────────────────────┤
│     character only for fresh typed run bound to @ID   │ This MiniCon instance │
│     scene overflow → All tabs, not extra tiny desks  │ host / PTY / gateway  │
│                                                      │ source + time         │
├──────────────────────────────────────────────────────┴───────────────────────┤
│ Recent observed changes: @ID / component · source · age     All tabs / details │
└──────────────────────────────────────────────────────────────────────────────┘
```

The scene should occupy roughly three fifths of a wide content column and
remain the largest single shape. The first row exposes **Focus last tab** when
one exists; otherwise **New Terminal** is primary. The attention rail can
raise a few exact tab or component issues, sorted by urgency then observation
time, without hiding the rest behind a single global badge. A separate system
card says **This MiniCon instance** so a host-process reading cannot be read
as whole-computer health. Do not show generic zero KPI tiles above the scene.
Keep the header, first attention item and at least one tab station visible
before the activity strip when height is limited.

### Visual language and copy

- Use the current theme palette and font scaling. Warm, architectural shapes
  (floor, desks, lights and windows) provide a distinct @0 identity while
  status text and controls retain the host's high-contrast tokens. No raster
  scene is required for acceptance; native shapes/text keep the one-file
  distribution and scaling contract.
- A desk is an **available place**, not a person. Before an accepted typed
  event source, there are no
  humanoid silhouettes, walking loops, fake status bubbles, avatars or
  elapsed timers. Request activity can appear as a labelled signal or light
  on a machine, explicitly named **model requests**, never **agent online**.
- Brand copy remains provisional as PRD 39 says. Avoid an unmeasured speed or
  uptime guarantee. A short localized title and one sentence of current state
  outrank decorative slogans on small windows.
- State words accompany color and icon: Ready, Needs attention, Unknown,
  Offline, Stale, Running (for an observed run only). Source and last observed
  time sit adjacent to a mutable value; a tooltip alone is insufficient.
- Stage motion is optional and event-driven. No idle animation, blinking,
  chart treadmill or periodic redraw merely to look busy. Respect OS reduced
  motion, pause when Home is hidden, and provide the same state in static text.

## State and source contract

The scene and inspector use an explicit evidence ladder. **Observed tab**
means MiniCon owns a stable @ID, kind and label. **Observed child** means the
PTY/browser owner reports a running or exited child; it does not mean a CLI
agent is working. **Observed request** means the local model gateway saw a
request, not that any particular tab made it unless an exact correlation ID
exists. **Observed agent run** requires an accepted typed event source bound
to one @ID, run identity, ordered stage and source time. No weaker rung is
silently promoted to the next. Display the rung in copy and public snapshots.

| State on @0 | Station/rail treatment | Safe route and claim | Proof owner |
| --- | --- | --- | --- |
| No real tabs | Still empty office; no zero-valued agent statistic | New Terminal primary, New Web secondary | PRD 39 workspace |
| One or many tabs, no agent observer | Stable @ID/kind/label station; child state if known; no character | Focus exact tab; agent work **Unknown / not connected** | PRD 25 tab/session lifecycle |
| Terminal child exits | Keep station with exited marker and observed code if available | Inspect/focus exact @ID; exit is not agent-task failure | PRD 25 PTY lifecycle |
| Typed run begins in @ID | One character on that tab's stable station | Open exact run inspector; show stage/time | Accepted tab-bound event owner |
| Typed observer stalls or disconnects | Freeze/clear actor motion, retain last observation time | **Lost contact / last known**, not Running or Completed; inspect @ID/source | Accepted observer heartbeat/sequence |
| Web tab without adapter | Web station with its own available page state | Open exact @ID; no agent status | PRD 25 Web owner |
| Model API request without tab correlation | Small gateway/request signal outside agent station | Inspect gateway request; no claimed tab or worker | PRD 36 model API |
| Applied profile, API stopped | System rail states listener stopped; office tabs unaffected | Open Harness; no claim that external clients stopped | PRD 36 applied/listener view |
| Partial/stale account or system reading | Keep unaffected stations; affected row names source/time | Last known/Unknown and exact inspect link | Each measured owner |
| Host collector unavailable | System rail uses Unknown rather than healthy/zero | Retry/inspect collector when available; tabs remain usable | Native host telemetry owner |

### What the screen may observe

| Surface | Present source or required seam | Scope shown to user | Prohibited inference |
| --- | --- | --- | --- |
| Tab count/order, @ID, label, kind, selected tab | PRD 25 workspace tree | This MiniCon window; complete current tree | Number of agents or task successes |
| PTY child running/exited, exit code where known | PRD 25 terminal session | Named terminal child, not all descendants | CLI/TUI agent's goal, stage, liveness or success |
| Output activity | Only if a bounded timestamp is intentionally instrumented at PTY drain | Last bytes observed at @ID, not user activity | Silence means idle, hung or waiting for model |
| Model API listener/request/usage | PRD 36 projection | This local API and its recorded window | External client process online or exact tab without correlation |
| App or tab-contained agent stage | PRD 36/40 H2 or future accepted tab-bound observer | Exact run ID + @ID + event source/time | Guessing from process name, OSC title, colors or terminal text |
| MiniCon system health | Native measured host process/queue owner | **This MiniCon instance**; metric, unit, window and sample time | Whole OS/cluster health, provider availability or unmeasured CPU/RSS |

The system rail has two visibly separated scopes. **This MiniCon instance**
starts with host-owned window/tab count, terminal child exits, bounded queue
pressure/drop counters when published and local API listener state. Its
resource section targets process CPU and resident memory through an
OS-qualified, bounded sampler. **This computer** may show whole-machine CPU
and memory only from a separately qualified OS sampler; otherwise it says
Unavailable. Every resource number needs unit, sampling window, source and
time. MiniCon-process memory is never labelled as total-machine memory.
Sampler work must stop or lower frequency while @0 is hidden and stay within
an observed idle cost; the canvas alone does not justify continuous polling.
Public snapshots omit hostnames, addresses and raw process command lines.

The Home adapter reads bounded projections from the owning workspace, PRD 36
and any accepted telemetry seam. It must not copy credentials, proxy addresses,
raw PTY lines, private task output, unrestricted model text or page URLs into
scene labels, public snapshots or screenshots. Values from different freshness
windows cannot be silently summed. An aggregate or rate needs a named unit,
window and complete source coverage. Entering Home never starts the API,
refreshes an account, sends inference, wakes an app or probes a child process.

### Attention and recovery

An attention item has a stable source/key, severity, affected @ID or component,
short observed fact, source time, freshness and **one exact next action**.
The default order is urgent actionable failures, then degraded/stale sources,
then informational changes; within a level, newest first. Coalesce repeated
notices from the same source/key and keep the count/last time. A status changing
back to healthy resolves its notice with a bounded recent trace, rather than
leaving a permanent red card. A missing collector reading is an Unknown row,
not an explosion of per-tab alarms. A global badge counts unresolved,
actionable items only; it does not equate every exited shell with a failure.

Examples: @7 child exited → **Open @7**; @3 agent observer lost contact →
**Inspect @3 observer**; applied account proxy failure → **Inspect exact
account in Harness**; measured PTY queue pressure → **Inspect this MiniCon
instance**. The first two are tab-scoped, the latter two component-scoped.
Home may focus or inspect; it does not kill an arbitrary CLI agent, restart a
provider login or silently dismiss an error. Notification dismissal, if later
added, is only a local view choice and does not change the underlying state.

## Interaction and responsive behavior

One click/Enter on **Focus @ID** selects that exact real tab without changing
its ID, text, page, proxy or process. Selecting a station first opens a compact
**tab inspector** on @0, with stable @ID and label, kind, child lifecycle,
last measured activity and time, agent-observer state, linked run/request only
when exactly correlated, and one Focus tab action. It shows Unknown explicitly
for missing agent data. A second explicit action opens the exact run in the
robot view when the typed seam provides one. The scene background is inert;
hover cannot be the sole way to discover a target. Clicking an attention item
selects the exact station/inspector or the owning component. A tab closed
between alert creation and click yields **Tab no longer exists** and retains
Home; it never retargets another tab by tree row position.
Existing rename/close/tree actions remain in the PRD 25 tab tree; the inspector
can route to that real tab's management affordances without silently closing
its child or starting a new task from Home.

An **All tabs** list preserves stable tree order, hierarchy, label and kind,
with attention-first filtering and a bounded scroll viewport. The stage
shows a small deterministic subset: selected tab, actionable tabs, then the
most recently observed tabs, without duplicating any @ID; remaining tabs
stay in All tabs. A 256-tab workspace is a list problem, not 256 tiny office
desks. A rename/reorder updates the same @ID station rather than replacing it.
Keyboard order follows visual reading order: summary/action, attention, scene
stations, inspector, system, All tabs. Arrow movement inside the station/list
group must not steal terminal keys while @0 is hidden. Escape closes an open
inspector before leaving @0; tab tree remains an independent direct route.
Visible focus, status text and screen-reader labels include the @ID so two
identically named tabs remain distinguishable.

The layout responds to **available content width and height after the tab
tree**, not the physical monitor size. Proposed cutoffs are design targets to
measure with real font metrics, not prequalified constants:

| Available content width | Composition | Required retention |
| --- | --- | --- |
| >= 1080 DIP | Fleet summary + broad tab scene + right attention/system rail + bounded activity strip | Scene is largest, @ID targets and first alert remain labelled |
| 720–1079 DIP | Scene above attention and system cards; activity below | First actionable tab and exact route stay visible; no horizontal scroll |
| 480–719 DIP | One column: summary/actions, attention, simplified tab scene/list, system, activity | Alerts precede decoration; no status text overlays art |
| < 480 DIP | Scrollable tab list replaces most decorative stage | Stable @IDs, first action, first alert and system Unknown remain reachable |

At short heights, content scrolls vertically while the host's global creation,
Settings and robot routes remain reachable. At 1x/1.5x/2x and all three UI
languages, no hit target overlaps another; text wraps within its card or moves
below it. The first viewport exposes identity, tab count, first attention
item or explicit none, and a Focus/New route. 16:9 is the hero composition;
4:3 and narrow windows retain the same evidence with fewer objects. One
screen-reader/a11y label describes each actionable @ID station, alert and
system state without announcing decorative desks.

The design follows the general responsive principle of reflowing around the
window's available space and keeping essential information accessible; see
[Microsoft's Windows layout guidance](https://learn.microsoft.com/windows/apps/design/layout/responsive-design).
For motion and color, use static equivalents and explicit text, consistent
with [WCAG 2.2's color and pause principles](https://www.w3.org/TR/wcag/).
These are design references, not a claim that a native MiniCon build has
passed web conformance.

## Office staging and release boundary

When PRD 36/40 H2 or a separately accepted tab-bound observer supplies typed
events, the visual office uses a bounded stage grammar. An external CLI/TUI
agent inside a terminal is eligible only through that tab-bound observer;
the worker app is one possible source, not the only product subject.

| Observed agent-run state | Office treatment | Text and interaction |
| --- | --- | --- |
| Starting | Character arrives at a stable assigned station | Starting; exact run opens robot inspector |
| Running with tool event | Character works at that station; one small tool cue | Running + last typed stage; no private tool output on Home |
| Waiting for model or allowed child | Character pauses at the same station | Waiting; no artificial progress percentage |
| Cancelling | Character remains visible, with a pending cue | Cancelling until cleanup receipt; Stop is not claimed complete |
| Completed/failed/cancelled | Character rests or leaves; bounded final marker remains | Exact final outcome and time, then inspect in robot view |
| Stale or disconnected event source | Freeze or remove animation and dim the station | Last observed at time / Unknown; never silently Running |

Station assignment is deterministic from stable @ID; a run may occupy only its
bound tab's station. Several runs in one tab appear as a labelled stack/count,
never several uncorrelated figures. Additional visible-tab overflow goes to
All tabs, and additional observed runs link to the exact robot/observer list.
The scene never invents characters or rearranges live identities on resize.
Visual mapping does not replace text or the source's final receipt. A playful
transition is allowed only on a real state change, under the reduced-motion
rule above.

**0.2.26 target:** deliver HC-01 through HC-04 and HC-06/07 as a polished,
truthful native watch floor: visible tab fleet, actionable attention, scoped
system status and exact drill-down. A still office with tab stations remains
useful even if no typed agent events exist. HC-05 remains `[_]` unless its
event seam passes. The application must never ship character movement backed
by a timer, request count, terminal pixel, fixture or unverified worker claim.

**Later typed-agent stage:** accept an explicit event contract with stable
`@ID`, `run_id`, ordered stage transitions, source time, observer health and
final/cancel receipt. The presentation join also needs the window/session
incarnation and event-source identity so a tab close/reopen or PID reuse cannot
transfer events to a new station. Event payloads and retained history stay
bounded; private task text remains in its owning inspector. This module owns
the required visual join, while the observer's wire/lifecycle protocol must
be accepted by its own implementation owner.
The app worker's own runs use PRD 36/40; arbitrary inside-tab agents need a
separately accepted observer/adapter owner before their stages appear. The
event source remains the sole lifecycle authority. A temporary redraw or
window resize never moves a run's station identity.

**Later network stage:** PRD 37 may add node origin only after explicit pairing
and observation. Remote-looking characters, cross-machine movement and team
coordination have no authority in this canvas today.

## Acceptance scenarios

1. From a fresh zero-tab window, @0 shows identity, still empty office and
   New Terminal/Web; it does not invent an agent or zero-valued resource.
2. With one terminal and a parked Web page, @0 shows two distinct @IDs. Focus
   each exact station and return; PTY/browser state, drafts and proxy choice
   survive. An agent-name-like title alone never creates a character.
3. With many tabs including duplicate labels, reordered children and one
   exited terminal, the scene and All tabs list retain exact @IDs. Selecting
   an alert after its tab closes reports missing tab rather than opening a
   different row. At 256 tabs, bounded overflow stays navigable.
4. With an applied profile and running listener but no typed agent runs, the
   system card says ready, while every tab's agent state is Unknown. A model
   request changes only its labelled gateway signal unless an exact tab/run
   correlation is supplied.
5. Combine one exited tab, one stale tab observer, a failed account and an
   unavailable host metric. Attention deduplicates by source, points to each
   exact target and resolves when its owner recovers; Unknown never reads as
   healthy, zero or a global failure of unaffected tabs.
6. On each platform that claims process CPU/RSS or machine capacity, compare
   a timestamped display sample with a separate OS observation under a known
   load. Scope, units and sampling interval agree; an unsupported or failed
   sampler produces Unavailable without taking down the tab fleet.
7. Resize across wide, medium, narrow and short windows at 1x/1.5x/2x in
   English and both Chinese variants. The first alert, @ID route and scoped
   system status stay reachable; the scene yields to a tab list before text
   clips or target geometry overlaps.
8. When a typed source exists later, run one exact-tab agent through start,
   tool, wait, observer loss, cancel/final and restart reconciliation. A
   character appears only at its bound station; a closed/reused tab ID cannot
   inherit that actor or its final state.

Use public `ui-snapshot`/CLI journeys plus native window captures on each
claimed OS/scale. Pixel review checks hierarchy and clipping; event/protocol
receipts establish truth. A pretty screenshot alone proves neither navigation
nor a real worker. Development observations and exact release-artifact courts
retain their separate scopes under PRD 27 and PRD 33.
