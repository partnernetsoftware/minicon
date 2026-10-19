# minicon-console: owned Rust Windows console backend

Parent: [product index](../PRD.md). Existing PTY behavior remains owned by
[terminal](PRD_02_24_con_terminal.md); reusable boundaries by
[shared core](PRD_02_28_shared_core.md). Research reviewed 2026-10-19.

The owner authorizes development of `minicon-console`, an owned Rust replacement
for the Windows console host and ConPTY-facing backend. Independence means control of the user-mode API
server implementation, lifecycle and servicing, rather than changing a process
name. The hypothesis that the system host causes reported instability still
needs a reproducible failure and attribution; process counts do not prove it.

Legend: `[v]` implemented/verified with named evidence, `[-]` partial,
`[_]` planned.

## Tree

```text
Outcome: own the Windows console backend with measured compatibility
├── [v] Establish the implementation boundary
│   ├── existing minicon-platform::pty facade and two Windows adapters
│   ├── public Microsoft C++ host, API dispatch and ConDrv transport sources
│   └── REA 6.7.0 release/source research; no decompilation execution claimed
├── [-] Rust crate: crates/minicon-console
│   ├── [v] actual ConDrv server, input event and non-inheritable reference
│   ├── [v] bounded receive/cancellation and explicit unsupported completion
│   ├── [v] accepted CONNECT, opaque handles and DISCONNECT reclamation
│   ├── [v] real cmd ASCII / CJK / emoji output and exact nonzero exit
│   ├── [v] deferred line/raw/record input, explicit interruption and input-ready event
│   ├── [v] job-owned client launch, actual parent/child teardown and input budgets
│   ├── [v] explicit experimental PTY adapter, native resize and key delivery
│   ├── [v] current input/output reopen, native access/duplicate/close probes
│   ├── [v] scoped PowerShell 5/7 and Node TTY/readline native cases
│   ├── [v] connection/file lifetimes, restricted inherited stdio and UTF-8 tails
│   ├── [v] scoped processed Ctrl+C: private Job admission, retained process handles,
│   │   native handler receipt and interrupted-read evidence; missing native
│   │   entry point fails explicitly; no host-console group broadcast
│   └── [-] general API service, signals, screen semantics and product selection
├── [_] Optional isolated, source-pinned C++ comparison build
│   ├── explicit selected-host launch; record actual image, hash and fallback
│   └── retain upstream notices and separate upstream baseline from our changes
├── [-] Replace behavior behind a reusable Windows adapter
│   ├── console API dispatch, handles and client attachment
│   ├── input records, processed/raw modes, Unicode and screen state
│   └── cancellation, process ownership, resize and bounded shutdown
└── [-] Qualify and integrate
    ├── differential clients, fault injection and current-user lifecycle evidence
    ├── Windows x86_64 and aarch64 separately; exact OS/build and feature scope
    └── explicit selection, package budget and rollback before default promotion
```

## Memory palace

```mermaid
flowchart LR
    CLIENT["unchanged cmd / PowerShell / Node / native client"] --> API["Windows console API and ConDrv"]
    API --> HOST["owned Rust console host: limited implementation"]
    HOST --> ADAPTER["explicit experimental adapter behind minicon-platform::pty"]
    HOST --> INPUT["deferred input / input-ready event"]
    HOST --> LIFE["connection defaults vs opened file objects"]
    LIFE --> DEFAULTS["DISCONNECT: caller reads and connection defaults"]
    LIFE --> FILES["CLOSE_OBJECT: shared file state and remaining reads"]
    HOST --> JOB["ConsoleChild: suspend / assign job / resume"]
    JOB --> SIGNAL["control targets admitted before CONNECT reply"]
    SIGNAL --> RETAIN["retained process handle / live check / native EndTask"]
    JOB -. "owned teardown" .-> CLIENT
    ADAPTER --> MC["MiniCon sessions and renderer"]
    ADAPTER --> AG["AgenTerm: separate pin and qualification"]
    KEYS["shared Windows VT-key decoder"] --> ADAPTER
    KEYS --> OLD["existing console-agent transport"]
    SRC["pinned public C++ source"] --> SPEC["API inventory and differential cases"]
    REA["REA: targeted binary evidence where needed"] --> SPEC
    SPEC --> HOST
    GATE["compatibility / lifetime / size / native evidence"] --> DEFAULT["explicit decision to change default"]
    HOST --> GATE
```

## Verified findings and source identities

Microsoft describes OpenConsole as the open-source console host. Its public
server source contains API dispatch and driver transport; this is a better
starting point than reconstructing those modules from decompiler output.
Public source and the locally installed Windows binary are separate identities:
no exact version or behavioral equivalence has been established.

Reference revision: `microsoft/terminal`
`9fd41b2baec77d25b8cfb584070d5998eab7df81`. Relevant source entry points:

- [README](https://github.com/microsoft/terminal/blob/9fd41b2baec77d25b8cfb584070d5998eab7df81/README.md),
  [license](https://github.com/microsoft/terminal/blob/9fd41b2baec77d25b8cfb584070d5998eab7df81/LICENSE)
  and [organization](https://github.com/microsoft/terminal/blob/9fd41b2baec77d25b8cfb584070d5998eab7df81/doc/ORGANIZATION.md).
- [API dispatch](https://github.com/microsoft/terminal/blob/9fd41b2baec77d25b8cfb584070d5998eab7df81/src/server/ApiDispatchers.cpp)
  and [ConDrv transport](https://github.com/microsoft/terminal/blob/9fd41b2baec77d25b8cfb584070d5998eab7df81/src/server/ConDrvDeviceComm.cpp).
- [ConPTY launch](https://github.com/microsoft/terminal/blob/9fd41b2baec77d25b8cfb584070d5998eab7df81/src/winconpty/winconpty.cpp)
  selects a host executable and carries native handles. Side-by-side launch is
  a feasibility reference, not proof our own host works on every supported OS.

REA official repository: [morluto/rea](https://github.com/morluto/rea).
Latest GitHub release observed on the research date:
[rea-agents-6.7.0](https://github.com/morluto/rea/releases/tag/rea-agents-6.7.0),
published 2026-10-18; tag target
`27dc42d10249f0410904a75c0997842eb03b931c`, package version `6.7.0`.
Its [Windows guide](https://github.com/morluto/rea/blob/27dc42d10249f0410904a75c0997842eb03b931c/docs/windows-ghidra-p0.md)
describes experimental native analysis; its
[CLI evidence contract](https://github.com/morluto/rea/blob/27dc42d10249f0410904a75c0997842eb03b931c/docs/cli.md)
is the operating reference. Native pseudocode and assembly are observations,
not an automatically recovered, buildable C++ project. Use REA for specific
unresolved binary/source differences, with symbols and differential execution.

Ten pinned reference files were downloaded and SHA-256 inventoried locally in
`target/conhost-research/source-inventory.json` (ignored). The observed system
host is file version `10.0.20348.1`, size `987136`, SHA-256
`b925ac56deef971ad6f83aed9eb8328568002c2b033917b70debc7f12e2e3553`.
No REA/Ghidra analysis or reference-host build was executed. REA, Java and
Ghidra were not discovered on the current command search path; that observation
does not establish absence from the machine.

## Ownership and architecture

Keep the public PTY facade stable. Windows-specific driver/bootstrap/ABI code
belongs in `minicon-console`, now a workspace member, behind the Windows
adapter with explicit experimental selection. Its initial Rust APIs are experimental;
the optional platform `rust-console` feature now depends on it; default
consumers do not enable or select it. AgenTerm can reuse the
mechanism, with its own explicit dependency and qualification.

The implementation language is Rust, as explicitly directed by the owner.
Microsoft C++ source and optional reference builds are specification and
differential evidence, not the new implementation. The component is named
`minicon-console`: it owns console service semantics; `minicon-platform::pty`
owns product-facing platform adaptation. If a C ABI is introduced, it must
bound ownership, buffer lengths, cancellation, errors and panic/exception
containment. Share host-neutral rules only after their behavior is understood.

The intended owned host still uses Windows kernel facilities, including ConDrv
where needed. Replacing that kernel driver is outside this user-mode project.
Raw redirected pipes alone cannot establish compatibility with console clients.
An initial build of upstream OpenConsole is a reference baseline, not a new
backend or a fix. Renaming that build proves neither independence nor stability.

## Delivery leaves

| Leaf | Invariant and observable evidence | Safe failure / dependency / non-goal |
|---|---|---|
| SOURCE | Pin source, toolchain and notices; map the required native structures and bootstrap contract into Rust. Optional reference builds record dependencies, binary hashes and actual imports. | Report missing tools explicitly; do not modify the installed system host. No binary/source equivalence claim from matching names. |
| BOOTSTRAP | A child built against the ordinary console API attaches to the selected host; record actual host image/hash and driver/bootstrap calls. Disabling custom-host selection must fail the identity assertion. | Refuse unsupported combinations or explicitly report fallback. Needs SOURCE; no silent system-host fallback satisfying an independence test. |
| SEMANTICS | Inventory implemented/unimplemented APIs and observed return codes, handles, text, cursor, events and exit behavior against reference and system hosts. | Unsupported behavior is explicit and session-local. Needs BOOTSTRAP; an echo-only demo is not shell/TUI compatibility. |
| LIFETIME | Current-user/SID-scoped launch identities survive PID-reuse checks; close-tab, close-window, crash, EOF, blocked I/O and resize races have bounded cleanup and sibling isolation. Mutating cleanup must fail. | Kill only owned fixture processes. Needs SEMANTICS; no cross-user process-count inference or global conhost termination. |
| PROMOTION | Per-OS-build and per-ISA runtime evidence, representative client journeys, host-inclusive resource accounting and artifact checks precede changing defaults. | Retain explicit compatibility backend; identify which backend actually ran. Needs prior leaves; no untested all-Windows claim. |

Required client cases include cmd, Windows PowerShell, PowerShell 7, Node-based
agent CLIs and native Console API probes. Cover raw/processed input, mouse,
Ctrl+C/Break, UTF-8/UTF-16, CJK, alternate screen, resize/reflow, redirected
handles, nested children, attach/free console and termination. Keep the matrix
small enough to diagnose individual differences before expanding it.

The experimental adapter now runs its Rust service in the consumer process,
without a service sidecar or embedded extraction. Shipping single-file delivery
still needs an explicit product-selection/packaging decision and exact-artifact
size/signature/scan and cleanup evidence. A C++ sidecar reference build may be
used only for research; no new shipping form is approved by this research
document. Retain current supported systems until the owner
chooses a separately evidenced compatibility boundary.

## Status and next executable milestone

[v] Source and REA research with pinned references. [-] Rust native transport
and API service; [v] initial connection/I/O/record/job subset and scoped Ctrl+C;
[-] remaining API/signal semantics; [_] optional binary analysis and product selection.
Sequence: [backend research plan](../plan/plan-windows-console-backend.md).
This work has no assigned release version and does not replace the approved
0.2.16 test-orchestration plan. No production backend or default changed.

### Initial Windows x86_64 native evidence

`scripts/build.sh test rust_console_ -- --nocapture` executes thirty tests:
sixteen library cases (thirteen native and three state cases) and fourteen
independent native Win32-client cases. The
canonical wrapper visits unrelated workspace/fork suites but filters those
cases out; this is a targeted pass. The original five bootstrap cases remain:

- `rust_console_driver_registers_noninheritable_server_and_reference`: real
  driver registration/reference creation; kernel handle flags checked.
- `rust_console_driver_rejects_a_non_console_handle` and
  `rust_console_driver_rejects_truncated_registration`: kernel rejects the
  invalid handle and incomplete registration, respectively.
- `rust_console_driver_receive_timeout_cancels_and_settles_io`: two real
  asynchronous waits time out and settle; no stale first read blocks the next.
- `rust_console_driver_receives_and_rejects_a_real_cmd_connection`: an
  unchanged cmd fixture receives our private pseudoconsole reference through
  native process attributes. Rust receives CONNECT with the actual child PID,
  completes it as unsupported, and observes non-successful client exit.
  This is not proof cmd commands work. The fixture never invokes the system
  `CreatePseudoConsole`; it is terminated by its owned process handle on failure.

Runtime negatives: making native handles inheritable fails the handle-ownership
assertion; removing the selected reference from process startup fails CONNECT
reception. Both mutations are restored before the positive run. Local logs
remain under ignored `target/conhost-research/`.

An earlier attempt to query `IOCTL_CONDRV_GET_SERVER_PID` returned Win32 error 1
on the observed driver. That attempted identity mechanism is not shipped or
silently treated as supported; the bootstrap proof instead observes a real
kernel-delivered client request on our private server. No kernel-driver rewrite,
Windows aarch64 native pass, complete host implementation or improved stability
is claimed by these initial tests.

Compile-only checks also pass for Windows aarch64 (including test sources) and
the non-Windows Linux x86_64 configuration. The Linux configuration exports no
Windows console service; compilation does not qualify a native runtime.

### Accepted-client service and current API inventory

`ConsoleHost` binds permanently to one driver-session identity. Kernel-provided
process/object tokens are stored as opaque integers rather than dereferenced
pointers. The current caps are 64 clients, 1024 handle records and 1 MiB per
input transfer; oversized transfers fail explicitly. A request borrows its
originating session and is consumed by completion, preventing double completion
through the Rust API. These are implementation boundaries, not compatibility
certification for all clients or OS releases.

| Driver/API operation | Current behavior | Remaining scope |
|---|---|---|
| CONNECT / DISCONNECT | Accept validated initial metadata, issue process/input/output tokens, cancel the caller's reads and reclaim its connection-default records on disconnect. | Arbitrary cross-process duplication, explicit attach/reconnect and broader descendant qualification remain open; native parent exit/FreeConsole cases are scoped below. |
| CREATE_OBJECT / CLOSE_OBJECT | Open current input/output and unambiguous generic objects; retain native access/share metadata, enforce API access and retire known kernel-finalizer tokens even without a live creator. Opened file objects can be used by another connected caller holding the native inherited handle. | Separate new screen buffers, other access masks and broad sharing remain unqualified. Native Win32 CON opens on the measured kernel normalize share metadata to 3, including a requested share mode of zero. |
| RAW_WRITE / WriteConsole | Capture actual client bytes, convert current code page or UTF-16 to UTF-8, send to the supplied writer, update screen state. | DBCS output fragments, processed-output/mode fidelity, pending surrogate at final stream end and complete output backpressure/cancellation. |
| GetCP / SetCP / GetLangId | Distinct input/output code pages, validated ID updates, real initial OEM page/current-user language. UTF-8 one-byte reads are tested. | Full legacy/stateful code-page and mixed-encoding qualification. |
| GetMode / SetMode | Store buffer modes and enforce echo's line-input dependency; basic cooked/raw input consumes those modes. | Full mode validation, signals, insert/history editing and processed-output fidelity. |
| RAW_READ / ReadConsole | Retain actual requests until input arrives; cooked line/echo/backspace, Unicode/byte delivery, caller initial text, wake mask/control state, bounded partial-read tails. | Full editing/history, mixed ANSI-to-Unicode transitions with a partially consumed byte, all control-character and cancellation variants. |
| GetNumberOfInputEvents / ReadConsoleInputW / PeekConsoleInputW / WriteConsoleInputW | Bounded typed key/mouse/resize/focus queue; counts, remove/peek, NOWAIT bypass, native input-handle readiness and Unicode append/prepend decoding. Native client-written key/focus records and returned counts are tested. | ANSI input-record variants, remaining record types and broad application qualification. |
| GetScreenBufferInfo | Return bounded initial geometry and cursor from the owned VT parser, plus initial attributes/colors. | Full legacy screen mutation, attributes, scrolling, resize, multiple buffers and readback. |
| GetCursorInfo / SetCursorPosition | Query initial cursor size and owned VT visibility; validated native position emits a VT cursor move into the same screen state. | Cursor-size mutation, complete window/scrolling interactions and general API fidelity remain open. |
| GetTitleW / SetTitleW | Preserve initial/current UTF-16 title; bounded metadata and client buffers. | ANSI title variants and independent native API differential coverage. |
| Processed Ctrl+C input | Private-Job admission before CONNECT reply, retained process identity, native handler/default exit and interrupted-read receipts; raw input remains data. | General process-group GenerateConsoleCtrlEvent, Ctrl+Break, other signals and broader OS/client qualification. |
| Other operations | Explicit NOT_SUPPORTED; no silent call to system conhost. | Remaining layer APIs and GUI/product qualification. |

The backend reuses this repository's owned `vt100` fork for screen state behind
the Windows adapter. That reuse does not make Windows console mode semantics
equivalent to VT semantics; remaining differences stay explicit above.
`minicon-platform` and the MiniCon binary still do not depend on this experimental
crate, so the shipped default and artifact dependency graph are unchanged.

Native evidence:

- `rust_console_driver_owned_host_executes_real_cmd_output_and_exit`: unchanged
  system cmd executes a built-in echo. Exact output reaches the Rust sink, exit
  is zero, actual dispatch includes successful output and DISCONNECT clears all
  client records. Every dispatch in this fixture must be supported.
- `rust_console_driver_owned_host_preserves_real_cmd_unicode_and_nonzero_exit`:
  unchanged cmd writes Chinese plus an emoji through WriteConsole, then exits
  with status 7; exact text/space/CRLF and the actual exit status are checked.
- `rust_console_driver_owned_host_rejects_a_foreign_session_without_harming_its_client`:
  two private driver sessions and two real clients; a host rejects the foreign
  CONNECT while its original client remains live and finishes successfully.
- Three state cases cover a surrogate pair split across writes, malformed
  surrogate recovery and writer failure without false screen advancement.

Runtime negatives drop actual sink output, disable session binding, discard the
pending high surrogate and swallow sink failure. Each corresponding selected
test fails; restored code passed the original eleven-case output suite. These
cases remain in the backend selector and expanded PTY suite. Local evidence:
`target/conhost-research/host-final-suite.log` and named `*-negative.log` files.

Debugging found two fixture/bootstrap details: standard handles must explicitly
avoid the parent's streams; cmd's quoted image spelling must use native path
separators. A mixed `System32/cmd.exe` spelling caused cmd command parsing to
fail even though native WriteConsole worked. No production workaround or
relaxed output assertion was used to qualify echo. A separate local native API
probe helped confirm output mode, screen-info layout and WriteConsole's result;
it is diagnostic evidence, not a registered cross-platform acceptance case.

### Deferred input, independent API probes and owned process lifetime

`host/input.rs` owns bounded event/read state: 4096 queued events, 64 pending
reads, 32768 UTF-16 line units and the existing 1 MiB transfer ceiling.
`ConsoleHost` borrows the originating driver while reads are deferred. Each
request is completed once; dropping an unhandled request also releases its
actual kernel wait. Disconnect cancels that caller's reads and retires its
connection defaults. Opened-file byte tails survive the creator's disconnect
until the final object close; that close retires the object's pending reads
regardless of caller. Input availability is the real event registered
with ConDrv, including input queued before CONNECT.

`ConsoleChild` is the reusable native launch/lifetime owner. It takes an
absolute image plus an explicit Windows command line, creates a suspended
client with the private reference, assigns a kill-on-close job, then resumes.
Resume checks the actual prior suspend count. The public process handle is
borrowed; membership/counts are queried from the private job. Dropping the owner
closes that job and waits for its root process with a bounded budget.
Native UTF-16 environment/cwd overrides and an explicit platform adapter are
implemented in the following milestone; broader product selection remains open.

The Cargo-built `minicon-console-probe` directly calls Windows APIs and reports
results to private files rather than using the console path it is testing.
Cargo supplies its exact executable path to `tests/native_input.rs`; the court
does not guess a hashed test path. It proves:

- ReadConsoleW exact UTF-16/counts, caller-supplied initial characters and a
  requested Tab wakeup with control-key state.
- Actual blocked cmd input, Enter-delimited completion and emoji backspace
  editing; killing the waiting client clears its deferred request.
- ReadConsoleInputW typed key/mouse/resize/focus fields, empty/nonempty peek,
  native stdin-handle waiting and NOWAIT from a second thread while a cooked
  read is already blocked.
- ReadFile one-byte UTF-8 reads preserve Chinese and a supplementary character;
  code-page conversion uses the native codec and retains per-handle byte tails.
- An overflow enqueue leaves the native-visible 4096-record inventory intact,
  and pre-CONNECT input signals the actual input handle.
- A blocked reader is reclaimed by job-owner drop. A real child spawned by the
  probe joins the same job and Rust console; root and descendant process
  objects are both observed terminated after owner drop. The descendant handle
  gains cleanup authority only after positive private-job membership checking.

The interruption protocol is console-specific. The pinned Microsoft
`readDataCooked`, `readDataDirect` and `readDataRaw` implementations use
`STATUS_ALERTED` for handle-closing/interruption cases. A generic
`STATUS_CANCELLED` reply caused the controlled cmd fixture to exit with an
access violation; that is a rejected implementation, not a successful cancel.
Native ReadConsoleW now returns BOOL success, zero characters and LastError
995 (`ERROR_OPERATION_ABORTED`) on the tested interruption path. This special
console behavior is documented by [ReadFile](https://learn.microsoft.com/en-us/windows/win32/api/fileapi/nf-fileapi-readfile)
and inherited by [ReadConsole](https://learn.microsoft.com/en-us/windows/console/readconsole).
The cancellation primitive alone does not deliver a process signal. Owned
Ctrl+C delivery is implemented in the separate milestone below.

Seventeen runtime negative controls cover payload delivery, interruption
status, input-event signaling, ANSI byte tails, raw surrogate pairing, Enter
waiting, NOWAIT ordering, native mouse fields, dead-client cleanup, initial
input offsets, kill-on-close, suspended launch, request-drop cancellation,
Unicode backspace, atomic queue overflow, wakeup control state and prebound
input signaling. Each selected test must fail with the changed behavior;
all edited source bytes are restored before the positive suite. An early mouse
mutation merely caused an unused-variable compile failure and was replaced by
a runtime mutation; compile failures do not count as negative proof.

Local receipts: `target/conhost-research/input-negative-audit.json`,
`input-full-suite.log`, and per-control `*-negative.log` files. The current
native scope is Windows x86_64 on the measured host; Windows aarch64 compilation
does not replace native execution. General signal/resize/readback compatibility,
legacy driver bootstrap and MiniCon/AgenTerm product selection remain open.

### Explicit experimental PTY integration

Enable MiniCon's `experimental-rust-console` feature (forwarding the platform's
`rust-console` feature), then call the Windows `ChildCommand::rust_console()`
builder before `spawn`. The ordinary `into_parts`, master reader/writer/resize,
wait clone, native keys, input ownership and verified Job cleanup contracts are
used. `PtyMaster::backend_kind()` identifies the actually opened Windows session;
the process-wide default backend report still describes default selection.
No GUI setting, environment flag or automatic fallback selects the Rust host.

The Windows adapter's `pty/rust_console.rs` composes the existing command-line
quoting, application search, environment merge, cancellable input writer and
64 KiB bounded output queue. One in-process worker owns its driver and borrowing
host locally. There is no console-agent child, CreatePseudoConsole call or
system-host launch in this path. It polls its private input pipe, retains split
UTF-8 and CRLF state, and services a bounded control queue. Invalid UTF-8 is an
explicit session failure. Native up/down keys bypass host console attachment.
The owned screen has a 1 Mi-cell budget; resize updates the reused VT screen
and emits a native window record when window input is enabled. This does not
yet establish full reflow, alternate-buffer or legacy screen API compatibility.

Shutdown switches output to drain mode before terminating the private job and
waiting, bounded, for the host worker. The worker reports transport errors and
EOF through the existing output queue. API-level unsupported replies go back
to their requesting clients without becoming authority to kill sibling clients.
A final native cancellation (995) becomes EOF only after the actual private
Job is observed empty. A live client's error remains visible. Independent
output-reader failures are sent to the main test thread, which also requires
EOF; a detached reader panic cannot supply positive evidence.

`scripts/build.sh test --features experimental-rust-console rust_console_ -- --nocapture`
adds seven platform integration cases and one actual-output-capacity shutdown
case to the original thirty tests. The facade cases cover environment/cwd and
nonzero exit, fragmented UTF-8 and CRLF, blocked/nonreading-client cleanup, prelaunch screen
budget rejection, native initial/resized dimensions and key fields. UTF-8
fragments are separated by two acknowledged worker laps, preventing pipe
coalescing from bypassing the decoder boundary. The output-pressure case
observes the queue at actual capacity before shutdown and requires worker exit,
not merely client exit. Native dimension/key reports come from a Cargo-built
Win32 fixture using a private result file; Cargo provides the exact binary path.

Twelve additional runtime negative controls cover actual backend selection,
environment/cwd forwarding, split UTF-8/CRLF, initial dimensions, resize event
fields, key repeat, job termination, output-drain release, the prelaunch screen
budget and the workspace/fork feature boundary. Each selected test/self-test
fails after its guarded behavior is changed; compile failures are excluded and
source restoration is SHA-256 checked. Local receipts are
`target/conhost-research/pty-negative-audit.json`, `pty-positive-suite.log` and
the named `pty-*-negative.log` files. These extend the earlier milestone's
negative evidence rather than claiming its old source hashes describe this one.

The measured native facade host reports Windows `10.0.20348`, build `20348`,
x86_64. The following real-client milestone extends this scope; older Windows
and Windows ARM64 runtime behavior remain unqualified. GUI integration,
complete control signals and screen semantics remain required before promotion.

### Real-client and native object milestone

The native qualification command adds explicitly selected external courts:
`scripts/build.sh test --features experimental-rust-console rust_console_ -- --include-ignored --nocapture`.
Set `MINICON_TEST_WINDOWS_POWERSHELL`, `MINICON_TEST_PWSH` and
`MINICON_TEST_NODE` to the absolute images being qualified. Their tests fail
with `BLOCKED` when a requested image is missing; default execution leaves the
five external/reference courts unrequested rather than counting them as passes.
Clear `AGENTERM_FORCE_CONSOLE_AGENT` and `FORCE_CONSOLE_AGENT` in that test
process for the reference court. It requires actual backend identity `conpty`;
an inherited fallback override initially selected `console-agent` and was
rejected, not accepted as reference evidence.

The measured images are Windows PowerShell `5.1.20348.859`, PowerShell `7.4.2`
and Node `v24.15.0`. `target/conhost-research/client-identities.json` records
their observed runtime versions, file versions and SHA-256 values. Both
PowerShell cases set UTF-8 input/output explicitly, read CJK plus an emoji and
return an exact nonzero exit code. Node requires actual stdin/stdout TTYs and
exercises raw Unicode delivery and readline cursor-left insertion (`ac`, left,
`b` becomes `abc`). The scripts use `TERM=xterm-256color`, matching MiniCon's
terminal launch; inherited `TERM=dumb` selected Node's simplified editor and
was a rejected fixture configuration. This does not qualify PowerShell's full
interactive editor, all default encodings, Node agent packages or general TUIs.

`host/objects.rs` owns handle identity/access/share checks. Current-buffer
CREATE_OBJECT completion transfers its token in native `Information`, rather
than an output byte buffer, following the pinned
[IoDispatchers](https://github.com/microsoft/terminal/blob/9fd41b2baec77d25b8cfb584070d5998eab7df81/src/server/IoDispatchers.cpp).
An unchanged Win32 fixture verifies read-only/write-only access failures,
DuplicateHandle survival after original close, generic CON input, and 1050
sequential open/close operations beyond the simultaneous handle cap. Its native
result vector matches the explicitly opened system ConPTY reference. Requested
Win32 share mode zero succeeds on this host and arrives at our server as share
metadata 3; this is not evidence that exclusive native sharing has been proved.

The cursor query exposes a concrete contract discrepancy: public
[GetConsoleCursorInfo](https://learn.microsoft.com/en-us/windows/console/getconsolecursorinfo)
documents READ access, but the measured 20348 server and pinned
`ApiDispatchers.cpp` require WRITE. The differential fixture covers both handle
directions; the implementation follows that observed/source-pinned behavior
without claiming identical behavior across all Windows builds.

Windows `console_input.rs` now owns the VT-key decoder used by both the legacy
console agent and the owned adapter; its existing agent tests retain their
identities. The owned worker uses it for escape keys while preserving its own
complete UTF-8/UTF-16 text path. Native records retain key identity even with
VT input mode enabled; only ReadConsole/ReadFile text delivery converts those
keys into supported VT sequences. A native probe checks both API directions
after a CSI prefix is split across acknowledged worker laps. Key modifiers,
all function-key forms, mouse transport decoding and the legacy decoder's full
Unicode behavior remain separate qualification obligations.

`WriteConsoleInputW` now decodes bounded native records, preserves append versus
prepend ordering, services actual waiting reads and returns the native count.
This also supplies the input wakeup used by libuv when canceling a previous
console read. The [Node v24.15.0 libuv source](https://github.com/nodejs/node/blob/v24.15.0/deps/uv/src/win/tty.c)
was a source guide; no binary/source equivalence is claimed. Close finalizers
remove the known handle's pending reads and byte tails before retiring it,
and refresh the actual input-ready event.

The expanded selector executes 46 targeted cases: the earlier 38, three more
native platform cases, four external-client cases and one ConPTY differential
case. Local positive evidence is `target/conhost-research/clients-positive-suite.log`.
The final selector also includes `selected::console_agent::tests::` after `--`
and executes the 29 existing agent unit cases, for 75 targeted cases in
`clients-final-suite.log`. This is scoped regression evidence, not the full GUI
or release court. Normal experimental execution without requested external
courts executes 41 of these backend cases and leaves five unrequested.

Twelve additional runtime negative controls cover current-buffer reopen for
both PowerShell versions, Node cursor-query initialization, access checking,
compatible share metadata, close-budget reclamation, native cursor permissions,
shared arrows in both Node and the legacy agent, record payload/count, and VT
text/record delivery. Source restoration is SHA-256 checked. An initial arrow
mutation only caused a dead-code compile failure; it was replaced with a
runtime mutation and the compile-only failure was excluded. Receipts are
`target/conhost-research/client-negative-audit.json` and named
`client-*-negative.log` files. Each earlier milestone's receipts retain their
own source scope; they are not relabeled as evidence for this revision.

### Connection lifetime versus opened-file lifetime

Actual standard stdin/stdout inheritance already survived the parent's exit
and explicit FreeConsole in the first native probe; this disproved the initial
assumption that all inheritance was broken. The narrower failing case passed
a separately opened, read-only CONOUT$ as child stdout. After creator exit,
the owned backend returned invalid handle (6) for a read query. The same fixture
on system ConPTY kept the read query valid and rejected writing with access
denied (5). The independent native result is `true,0,true,0|false,5,0`.

The old record tied every handle to its creator's console-process token and
erased them all on disconnect. `HandleLifetime` now distinguishes connection
defaults from CREATE_OBJECT files. The connection case carries its owning
process token; an opened file is instead retained until the driver's final
CLOSE_OBJECT. This follows the pinned
[ProcessHandle](https://github.com/microsoft/terminal/blob/9fd41b2baec77d25b8cfb584070d5998eab7df81/src/server/ProcessHandle.h)
ownership of connection defaults and
[IoDispatchers](https://github.com/microsoft/terminal/blob/9fd41b2baec77d25b8cfb584070d5998eab7df81/src/server/IoDispatchers.cpp)
transfer of created-file ownership to the driver. `open_handles()` exposes a
count of live server records alongside `connected_clients()`, not process IDs
or OS handle authority.

Every operation still requires a known calling connection, a valid object from
the already-bound driver session, the proper object kind and native access.
The creator is not an access boundary for a file that the kernel has inherited
into another connected caller. The read-only inheritance case proves that
allowing that caller does not grant WRITE access. Kernel object identity is
not inferred from an application-supplied numeric handle or a PID scan. The
public [console-handle contract](https://learn.microsoft.com/en-us/windows/console/console-handles)
distinguishes creation-time sharing from arbitrary interprocess duplication;
this milestone does not certify the latter.

`cancel_process_reads` cancels operations belonging to the departing caller.
`retire_input_object` removes an object's conversion tail and all requests for
that object when its lifetime ends. A parent reading the first byte of UTF-8
CJK input from an opened CONIN$ can exit; the child consumes the remaining two
bytes from the inherited file without replacement or loss. Conversely, closing
the last file handle discards a partial tail and clears the real native input
signal. The native fixture observes first byte 228, remaining bytes 184/173,
and WAIT_TIMEOUT/zero queued events after final close. Separate actual-cmd
evidence requires all connection-default records to be gone after disconnect.
A three-cmd private family additionally keeps the opened input file alive while
its reading child is terminated through an owned, Job-verified process handle.
After the actual child DISCONNECT, exactly one live parent read must remain and
consume the supplied line. This isolates caller cancellation from final file
close and connection-default retirement.

The Cargo-built `tests/support/inheritance.rs` fixture coordinates only its own
process family through private files. The controller observes creator exit
before releasing the child; SDK mode/count/error results are independent of
the output path under test. Native stdio inheritance, parent FreeConsole,
restricted output, a shared byte tail and final-close input readiness are
covered. A real Node child_process spawn also inherits TTY output and propagates
the child's nonzero exit code. Cleanup remains the existing private Job owner.

The expanded command from the preceding milestone, also selecting the 29
legacy-agent units, executes 83 targeted cases: 54 backend cases plus those 29
units. Seven explicit external/reference cases are requested in this receipt;
normal experimental execution leaves them unrequested. Local baseline evidence
is `target/conhost-research/inherit-final-suite.log`, with the first failing
reference comparison in `inherit-reference-repro.log`. This adds scoped
inheritance evidence on the measured Windows x86_64 host, not general attach,
new-buffer, signal, TUI or Windows ARM64 runtime qualification.

Eight runtime mutation controls cover connection-default cleanup, opened-file
lifetime, early file retirement, early and final byte-tail cleanup, native
access checks, nested connections and dead-reader cancellation. Removing each
guard makes its named native test fail; sources are restored and the final
83-case suite passes again. Local mutation identities and restored source hashes
are in `target/conhost-research/inherit-negative-audit.json`.

### Owned Ctrl+C delivery

`ConsoleHost::for_child` binds control authority to the launch owner's private
Job. The control scope borrows that launch owner through the host lifetime;
it does not duplicate the Job handle or extend its kill-on-close ownership.
Each kernel CONNECT is checked against that Job before its success reply.
The host retains the admitted process handle until DISCONNECT; `control_targets`
reports those scalar inventory counts. There is no later PID lookup, global
console attachment or GenerateConsoleCtrlEvent broadcast from the host. A
foreign private-Job fixture is rejected with native access denied before any
client handle is admitted. The normal cmd teardown fixture observes one retained
target after CONNECT and zero after DISCONNECT.

The pinned public `src/host/conwinuserrefs.h`, `src/interactivity/win32/ConsoleControl.cpp`
and `src/server/IoDispatchers.cpp` supply the native EndTask ABI and console-app
notification. Rust loads user32 only from the system directory and resolves the
control entry point; absence fails explicitly. Delivery iterates only retained,
live targets. The PID-shaped ABI argument comes from the retained process handle;
its identity cannot be reused while that handle pins the object, as explained
in Microsoft's [process-ID lifetime discussion](https://devblogs.microsoft.com/oldnewthing/20110107-00/?p=11803/).
A native return code is not delivery evidence: `send_ctrl_c` counts attempts.

Processed key-down character 0x03 is intercepted on the external input route,
including when no read is pending. Pending console reads receive STATUS_ALERTED
with zero native counts. With processed input disabled, that character remains
ordinary input. WriteConsoleInputW record injection is a separate native API
route and is not qualified as a signal generator by this milestone. General
GenerateConsoleCtrlEvent/process-group targeting and Ctrl+Break remain open;
this is console-wide private authority, not a foreground-process contract.

The Cargo-built Win32 client explicitly clears its inherited ignore-Ctrl+C
attribute before installing a real handler. Microsoft documents that this
[ignore attribute is inherited independently of the handler list](https://learn.microsoft.com/en-us/windows/console/ctrl-c-and-ctrl-break-signals).
The initial native call returned success without executing the handler while
that inherited attribute was still enabled (`control-native-status.log`). This
is preserved as a rejected success criterion, not a backend-delivery receipt.

The handler writes only to an atomic; its ordinary main thread reports native
results through the private file fixture. Real observations cover handler-only
delivery without a pending read, interrupted ReadConsoleW (BOOL true,
LastError 995 and zero units), unprocessed input (one unit 3, no handler),
default STATUS_CONTROL_C_EXIT and isolation between two simultaneously live
private consoles. Explicit system-ConPTY comparison repeats handler, interrupted
read and unprocessed-input cases and matches their independently observed values.
The expanded targeted command now executes 90 cases: 61 backend and 29 legacy
agent units, including eight requested external/reference cases. Baseline local
evidence is `target/conhost-research/control-positive-suite.log`; the final
post-mutation receipt is `control-final-suite.log`.

Remaining control qualification includes synchronous console I/O from a native
handler, input-queue saturation before interruption, inherited ignore-state
journeys in real shells and delivery under sustained connection churn. The
current input-budget precheck can reject new input before control-key dispatch;
this milestone does not claim overload recovery. These cases remain required
before promoting the experimental backend.

Eight runtime mutation controls cover native dispatch, processed-mode gating,
blocked-read interruption, private-Job admission, target retirement on disconnect,
control-key recognition, missing native export and the system-reference event
comparison. Each new test fails with the relevant behavior changed. The local
`control-negative-audit.json` records test/log identities and restored hashes;
`control-final-suite.log` records the fresh 90-case pass after restoration.
