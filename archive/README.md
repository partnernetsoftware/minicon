# Archive

Finished experiments and retired tooling, kept for their conclusions. Nothing
here is built, run or referenced by a workflow, test or script; anything that is
belongs outside `archive/`. Current truth starts at `PRD.md`.

Plans and release histories have their own archives: `plan/archive/` and
`prd/archive/`. Never reuse an archived run as evidence for newer source bytes.

## `research/` — host memory track (paused 2026-09-06)

The evidence behind the effort to bring idle one-tab host RSS toward 10 MiB.
The target is still open (see `plan/plan-carried-debt.md` C4, claimed but
`BLOCKED` by `plan/archive/plan-v0.2.1.md`'s `UI` leaf pending a display-capable
host); these are the
measurements and probes it was built on. Each has a written conclusion:

| directory | question | conclusion |
| --- | --- | --- |
| `hello-memory/` | what a minimal window costs per platform | `plan/archive/research-hello-memory.md` |
| `minicon-memory/` | where MiniCon's own resident memory goes | `plan/archive/research-minicon-memory.md` |
| `pixel-platform/` | the pixel host's share of RSS | `plan/archive/research-pixel-host.md` |
| `frame-lifetime/` | frame storage and AppKit initialisation costs | `plan/archive/research-frame-lifetime.md` |
| `rss-ledger/` | a per-component resident ledger | `plan/archive/research-rss-ledger.md` |
| `windows-memory/` | the Windows RSS branch | `plan/archive/research-windows-memory.md` |
| `screenshot-memory/` | what a screenshot costs resident | receipts cited in `prd/PRD_02_27_con_delivery.md` |
| `osx-x86-64-court/` | whether Intel macOS can be qualified under emulation | `plan/archive/design-osx-x86-64-court-experiment.md`: routine Intel qualification uses real Intel runners; Rosetta is userspace evidence only |

The forward plan for that track is `plan/archive/plan-runtime-memory-next.md`.

## `research/minicon-com-loader-retired/`

Files from the live `research/minicon-com-loader/` that nothing uses any more:

- `signpath-application.md` — the SignPath Foundation application. SignPath
  declined in September 2026; Windows signing is Azure Artifact Signing. Not a
  route to retry.
- `v0.1.3-candidate-plan.md` — the v0.1.3 Candidate plan, long shipped.
- `six-cell-smoke.sh`, `utm-win-defender-diagnostic.sh`, `win-status.ps1` —
  superseded probes; `scripts/six-cell-qualify.sh` and the Defender court
  replaced them.

## `lab/`

- `hello-window/` — three conventional Win32 GUI executables that decided the
  360 QVM false-positive question: the compact Windows build shape, not missing
  resources alone, triggered the heuristic. Decision record:
  `plan/archive/design-qvm-false-positive-experiment.md`.
- `tinygui/` — MiniCon's own window stack with nothing drawn, to measure the
  host floor for RSS and CPU. Results in `tinygui/RESULTS.md`.

## `ui-design/`

The raw mockups behind the current host-UI chrome — a four-turn visual study
(structure, themes, a button/geometry audit, and the toolbar reduction) plus
the parked NewAgent create/running exploration. HTML mockups with their own
`support.js` runtime; open them in a browser, nothing here is built. The
durable conclusions are in `prd/PRD_02_25_con_workspace.md` "Design study",
and the NewAgent exploration is owned by `prd/PRD_02_31_v0_2_horizon.md`.

## `reference/`

Background knowledge that was never product truth — the directory's own README
labelled it "非产品承诺、非 PRD", and nothing linked to it. Kept for the concepts,
not as requirements:

- `virtualization-emulation-sandbox.md` — emulation / simulation /
  virtualization / sandbox boundaries, container-and-image internals
  (Docker/Podman/OCI/runc, image vs container, layered filesystems), a
  tool-placement table (VMware / QEMU / VirtualBox / UTM), VirtualBox↔UTM and
  the 0x7B pitfall, and a "how to place a new tool" four-question method.
  Gathered while reproducing the console-agent CJK rendering bug in an emulated
  x86 Windows guest under UTM on Apple Silicon.

- `research/host-ui-frames/v0.2.22.png` — preserved Q1 host-UI diagnostic frame
  from the public 0.2.22 source. Current generation/output ownership is in
  [PRD 25 Host UI](../prd/PRD_02_25_con_workspace.md#host-ui).

- [Public 0.2.22 browser captures](research/browser-v0.2.22/README.md) — original
  page/proxy and failure pixels; release history owns provenance and scope.
