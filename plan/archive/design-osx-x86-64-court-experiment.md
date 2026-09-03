# macOS x86-64 court decisive experiment

| Field | Value |
|---|---|
| Date | 2026-08-28 |
| Status | **Completed decision record — local Intel-macOS VM branch rejected for routine qualification** |
| Purpose | Decide whether Apple Silicon UTM can own the true Intel macOS court, or whether that cell must move to a real Intel Mac runner |
| Implementation | `research/osx-x86-64-court/` |
| Read first | `PRD.md`, `prd/PRD_02_27_con_delivery.md` |
| Source discipline | Apple-origin installer only; OpenCore/QEMU components must retain upstream provenance and hashes |

This document is a completed decision record for the bounded Intel-kernel
experiment. It does not mark a local `minicon-osx-x86-64` VM ready. Routine
x86-64 userspace qualification is owned by host Rosetta under the delivery PRD;
true Intel-kernel evidence moves to a real Intel Mac runner when required.

## §0 — settled context

1. The required cell is a real x86-64 macOS kernel and guest, named
   `minicon-osx-x86-64`.
2. Rosetta on the host or inside an ARM64 guest is useful supplemental evidence,
   but cannot close this cell.
3. UTM Apple Virtualization restores ARM64 macOS only on this Apple Silicon host.
4. The two candidates are a local QEMU/TCG + OpenCore court and a remote real
   Intel Mac runner. More architectural argument will not establish whether the
   local boot chain is repeatable; execution evidence can.
5. The court is a test target, not a build machine. It must stop when idle and
   must not become a permanently running memory consumer.

## §1 — hard constraints

An observation is invalid unless all of these hold:

- Installer provenance is Apple-origin and its product/version/hash are recorded.
- The guest reports an x86-64 kernel; translated user-space is insufficient.
- The experiment uses a separately named scratch VM until qualification. It must
  not overwrite another canonical six-cell VM.
- Every successful boot has a timestamped console or screenshot receipt; a black
  window or an OpenCore picker alone is not a boot.
- Success includes two cold boots from the installed disk with installer media
  detached, automatic test-user login, artifact transport, and visible MiniCon
  GUI launch.
- Secrets and host-expanded home paths never enter results or repository files.

Any impulse to patch arbitrary identifiers, add unreviewed kernel extensions, or
keep expanding the boot recipe merely to pass the next screen is the pathology
this experiment detects. Record the required exception; do not silently add it.

## §2 — minimum experiment

| Dimension | Fixed choice | Why |
|---|---|---|
| Host | Current Apple Silicon Mac mini | Tests the disputed local ownership claim |
| Hypervisor | UTM QEMU/TCG, x86-64 | Hardware virtualization cannot execute Intel macOS here |
| Firmware | One pinned OpenCore release and config | Prevents an unbounded bootloader search |
| Supply | One reviewed preinstalled image; otherwise one supported Apple-origin Intel installer | Tests the cheapest reproducible route first without weakening provenance |
| Guest resources | 2 vCPU, 4–6 GiB RAM, sparse 64 GiB disk | Enough for installation without making it a resident service |
| Checkpoints | firmware picker, installer UI, installed-disk boot, second cold boot, agent transport, MiniCon GUI | Each checkpoint answers a distinct feasibility boundary |
| Comparison | Real Intel Mac runner availability and cold-start automation | Ensures the fallback is evaluated by the same delivery need |

The implementation first evaluates one preinstalled-image candidate if it has a
builder manifest, original-media provenance, immutable hash, disclosed default
credentials, and a documented QEMU machine contract. A qualifying image proceeds
directly to C3 after import and credential rotation. If none exists, the experiment
stops after one pinned OpenCore release, one pinned installer, and the checkpoint
sequence. Version hunting and performance tuning are excluded.

## §3 — preregistered criteria

| ID | Criterion | Nature | Pass condition |
|---|---|---|---|
| C1 | Provenance gate | Boolean | A preinstalled image has builder/media provenance plus an immutable hash, or Apple installer and OpenCore identities/hashes are recorded |
| C2 | Installer reachability | Boolean | Installer UI is visible and accepts input in the scratch VM |
| C3 | Deterministic disk boot | Boolean | Two consecutive cold boots reach automatic desktop login with installer detached |
| C4 | True ISA | Boolean | Guest evidence identifies an x86-64 kernel and Intel CPU presentation |
| C5 | Test control plane | Boolean | Exact artifact push, command execution, screenshot capture, and result pull succeed |
| C6 | Product evidence | Boolean | Exact x86-64 MiniCon artifact launches visibly and returns CLI/UI evidence |
| C7 | Recipe exceptions | Safety/list | No arbitrary serial spoofing, unreviewed kext, or manual-only boot step is required |

Every receipt records the command, component hashes, UTM/QEMU versions, elapsed
wall time, execution state, and artifact hash. Performance is diagnostic only;
this experiment decides repeatability and ownership, not speed.

## §4 — decision tree, kill criteria, and time-box

1. Run C1 preflight. Prefer one reviewed preinstalled image. If it passes, import
   it into a scratch VM, rotate all credentials, and continue at C3. If no image
   qualifies, evaluate the single pinned installer/OpenCore pair. If neither
   supply branch passes C1, do not create the VM; select the Intel runner.
2. On the installer branch, attempt C2 with the single pinned firmware/media pair. If the installer UI is
   not reached, or input is not usable, kill the local branch and select the
   Intel runner.
3. If C2 passes, install once and test C3. Any manual boot-picker intervention,
   attached installer dependency, or failure of either cold boot kills the local
   branch.
4. If C3 passes, C4 is mandatory. A non-x86-64 kernel kills the local branch.
5. If C4 passes, run C5 then C6. Failure of either makes the local court
   `runner-unavailable`; it cannot be labelled ready.
6. C1–C6 pass and C7 is empty: promote the scratch recipe to the canonical local
   `minicon-osx-x86-64` court. Otherwise assign the cell to a real Intel runner.

**Kill criteria:** one pinned pair exhausted; inability to reach installer UI;
non-deterministic disk boot; non-x86 kernel; or any required item in C7.

**Time-box:** for a preinstalled candidate, ends when C3 produces its first valid
pass/fail receipt; for the installer branch, ends when C2 produces its first valid
pass/fail receipt. Only that pass authorizes later work through C6. Before the
owning gate, do not tune performance, try a second image/macOS/OpenCore version,
or configure the product agent.

Criteria reconciliation: C1–C7 all appear in the tree. The Boolean feasibility
gates precede the safety list because any failed gate already rejects the local
owner. Every pass/fail combination exits to either local promotion or Intel
runner; there is no “keep trying” state.

## §5 — result layout

```text
research/osx-x86-64-court/
├── README.md
├── preflight.sh
└── RESULTS.md
```

Generated screenshots, installers, disks, VM bundles, and receipts containing
machine-local paths stay outside the repository. `RESULTS.md` contains only
redacted hashes, commands from repository root, checkpoint outcomes, and the
decision trace.

## §6 — excluded alternatives

| Alternative | Exclusion reason |
|---|---|
| Rosetta as the x86 court | Translates a process; does not provide an Intel macOS kernel |
| ARM64 macOS VM + translated MiniCon | Useful supplement, but tests the wrong guest ISA |
| Unreviewed downloadable Hackintosh appliance | Provenance, credentials, patch set, and reproducibility are unknown |
| Endless OpenCore/version combinations | Converts a bounded test-infrastructure decision into maintenance research |
| Nested virtualization | Apple Silicon cannot expose Intel hardware virtualization to this guest |

## §7 — not answered

- Intel macOS performance under TCG.
- General Hackintosh compatibility or redistribution.
- Whether every future macOS release remains bootable.
- Product performance qualification; real Intel hardware remains authoritative.

## §8 — completed decision

Status: **completed — the local UTM/OpenCore branch is not a routine release
court; VM creation and further version hunting remain unauthorized**.

The first preflight passed the host-capability and explicit-unqualified-state
checks, but found no local Apple Intel installer, pinned OpenCore image, or
provenance-qualified preinstalled image. It exited 2 through §4 step 1, so C2
was not attempted. One bounded candidate pair is now identified for a future
C1 retry: `EFI-MODERN` 0.6.6 plus Apple Catalina recovery fetched through
OpenCorePkg 1.0.7 `macrecovery.py`. Candidate identification is not media
identity and did not authorize a local court. Subsequent delivery work
established that MiniCon's routine need is x86-64 userspace behavior, which
Rosetta can prove without pretending to provide an Intel kernel. The stopped
Catalina/OpenCore work remains recoverable research evidence, not a release
prerequisite or a planned resident VM.

Reopen this experiment only when at least one of these conditions becomes true:

1. a defect requires an Intel kernel, CPUID or untranslated timing, a kernel
   extension/driver, an old installer, or an Intel-only macOS release;
2. UTM or its image ecosystem provides a provenance-qualified, reusable and
   automatable x86-64 macOS baseline with materially lower acquisition and
   execution cost than the bounded OpenCore/TCG attempt; or
3. a real Intel Mac runner becomes available and needs qualification against
   C3–C7.

Any reopened run starts as a new bounded execution, preserves the preregistered
criteria, records a new decision trace, and must not relabel Rosetta evidence as
an Intel-kernel result. Until then, `runner-unavailable` is the safe result for
Intel-kernel-only leaves and routine userspace qualification continues through
Rosetta.
