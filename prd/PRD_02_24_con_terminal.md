# `minicon` terminal and rendering

Parent: [Lightweight terminal host (`minicon`)](PRD_02_23_minicon.md)

This module owns the standalone host's PTY ownership, VT parsing, damage,
rasterization, native present, glyph and ISA behavior. Shared kernels stay in
[terminal runtime](PRD_02_01_terminal_runtime.md) and
[native platform](PRD_02_20_native_platform.md); this module owns only what con
decides for itself.

Legend: `[x]` shipped, `[~]` partial, `[ ]` planned.

## Session ownership

- [x] each tree tab owns an independent PTY, parser, viewport and failure state
  in a single GUI process. It is explicitly not a mux, persistent workspace,
  Fleet authority or script runtime.
- [x] PTY delivery uses a platform-owned fixed 1 MiB byte ring per session
  instead of allocating a `Vec` for every native read. Each read commits
  atomically or waits for capacity; close wakes blocked producers while
  preserving committed tail bytes for draining.
- [x] parsing remains bounded to 128 KiB per GUI turn. Reader wakes are
  coalesced, inactive tabs are drained without forcing unrelated active-tab
  paints, and remaining backlog yields to input before self-scheduling another
  turn.
- [x] the Windows production graph no longer contains `rmux-pty`. The direct
  adapter owns synchronous ConPTY endpoints, a cancellable overlapped writer, a
  drain-safe output pump, build-gated passthrough fallback, PowerShell DSR
  fragments, suspended process creation, exact wait/exit status, and a
  `KILL_ON_JOB_CLOSE` Job Object. It never resumes an unassigned child and never
  reuses a ConPTY whose failed first child may have closed its output pump.
- [x] the primary thread handle from suspended `CreateProcessW` is owned only
  until `ResumeThread` and PID validation succeed, then closed immediately
  through `OwnedHandle` Drop. Failure before successful resume remains armed to
  terminate the partial child; Job assignment-before-resume and independent
  process, Job and HPCON ownership are unchanged.
- [x] Windows process termination keeps its native wait leaf allocation-free:
  the 500 ms `WaitForSingleObject` result and immediately captured
  `GetLastError` become `Exited`, `Running`, or `Failed(code)`, and only the
  explicit terminate caller constructs a typed error.
- [x] the child waiter publishes one completion bit directly rather than
  instantiating a production channel for a `()` value. Release stores the state
  before the existing window wake; the GUI consumes it once with an
  acquire-release swap, keeping process-exit authority independent from ConPTY
  pipe EOF.
- [x] Windows ConPTY environment inheritance calls `GetEnvironmentStringsW`
  directly and frees that borrowed block through one adapter-owned RAII guard. A
  bounded UTF-16 scan and ordered streaming merge preserve hidden `=C:` drive
  entries, inherited variables, ASCII-case-insensitive last-override semantics,
  validation and the required double NUL before `CreateProcessW`.
- [x] Windows environment-block construction uses a PTY-private sorted vector
  with explicit binary insertion instead of the generic BTree node allocator,
  preserving normalized UTF-16 key order, case-insensitive last-write
  replacement, original override spelling and the exact double-NUL block.
- [x] Windows executable admission compares the exact `.exe`/`.com` PATHEXT leaf
  directly over native UTF-16 code units, including non-Unicode rejection.
  PATHEXT enumeration stays native end to end through a bounded four-unit
  streaming classifier that preserves extensionless-first lookup, configured
  order, duplicates, empty-list fallback and invalid-nonempty suppression while
  emitting only canonical `.EXE`/`.COM` candidates.
- [x] PTY teardown closes bounded output backpressure on the event thread, then
  transfers master/child ownership to platform-managed background teardown. That
  owner terminates the child tree, closes the pseudoconsole and releases both
  halves through one bounded platform reaper rather than creating a native
  thread per closed tab, without stalling UI or control progress. Reaper
  readiness is a prerequisite to native PTY creation, so thread-start failure
  rejects the new terminal before any potentially blocking native ownership
  exists. A black-box journey runs four concurrent million-line producers,
  closes every flooded tab within a strict per-tab deadline, verifies stable IDs
  leave the tree, and proves an unaffected sibling remains interactive after
  every close. The generic bounded-close rule for a tab and its workers stays
  with [terminal runtime](PRD_02_01_terminal_runtime.md).
- [x] a public multi-tab journey uses `wait-tab-exit` to prove one child can
  exit with a non-zero code while a sibling continues accepting input, the dead
  tab remains capturable but rejects further PTY input, parent promotion still
  holds, and closing the final retained tab ends the GUI cleanly.

## PTY backend selection

- [x] the pseudoconsole entry points are resolved at run time rather than
  imported. Every static import is resolved by the PE loader before `main`, so
  importing an API a supported Windows lacks does not degrade a feature — it
  refuses to start the program, naming a symbol the user cannot act on. This is
  a load-time property and belongs to [27](PRD_02_27_con_delivery.md); it is
  restated here only because it is what makes a second backend reachable.
- [x] the backend is selected by **capability, not by version number**. The
  system either exports the pseudoconsole entry points or it does not, and that
  is the whole decision. A build-number comparison would need revisiting the
  moment a redistributable or servicing update changed the floor. The build
  number is used only in the message a person reads.
- [x] where ConPTY is absent, a **console agent** stands in: the executable
  re-executes itself with an internal argument, that process takes a hidden
  console, spawns the child into it, and polls the screen buffer with
  `ReadConsoleOutputW`, synthesizing a terminal stream from what changed. The
  agent is this product, not a third-party binary — nothing extra is deployed.
- [x] the agent argument carries no product name. This adapter is shared with
  `agenterm`, so any product name in it would appear in some other product's
  command line, and a process list is public.
- [x] the difference is sealed below the session contract: both backends use the
  same pipes, the same output pump, the same command line and the same
  environment block, and differ only in who creates the child. Nothing above
  the adapter can tell which backend it got, and no terminal, chrome or control
  code branches on it.
- [x] the agent survives the interrupts it raises. `WriteConsoleInput` does not
  generate a console control event — the console synthesizes one only for real
  keyboard input — so Ctrl+C is delivered as `GenerateConsoleCtrlEvent` for the
  console's process group, with pending keystrokes flushed first so they arrive
  before the interrupt. The agent installs a handler that reports the event
  handled; without one it dies and its Job Object takes the shell with it.
  The inherited Ctrl+C-ignore flag is cleared, because that flag is inherited by
  children and would leave the shell permanently uninterruptible.
- [x] a resize is recorded by the control thread and applied by the polling
  thread. Resizing the console from another thread races the poll's
  `ReadConsoleOutputW`, whose rectangle is then larger than the buffer it reads;
  a single failed read is survivable and no longer ends the session.
- [~] the fallback is a screen-buffer mirror on a timer, not a pseudoconsole.
  Output latency has a floor set by the polling interval, and a program driving
  the console API in ways a screen buffer cannot express will not round-trip
  perfectly. Both are accepted costs of running where no pseudoconsole exists.

Evidence: seven journeys against a real forced-backend session — a shell starts
and paints, typed input reaches the child and its *computed* output returns, a
session survives resizes and stays usable, a wide character is not emitted twice
for its two cells, closing the host leaves no orphan on an invisible console,
Ctrl+C interrupts a running command and leaves the shell alive, and both sides
agree on the argument. `AGENTERM_FORCE_CONSOLE_AGENT=1` selects the fallback on
a modern system, without which this path would be reachable only on a machine
old enough to need it — which is neither CI nor any developer's.

Verified on a user's Windows Server 2016 (build 14393) on 2026-08-23.

## Scheduling

- [x] a native Wake services the bounded control queue first and shares one
  fixed PTY byte budget across all live tabs. Background output therefore cannot
  multiply one event's work by tab count or starve the command that stops it.

## Input delivery to the child

- [x] Windows native key delivery uses one platform `ConsoleGuard` to serialize
  `FreeConsole`/`AttachConsole`, retries the documented already-attached case,
  opens `CONIN$`, and writes an exact key-down/key-up `INPUT_RECORD` pair with
  `WriteConsoleInputW`. The real `cmd.exe` cursor journey and the
  alternate-screen `less` arrow/wheel journey are default black-box tests rather
  than ignored known gaps.
- [x] Windows physical Left/Right keycodes are normalized at the native pixel
  host boundary, and cursor-key PTY encoding follows live DECCKM state: normal
  mode emits CSI while application-cursor mode emits SS3. Missing native key
  normalization is an implementation defect, not a PRD permission to drop keys.
- [x] named terminal-key aliases are platform-owned rather than duplicated in
  con and the workbench. The shared parser is allocation-free and
  ASCII-case-insensitive; con rejects an unknown multi-character key while
  workbench UI injection still treats unsupported names as literal text.

## Damage, raster and present

- [x] vendored `vt100` emits allocation-free conservative row damage from
  mutation sites. Exact visible-cell comparison is its collision-free test
  oracle, while unknown callbacks, viewport changes, resize and alternate-screen
  transitions fail safely to full. PTY Wake drains this evidence before
  invalidation, so ordinary output requests only the affected terminal rows and
  the old/new cursor overlays instead of invalidating the complete client.
- [x] the pixel-window contract identifies retained versus transient host
  backing and requires each frame to commit `None`, `Full`, or a bounded partial
  rectangle. Windows rasterizes directly into the retained native XRGB buffer,
  forces full raster after allocation/resize/DPI invalidation, and has removed
  the former product-to-host full-frame copy; Unix/macOS retain the
  product-owned bounded frame and full-copy it into explicitly transient
  softbuffer frames.
- [x] Windows maps typed physical damage to `InvalidateRect` and uses
  `PAINTSTRUCT.rcPaint` for top-down `StretchDIBits` partial present. Every
  successful `BeginPaint` is paired with exactly one `EndPaint`, short scanline
  copies are rejected, and a renderer error is never presented. Unix catches
  application callback panics at the event-loop boundary and converts them to
  typed failure.
- [x] Windows retained pixel windows deliberately omit `CS_HREDRAW` and
  `CS_VREDRAW`; system expose, typed invalidation and settled geometry redraw
  are the only paint authorities.
- [x] the native ledger times `StretchDIBits` or softbuffer `present` itself
  without a GUI-thread lock, and reports OS expose pixels separately from
  product damage rather than relabeling expose as damage.
- [x] Win32 maps `WM_ENTERSIZEMOVE`/`WM_EXITSIZEMOVE` to shared optional
  interaction events. During that lifetime current metrics track the client
  while GDI scales the last complete retained DIB under the native paint clip;
  product generation, VT/PTY geometry and raster settle once on exit.
  Unix/macOS retain ordinary geometry plus trailing-edge debounce fallback.
- [x] when capture temporarily renders an inactive target, the platform receives
  an explicit discarded-frame receipt: neither retained nor transient hosts may
  present that scratch frame before the restored active tab redraws. Public
  performance evidence counts discarded capture frames, so the select/capture
  black-box journey proves the running host used that path exactly once.
- [x] the Win32 native host keeps `GWLP_USERDATA` behind a reentrancy-checked
  owner, defers callback-issued synchronous User32/IMM commands until
  application and framebuffer borrows end, snapshots stateful reentrant messages
  without retaining pointer-backed parameters, validates and reschedules nested
  paint, and fails closed on bounded-queue overflow or nonconvergence.

## Fonts and glyphs

- [x] the grid face is chosen by **measuring** candidates, not by taking the
  first name that resolves. `CreateFontW` never fails on a missing family: the
  GDI mapper substitutes the closest installed face and `FIXED_PITCH|FF_MODERN`
  is a scoring hint, not a constraint — so the family a build asks for and the
  face a machine renders with are different questions. Variable-pitch faces are
  refused; `i`, `W` and `A` must share an advance; a family whose full-width
  characters measure exactly two cells wins outright, because then one face
  covers both halves and the ratio is exact by construction.
- [x] cell width is the measured advance, not `tmAveCharWidth` — a font-wide
  average that equals the real advance only when the face is monospaced, which
  was the unchecked property. Glyph lookup starts at the selected family, or
  ASCII would be drawn from a face whose advance has nothing to do with the
  cell width.
- [x] the resolved face is never validated by name. `GetTextFaceW` returns the
  localized name, so a requested family and its own report of itself do not
  compare equal; measurement is the only test that works.
- [x] selection is memoized per size. `primary_metrics` is on the paint path —
  every chrome string asks for the metrics of its own size — and measuring per
  call turned one repaint into dozens of `CreateFontW` calls, which showed up as
  intermittent failures under rapid font-size change. A test pins the cost,
  since the absence of one is what let it through.
- [x] the invariant is asserted, not assumed: the selected face is monospaced at
  every size the product offers, and a full-width character occupies exactly two
  cells. Nothing tested this before, which is why a machine-dependent
  substitution could ship. Reported by `--status` as a measurement so a user can
  confirm it on their own machine — see [26](PRD_02_26_con_control_cli.md).

- [x] Windows glyph selection and gray8 coverage execute behind the platform
  `RasterGlyph` contract through bounded GDI calls with deterministic DC/font
  cleanup. Con no longer opens or parses font files; ab_glyph and ttf_parser are
  absent from its Windows production graph. Linux/macOS retain equivalent
  file-font behavior inside a shared platform portable adapter.
- [x] the Windows GDI leaf maps supplementary scalars through the selected
  face's bounded OpenType `cmap` format-12 table, then rasterizes the resulting
  glyph index through the existing bounded `GetGlyphOutlineW` path. BMP lookup
  remains on `GetGlyphIndicesW`; malformed, oversized or absent tables fail as
  a local missing glyph without splitting surrogate pairs. Color emoji,
  variation selectors and run shaping remain an explicit later DirectWrite
  tradeoff rather than a shipped claim.
- [x] Windows native glyph faces survive platform raster cache misses at one
  active pixel size: the adapter lazily creates only the GDI families reached by
  coverage, keeps each HDC/HFONT on its creating thread, and drops the whole RAII
  set when zoom selects another size, following the documented
  `CreateCompatibleDC(NULL)` thread-ownership rule rather than asserting an
  unsafe cross-thread `Send`. A deterministic 94-printable-ASCII probe reduced
  native face creation from 94 sequences to one.
- [x] shared glyph caching stores its read-heavy bounded FIFO values in a sorted
  contiguous vector, preserving deterministic eviction without retaining
  randomized hashing.

## Architecture kernels

- [x] x86_64 ISA dispatch has one shared authority rather than independent
  standard-library and dependency detectors. Vendored vt100 disables VTE's
  std-only memchr runtime dispatch (VTE parser semantics unchanged, ESC scans
  retain mandatory x86_64 SSE2); UI-core caches pixel blend and RGB pack
  function pointers from one CPUID probe, using bounded `xgetbv` inline assembly
  before any AVX2 call to require OS-managed XMM/YMM state. The probe agrees
  with the standard oracle in tests.
- [x] shared x86 pixel dispatch gives blend and RGB8 pack independent lazy ISA
  selectors, so the standalone host — which consumes only blend — drops the
  unused SSSE3 pack kernel, scalar pack kernel and SSSE3 selector to zero bytes
  in final-link evidence while retaining AVX2/SSE2 blend.
- [x] native geometry does not delegate basic IEEE-754 rounding to CRT math
  imports: a shared product-neutral platform leaf implements exact bit-level
  `round_f32`, `round_f64`, `ceil_f32` and `trunc_f32`, consumed by con UI,
  font, wheel and the Windows pixel host, with standard-library oracle tests over
  boundary values and sampled bit patterns.

## Rendering performance evidence

Frame, damage and present counters are exposed through `perf-stats` /
`reset-perf-stats` ([26](PRD_02_26_con_control_cli.md)): conservative
full/partial raster-candidate frames, dirty and frame pixels, platform-owned
native-present count, latency, requested/completed pixels, and host direct/copy
frame and pixel counts.

- [x] the 16-step public resize journey dropped from 35 to 18 frames, 17 to 8
  full candidates, 9,632,800 to 6,485,040 dirty pixels, and 25.715 ms to
  16.302 ms native present time with unchanged PNG geometry, zero present
  failures and zero host copies. After the size-move interaction contract the
  same journey needs only 3-5 product frames and 1-2 full candidates, with 11-13
  successful platform presents, no failures or copies, and at least a 10x
  dirty-pixel reduction in the best observed run. The frontend now aggregates
  resize-only arrivals for a fixed 4 ms window from the first request and
  submits the final geometry once; any following non-resize command is an order
  barrier, and every caller still receives its own result. Three repeated
  exact-profile journeys stayed within the six-frame ceiling while acknowledging
  all 18 requests.
- [x] a paired Windows release probe measured seven idle partial frames at
  895 us average before direct backing versus eight at 360 us after it
  (59.8% lower); a 50-step send/wait journey measured 244 frames at 1,310 us
  average versus 250 at 992 us (24.3% lower), with all 250 new frames direct and
  zero copied frames or pixels.
- [x] a post-row-damage Windows release probe produced 33/33 partial raster
  candidates with 70,560 dirty pixels over 17,476,272 frame pixels (about
  0.40%), 33/33 successful native presents, a 1,589,501-byte PNG and captured
  command text before explicit `@1` close. The native ledger separately reported
  529,584 full-expose pixels and 70,560 partial pixels.
- [x] a public 100-title OSC stress completed 883/883 direct frames with zero
  host copies and zero present failures while producing a valid 1,589,501-byte
  screenshot.
- Historical only: pre-row-damage probes observed 2/5 partial candidates at
  about 60.0% dirty pixels for blink/idle, falling to 2/13 and about 84.6% after
  mixed PTY output. These are directional evidence, not release qualification.
- [x] con's dedicated public sustained-output qualification delivers a fixed
  31.9 MiB payload at no less than 2 MiB/s with a 30-second hard deadline while
  a sibling tab responds within five seconds and repeated `list-tabs` /
  `perf-stats` observations stay below two seconds. Its receipt requires the
  complete PTY byte count, at least one bounded-budget yield, zero native-present
  failures, zero host copies and clean `close-window` shutdown. This is distinct
  from the OSC and close-under-flood robustness journeys.
