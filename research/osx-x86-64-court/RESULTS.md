# macOS x86-64 court results

Status: **not run**

## Fixed inputs

| Input | Identity | SHA-256 | Provenance |
|---|---|---|---|
| Preinstalled image | no qualified candidate found | not measured | builder and Apple-media manifest required |
| Apple Intel installer | not selected | not measured | Apple origin required |
| OpenCore | not selected | not measured | pinned upstream release required |

## Criteria

| Criterion | Result | Evidence |
|---|---|---|
| C1 provenance | not run | `./research/osx-x86-64-court/preflight.sh` |
| C2 installer reachability | not authorized | C1 installer branch must pass; skipped by a qualified preinstalled image |
| C3 deterministic disk boot | not authorized | C2 must pass |
| C4 true x86-64 kernel | not authorized | C3 must pass |
| C5 test control plane | not authorized | C4 must pass |
| C6 MiniCon evidence | not authorized | C5 must pass |
| C7 recipe exceptions | empty so far | update during execution |

## Decision trace

No decision yet. `planned` remains the truthful registry state.

## Reproduction

```bash
# From repository root
./research/osx-x86-64-court/preflight.sh
```

No criterion will be changed after observing a result merely to improve the
outcome.
