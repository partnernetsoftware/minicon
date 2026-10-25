# Current MiniCon product roadmap

Current baseline and next sequence are indexed by [PRD](../PRD.md).
This is an index of owning leaves; the selected version plan sequences delivery.
Each owner carries its invariant, evidence, safe failure, dependencies and non-goals.
Historical ordering, rejected priorities and earlier evidence remain in the
[2026-10-24 snapshot](archive/plan-v0.2.x-product-roadmap-2026-10-24.md).

## Outcome tree DAG

```text
Outcome: improve daily use without losing open product obligations
├── [_] Core product track: visual Harness operation / monitoring → PRD 36
│   ├── H0 robot entry + real overview + Settings deep links → 0.2.23 reservation
│   ├── H1 account/client/request/usage observation → pool and monitoring owners
│   └── H2 built-in task operation, then H3 future engine decisions → own staged plan
├── [_] Important future track: cluster / cross-machine coordination → PRD 37
│   ├── C0 identity, trust and observation contract → next design milestone
│   ├── C1 explicit pairing and read-only two-node dashboard → subsequent selected version
│   └── C2 task handoff/cancel, then C3 model/capacity coordination → gated stages
├── [_] Future selected-window computer use → PRD 38 / plan-minicon-cu.md
│   └── observation, single action, Harness and paired-node gates → owning CU plan
├── [v] Public 0.2.22 → release history; exact artifact evidence only
├── [-] 0.2.23 browser controls and first harness connection → current version plan
│   ├── browser control consistency → PRD 25.02
│   └── Harness settings / manual pool / local model API → PRD 31.01
├── [_] 0.2.24 tab-tree drag and reliable inline rename → next version plan
│   └── reported inside-click repair first; atomic subtree move and native gestures → PRD 25.03
├── [-] Modern-agent / Windows provider / terminal-library qualification → PRD 24.01/34/35
├── [-] Test inventory, selection, composition and measured feedback → PRD 33
├── [-] RSS, artifact size and exact-source delivery costs → PRD 27
└── [_] Installation → PRD 27.01; deferred to coordinated AgenTerm planning
```

```mermaid
flowchart LR
    B[Public 0.2.22] --> P[0.2.23 controls and first connection plan]
    P --> A[PRD 31.01 auth feasibility and manual account binding]
    V[Core Harness workspace / PRD 36] --> P
    A --> V
    V --> L[Later bounded built-in task operation]
    CL[PRD 37 / C0 future cluster design] -. origin-scoped views .-> V
    CL --> CP[Subsequent C1 paired observation]
    CU[PRD38 / CU plan: observation then action] -. explicit later tool selection .-> L
    CP -. later remote CU authority .-> CU
    L --> CT[Later C2 deliberate task handoff]
    CP --> CT
    A --> Q
    F[Future llm-gate / ujs + tinyvm] -. separate scheduling horizon .-> A
    P --> Q[Scoped evidence and separate release decision]
    P --> NEXT[0.2.24 tree interaction plan]
    NEXT --> TREE[PRD 25.03 rename repair / atomic drag]
    TREE --> Q
    T[Modern-agent and provider tracks] --> Q
    O[Test orchestration and delivery owners] --> Q
    H[Archived roadmap and release histories] -. decisions and experience .-> P
    I[Deferred installation] -. separate coordinated planning .-> B
```

Next work is sequenced in [0.2.23](plan-v0.2.23.md), not inferred from an old
version number. Predictable editing/menu/focus behavior is owned in
[PRD 25.02](../prd/PRD_02_25_02_browser_controls.md); the first shared inference
path, auth feasibility, manual pool and later selection seam are owned in
[PRD 31.01](../prd/PRD_02_31_01_provider_gateway.md). First connection does not
implement the later dynamic scheduler.
The newly selected [0.2.24 plan](plan-v0.2.24.md) owns the next tab-tree drag
and inline-editing delivery, with behavior/evidence in
[PRD 25.03](../prd/PRD_02_25_03_tab_tree_interactions.md). Current delivery
remains 0.2.23; 0.2.24 keeps its planned scope. Qualification is source-specific.
The owner's 2026-10-25 direction makes the
[visual Harness workspace](../prd/PRD_02_36_harness_workspace.md) a continuing
core product track; [its plan](plan-harness-workspace.md) stages the robot
reservation, real monitoring and later task operation. Console/Web improvements,
input correctness and bug fixes remain maintained foundation work throughout.
Independent tracks retain their existing requirements and gates in
[TUI compatibility](../prd/PRD_02_24_01_tui_compatibility.md),
[Windows backend](../prd/PRD_02_34_windows_console_backend.md),
[terminal library](../prd/PRD_02_35_terminal_library.md),
[test orchestration](../prd/PRD_02_33_test_orchestration.md), and
[delivery](../prd/PRD_02_27_con_delivery.md).
Missing native evidence is BLOCKED; no neighboring release closes these gates.
Installation remains [deferred](../prd/PRD_02_27_01_install.md).

## 未来构想

The owner requests early staged design of MiniCon cluster/cross-machine
coordination. [PRD 37](../prd/PRD_02_37_cluster_horizon.md#未来构想) owns behavior;
[the cluster plan](plan-cluster-horizon.md) starts C0 with the local Harness
foundation, then selects subsequent implementation versions from actual
prerequisites. This is a first-class future track, not a current cluster claim.

The owner's computer-use planning request is recorded in
[PRD 38](../prd/PRD_02_38_computer_use.md). [The CU plan](plan-minicon-cu.md)
is the sole owner of its candidate release placement and dependency gates;
this roadmap does not select those future versions or expand today's worker.
