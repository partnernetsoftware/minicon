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
   build is a baseline, not a rewritten backend. Initial Rust proof receives
   CONNECT from unchanged cmd and rejects it explicitly; accepting the connection
   and executing commands remain unimplemented. Reference-selection and native
   handle-inheritance mutations must fail the corresponding runtime tests.
4. [_] Produce the API/dependency inventory and differential fixtures. Use REA
   on an immutable copy of the exact system binary only for named gaps that
   source and behavior do not resolve; preserve evidence and uncertainty.
5. [_] Implement one bounded replacement slice at a time behind the existing
   Windows adapter: lifecycle/transport, input and modes, screen/output, resize
   and remaining required API semantics. Keep unsupported cases visible.
6. [_] Qualify real shells/agent clients, cancellation/crash races and both
   Windows ISAs independently. Compare CPU/RSS including all helper processes,
   throughput, input latency and cleanup. Retain behavior-changing negatives.
7. [_] Resolve single-file delivery and native ABI boundaries, then qualify
   exact packaged artifacts before any separate default-promotion decision.

Each implementation step records its invariant, observed result, failure and
dependency in the owning PRD. Avoid global system-host replacement, live-session
experiments, process renaming as evidence, blanket fallback and early crate
extraction. Existing 0.2.16 test improvements remain independently scoped.
