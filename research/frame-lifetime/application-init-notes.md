# sharedApplication: one bounded sampled initialization

No production changes or private overrides. Existing external report and
previous probes remain frozen. The new script copies the fine-grained native
stage source, adds a stdin handshake immediately before sharedApplication,
starts `sample PID 3 1`, then releases the handshake after sampling confirms
it has started. The complete default native lifecycle runs afterward.

The native process and sampler both exited zero. All processes were reaped.
Raw source, exact binary identity, page/ledger receipts, stdout and sampled
stacks are in `target/frame-lifetime/application-init/`. Reproduce in an
assigned GUI window with `python3 research/frame-lifetime/sample-application-init.py`.

Direct external ledger increased from 6.750 MiB observer-warm to 20.797 MiB
after sharedApplication: +14.047 MiB. The prior uninstrumented detailed run
was 6.750→21.016 (+14.266 MiB). This is broadly the same phase cost, not a
promise of exact reproducibility under a debugger observer or concurrent
asynchronous initialization.

There are 49 main-thread samples under sharedApplication in the captured
stack (sample.txt around lines 855–1003):

- 37 descend through NSApplication init, _currentAppIsViewService,
  _NSSoftLinkingLoadFramework and dlopen. Of those, 35 wait on the dyld loader
  lock; two descend into image/debugger notification. The framework argument
  is not captured, so do not claim its pathname from this stack alone.
- Eight descend through _registerForAppearanceNotifications,
  NSSystemAppearanceProxy and theme/appearance setup, including CoreUI asset
  storage and system accent/hardware color lookup.
- Remaining samples show application/window-server registration, event-system
  setup, display configuration and concurrent-event processing.

A simultaneous utility queue shows 40 samples in the asynchronous
_registerApplicationWithUIIntelligence block: 35 through the AppKit
LNProcessInstanceRegistryClient class soft-load and five through registration
and AppIntents loading. The full stack also shows AppIntents
LNProcessInstanceRegistryClient XPC setup and LinkServices. These are candidate
initialization owners, not byte attribution. The sample includes dyld
synchronous debugger notifications, which prolong loads and affect overlap;
do not convert sample proportions into memory percentages or normal latency.

AppKit UUID: CF57A4FC-4BE3-3D95-B543-D744E8718B26, base 0x18e4b7000.
sharedApplication PC: 0x18e4bcd00. _currentAppIsViewService PC: 0x18ecd7248.
UIIntelligence registration block PC: 0x18eccf520 / 0x18eccf544.
These coordinates let a reviewer identify the exact current image without
requiring runtime symbolization inside the measured application.

The next causal discriminator is to observe which framework each of the two
soft-load owners requests, then isolate its before/after external ledger while
preserving the other owner. Global dlopen interposition can miss direct dyld
shared-cache bindings; success must first be calibrated, and absence of an
interposer call does not exclude the path. No new private suppression is
implemented or qualified by this observation. View-service classification and
UIIntelligence registration have different semantics and must not be treated
as interchangeable optional features.
