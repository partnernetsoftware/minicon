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

Decision trace: the conventional tiny GUI reproduced the same named heuristic,
so PTY, IPC, subprocesses, MiniCon's custom entry point and terminal behavior
are not necessary conditions. The next controlled sample adds only ordinary PE
resources. Scanner version/time and a repeated baseline scan remain pending.

Build-side structural receipt: PE32+ x86_64, Windows GUI subsystem,
conventional Rust/MSVC entry RVA `0x1b9f4`, no Authenticode Security Directory,
and no `VCRUNTIME*.dll`, `MSVCP*.dll` or `MSVCR*.dll` import. The complete DLL
set is `KERNEL32.dll`, `USER32.dll`, `api-ms-win-core-synch-l1-2-0.dll`, and
`ntdll.dll`; the CRT is statically linked.
