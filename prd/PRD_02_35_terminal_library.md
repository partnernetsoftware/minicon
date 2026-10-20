# Reusable terminal library: minicon-console

Parent: [product index](../PRD.md). Accepted direction: keep `minicon-console`
as an independent terminal-specific library, with reusable common mechanisms
and explicit native adapters. A possible later merge into `minicon-platform`
is a separate design decision, not a prerequisite or a current migration.
Implementation sequencing belongs to
[the terminal-library plan](../plan/plan-terminal-library.md); Windows service
compatibility continues in its own backend plan.
Modern TUI protocol and presentation acceptance belongs to
[PRD 24.01](PRD_02_24_01_tui_compatibility.md); library reuse alone does not
certify an interactive application.

## Outcome and delivery tree

```text
Outcome: another application can embed our terminal without MiniCon product state
├── [-] Common terminal state and streaming facade — terminal::TerminalStream / Utf16Decoder
│   invariant: one owned parser/framer pair; no OS handles or ISA assumptions
│   evidence: public facade unit, shared parser/framer units, target build receipts
│   safe failure: bounded-frame error or explicit I/O failure; no silent truncation
│   dependency: owned vt100/vte; non-goal: GUI/event loop or process spawning
│   └── bounded encoded runs / read-only lexer text_prefix / shared scalar width
│       └── invalid UTF-8 prefixes recover before native edits; new grammar stays pending
│       └── absolute drawing-cursor edits preserve client modes and pending grammar
│   └── [v] shared CSI query replies from live screen state; no transport ownership
│       invariant: one bounded reply per recognized query; unknown capability is not advertised
│       evidence: common mode oracle, actual GUI callback and native child round trip
│       safe failure: malformed/non-query metadata produces no reply; caller owns I/O failure
│       dependency: shared validated grammar; non-goal: presenting synchronized frames
│   └── [v] bounded status-response tracking and CPR/key ambiguity window
│       invariant: only committed outgoing CPR queries admit ambiguous replies; state expires
│       evidence: split grammar, numeric bounds, timeout/quota, native DA/DSR/CPR and F3 records
│       safe failure: explicit tracking quota failure; no permanent CPR key interception
│       dependency: owned lexer + caller clock/transport; non-goal: distinguishing simultaneous identical keys/reports
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
│   └── explicit Fixed / Vertical / Cursor policy; bool callers remain compatible
│       └── displayed-stream activation / logical layer / portable projection quota
│       └── independent bounded title publication; native metadata stays in adapter
│       └── hidden stream publication: recognized effects without painting another stream
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
│   └── [_] terminal identity/capability profiles, consumer-selected TERM and independent native provider
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
    APP --> QUERY["shared csi_query_reply / caller-owned reply transport"]
    QUERY --> VT
    ACCEPT["accepted outgoing bytes / caller monotonic clock"] --> RESP["TerminalResponses: owned lexer / finite CPR expectations"]
    RESP --> INPUT["classify report / accept only after native input succeeds"]
    VT --> VTE["owned vte: authoritative VT / UTF-8 grammar"]
    APP --> PLATFORM["minicon-platform::pty"]
    PROFILE["planned consumer-selected identity / capability profiles"] -.-> COMMON
    APP -. selection .-> PROFILE
    PLATFORM -->|"rust-console opt-in"| WIN["minicon-console Windows service"]
    VP["TerminalViewport / mapped publication / staged screen edits"] --> COMMON
    FOLLOW["ViewportFollow: Fixed / Vertical / Cursor"] --> VP
    ACT["write_viewport_activation / ProjectionLimit"] --> VP
    HIDDEN["write_output_viewport_hidden / committed title events"] --> COMMON
    TEXT["text_prefix / TEXT_BATCH_BYTES / scalar placement policy"] --> COMMON
    WIN --> VP
    MOVE["shared region movement / cell invariants"] --> COMMON
    WIN --> MOVE
    WIN --> COMMON
    WIN --> KERNEL["Windows ConDrv"]
    PLATFORM --> POSIX["macOS / Linux native PTY adapters"]
    COMMON & WIN & POSIX --> E["separate OS × ISA build / runtime / consumer evidence"]
```

## Shared terminal query replies

`terminal::csi_query_reply` consumes validated CSI callback metadata plus a
borrowed live `Screen`. It returns one optional reply of at most 16 bytes,
without mutating terminal state, owning a queue, writing a PTY or depending on
an OS/ISA. The GUI now directly depends on `minicon-console` and uses this
helper for its existing DA1, status and cursor reports as well as private
DECRQM. Native Windows hosting remains an explicit separate provider choice.
The parser ignores CSI marked invalid by its bounded parameter/intermediate
inventory; truncated metadata must not execute an abbreviated command or query.
Numeric parameter normalization remains the shared lexer's existing policy.

Single scalar DECRQM (`CSI ? Ps $ p`) uses the
[xterm control sequence contract](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html):
status 1 means set, 2 reset and 0 unrecognized. The helper reports live modes
1, 6, 7, 9, 25, 47, 1049, 1000, 1002, 1003, 1005, 1006 and 2004. Origin
mode comes from the current grid, not the cursor position or a duplicate flag.
Mouse mode/encoding follows the shared screen's active protocol. This is a
qualified query subset, not complete xterm conformance. Mode 2026 remains
unrecognized because complete-frame presentation has not been implemented.
Unknown modes also report zero; unrelated, subparameter or multi-mode commands
are unanswered. Primary DA accepts only default/zero, and DSR/CPR accepts one
scalar 5/6. Consumers retain transport, queue and failure policy.

Named courts: `terminal_query_replies_follow_live_modes_and_reject_nonqueries`
checks literal replies and every listed mode's set/reset transitions;
`private_mode_queries_use_shared_truthful_replies_across_split_and_invalid_csi`
checks the actual GUI callback at every two-way byte split, malformed queries
and parser inventory overflow. Native return transport and its independent SDK
expectations belong to [PRD 34](PRD_02_34_windows_console_backend.md#private-mode-query-return-transport).
Gate/fault receipts and native-runtime limitations are recorded with that
return-transport court. User-selectable identity profiles remain
planned; this helper does not implement that settings/API surface.

## Status response grammar and query windows

`terminal::TerminalResponses` observes accepted outgoing bytes with the same
owned `vte` grammar as the screen. It owns no screen, PTY, clock or OS handle;
the caller supplies monotonic elapsed `Duration` values and a timeout. The
crate names the owned lexer by a direct path dependency so external consumers
cannot accidentally resolve incompatible grammar. `reply_length` classifies
one complete report without consuming state; `accept_reply` spends a CPR
expectation only after the input sink accepts that span. Backpressure/retry
must not spend it. Other bytes retain the caller's keyboard path.

The common grammar admits primary DA reports (`CSI ? ... c`), status OK
(`CSI 0 n`), the preceding private-mode reports and positive row/column CPR
(`CSI r ; c R`). Report scanning is bounded to 64 body bytes; scalar numeric
fields have at most five digits. Mode/attribute numbers fit u16 and CPR
coordinates allow 1..65536, matching the shared query helper's one-based
cursor conversion. Malformed/incomplete reports are not guessed at. The
preceding private-report classifier/unit moves from the Windows adapter into
this common owner without changing its named evidence identity.

[xterm's DSR description](https://invisible-island.net/xterm/ctlseqs/ctlseqs.html)
explicitly identifies the collision between CPR and modified F3 bytes. The
tracker therefore admits CPR only after a complete valid outgoing `CSI 6 n`,
for one response and only until that query's timeout. Each query has its own
deadline; inspecting a response does not renew it. At most 64 expectations
are retained. Overflow yields `ResponseTrackingError`, clears expectations
and permanently fails the observer; the caller must retire that transport.
A zero timeout disables CPR admission. This is finite contextual routing,
not a proof of identity: an identical physical key during the live response
window, a late reply crossing a newer query, nested consumers and correlation
with a specific process still need a richer ownership contract.

`terminal_responses_track_split_queries_acknowledgement_expiry_and_quota`
checks literal reports, split queries, no-response defaults, repeated peeks,
acknowledgement, independent deadlines, malformed requests/replies, limits
and irreversible overflow. Native clock, output admission, F3 identity and
owned-session retirement are qualified separately by
[PRD 34](PRD_02_34_windows_console_backend.md#status-response-routing-and-cpr-key-boundary).
Gate/fault receipts and composed-versus-single-suite limitations are recorded
with that native court. Synchronization/profile settings remain separate work.

## Dependency and public API boundary

Terminal type is a planned user/consumer choice, not the `vt100` crate name.
The common library should expose bounded identity and implemented-capability
contracts independently of the OS provider. A modern VT/xterm-style baseline
is the intended default; qualified compatibility profiles and an advanced TERM
override remain selectable. Changing TERM selects an application description;
it does not load a different parser or implement new protocols. Query replies
must reflect implemented capabilities and the applicable profile, even when an
override names a richer terminal. Selectable VT220/kitty/etc. emulation claims
need their own evidence. These contracts are planned, not existing public APIs.
Product settings and new-session application belong to
[workspace configuration](PRD_02_25_con_workspace.md#configuration).

The integration boundary is the terminal domain: applications should eventually
compose terminal state, input, a native session and output publication through
this library. Cross-ISA reuse and cross-OS adaptation are separate requirements;
neither changes the meaning of the common terminal API.

| Layer | Public responsibility | Current implementation and remaining work |
| --- | --- | --- |
| Common terminal mechanisms | State, streaming, Unicode, cells, damage and viewport publication | Implemented common facade; compatibility and evidence remain scoped below |
| Portable session contract | Input events, capabilities, size, I/O ownership, backpressure and close/EOF | Planned; define contracts before relocating PTY implementations |
| Native terminal adapters | OS handles, console/PTY protocols, process ownership and native errors | Windows service is experimental; macOS/Linux PTY implementations remain in platform pending consolidation |
| Internal ISA adaptation | Native ABI/layout or measured acceleration behind the same semantic contract | Keep target-specific details internal; qualify each OS/ISA combination separately |
| Application integration | Rendering, event loops, tabs and product policy consume terminal APIs | Remain outside the library; migrating a consumer requires its own evidence |

The intended dependency direction is application to terminal contracts to native
adapter. Existing platform PTY entry points remain compatibility consumers during
consolidation. Do not introduce a reverse library dependency or relocate unrelated
platform facilities to achieve a single terminal entry point.

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

The facade's shared Screen also exposes absolute drawing-cursor mutation;
its coordinate, delayed-wrap and damage contract belongs to
[terminal state](PRD_02_24_con_terminal.md#absolute-drawing-cursor).
The Windows cooked editor uses it to stage text/cursor publication while
retaining client grammar. Native editing policy and compatibility remain in
the Windows owner, rather than becoming portable session semantics by accident.

### Integration acceptance contract

The target is one independently consumable terminal library, not merely a
Windows service with portable helper exports. Keep the `minicon-console` name
while state/stream, session/input and native adapters converge. A future platform
merger needs a separate dependency, consumer and maintenance review after those
boundaries have actual consumers.

Before publishing a portable session interface, resolve these requirements with
both the existing Windows and POSIX implementations:

| Contract | Required observable behavior |
| --- | --- |
| Dimensions | Cell rows/columns have one meaning; optional pixel size is distinct; invalid or unsupported resize fails explicitly |
| Input | Text, paste, encoded bytes and native key/control capabilities are distinguishable; unsupported events cannot report success |
| Ownership | Exactly one owner controls the child/session lifetime; readers/writers have explicit concurrency and cancellation rules |
| Flow control | Input/output budgets, saturation behavior and shutdown bounds are stated; slow consumers cannot create unbounded queues |
| Completion | Natural child exit, output EOF, explicit close and forced termination remain distinguishable; accepted final output can be drained |
| Native differences | Provider identity, foreground control and cleanup capabilities remain queryable; Windows Job and POSIX process-group semantics are not conflated |
| Errors | Portable categories retain actionable native context without exposing native handles as common types |

Move mechanisms only after a working consumer demonstrates the contract. Reuse
the existing POSIX implementation for macOS and Linux; do not create a second
copy for the new crate. The platform facade may forward to console-owned
contracts/adapters, but the console library must never import platform to reuse
them. Migrate neutral dependencies first or keep the adapter in place until
that dependency can be resolved. General process-management policy stays with
its existing owner; session-specific authority must not gain a second owner.

Each delivered slice needs a direct library consumer without MiniCon GUI state,
positive behavior and an observed behavior-changing runtime negative. Record
source/features, OS, ISA and provider separately for build and execution.
Windows x86_64 evidence cannot certify Windows ARM64, and either Linux target's
compilation cannot certify macOS runtime. A common API is the reuse boundary;
these individual receipts determine its supported implementation scope.

### Independent title publication

`TerminalStream::write_window_title` publishes a complete OSC 0 title with ST
termination, independent of client grammar, callbacks and viewport projection.
It borrows the stream read-only and does not change screen cells, layers or
pending CSI/UTF-8. The caller owns title metadata and commits it only after
publication succeeds. Partial sink failure still requires retiring the transport.

Input is a Rust string without control characters; C0, DEL and C1 are rejected
before any output. A caller-owned encoded-byte limit includes the six framing
bytes and uses the common typed `ProjectionLimit` error. Native character
conversion and display policy stay in the adapter, not the portable formatter.
Client VT titles use the separate committed-event API described below. The
owned receiving parser now preserves title payload semicolons; its grammar and
framing contract belongs to [terminal state](PRD_02_24_con_terminal.md).

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
ViewportFollow is public common policy; the Windows adapter chooses vertical
movement for ordinary stream output and both axes for native cursor commands.
Its mechanics belong to [terminal state](PRD_02_24_con_terminal.md#explicit-viewport-follow-policy),
with native scrolling/viewport evidence owned by PRD 02.34.
Displayed-stream activation and typed projection-budget failures are common
mechanisms as well; Windows buffer IDs and native status translation stay in its
adapter. The boundary is owned by [terminal state](PRD_02_24_con_terminal.md#displayed-stream-activation-and-projection-budgets).
The contracts, measured compatibility and remaining lifecycle scope belong to
[native buffers](PRD_02_34_windows_console_backend.md#accepted-multiple-buffer-leaf).
Process-family retirement and the native connected-process query remain
Windows-private; their SDK/PTY lifetime evidence is owned by the same module's
[process-family leaf](PRD_02_34_windows_console_backend.md#accepted-buffer-process-family-lifetime-leaf).

The public terminal facade now exposes text_prefix and TEXT_BATCH_BYTES for
bounded encoded producers. Native UTF-16/UTF-8 text output is an actual consumer
in both raw and mapped sessions; Windows newline/margin/code-page policy remains
in its adapter. Low-level lexer, publication and scalar-width contracts belong
to [terminal state](PRD_02_24_con_terminal.md#bounded-unicode-text-publication).
Native bulk clients, system-layout differences and remaining compatibility scope
belong to [Unicode publication](PRD_02_34_windows_console_backend.md#accepted-unicode-publication-leaf).

Invalid UTF-8 prefix recovery also runs through the common facade in raw and
mapped sessions. Rejected prefixes use canonical U+FFFD before the new input;
valid frames retain their wire bytes. The mechanism and failure invariants belong
to [shared recovery](PRD_02_24_con_terminal.md#invalid-utf-8-prefix-recovery), with
native SDK interleaving/final-tail evidence owned by PRD 02.34.


Title publication is exercised by
`rust_console_terminal_title_publication_preserves_grammar_and_rejects_injection`.
Its actual native consumer is guarded by
`rust_console_native_title_commits_after_sink_and_retains_raw_metadata` and
`rust_console_pty_native_wide_titles_publish_independently_of_client_grammar`.
Runtime mutation, selected-suite and cross-target qualification for this leaf
are recorded by [the native owner](PRD_02_34_windows_console_backend.md#native-wide-title-publication-leaf).


## Committed title events and undisplayed streams

`TerminalStream::take_window_title_update` takes the latest committed OSC 0/2
window-title event without exposing mutable decoder state. It coalesces events;
callback consumers can observe each intermediate event. The facade enables
tracking under its frame budget; raw Parser consumers must opt in separately,
so unused event copies are not added to existing GUI parsers. Native metadata remains
caller-owned: consuming applications interpret the returned raw title bytes
(normally UTF-8) using their own validation policy. Drain the slot before
consuming `finish` if the final title is required without callbacks; the returned
Screen is drawing state. Low-level raw OSC, parser-event and framing semantics belong
to [terminal state](PRD_02_24_con_terminal.md#osc-payloads-parser-events-and-framed-commit).

The default-callback stream also supports `write_output_viewport_hidden` with
the same viewport, follow policy, pending and wire budgets as visible mapped
output. It commits the undisplayed stream's cells, modes, layers and grammar,
while forwarding only recognized nonvisual effects: titles/icons, bells and
clipboard operations. Drawing, alternate-layer transitions, resize requests and
unknown extensions do not reach the displayed stream. Native activation can
later publish the stored logical image. The API is not a general sanitizer for
unknown extension protocols or a transaction for external custom callbacks.

The publisher uses the same sink-before-parser/view transaction as visible
output. Rejected effect publication and projection quota leave the event and
view uncommitted. Pure hidden painting requires no transport write. Activation
is read-only and does not replay consumed metadata. Native Windows producers
are concrete consumers; the common implementation has no OS handles, code-page
policy or ISA branches. Portable process/session composition remains open.

Named common evidence is
`rust_console_terminal_title_events_commit_complete_frames_and_survive_screen_reset`,
`rust_console_terminal_hidden_output_preserves_effects_and_isolates_paint` and
`stream_boundary_raw_osc_preserves_payload_beyond_parameter_limit`. The native
owner records precise SDK, runtime-guard and cross-target scope. Clipboard,
custom callbacks and unknown extension behavior require their own broader
consumer qualification; the new hidden scenario specifically asserts titles,
bells, painting/layer isolation, quota and failed-sink transaction.

Committed title events also inherit shared OSC cancellation/termination rules;
no native-only abort filter is added to this facade. Callback migration and
UTF-8 ST bounds belong to
[terminal state](PRD_02_24_con_terminal.md#osc-cancellation-and-termination),
with actual Windows/reference evidence in PRD 02.34's termination leaf.
