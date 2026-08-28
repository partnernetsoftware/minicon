# macOS x86-64 court results

Status: **C1 executed and failed; no VM creation authorized**

## Fixed inputs

| Input | Identity | SHA-256 | Provenance |
|---|---|---|---|
| Preinstalled image | no qualified candidate found | not measured | UTM Gallery has no macOS appliance; reviewed third-party images lack the required builder/media manifest |
| Apple Intel installer | Catalina 10.15.8 Recovery candidate, board `Mac-00BE6ED71E35EB86` | not downloaded; not measured | Apple download through OpenCorePkg 1.0.7 `macrecovery.py` |
| OpenCore | `EFI-MODERN` 0.6.6 candidate, introduction commit `719063bf227dc6027301a67b012c45058ec98894` | upstream ZIP `db8d827edc0888c8d3c8dee05801173f7b7a3be72233dea87759d0a4f47548d8`; upstream image `173404f90bb26eb6be27b53d7fbe81b437da4cae398bc219b3c5b957ce4cc2dc`; neither locally remeasured | KhronoKernel Apple-Silicon QEMU experiment; `MacPro7,1`, Lilu 1.5.0, VirtualSMC 1.1.9 |

## Criteria

| Criterion | Result | Evidence |
|---|---|---|
| C1 provenance | **FAIL** | preflight exit 2: host capabilities pass, but neither fixed media file is present |
| C2 installer reachability | not authorized | C1 installer branch must pass; skipped by a qualified preinstalled image |
| C3 deterministic disk boot | not authorized | C2 must pass |
| C4 true x86-64 kernel | not authorized | C3 must pass |
| C5 test control plane | not authorized | C4 must pass |
| C6 MiniCon evidence | not authorized | C5 must pass |
| C7 recipe exceptions | empty so far | update during execution |

## Decision trace

The first real preflight followed §4 step 1 and exited 2. UTM, its bundled
x86-64 QEMU engine, the image-source registry, and the explicit
`no-qualified-image` selection all passed. No local Apple Intel installer,
OpenCore image, provenance-qualified preinstalled image, or
`minicon-osx-x86-64` VM was found. Therefore C1 failed and the decision tree
forbids VM creation; C2 was not attempted. `planned` remains the truthful
registry state.

The bounded next candidate is one pair, not an open-ended version search:

- [`EFI-MODERN` 0.6.6](https://github.com/khronokernel/khronokernel.github.io/blob/master/Binaries/OpenCore/README.md), selected because the upstream
  [Apple-Silicon QEMU experiment](https://khronokernel.com/apple/silicon/2021/01/17/QEMU-AS.html)
  actually reached Catalina recovery and installation under UTM/QEMU TCG;
- Apple Catalina recovery fetched with upstream OpenCorePkg 1.0.7
  [`macrecovery.py`](https://github.com/acidanthera/OpenCorePkg/tree/master/Utilities/macrecovery)
  for board `Mac-00BE6ED71E35EB86`.

This identifies inputs eligible for a later C1 attempt. It is not a C1 pass:
the files have not been downloaded, their bytes have not been locally hashed,
and no installer has booted. The original experiment reports near-unusable
performance, so C2 remains a genuine kill gate rather than a presumed pass.

## Reproduction

```bash
# From repository root
./research/osx-x86-64-court/preflight.sh
```

Observed result: exit 2 with `DECISION: C1 not passed; VM creation is not
authorized`.

After the single fixed pair is obtained and its local hashes are recorded, the
only authorized retry is:

```bash
MINICON_OSX_X86_APPLE_MEDIA=~/Downloads/minicon-osx-x86-64/BaseSystem.dmg \
MINICON_OSX_X86_OPENCORE_MEDIA=~/Downloads/minicon-osx-x86-64/EFI-MODERN.img \
./research/osx-x86-64-court/preflight.sh
```

## Deviations and honesty

- The preinstalled-image branch found no candidate meeting C1, so no appliance
  was imported and no credential rotation occurred.
- Hashes listed for the OpenCore candidate are upstream reference values, not
  local measurements; they cannot prove local media identity.
- No criterion, kill condition, or time-box was changed after observing the
  failure. The failed C1 result is retained rather than being softened into
  “research complete”.

No criterion will be changed after observing a result merely to improve the
outcome.
