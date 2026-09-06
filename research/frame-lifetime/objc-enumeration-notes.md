# Objective-C class enumeration: semantic and hook review

Read-only source review for the macOS finishLaunching external-residency
investigation. No GUI ran and no production file changed. Published objc4
source explains a mechanism; it is not claimed to match the installed runtime
byte for byte. API-call receipts still determine whether this mechanism occurs
in MiniCon's actual launch.

## Confirmed mechanism

Both `objc_getClassList` and `objc_copyClassList` acquire `runtimeLock` and call
`realizeAllClasses()` before producing their result. Therefore even
`objc_getClassList(NULL, 0)` is not a harmless count-only warm-up. Enumeration
covers classes in known loaded images, not every image that merely exists in
the shared cache. Subsequent calls can reuse prior realization.

`realizeAllClassesInImage` walks class lists and Swift stubs. Ordinary
realization reads class read-only metadata, allocates writable class data,
changes class/cache state, and recursively realizes superclass/metaclass.
Swift metadata initializers can run with the runtime lock released and call
back into libobjc. Realization is not synonymous with sending every class its
Objective-C `+initialize` message.

These paths can touch clean shared-cache metadata and private writable state.
They support the hypothesis, but do not establish the measured 22.594 MiB
increase's owner before interception supplies actual before/after receipts.
`objc_copyRealizedClassList` is a different operation; substituting it would
change enumeration semantics and is not an approved optimization.

Source: [Apple objc4 runtime, enumeration](https://github.com/apple-oss-distributions/objc4/blob/fb265098298302243cd7eeaa1f63f0ba7786dd9a/runtime/objc-runtime-new.mm#L5862),
[realizeAllClassesInImage](https://github.com/apple-oss-distributions/objc4/blob/fb265098298302243cd7eeaa1f63f0ba7786dd9a/runtime/objc-runtime-new.mm#L3321),
[ordinary class realization](https://github.com/apple-oss-distributions/objc4/blob/fb265098298302243cd7eeaa1f63f0ba7786dd9a/runtime/objc-runtime-new.mm#L2953).

## Exact API and interception boundaries

The local SDK's `objc/runtime.h` declares `int objc_getClassList(Class *, int)`
and `Class *objc_copyClassList(unsigned int *)`. The get API returns total
class count even when output capacity is smaller; copy returns an allocated,
nil-terminated array for the caller to free. NULL output/count arguments remain
valid. Preserve the original pointer, buffer, count, return value and ownership;
never pre-call enumeration to size diagnostic buffers.

The local `usr/lib/libobjc.tbd` also exports `objc_copyRealizedClassList`,
`objc_copyClassesForImage`, `objc_enumerateClasses`, and internal enumeration
symbols. Missing hits on the two public APIs cannot rule out another entry
point or runtime-internal calls that bypass an interposed binding. The SDK
public `objc_enumerateClasses` also takes caller-relative image semantics when
its image argument is NULL; indiscriminately wrapping that variant can itself
change behavior by changing the apparent caller image.

Apple's [DYLD_INTERPOSE header example](https://github.com/apple-oss-distributions/dyld/blob/main/include/mach-o/dyld-interposing.h)
uses a differently named replacement and calls the original API directly.
Its current macro also includes pointer-authentication annotations. Use the
correct ABI and supported interpose layout; do not create ad hoc arm64e
trampolines or assume every process/image binding can be intercepted.

## Return-preserving diagnostic design

These are research recommendations, not a product patch:

- Resolve/validate any required original symbol before collecting the timed
  launch interval. A first-call `dlsym`/`dlopen` can add loader work and recursive
  entry; `RTLD_DEFAULT` may resolve back to the replacement. The official
  interpose pattern avoids lazy hook-time symbol resolution. Always exercise
  the hook on a known direct call before interpreting a missing launch hit.
- Use a thread-local recursion depth and fixed-size records. Only the outer
  hook takes before/after receipts; nested calls still forward unchanged.
  Do not hold a global diagnostic mutex across the original runtime call:
  Swift metadata initialization can re-enter runtime functions.
- Preserve errno around diagnostics: save entry errno, restore it immediately
  before the original call, save the original call's exit errno, and restore
  that exit value after recording. Never alter the API arguments or return.
- Record raw caller PC, thread ID, API identity, returned count where present,
  and TASK_VM_INFO before/after. Do not walk returned Class pointers, call
  `class_getName`, invoke Objective-C logging/formatting, or symbolize in the
  timed hook; those can touch precisely the metadata under investigation.
- A raw immediate return PC is the least intrusive caller clue. Save the
  process's image load addresses outside the measured interval and symbolize
  offline with atos. If a deeper stack is needed, collect a separate
  instrumented run with a bounded raw-address backtrace and warmed unwinder;
  do not mix that larger observer's RSS with the minimal-hook result.
- Keep loaded-only and active-hook controls. Log errors through a preopened FD
  and bounded write buffer; avoid recursive allocation during error handling.
  Kernel ledger deltas around one call remain process-wide and may include
  other threads, so require repeated timing/stack agreement before ownership
  attribution. RSS/external are not replaced by footprint.

No enumeration is skipped, filtered, cached across calls, or replaced by a
realized-only list. If the caller proves to be an OS service, the next question
is why the application starts that service, not how to falsify its class list.
