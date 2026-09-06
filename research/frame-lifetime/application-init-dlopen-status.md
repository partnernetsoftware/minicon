# dlopen pathname observer: failed calibration and bounded recovery

The first bounded calibration timed out after 10 seconds before producing a
DLOPEN record. Python subprocess.run killed/reaped the calibration child on
TimeoutExpired. The native GUI was not started.

The first prototype combined __DATA,__interpose with dlsym(RTLD_NEXT, "dlopen")
to retain the real function. The calibration result does not establish that
RTLD_NEXT produced a callable original under dyld's interposition semantics.
A re-bound/self-referential function pointer is a candidate failure mechanism,
not a confirmed stack trace. An intermediate pointer-equality check against the replacement was not
rerun in that form; it would not detect an indirect trampoline cycle.

The earlier successful sample shows real dlopen paths. This calibration
failure is not evidence that no framework loads occurred, and yields no
pathname/ledger attribution. No production code or system settings changed.
The next native attempt required corrected symbol binding and calibration,
without a broad flat-namespace change that could alter all application
symbol resolution. That bounded recovery is recorded below.

## Second and final bounded attempt: calibrated, partial coverage

Following review, the observer was simplified to one mechanism:
DYLD_INTERPOSE names a wrapper which calls the original dlopen symbol directly
from the same image. The RTLD_NEXT resolution is removed. No flat-namespace
setting was added. This second calibration successfully observed the explicit
`/usr/lib/libSystem.B.dylib` call and returned its original result. Native then
ran once, reached READY with responder/context both true, and was terminated
and reaped by the bounded runner. No private feature suppression was applied.

The actual path behind the earlier main-thread soft load is now identified:

```text
sample: _currentAppIsViewService -> _NSSoftLinkingLoadFramework (PC 0x18b8e2674)
interposed call at same caller PC:
/System/Library/PrivateFrameworks/ViewBridge.framework/Versions/A/ViewBridge
```

That call returned success. Its process external ledger rose
15,515,648→21,184,512 bytes (14.797→20.203 MiB), a **5.40625 MiB call-span
increment**. TASK_VM_INFO succeeded both times. The observer's active-call
counter was zero at entry and zero after exit, so no other observed outer
public dlopen call overlapped. It does **not** prove the absence of concurrent
loading through other entry points or nested work suppressed by TLS guards.

Native sharedApplication as a whole increased external from 6.750 to
21.016 MiB (+14.266), matching the earlier uninstrumented stage delta.
The observer's warm check itself remains at 6.750 MiB. This does not imply
zero observer impact in every metric or prove ViewBridge owns every page
within its call-span increment.

Coverage limitation is demonstrated, not hypothetical: the known
WritingToolsUI soft load and the UIIntelligence/AppIntents soft load were not
recorded by this public dlopen interposer, while all of ViewBridge, AppIntents,
UIIntelligenceSupport and WritingToolsUI appear in the final vmmap. The earlier
sample placed those soft loads through `_sl_dlopen -> dlopen_from`, so public
dlopen interception does not cover that path on this system. Consequently we
cannot report the UIIntelligence loader's requested pathname or exclusive
ledger contribution from this tool. We do know the sampled client symbols
belong to AppIntents, independently of the missing public API hit.

The result narrows the main-thread owner to ViewBridge, but remains incomplete
for concurrent UIIntelligence loading. It is not a feature-removal proposal.
The allocated one extra calibration and native run are complete; no broader
tracer or production patch was added. Exact hashes, call records, stage
ledgers and mapped-presence summary are in
`research/frame-lifetime/application-init-dlopen-results.json`; raw receipts
are under `target/frame-lifetime/application-init-dlopen/`. The GUI is free.
