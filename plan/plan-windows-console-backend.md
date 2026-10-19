# minicon-console: Rust backend implementation sequence

Owner: [PRD 02.34](../prd/PRD_02_34_windows_console_backend.md). This plan
sequences that contract without assigning a release or changing current defaults.

1. [v] Locate official REA release and public C++ host sources, pin identities,
   read native-analysis limitations and map them to the existing PTY boundary.
2. [v] Map the initial native protocol and bootstrap structures into Rust in
   `minicon-console`, retaining source provenance and notices. Inventory build
   and provider prerequisites; C++ reference builds are optional comparison
   tools rather than a prerequisite for every Rust implementation step.
3. [-] Prove selected-host bootstrap with a minimal unchanged Windows console
   client. Capture actual executable/hash, API calls and lifecycle. Intentionally
   disable selection and require the identity test to fail. An upstream host
   build is a baseline, not a rewritten backend. Rust now accepts unchanged cmd,
   executes built-in ASCII/Unicode echo and preserves its nonzero exit status.
   The original explicit-rejection case remains a regression. Reference-selection and native
   handle-inheritance mutations must fail the corresponding runtime tests.
4. [_] Produce the API/dependency inventory and differential fixtures. Use REA
   on an immutable copy of the exact system binary only for named gaps that
   source and behavior do not resolve; preserve evidence and uncertainty.
5. [-] Implement one bounded replacement slice at a time behind the existing
   Windows adapter: lifecycle/transport, input and modes, screen/output, resize
   and remaining required API semantics. Keep unsupported cases visible.
   The explicit experimental PTY path now reuses existing transport/contracts;
   native resize/key probes and output-capacity shutdown cover its first slice.
6. [-] Initial native Windows x86_64 cmd/Unicode/session-isolation, deferred
   input, independent Win32 records and real job-tree teardown cases pass.
   Scoped PowerShell 5/7, Node raw TTY/readline and system-ConPTY object/access
   comparisons now extend the explicit experimental adapter evidence.
   Scoped inherited/restricted stdio, creator FreeConsole/exit and shared input
   tails distinguish connection defaults from kernel file object lifetimes.
   Private-Job Ctrl+C admission, actual native handler/interrupted-read/default
   exit receipts and system-ConPTY comparisons extend the scoped signal slice;
   Attached-group native C/Break generation, standalone full-queue interruption
   and native handler API reentry extend that slice. Broader keyboard/transport
   qualification and unattached-group compatibility remain open.
   The Rust native-key facade now delivers Break in raw/cooked modes; private
   live-file readback distinguishes flushed ANSI data from retained completed
   UTF-16 text and native-ready reset. Other key transports remain unqualified.
   Input staging now uses a bounded Rust queue behind the same PTY writer;
   mixed processed C can bypass input pressure without changing raw byte order,
   and Break invalidates in-flight old writes. The owner PRD records native
   process fixtures and behavior-changing negatives for this scoped slice.
   Output staging is now bounded and nonblocking for the API actor; the core
   retains ordered native requests so private controls/queries stay live under
   output pressure. Native capacity, deferred-read interruption and root-exit
   tail fixtures extend that slice. Private Job members which never attach keep
   EOF pending; native input closes while accepted output can still be drained.
   Broad simultaneous API/mode/screen behavior still needs compatibility evidence.
   Final ConDrv cancellation now waits briefly for the private Job to settle
   empty; actual delayed/live-member fixtures guard that EOF classification.
   Qualify further real shells/agent clients, cancellation/crash races and both
   Windows ISAs independently. Compare CPU/RSS including all helper processes,
   throughput, input latency and cleanup. Retain behavior-changing negatives.
7. [-] Resolve single-file delivery and native ABI boundaries, then qualify
   exact packaged artifacts before any separate default-promotion decision.
   The experimental service runs in-process; shipping selection and packaged
   artifact evidence remain unqualified.

Each implementation step records its invariant, observed result, failure and
dependency in the owning PRD. Avoid global system-host replacement, live-session
experiments, process renaming as evidence, blanket fallback and early crate
extraction. Existing 0.2.16 test improvements remain independently scoped.
