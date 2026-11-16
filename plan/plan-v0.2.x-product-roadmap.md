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
│   └── H2 optional minicon-harness app operation, then H3 future engine decisions → own staged plan / PRD 40
├── [-] Plugins and Apps layer / independent minicon-harness → PRD 40 / extraction plan
│   ├── X0 released-worker parity and exact two-repo source inventory → 0.2.25 preparation
│   ├── X1 app contract, then X2 future Settings manager and X3 robot event join → later selected version
│   └── X4 one-implementation cutover only after exact native artifact pair qualifies
├── [_] Important future track: cluster / cross-machine coordination → PRD 37
│   ├── C0 identity, trust and observation contract → next design milestone
│   ├── C1 explicit pairing and read-only two-node dashboard → subsequent selected version
│   └── C2 task handoff/cancel, then C3 model/capacity coordination → gated stages
├── [_] Future selected-window computer use → PRD 38 / plan-minicon-cu.md
│   └── observation, single action, Harness and paired-node gates → owning CU plan
├── [v] Earlier public 0.2.x releases → prd/archive/README.md
├── [v] Public 0.2.26 @0 watch floor → archive/plan-v0.2.26.md / PRD 39.01
├── [_] Proposed, parked 0.2.27 addressable agent workspace → plan-v0.2.27.md
│   ├── same-instance Console child CLI and exact machine results → PRD 26.01
│   └── @0 Visual V1 session arrangement with List as default → PRD 39.01
├── [-] Modern-agent / Windows provider / terminal-library qualification → PRD 24.01/34/35
├── [-] Test inventory, selection, composition and measured feedback → PRD 33
├── [-] RSS, artifact size and exact-source delivery costs → PRD 27
└── [_] Installation → PRD 27.01; deferred to coordinated AgenTerm planning
```

```mermaid
flowchart LR
    B[Public 0.2.26 watch floor] --> N[Proposed, parked 0.2.27 local control and Visual V1]
    N --> Q[Scoped evidence and separate release decision]
    A[PRD 31.01 provider gateway] --> V[Core Harness workspace / PRD 36]
    V --> L[Later bounded app-backed task operation]
    APP[PRD 40 optional minicon-harness app] --> L
    APP --> SET[Future Settings / Plugins and Apps manager]
    CL[PRD 37 / C0 future cluster design] -. origin-scoped views .-> V
    CL --> CP[Subsequent C1 paired observation]
    CU[PRD38 / CU plan: observation then action] -. explicit later tool selection .-> L
    CP -. later remote CU authority .-> CU
    L --> CT[Later C2 deliberate task handoff]
    CP --> CT
    F[Future llm-gate / ujs + tinyvm] -. separate scheduling horizon .-> A
    CLI[PRD 26.01 same-instance agent CLI] --> N
    HOME[PRD 39.01 Visual V1] --> N
    T[Modern-agent and provider tracks] --> Q
    O[Test orchestration and delivery owners] --> Q
    H[Archived roadmap and release histories] -. decisions and experience .-> B
    I[Deferred installation] -. separate coordinated planning .-> Q
```

Earlier outcomes and evidence are in the [release histories](../prd/archive/README.md)
and [archived plans](archive/README.md). The [proposed 0.2.27 plan](plan-v0.2.27.md)
is parked during documentation cleanup; it selects no implementation or release.
Current behavior and open acceptance remain in the owning PRDs above.
The owner's 2026-10-25 direction makes the
[visual Harness workspace](../prd/PRD_02_36_harness_workspace.md) a continuing
core product track; [its plan](plan-harness-workspace.md) stages the robot
reservation, real monitoring and later task operation. Console/Web improvements,
input correctness and bug fixes remain maintained foundation work throughout.
The 2026-10-27 [Plugins and Apps decision](../prd/PRD_02_40_plugins_apps.md)
moves future file/exec worker delivery into `partnernetsoftware/minicon-harness`.
[Its extraction plan](plan-minicon-harness-extraction.md) records the parity,
contract, Settings-management and cutover gates; no implementation or release
version is inferred from a new-repo source copy.
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
