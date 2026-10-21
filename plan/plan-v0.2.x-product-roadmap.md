# MiniCon 0.2.x product roadmap

Status: product proposal, updated 2026-10-21. v0.2.15 is published; v0.2.16 has
partial fixture-lifetime implementation but no version bump. The later version
assignments below are proposed release gates, not dates or shipped claims.
If a gate is missed, carry its named gap forward or split a patch release;
do not turn a version number into evidence.

MiniCon's product promise remains one small local terminal executable with
independent tabs, a dedicated composer and bounded local control. The next
0.2.x outcome is a terminal that users can trust with a modern interactive
CLI, while another product can reuse the terminal mechanism without taking
MiniCon's GUI or policy. The reported 0.2.14 Claude Code screen corruption is
the highest-priority user symptom. Current-source comparisons with Claude Code
2.1.304, Grok Build 1.0.62 and Codex CLI 0.162.1 reproduce Unicode, spacing,
redraw and resize failures in the default Windows path. The input low-surrogate
loss is fixed; output loss and live TUI defects remain open in
[the compatibility owner](../prd/PRD_02_24_01_tui_compatibility.md#report-and-current-evidence).
The explicit Rust backend preserves these scoped journeys. It is a promising
user-selectable route, not yet a qualified default or packaged preview.

Behavior owners: [modern TUI compatibility](../prd/PRD_02_24_01_tui_compatibility.md),
[Windows backend](../prd/PRD_02_34_windows_console_backend.md),
[terminal library](../prd/PRD_02_35_terminal_library.md),
[workspace settings](../prd/PRD_02_25_con_workspace.md),
[reuse map](../prd/PRD_02_28_shared_core.md),
[test orchestration](../prd/PRD_02_33_test_orchestration.md), and
[delivery](../prd/PRD_02_27_con_delivery.md). This plan owns sequencing and
release decisions; each owner retains its behavior and evidence record.

## Users and priority

| User/job | Current friction | 0.2.x product priority |
| --- | --- | --- |
| Interactive CLI and harness user | A dense redraw can make the screen hard to read or trust. | P0: reproduce and correct the visible defect, then keep input, resize and exit usable through the same journey. |
| Ordinary shell user | Native console compatibility and orphaned helpers can turn one failed tab into a product reliability problem. | P0: keep the stable default, qualify an owned Windows preview with real programs and bounded teardown. |
| Developer/integrator | Terminal pieces exist in several crates and a full test run costs too much during a narrow edit. | P1: make focused feedback reliable, then prove a small independent consumer of the terminal library. |

The product order follows the user's visible failure first, then provider
independence and reusable contracts. A new setting earns a place only after
its behavior can be explained and tested: `TERM`, protocol capabilities and
native backend selection are separate controls.
Triage the TUI report immediately, in parallel with 0.2.16 tooling. A confirmed
P0 fix may ship as an earlier patch with the current full release gates; the
test-runner redesign must not delay a user-visible repair.

## Product outcome tree

```text
Outcome: a dependable, portable MiniCon terminal with a reusable engine
├── [v] 0.2.15 — explicit crate/reuse boundaries and published baseline
│   evidence: 0.2.15 release history; non-goal: claiming modern TUI acceptance
├── [-] 0.2.16 — fast, trustworthy development feedback
│   invariant: targeted suites save time while full release obligations remain visible
│   evidence: planned/observed test identities, mutation catches, cost comparison
│   safe failure: unknown dependency selects full; missing court is BLOCKED
│   dependency: PRD 33 and existing 0.2.16 plan; non-goal: feature expansion
├── [_] 0.2.17 — readable modern agent sessions on a named selectable provider
│   invariant: valid text, settled layout and presented pixels agree during repeated use
│   evidence: three versioned agents, independent fixtures and source/wire/state/pixel traces
│   safe failure: narrow the release claim when the classic path or preview fails a cell
│   dependency: PRD 24.01 and plan-v0.2.17; full gate remains required without 0.2.16 profiles
│   non-goal: automatic provider-default change or declaring every TUI fixed
├── [_] 0.2.18 — broaden the owned Windows backend preview
│   invariant: selected sessions use the actual Rust provider and clean up their own tree
│   evidence: wider real shell/API/ISA journeys, wire/frame/lifetime comparison and cost
│   safe failure: reject unavailable selection; preserve stable product default
│   dependency: PRD 34 and the 0.2.17 scoped app court; non-goal: global conhost removal
├── [_] 0.2.19 — reusable terminal contract across OS and ISA
│   invariant: one common state/stream contract with explicit native capabilities
│   evidence: independent consumer and MiniCon on supported native runtime cells
│   safe failure: typed unsupported capabilities; no false cross-ISA claim
│   dependency: PRD 35 and reuse map; non-goal: automatic platform-crate merger
│   └── qualified identity profiles and new-session TERM choice
└── [_] 0.2.20 — evidence-based Windows default and product-cost decision
    invariant: any default change improves user journeys without hidden regression
    evidence: exact-artifact real apps, reliability, latency/RSS and rollback courts
    safe failure: retain explicit preview if a gate is missing
    dependency: all preceding gates and PRD 27; non-goal: a date-driven switch
```

```mermaid
flowchart LR
    PAIN["Claude / Grok / Codex default-path corruption"] --> TRACE["versioned reproduction and byte / state / pixel trace"]
    TEST["0.2.16 targeted feedback and full qualification"] --> FIX
    TRACE --> FIX["0.2.17 diagnosed default fixes + qualified explicit choice"]
    FIX --> APP["three-agent acceptance on named provider"]
    APP --> PREVIEW["0.2.18 broader Rust Windows preview"]
    WIN["PRD 34 native API and lifetime courts"] --> PREVIEW
    PREVIEW --> CONTRACT["0.2.19 independent terminal consumer"]
    POSIX["macOS / Linux native PTY inventory"] --> CONTRACT
    CONTRACT --> PROFILE["qualified capability / TERM profiles"]
    APP & PREVIEW & CONTRACT & PROFILE --> DECIDE["0.2.20 default and cost decision"]
    DECIDE -->|"gate met"| RELEASE["exact signed artifact promotion / PRD 27"]
    DECIDE -->|"gate missing"| KEEP["stable default + explicit preview; named open gap"]
```

## Version decisions and acceptance

| Version | User-facing result | Release acceptance and decision |
| --- | --- | --- |
| 0.2.16 — developer feedback | Changes to one module get relevant tests promptly; integration and release still show every required court. | Implement the existing [0.2.16 plan](plan-v0.2.16.md): inventory, bounded fixture lifetime, explained selection, deduplicated composition and measured cold/warm cost. A mutation of an impacted dependency must fail its selected profile. `l3`/`all` still reaches the canonical full local gate; signing and artifact courts remain separate. Ship this before adding a larger compatibility matrix. |
| 0.2.17 — modern agent quality | A user can select a named provider that keeps Claude Code, Grok Build and Codex CLI readable and usable through repeated output and resize. | Follow [the dedicated 0.2.17 plan](plan-v0.2.17.md): locate each failure across original output, backend wire, shared state and pixels; fix representable classic-path defects, qualify Unicode/SGR color fixtures and repeated real-agent sessions, and include an explicit Rust selection in the exact Windows candidate only if its native/lifecycle courts pass. State failed default-path behavior and provider-specific coverage. A developer-only feature is not a user-facing result; no default promotion or positive 2026 claim without its separate gate. |
| 0.2.18 — broader Windows backend preview | The explicit Rust choice supports a wider set of ordinary Windows programs, with clear errors for unsupported APIs and bounded teardown. | Build on the 0.2.17 named-provider court. Compare installed ConPTY and Rust with PowerShell/cmd/Node and further real harnesses across startup, input, Ctrl+C/Break, resize, exit tail, child-tree cleanup and close. Report native API input, emitted wire, frame, latency and total host/helper RSS. Windows x64 and ARM64 runtime claims require separate runs; cross-compilation is build evidence only. A missing app or ISA court narrows the preview claim, not the stable product. |
| 0.2.19 — integrated terminal library | MiniCon and an independent small consumer use `minicon-console` for the same common terminal state, while native providers stay explicit. | Define the smallest input/session/flow-control/capability contract from actual Windows and POSIX consumers. Preserve `minicon-platform::pty` compatibility during migration; move one proven mechanism at a time and check public type identity, features and OS/ISA runtime. Provide a truthful modern VT/xterm-style default profile; expose only qualified compatibility profiles and an advanced TERM override for **new sessions**. Changing TERM describes capabilities to applications and does not switch parser or native backend. If the contract is not proved on supported runtime cells, keep the existing platform adapter and narrow the release claim. |
| 0.2.20 — default decision and product cost | Users receive the best evidenced Windows backend choice with a known path to recover from failures. | Decide whether to make the owned backend the Windows default using exact packaged artifacts and versioned real-app journeys, comparison of failure/cleanup, input latency, sustained redraw and total process RSS. Include product GUI pixel/snapshot consistency and rollback/selection evidence. A failed or unavailable criterion keeps the stable default and explicit preview. Track the 10 MiB idle-host intent, build/pack latency and artifact size separately; do not exchange correctness or signed-artifact requirements for a speed claim. |

These version numbers express priority and dependencies. 0.2.17 may ship a
well-bounded repair with a narrower real-agent claim when a named cell fails,
but cannot call the three-agent use case solved without a packaged selectable
provider that passes it. 0.2.18 broadens the explicit preview rather than
changing the Windows default by implication.
0.2.19 may ship the common contract before every adapter is consolidated.
0.2.20 is a decision point, not a promise that the default will switch.

## Cross-version product rules

1. Treat terminal identity, native backend and UI rendering as separate choices.
   The current code starts child sessions with `TERM=xterm-256color` and
   `COLORTERM=truecolor`; the `vt100` crate name is an implementation detail.
   Until the capability contract is qualified, retain this default and do not
   offer a misleading menu of VT100/VT220/kitty emulation claims.
2. Diagnose a visible defect at four observable boundaries: original child
   bytes/native API calls, PTY wire, parsed/presented terminal state and actual
   GUI pixels. One passing SDK test does not close a user-visible TUI report.
3. Keep reuse inside the terminal domain: `minicon-console` owns common
   stream/screen mechanisms and its Windows service; `minicon-platform` owns
   capability adaptation; MiniCon owns tabs, settings, backend choice and UX.
   AgenTerm adopts an immutable revision only after its own consumer evidence.
4. Run focused developer profiles while editing, integration profiles before a
   candidate and the complete exact-source/artifact release chain before
   publication. Every released OS/ISA/provider claim names the runtime cell;
   unavailable evidence stays BLOCKED.
5. Keep MiniCon's authority local. Agent management, Fleet, persistent service,
   plugin marketplace and MiniCon-driven update orchestration belong outside
   this roadmap. The Win+R administrator checkbox belongs to WinMgr, not a new
   MiniCon launcher.

## Review cadence and evidence

At each version candidate, review one product scorecard: reproducible TUI
defects, session failures and orphaned processes, input-to-frame latency under
declared load, total host/helper RSS, package size, cold/warm test feedback
cost, and matrix coverage by OS × ISA × backend × application. Record baseline,
target and observed value in the owning PRD or release receipt; do not invent a
cross-machine threshold from one developer host. The release decision includes
the named unresolved cells and an explicit keep/change backend choice.
