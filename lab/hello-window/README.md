# Hello-window QVM resource split

Purpose: compare two conventional Rust/Win32 GUI executables with identical
application code. Both open one ordinary window containing `Hello, world!`.
The second differs only by linking the MiniCon icon and complete Windows
product/version metadata through the normal resource compiler.

Excluded from this binary: MiniCon code, custom `/ENTRY`, PTY, child processes,
IPC, control protocol, configuration, filesystem access, network access,
threads, embedded font, image codec and signing.

Like MiniCon, the baseline does **not** depend on the separately installed
Visual C++ Redistributable. Its conventional Rust/MSVC startup and CRT are
linked statically; `build.sh` rejects any `VCRUNTIME*.dll`, `MSVCP*.dll` or
`MSVCR*.dll` import.

Build from repository root:

```sh
./lab/hello-window/build.sh
```

Generated, gitignored files:

```text
lab/hello-window/dist/helloworld-x86-64.exe
lab/hello-window/dist/helloworld-x86-64.exe.sha256
lab/hello-window/dist/helloworld-resourced-x86-64.exe
lab/hello-window/dist/helloworld-resourced-x86-64.exe.sha256
```

The no-resource baseline reproduced `HEUR/QVM202.0.B951.Malware.Gen` on the
same Windows court. Next, copy the resourced EXE to that machine without
changing scanner state. Scan it twice, then launch it and confirm the text.

Interpretation:

| Result | What it establishes | What it does not establish |
|---|---|---|
| resourced clean, baseline flagged | Ordinary PE resources are an actionable classifier input for this court | That an icon alone, rather than complete metadata or reputation, is causal everywhere |
| both flagged | Resources are insufficient; continue with signing/reputation/toolchain splits | That Rust or any particular API is malicious |
| both clean on retest | Scanner state changed; the comparison is invalid | A product fix |

Record exact hashes, results, full detection name, scanner identity and time in
`RESULTS.md`. Do not submit either sample or update definitions between scans.
