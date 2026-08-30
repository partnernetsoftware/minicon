# Hello-window QVM baseline

Purpose: produce the smallest useful conventional Rust/Win32 GUI control for
the 360 QVM false-positive tree. It opens one ordinary window containing
`Hello, world!` and does nothing else.

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
```

Copy the EXE to the same Windows machine that flagged public v0.1.3. Before
opening it, update 360 definitions and record its product/engine/database
version. Scan the file twice, then launch it and confirm the visible text.

Interpretation:

| Result | What it establishes | What it does not establish |
|---|---|---|
| clean | A conventional unsigned Rust/Win32 GUI alone does not reproduce the named verdict in that scanner state | Which MiniCon subsystem triggers it |
| same QVM verdict | The trigger is already present in this very small toolchain/PE/reputation baseline | That Rust or any particular API is malicious |
| different verdict | Scanner sees a different feature combination | A causal byte or API without another controlled split |

Record the exact EXE SHA, result, full detection name, scanner identity and
time in `RESULTS.md` after the test. Do not submit or mutate the sample between
the two local scans.
