# Hello-window results

Status: **awaiting test on the Windows/360 reproduction machine**.

| Field | Value |
|---|---|
| EXE SHA-256 | `d705d8c08b783800211ab3c7fdd4dc07f0376def98d09ba3ca419e52082582d8` |
| EXE bytes | 196,608 |
| 360 product version | pending |
| 360 engine/database | pending |
| Scan time | pending |
| First local scan | pending |
| Second local scan | pending |
| Visible GUI launch | pending |

Decision trace: pending. Follow `README.md`; do not infer a MiniCon culprit
until this baseline produces a repeatable verdict.

Build-side structural receipt: PE32+ x86_64, Windows GUI subsystem,
conventional Rust/MSVC entry RVA `0x1b9f4`, no Authenticode Security Directory,
and no `VCRUNTIME*.dll`, `MSVCP*.dll` or `MSVCR*.dll` import. The complete DLL
set is `KERNEL32.dll`, `USER32.dll`, `api-ms-win-core-synch-l1-2-0.dll`, and
`ntdll.dll`; the CRT is statically linked.
