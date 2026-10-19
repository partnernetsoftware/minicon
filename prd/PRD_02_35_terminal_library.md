# Reusable terminal library: minicon-console

Parent: [product index](../PRD.md). Accepted direction: keep `minicon-console`
as an independent terminal-specific library, with reusable common mechanisms
and explicit native adapters. A possible later merge into `minicon-platform`
is a separate design decision, not a prerequisite or a current migration.

## Outcome and delivery tree

```text
Outcome: another application can embed our terminal without MiniCon product state
├── [-] Common terminal state and streaming facade — terminal::TerminalStream / Utf16Decoder
│   invariant: one owned parser/framer pair; no OS handles or ISA assumptions
│   evidence: public facade unit, shared parser/framer units, target build receipts
│   safe failure: bounded-frame error or explicit I/O failure; no silent truncation
│   dependency: owned vt100/vte; non-goal: GUI/event loop or process spawning
│   └── bounded encoded runs / read-only lexer text_prefix / shared scalar width
├── [-] Shared cell operations — terminal::CellRect / Screen region and literal spans
│   invariant: overlap reads a snapshot; clipping limits writes; no OS/ISA branching
│   evidence: shared grapheme/overlap/span units, native SDK and actual managed consumers
│   safe failure: typed move error or clipped span count; native sink-before-commit
│   dependency: common cell model; non-goal: Win32 ABI or incidental history scroll
├── [-] Independent drawing buffer / terminal viewport
│   invariant: one authoritative parser/screen; transient snapshots for output
│   evidence: common viewport unit, native geometry/wrap/parent-resize SDK and real properties
│   safe failure: staged grammar/view commit after sink success; explicit output budget
│   dependency: shared exact lexer fork/framer; non-goal: duplicating a Windows shadow grid
├── [v] Windows host adopts the common stream facade
│   invariant: native projections preserve client grammar and commit after sink success
│   evidence: public projection unit plus SDK mixed-output and existing host/screen courts
│   safe failure: reject unfinished projections; preserve state on pre-write failure
│   dependency: common stream; non-goal: GUI migration or native Job policy changes
├── [-] Windows native console service — PRD 02.34
│   invariant: owned user-mode host; existing platform PTY integration
│   evidence: actual console programs, SDK receipts and private-Job lifecycle
│   safe failure: explicit unsupported/invalid/native errors; no system-host fallback
│   dependency: ConDrv and Windows adapters; non-goal: replacing the Windows kernel
├── [_] Portable session composition and input vocabulary
│   invariant: shared terminal semantics; OS-specific capabilities stay explicit
│   evidence: same external consumer and scenarios on macOS/Linux/Windows
│   safe failure: typed unsupported capabilities, bounded shutdown/backpressure
│   dependency: stable common contracts; non-goal: copying all platform facilities
└── [_] Cross-ISA and external-consumer qualification
    invariant: portable API does not imply native ABI/runtime certification
    evidence: minimal consumer, enabled features, exact source, OS/ISA build + runtime
    safe failure: unavailable cells remain unqualified, never implicit passes
    dependency: native courts; non-goal: premature stable ABI or platform merger
```

```mermaid
flowchart LR
    APP["MiniCon / external terminal consumer"] --> COMMON["minicon-console::terminal"]
    COMMON --> VT["owned vt100: screen / damage / framing"]
    VT --> VTE["owned vte: authoritative VT / UTF-8 grammar"]
    APP --> PLATFORM["minicon-platform::pty"]
    PLATFORM -->|"rust-console opt-in"| WIN["minicon-console Windows service"]
    VP["TerminalViewport / mapped publication / staged screen edits"] --> COMMON
    TEXT["text_prefix / TEXT_BATCH_BYTES / scalar placement policy"] --> COMMON
    WIN --> VP
    MOVE["shared region movement / cell invariants"] --> COMMON
    WIN --> MOVE
    WIN --> COMMON
    WIN --> KERNEL["Windows ConDrv"]
    PLATFORM --> POSIX["macOS / Linux native PTY adapters"]
    COMMON & WIN & POSIX --> E["separate OS × ISA build / runtime / consumer evidence"]
```

## Dependency and public API boundary

`minicon-console` must not depend on `minicon-platform` while platform's optional
Windows provider depends on it. Common screen/stream mechanisms depend only on
the owned parsers and Rust standard library. Native console exports remain
Windows-gated; `terminal` is available on other operating systems. The current
facade owns parser and framer together and exposes cells/styles, damage,
callbacks and bounded output, with consuming finalization. Low-level grammar
semantics remain owned by [terminal state](PRD_02_24_con_terminal.md).

The Windows host now owns `terminal::TerminalStream` instead of separate
low-level parser/framer fields. Client output and complete native screen
commands use the facade; readback, fills, resize and mode queries use its screen
accessors. Win32 mode conversion, legacy error mapping and private-Job lifecycle
remain in their native owners. This is an actual service consumer of the public
library boundary, not only a parallel export. MiniCon's GUI parser remains a
direct shared-parser consumer; portable process/session composition is open.

`write_projection()` interprets generated complete commands with a transient
screen decoder, commits only after sink success, and preserves pending client
VT/UTF-8 and callback state. Projection events do not invoke client callbacks;
this path is for generated cursor/style/mode commands, not general client output.
Unfinished projections fail with InvalidInput before publication. Public
`finish()` consumes the stream. The service alone retains a crate-private
borrowed tail flush for its existing final-Job lifecycle; it does not expose
mutable client decoder state.

The next abstractions require explicit contracts for terminal size versus pixel
size, input events/encoding, capability reporting, read/write ownership,
backpressure, explicit-close versus natural EOF, and errors portable across OSes.
Native process and Job/foreground-group authority must remain in one owner.
POSIX PTY consolidation is tracked in [reuse map](PRD_02_28_shared_core.md).
Do not move general filesystem/network/window APIs into this terminal library.

## Evidence frontier

The facade test is
`rust_console_terminal_stream_preserves_pending_grammar_across_screen_edits`,
run through `scripts/build.sh test`. It exercises fragmented output, a public
screen edit, retained decoding and final accepted tail through public library
calls. Runtime mutation and target-build receipts must accompany portability
claims. macOS runtime, Windows ARM64 native behavior, portable session composition,
external consumer migration, packaging and stable API guarantees remain open.

Local qualification for the initial common facade: `cargo check --locked -p
minicon-console --lib --target <target>` passed for
`x86_64-unknown-linux-gnu`, `aarch64-unknown-linux-gnu`,
`aarch64-pc-windows-msvc` and `x86_64-apple-darwin`. Native Windows x86_64
library checks and strict all-target Clippy also passed. These are build
receipts, not runtime claims for the four cross targets. The ignored
`terminal-library-negative-audit.json` records four observed runtime failures
and restored source hashes: bypassed public framing, relaxed pending budget,
disabled grammar reset and omitted UTF-8 boundary tracking. The focused
`mixed-output-library-positive.log` records nine passing shared/native tests.

Host-adoption evidence is the public
`rust_console_terminal_projection_is_transactional_and_preserves_client_events`
unit plus the existing native SDK mixed-output, fill/readback/mode and final-tail
courts. `terminal-host-adoption-positive.log` records the focused positive run.
Runtime negative evidence and the final selected suite are recorded separately;
these checks do not extend platform/ISA runtime scope beyond the actual host.

`terminal-host-negative-audit.json` records three runtime failures with restored
source hashes: accepting an unfinished projection, committing before sink
success, and bypassing the common projection API in the actual Windows host.
The last mutation fails the native SDK mixed-output scenario, establishing that
the service really exercises the shared public boundary.

The host-adoption final selected suite passed 176 tests (including 17 alignment
checks), recorded in `terminal-host-adoption-final-suite.log`. Workspace default
and experimental all-target strict Clippy and root fmt checks passed. Updated
library checks passed for Linux x86_64/aarch64, Windows aarch64 and macOS x86_64;
these are compile-only receipts, not new native runtime certification. Detailed
hashes and remaining scope are in ignored `terminal-host-adoption-final-receipt.json`.

## Independent text producers

`terminal::Utf16Decoder` is the common incremental UTF-16 mechanism. Each
producer owns one decoder; `push()` yields zero to two owned Unicode scalars,
retaining an unmatched high surrogate and replacing invalid pairings. Its
iterator does not borrow the decoder, so a caller can publish immediately.
`discard_pending()` lets a line editor delete an undisplayed high unit without
erasing a screen cell. This library mechanism has no native handles or ISA
branches. The Windows client's output decoder and each pending cooked read's
echo decoder are concrete, independent consumers. Native interruption/drop
also drops the read's private echo state. GUI migration remains separate.

The accepted cooked-echo behavior, its actual SDK and system reference scope,
and remaining mode/editing/performance qualification belong to
[the native backend](PRD_02_34_windows_console_backend.md).

The common screen/parser now preserves U+FFFD as visible text and in literal
full/diff projection. The display contract and callback migration note are
owned by [terminal state](PRD_02_24_con_terminal.md); native SDK evidence is in
PRD 02.34. It is one shared OS/ISA-neutral implementation, consumed by GUI
parsing and the owned service; native runtime qualification remains scoped.

## Accepted consolidation sequence

1. Keep common stream, text producers, cells/styles, damage and region operations
   behind the terminal namespace. The native service is a real consumer of this
   boundary. Native wire errors and structures do not become common types.
2. Define portable session/input/capability and shutdown contracts before moving
   existing PTY code. Retain one process-lifecycle authority and an acyclic
   dependency graph; no platform-to-console-to-platform dependency cycle.
3. Consolidate the existing macOS/Linux PTY implementation with native runtime
   evidence for each OS. Its current platform ownership is not changed by the
   common screen API. Renderers, product tabs and settings remain consumers.
4. Qualify external consumers and OS × ISA feature combinations separately.
   Reconsider a merger with minicon-platform only after these boundaries and
   actual consumers provide evidence; no merger is authorized by this plan.

The library name is retained: console denotes the integrated terminal domain,
not an assertion that every export is Windows-only. Common modules contain no
architecture-specific native ABI. Native modules remain explicitly gated; a
portable build is not a native runtime guarantee. Shared region semantics belong
to [terminal state](PRD_02_24_con_terminal.md), with native compatibility evidence
in [the Windows backend](PRD_02_34_windows_console_backend.md).

Shared cursor appearance restoration is now carried by the same exposed Screen
full/diff/cursor formatting methods. Its common contract and replay migration
note belong to [terminal state](PRD_02_24_con_terminal.md); Windows percentage
metadata remains native and is not part of the portable appearance vocabulary.
The scoped service consumer and actual property clients are recorded in PRD 02.34.

## Viewport rendering facade

terminal::TerminalViewport describes a signed source region and an independently
sized output surface. The default-callback TerminalStream can write_output_viewport
through exact lexer/screen forks, forward complete nonvisual/extension frames,
project cells/modes and commit after sink success. Alternate-grid transitions
forward their original control frame followed by a full projected image. The
native host is its actual consumer after a native geometry change; ordinary
sessions keep raw framed output. Custom callback streams retain raw operation;
the new default-callback mapped path does not pretend to transact arbitrary
external callback side effects.

commit_viewport_screen and write_viewport_projection provide staged cell edits
and independent-decoder native commands without resetting unfinished client
input. Entering viewport mode neutralizes physical origin/margins and publishes
a complete image. Viewport operation continues until session close, including
when source/output dimensions later match. Each projection is transient and
read-only; native readback uses the authoritative drawing buffer.

rust_console_terminal_viewport_preserves_buffer_grammar_effects_and_commit
covers cropped rendering, native commands between UTF-8 fragments, C0 effects
inside CSI, title forwarding, alternate-grid transitions and failed/budgeted
publication. Input bytes and projection output have distinct caller budgets.
Non-ASCII/control-heavy large-buffer performance, richer callback consumers and
portable session/process composition remain separate qualification work.

The service exercises this public boundary through native geometry and expanded
buffer rendering, not a Windows-only duplicate. Shared/native SDK and real-client
positive runs, runtime guard failures, exact-source identities and the distinction
between cross-target compilation and native execution are recorded by
[native geometry qualification](PRD_02_34_windows_console_backend.md#accepted-native-geometry-leaf).
Portable session composition and independent external consumers remain planned
in the consolidation sequence above.

Native multiple-buffer support composes independent instances of the same
TerminalStream and shared viewport renderer. Buffer identities, Win32 handle
sharing/closure, cursor percentages and native output-mode metadata remain
Windows-private; there is one parser/screen authority for each logical buffer,
not a second native cell model. Input/Job ownership is not duplicated per buffer.
The contracts, measured compatibility and remaining lifecycle scope belong to
[native buffers](PRD_02_34_windows_console_backend.md#accepted-multiple-buffer-leaf).

The public terminal facade now exposes text_prefix and TEXT_BATCH_BYTES for
bounded encoded producers. Native UTF-16/UTF-8 text output is an actual consumer
in both raw and mapped sessions; Windows newline/margin/code-page policy remains
in its adapter. Low-level lexer, publication and scalar-width contracts belong
to [terminal state](PRD_02_24_con_terminal.md#bounded-unicode-text-publication).
Native bulk clients, system-layout differences and remaining compatibility scope
belong to [Unicode publication](PRD_02_34_windows_console_backend.md#accepted-unicode-publication-leaf).
