# Hello-window results

Status: **awaiting test on the Windows/360 reproduction machine**.

| Field | Value |
|---|---|
| EXE SHA-256 | `f1e9937339af3a5ea648a0f2dfa772f397b883476fc11f8dcf5dda53aabf7ca3` |
| EXE bytes | 100,352 |
| 360 product version | pending |
| 360 engine/database | pending |
| Scan time | pending |
| First local scan | pending |
| Second local scan | pending |
| Visible GUI launch | pending |

Decision trace: pending. Follow `README.md`; do not infer a MiniCon culprit
until this baseline produces a repeatable verdict.

Build-side structural receipt: PE32+ x86_64, Windows GUI subsystem, conventional
Rust/MSVC entry RVA `0x11040`, no Authenticode Security Directory, Load
Configuration present. Imports are limited to `KERNEL32.dll`, `USER32.dll`,
`ntdll.dll`, `VCRUNTIME140.dll`, and operating-system Universal CRT API sets.
This baseline intentionally uses the conventional dynamic VC runtime; a target
without that redistributable may fail to load, which is an environment failure
rather than an antivirus result.
