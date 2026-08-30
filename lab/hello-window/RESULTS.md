# Hello-window results

Status: **both the no-resource baseline and resource-only comparison reproduce the report**.

| Field | Value |
|---|---|
| Tested EXE SHA-256 | `d705d8c08b783800211ab3c7fdd4dc07f0376def98d09ba3ca419e52082582d8` |
| EXE bytes | 196,608 |
| 360 product version | pending |
| 360 engine/database | pending |
| Scan time | pending |
| First local scan | `HEUR/QVM202.0.B951.Malware.Gen` (user-reported) |
| Second local scan | pending |
| Visible GUI launch | pending |

The build now passes `/Brepro`; two consecutive builds produced the same bytes.
Current reproducible baseline: 196,608 bytes, SHA-256
`2b06bb99445392677f7bba0e5432a268b8eee69b4fd0f824f9492b5f2d13a6a2`.
Resource-only comparison: 205,312 bytes, SHA-256
`0875acf3e4a8c4d84ac4b3341ca52e4050473feecd6cd38502d69c45a91c5d9d`.
Its local 360 verdict is `HEUR/QVM202.0.B951.Malware.Gen` (user-reported).

Resource verdict: **rejected as a sufficient remedy**. Adding the compact icon,
ProductName, FileDescription, OriginalFilename, InternalName and ordinary PE
Resource Directory did not change the named detection. The remembered clean
result for an older `agenterm-con.exe` is historical context only: without its
exact bytes and same-time scanner control, it cannot establish causality.

Pure-C comparison: 91,648 bytes, SHA-256
`6c2955b858ff28c1128a10101210fed711b25dbe80010f79c553be806f8e20cd`.
Two consecutive `/Brepro` builds were byte-identical. It uses the same Windows
SDK/MSVC libraries and static CRT, but contains no Rust code and no PE
resources. Its only DLL imports are Windows components `USER32.dll` and
`KERNEL32.dll`; local 360 verdict and visible GUI launch are pending.

Pure-C court result: **clean** (user-reported). A previously built debug
MiniCon of roughly 2 MiB was also clean, while the 892,416-byte release-fast
MiniCon at SHA-256
`2f273ee37d615045d9d1fef57cff5977ecf9781f607dfe92558f93109ab8a608`
reproduced `HEUR/QVM202.0.B951.Malware.Gen` (user-reported). ZIP/archive and
`minicon.com` are therefore not necessary triggers.

Current-source profile controls:

| Variant | Bytes | SHA-256 | Verdict |
|---|---:|---|---|
| debug + strip | 2,346,496 | `0412912bce59149aff2f3f37004b625f3799f8419d962e4367d8fd6263863525` | clean (user-reported) |
| debug + no strip | 2,346,496 | `392153b3ce82ef15e03129c4658ea62b23115366a1613f3c1e675dd6a4fd402f` | pending |
| release-fast + strip | 893,952 | `9ea3538c06bebceac8a0b594aa4ca9a1b27da9eb55ec3365db0c7b421c03efa3` | pending |
| release-fast + no strip | 893,952 | `b9b2868f711de0ebbbc787f39df34b46072c45402c8e0e9aaa7037959b3e2ea6` | `HEUR/QVM202.0.B951.Malware.Gen` (user-reported) |
| debug + `opt-level=z` | 1,111,552 | `fad11e90bab429a9fa43324d396e8a57ec3d72517b87f4f5f42c3027b4364098` | pending |
| release-fast + `opt-level=0` | 1,949,184 | `11897db91310c555625ac1c7f88331ce129929e82cb9cbb865c180d4039cf407` | pending |

The equal sizes and clean/flagged cross rule `strip` out: the verdict follows
the debug versus release-fast profile. The next pair changes only whole-graph
`opt-level` (`debug:z` versus `release-fast:0`) to separate size optimization
from the profiles' remaining debug-assertion, overflow-check and codegen-unit
differences.

Decision trace: the conventional tiny GUI reproduced the same named heuristic,
so PTY, IPC, subprocesses, MiniCon's custom entry point and terminal behavior
are not necessary conditions. The next controlled sample adds only ordinary PE
resources. Scanner version/time and a repeated baseline scan remain pending.

Build-side structural receipt: PE32+ x86_64, Windows GUI subsystem,
conventional Rust/MSVC entry RVA `0x1b9f4`, no Authenticode Security Directory,
and no `VCRUNTIME*.dll`, `MSVCP*.dll` or `MSVCR*.dll` import. The complete DLL
set is `KERNEL32.dll`, `USER32.dll`, `api-ms-win-core-synch-l1-2-0.dll`, and
`ntdll.dll`; the CRT is statically linked.
