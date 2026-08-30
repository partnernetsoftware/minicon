# Hello-window QVM resource split

Purpose: compare three conventional Win32 GUI executables. The first two use
identical Rust application code, with the second adding only normal Windows
resources. The third is an ordinary pure-C `/MT` implementation of the same
small window, built against the same cargo-xwin MSVC headers/libraries and
linker. It removes Rust from the experiment without adding a runtime DLL.

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
lab/hello-window/dist/helloworld-pure-c-x86-64.exe
lab/hello-window/dist/helloworld-pure-c-x86-64.exe.sha256
```

Both Rust variants reproduced `HEUR/QVM202.0.B951.Malware.Gen` on the same
Windows court, ruling resources insufficient. Next, copy the pure-C EXE to
that machine without changing scanner state. Scan it twice, then launch it and
confirm the text.

Interpretation:

| Result | What it establishes | What it does not establish |
|---|---|---|
| resourced clean, baseline flagged | Ordinary PE resources are an actionable classifier input for this court | That an icon alone, rather than complete metadata or reputation, is causal everywhere |
| both flagged | Resources are insufficient; continue with signing/reputation/toolchain splits | That Rust or any particular API is malicious |
| both clean on retest | Scanner state changed; the comparison is invalid | A product fix |
| pure C clean, Rust controls flagged | Rust-linked PE shape is a necessary input on this court | The exact Rust section/symbol responsible |
| pure C also flagged | Rust is not necessary; unsigned-new-file reputation or the shared MSVC/PE shape becomes the leading branch | That signing is guaranteed to fix every engine |

Record exact hashes, results, full detection name, scanner identity and time in
`RESULTS.md`. Do not submit either sample or update definitions between scans.
