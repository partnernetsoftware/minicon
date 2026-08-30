# MiniCon product requirements

MiniCon is a terminal that is one file: no installer, no bundled runtime, and
no authority beyond local terminal hosting. This compact root is the product
index and decision map. Details and history live in the owning `prd/PRD_*.md`;
machine alignment is `alignment-contract.json`, and public evidence identities
are in `evidence-registry.json`.

Legend: `[x]` shipped, `[~]` partial, `[ ]` planned, `[-]` explicit non-goal.

## Product outcome

A user can launch one small executable, organize independent local terminals,
type through a dedicated input area, and automate the visible GUI through a
bounded local CLI. Failure in one child, tab, callback, parser or request does
not destroy unrelated sessions or the host.

MiniCon remains useful precisely because it is not AgenTerm: it has no
workbench server, persistent workspace, Fleet, mux, MCP, script runtime,
plugin host, or Agent permission policy. AgenTerm is the Agent-era workbench
on the same platform layer. MiniCon `--control` is a GUI-lifetime local
socket/pipe inside that window, not AgenTerm's server. Details:
`prd/PRD_02_23_minicon.md`, `prd/PRD_02_26_con_control_cli.md`.

## Markdown-tree DAG PRD

The root tree records outcomes, owners and current decisions only. Follow the
linked module for behavior, evidence, delivery mechanics and history.

```text
MiniCon — one-file local terminal
├── Charter and authority
│   ├── local terminal host; one executable; operating-system libraries only
│   ├── [-] no server, persistence, Fleet, mux, MCP, scripts or Agent policy
│   └── prd/PRD_02_23_minicon.md
├── User capabilities
│   ├── Terminal runtime and rendering
│   │   ├── PTY lifecycle, bounded VT/rendering, CJK and native presentation
│   │   └── prd/PRD_02_24_con_terminal.md
│   ├── Workspace and human input
│   │   ├── stable tab tree, composer, IME, selection, clipboard and scrollback
│   │   └── prd/PRD_02_25_con_workspace.md
│   └── Scriptable observation and control
│       ├── GUI-lifetime endpoint, bounded protocol, waits, snapshots and PNG
│       └── prd/PRD_02_26_con_control_cli.md
├── Delivery and portability
│   ├── six cells: {win,lnx,osx} × {x86_64,aarch64}
│   ├── build once; runtime courts execute exact artifacts without compiling
│   ├── independent evidence lanes
│   │   ├── GitHub native runners — fast regression and real ISA
│   │   └── local UTM — controlled-image release and interactive reproduction
│   ├── [x] v0.1.2 stable — 3 archives covering 4/6 cells + SHA-256 sidecars
│   ├── [x] v0.1.3 released — 5 native archives cover all 6 cells
│   │   ├── 5 archives: win/lnx × {x86_64,arm64} + macOS Universal
│   │   ├── unsigned by explicit release-policy.json configuration
│   │   ├── exact Candidate bytes + Defender on both Windows executables
│   │   └── exact Candidate promoted without rebuild after gates + human authority
│   ├── [ ] v0.1.4 — minicon.com + trusted signing
│   │   ├── same workflows; switch policy signing.mode to required
│   │   ├── minicon.com hard ceiling: 9437184 bytes (9 MiB)
│   │   └── SignPath transition or later company publisher
│   └── prd/PRD_02_27_con_delivery.md
├── Reuse boundaries
│   ├── host-neutral shared rules only
│   └── prd/PRD_02_28_shared_core.md
├── Future experiments — not current-version scope
│   ├── qjswasm + TinyVM portable logic; six native OS shells remain
│   │   └── prd/PRD_02_28_qjswasm_horizon.md
│   └── dedicated OS/HarmonyOS feasibility belongs to portfolio horizon
└── Executable truth
    ├── alignment-contract.json — capability → owner → command → evidence
    ├── evidence-registry.json — evidence identity → public test target
    ├── tests/ — black-box journeys
    └── .github/workflows/ — qualification and release automation
```

## Mermaid flowchart memory palace

Read left to right: user value enters a capability room, crosses the product/
platform boundary, is judged by exact-artifact evidence, and only then reaches
a release decision. Dashed paths are optional or future work.

```mermaid
flowchart LR
    U["User value<br/>one file · real terminals<br/>human + script control"]

    subgraph UX["Capability rooms"]
        T["Terminal<br/>PTY · VT · rendering"]
        W["Workspace<br/>tabs · composer · IME"]
        C["Control<br/>CLI · waits · snapshots"]
    end
    subgraph B["Authority boundary"]
        H["MiniCon host<br/>local product policy"]
        P["Platform adapters<br/>window · PTY · input · IPC"]
        K["minicon-core<br/>host-neutral rules"]
        N["Explicitly absent<br/>server · persistence · mux · scripts"]
    end
    subgraph E["Exact-artifact evidence"]
        X["Build owner<br/>six artifacts · source identity"]
        GH["GitHub native six-grid<br/>fast regression · real ISA"]
        VM["Local UTM courts<br/>controlled images · permissions"]
        R["Lane-labelled receipts<br/>same contracts · independent verdicts"]
    end
    subgraph REL["Release evolution"]
        V12["v0.1.2 stable<br/>three packages"]
        C13["v0.1.3 Candidate<br/>5 native archives · 6 cells · unsigned"]
        G13{"runtime + control<br/>checksum + Defender pass?"}
        V13["user: exact version + promote<br/>exact immutable bytes"]
        C14["v0.1.4 policy switch<br/>minicon.com + signing required"]
        KEEP["retain stable release<br/>revise failed gate"]
    end
    subgraph F["Future, dependency-gated"]
        SP["SignPath Foundation transition<br/>trusted Authenticode · timestamp"]
        SI["PARTNERNET SOFTWARE PTY LTD<br/>future company-publisher adapter"]
        QE["agenterm qjswasm + TinyVM ready"]
        QX["portable-logic experiment<br/>one body + six native shells"]
        QG{"total size + startup + memory<br/>six-cell parity pass?"}
    end

    U --> T & W & C
    K & P --> H
    N -. constrains .-> H
    H --> T & W & C
    T & W & C --> X
    X --> GH & VM
    GH & VM --> R
    R --> V12 --> C13 --> G13
    G13 -->|yes| V13 --> C14
    G13 -->|no| KEEP
    SP --> C14
    SI -. later .-> SP
    QE -. later .-> QX
    X -. native baseline .-> QX
    QX --> QG
    QG -->|fail| KEEP
```

## Governing invariants

- Child exit retains its tab, final screen and exit status until explicit close.
- Closing a parent promotes direct children; parent cycles are rejected.
- One tab cannot corrupt, indefinitely block or terminate another tab or host.
- Composer editing and terminal input have exactly one focus owner.
- Structured state, pixels, pointer ownership and screenshots describe the
  same presented frame.
- PTY traffic, parsing, control, waits, queues, dimensions, screenshots,
  allocations and shutdown are bounded and fail locally.
- Native callbacks never unwind across FFI.
- A public claim names its target/profile and exact evidence; one platform's
  measurement never silently becomes universal.

## Capability ownership

| User problem | Owner | Observable success | Safe failure |
|---|---|---|---|
| Run real local shells | [Terminal](prd/PRD_02_24_con_terminal.md) | real-child and sustained-output journeys | reject or close only the affected session |
| Organize and type without output corrupting drafts | [Workspace](prd/PRD_02_25_con_workspace.md) | multitab, composer, IME and interaction journeys | cancel unfinished interaction; never target a stale tab |
| Automate what the GUI really shows | [Control](prd/PRD_02_26_con_control_cli.md) | catalog, wait, snapshot and PNG black boxes | bounded typed error; cancel waits with their owner |
| Ship portable exact artifacts | [Delivery](prd/PRD_02_27_con_delivery.md) | six-cell runtime and release receipts | block the artifact or claim; never weaken it silently |
| Reuse host-neutral rules | [Shared core](prd/PRD_02_28_shared_core.md) | dependency and source-boundary tests | keep code product-local until proven neutral |

## Current frontier

- [x] v0.1.3 is the unsigned native-six-cell release: five archives cover
  Windows/Linux x86_64+arm64 and both macOS slices through one Universal
  archive. `release-policy.json` is the machine-readable switch: signing is
  `off`, `minicon.com` is absent, and Defender must scan the exact two Windows
  executables sealed inside the Candidate. Exact-source build `33286599671`,
  Candidate `33286811086`, and reputation qualification `33287684429` are
  green for source `39774ed`; Promotion run `33289364944` published those exact
  bytes and re-downloaded/ran the public Linux and Windows packages. Tag
  `v0.1.3` points to the Candidate source; the Release is public and immutable.
- [ ] v0.1.4 owns `minicon.com` and trusted signing. The same Candidate,
  reputation and Release workflows consume the policy; changing the version,
  enabling `assets.minicon_com` and setting `signing.mode=required` selects the
  signing court without another workflow rewrite. SignPath review may continue
  independently.
- [x] Unsigned exact-SHA rehearsal `33259779184` at `033e582` independently
  re-proved the new signing input contract: one pack, G3 green, all six native
  GUI/control cells green, APE Authenticode layout ready, SHA matched, and
  8,909,562 bytes under 9 MiB. It is rehearsal evidence, not a Candidate,
  because it belonged to the superseded signed-APE v0.1.3 plan. It remains
  useful v0.1.4 mechanism evidence.
- [~] Formal releases should eventually carry `PARTNERNET SOFTWARE PTY LTD`
  publisher identity. Authenticode-compatible APE header layout is proven;
  Azure/company Public Trust is paused because no short-term company payment
  route exists. The user approved a SignPath Foundation OSS application as a
  truthful transition: its publisher will be SignPath Foundation, never
  misrepresented as the company. The application was submitted and SignPath
  acknowledged receipt on 2026-08-30; provider review, timestamped signed-byte
  six-grid and final-SHA Defender evidence remain before v0.1.4 can claim signing.
- [x] Defender diagnostic run on unsigned rehearsal SHA
  `b2551896e7985c80eea4ea0ac785d532e9efb477aafd475955afbe9f735b0c2a`
  was clean with active engine `1.1.26080.3` and signatures `1.457.375.0`.
  This applies only to that exact diagnostic byte sequence and is retained for
  the v0.1.4 APE court.
- [x] The SignPath metadata prerequisite is no longer speculative: the APE
  outer PE carries a standard VERSIONINFO Resource Directory while preserving
  the empty Authenticode slot and ZIP overlay. A local six-payload 0.1.3 pack
  was 7,517,312 bytes; self-sign/Darwin/ZIP/tamper passed, and disposable
  Windows x86_64 UTM independently reported `ProductName=MiniCon`,
  `ProductVersion=0.1.3` plus ConPTY status. This dirty-tree rehearsal is not a
  Candidate or G6 signature.
- [x] Unsigned one-pack rehearsal `33263135546` at `be2465f` bound clean-source
  G1/G3/G4 evidence and passed G2 GUI/control on all six native runners with
  one `minicon.com` SHA (`ffc5b5aa…`). The 7,517,987-byte APE identifies every
  embedded payload as 0.1.3 and remains under 9 MiB. This is rehearsal evidence,
  not a v0.1.3 Candidate; its payloads may be rebuilt once for the new exact
  native archive Candidate.
- [~] Workspace chrome readability is reopened: tab/header and composer-button
  text remains too small on macOS. Make `z / 0 / Z` affect those roles, enlarge
  their nominal text, and reclaim padding/gaps/margins instead of growing empty
  toolbar space. Owner: `prd/PRD_02_25_con_workspace.md`.
- [~] Run `33263135546` localized the remaining cloud pack latency: macOS cells
  compiled in about 4s, Windows cells in 2.65s/1.27s, but Linux dual-LTO took
  about 2m51s and cargo-xwin re-downloaded the MSVC CRT for 1m56s because the
  workflow cached Linux's `~/.cache/cargo-xwin` path on a macOS builder. Cache
  the actual `~/Library/Caches/cargo-xwin` path, then remeasure before changing
  compilation semantics. The approximately one-minute target remains open.
  Owner: `prd/PRD_02_27_con_delivery.md`.
- [~] Linux LTO already spent the remaining **linker** size knob. Linux
  dual desktop (winit Wayland+X11 + `x11rb`) is **retained** — not a size
  cut. Further APE shrink is still a product ruling (optional AT-SPI,
  Darwin native pixel host, or Darwin/Win LTO), not strip/RELR/
  `panic=abort`/ceiling raise. Owner: `prd/PRD_02_27_con_delivery.md`.
- [~] GitHub-native and local-UTM lanes are independent. Neither inherits the
  other's verdict; unavailable runtime evidence is `BLOCKED`.
- [~] Local courts are automation-capable but not sealed release baselines.
  Lima is optional acceleration; Rosetta is a provisional OSX x86_64 userspace
  court; real native runners retain claims translation cannot make.
- [ ] Ordinary push/PR CI remains parked.
- [ ] qjswasm portable logic waits for a stable agenterm engine and a decisive
  complete-product experiment; it is not part of v0.1.3.

Details and historical run records stay in the owning modules.

## How product work advances

1. Select one owning leaf and state its problem, invariant, evidence, safe
   failure and non-goal.
2. Update the owning module; keep this root an index, not a duplicate.
3. Implement at the narrowest shared/native/product boundary.
4. Add public black-box evidence and register public capabilities/commands.
5. Qualify one exact source state; a failed cell narrows or blocks the claim.
