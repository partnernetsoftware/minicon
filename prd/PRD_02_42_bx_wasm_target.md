# `bx` WebAssembly target (`.wasm`)

Owner of the experimental `.wasm` backend and its host ABI. The goal is to
replace the private `.bxb` container as the long-term execution format; `.bxb`
stays as the legacy path until parity is proven.

## Product outcome

bx source compiles to a **standard WebAssembly core module** that any engine can
run. bx itself executes those modules with its own decoder and interpreter, so
the product keeps its "one native binary, no runtime install" shape. The same
module should run in a browser once the Web ABI lands.

## Decision

- Encoder, decoder and interpreter are **in-tree** (`bx/src/wasm.rs`,
  `bx/src/wasm_target.rs`). `cargo tree` shows only `bx`; no third-party wasm
  crate is linked, so the zero-dependency claim still holds.
- The emitted artifact is a standard module (`\0asm`, version 1), not a
  bx-private container. Provenance moves to a sidecar/metadata only if a later
  milestone needs it.
- `.bxb` is now the **legacy** path. The default flip, its parity gates and the
  `.bxb` retirement are owned by
  [PRD_02_43_bx_wasm_migration.md](PRD_02_43_bx_wasm_migration.md); this module
  only supplies the backend and its feature evidence.
- Host imports are **web-shaped**, not WASI: the ABI mirrors browser surface
  names so one module can serve both native and browser hosts.

## Tree DAG

```text
bx wasm target
├── emitter (AST → core wasm)
│   ├── [v] numbers: ints and floats (`+ - * / %`, mixed promotion, VM-identical formatting)
│   ├── [v] strings: literals, runtime `+`, comparisons (`env.str_compare`),
│   │   data segments, `print_utf8`
│   ├── [v] arrays of ints: `[..]`, `arr[i]`, `len(arr)`, `arr.push(x)` growth
│   │   (allocate-and-copy over the module heap)
│   ├── [v] records: `{a: 1}`, `o.a`, `for k, v in o` (string key / int value,
│   │   declaration order via a per-shape key table)
│   ├── [v] string methods: `.len()`, `.contains(s)`, `.starts_with(s)`,
│   │   `.ends_with(s)` (host `env.str_test`)
│   ├── [v] runtime strings: bindings (ptr,len globals), `s + t`, `len(s)`
│   ├── [v] fixed-layout records `{a: 1, b: 2}` with `o.a` field loads
│   ├── [v] control flow: `if/else`, `while`, `for`, `for v in` / `for k, v in`
│   │   over arrays and maps, `break`/`continue`
│   ├── [v] bool values: `true`/`false`, comparisons as expressions, `&&`/`||`/`!`
│   │   (short-circuit), ternary, bool globals/params/returns, `str(bool)`,
│   │   `print(bool)`
│   ├── [v] user functions: params, int locals, `return <int>`, calls, recursion
│   ├── [v] functions may read/write string, array and record globals
│   └── [-] closures, non-int params/locals, nested functions
├── runtime (standard module execution, own code)
│   ├── [v] decode + validate (type/import/function/global/export/code)
│   ├── [v] interpreter: consts, globals, locals, calls, control flow, numeric ops
│   ├── [v] linear memory + active data segments + exported `memory`
│   ├── [v] i32/i64/f32/f64 loads+stores, memory ops for arrays
│   ├── [v] bulk `memory.copy` for runtime string concatenation
│   └── [-] table sections, full conversions, float arithmetic
├── host ABI (web-shaped)
│   ├── [v] prototype imports: `env.print_i64`, `env.print_utf8(ptr,len)`,
│   │   `env.print_f64` (VM-identical float formatting), `env.fmod`
│   ├── [v] module-internal bump heap (mutable global, starts at 8192) — no `alloc` import
│   ├── [v] argv: `bx.argc()` / `bx.arg(i)` (length-prefixed UTF-8 in page 2)
│   ├── [v] files: `bx.read_file` / `bx.write_file` (contents in page 3)
│   ├── [v] native processes: `bx.run` (inherited stdio) / `bx.capture` (stdout in page 4)
│   └── [-] `web.console.*`, `web.performance.*`, `bx.*` native process, `webgl.*`
└── delivery
    ├── [v] `bx build --wasm`, `bx run <file.wasm>`, `bx check <file.wasm>`
    └── [-] `--target wasm` default + six-cell evidence + `.bxb` removal
```

## Host ABI (v0 draft)

- **Namespaces**
  - `env.print_i64(i64) -> i64` — prototype integer output import (echoes the value).
  - `env.print_utf8(ptr: i32, len: i32)` — UTF-8 out of the linear memory.
- **Heap**: a mutable module global starts at 8192 and grows upward (bump); every
  allocation is emitted inline, so a module needs no allocator import. The heap
  is per-instance state inside the linear memory.
- **argv** (native extension, `bx.*`): `bx.argc() -> i32` and
  `bx.arg(i32) -> i32`. `bx.arg` returns a pointer to `[len: u32 LE][utf8]` in
  the second memory page; the module reads `len` and passes `ptr+4,len` to
  `env.print_utf8`. Browser hosts simply do not provide the `bx` namespace.
- **Files** (native extension, `bx.*`): `bx.read_file(path_ptr, path_len) -> i32`
  returns a pointer to `[len][bytes]` in the third page (0 on failure);
  `bx.write_file(path_ptr, path_len, data_ptr, data_len) -> i32` returns 0 on
  success, -1 on failure. Paths and data are UTF-8 byte ranges in memory.
- **Argv/capture bounds**: `argc()`/`arg(i)` pass through spaces and unicode and
  return an empty string for out-of-range indexes; `capture` returns the raw
  stdout in page 4 (not trimmed) with the exit code in its header, and aborts
  with `capture output ... does not fit` instead of silently returning an empty
  string when the result region is exhausted.
- **Processes** (native extension, `bx.*`): `bx.run(cmd_ptr, cmd_len) -> i32` runs
  the command line through the platform shell (`cmd /C` on Windows, `sh -c`
  elsewhere) with inherited stdio and returns its exit code; `bx.capture(...)`
  does the same with captured stdout and returns a pointer to
  `[code: i32][len: u32][bytes]` in the fourth page. Language builtins:
  `run(cmd)` and `capture(cmd)`.
  - `web.*` — console / performance / timers first, `fetch` later; names follow
    the browser globals so browser and native hosts share one module ABI.
  - `bx.*` — native extensions (process spawning, files, argv). Explicitly
    unavailable in a browser; a module that imports them is a native module.
  - `webgl.*` — WebGL 1.0 method surface, 1:1 with the browser API, i32 handles.
    Native host: deterministic software backend; browser host: real WebGL through
    a future JS glue layer.
- **Memory model**: one linear memory. Numbers are `i64`; strings are
  `(ptr, len)` UTF-8; arrays/objects live in an in-module heap with
  `__heap_alloc` / `__heap_free` exports (M2).
- **Exports**: `_start` is the script entry; `memory` and the heap helpers are
  exported once the data model lands.
- **Errors**: a trap aborts the module. bx-side `attempt` maps to status-code
  returns from imports in M3+.

## Evidence

- `cargo test`: the wasm module has unit tests for encoder round-trip, operator
  precedence and rejection of unsupported constructs; the whole crate suite is
  part of `bx/scripts/test.sh`.
- **Differential testing**: `bx/tools/wasm-diff.bxs` generates seeded random programs
  from the shared numeric subset (including float literals, `/`, user functions,
  recursion and rich local variables in functions) and runs each one through both the bx VM and the wasm
  backend; stdout and exit codes must match byte-for-byte. The suite runs 15 cases
  and the verifier 8; a 100-case local run passes. This is what caught the VM/floats
  divergence: bx's `/` is float division, so the wasm target now rejects `/` with a
  clear message instead of silently producing integer results.
- `bx build --wasm` output starts with `\0asm` and is instantiated by **node v24**
  (an independent engine), which proves the bytes are standard WebAssembly rather
  than a bx-private format. String scripts are executed by node too, reading the
  exported linear memory (`Buffer.from(instance.exports.memory.buffer, ptr, len)`).
- 75 in-tree unit tests (plus the self-hosted differential tool) cover encoder
  round-trip, memory/data decoding, UTF-8
  imports, operator precedence, control flow, user functions/recursion, argv,
  file I/O, process execution, operator spec mappings and rejection of
  unsupported constructs. `i64.div_s`/`i64.rem_s` and the six comparison opcodes
  are asserted against the wasm spec directly, after node cross-checks exposed an
  off-by-one in the interpreter's `rem`/`le`/`ge` mappings.
- CLI round-trip (`build --wasm` → `run` → `check`) is covered by the self-hosted
  suite and therefore by the packed six-cell CI lane.
- `bx check <module> --json` reports the decoded import/export names and defined
  function count, so agents can introspect a module without a JS toolchain.
- **Bools**: a real `bool` type (`i32` 0/1) so comparisons are expressions
  (`let ok = a < b`, `return x % 2 == 0`), plus `true`/`false`, short-circuit
  `&&`/`||`/`!` through if/else locals, ternary with int/float/bool branches
  (string branches still rejected) and `str(bool)`/`print(bool)` via host
  `env.bool_to_str`.
- **Interpolation and mixed concatenation**: the parser lowers `"x=${e} end"` to
  string `+`; the emitter stringifies int/float/bool operands (host
  `int_to_str`/`f64_to_str`/`bool_to_str`) so interpolation and mixed forms like
  `1 + "x"` match the VM. Float negation (`-x` for floats) maps to `f64.neg`
  (0x9a) and the interpreter now decodes the f64 unary range 0x99–0x9f.
- **Record/array writes**: `r.a = v`, `r.a += v`, `xs[i] = v`, `xs[-1] = v` and
  `xs[i] += v` are supported with the same negative-index normalisation and
  bounds checks as reads (`+ - * %` for compound).
- **Numeric/string/`type()` builtins**: `float`, `abs`, `floor`, `ceil`, `round`
  (lowered to `floor(x±0.5)` so half-away-from-zero matches the VM), `sqrt`,
  `trim`, `upper`, `lower`, `replace`, `index_of`, `repeat` and `type()` (static
  type name: int/float/str/bool/array/object/function). String results flow
  through host `env.str_*` block helpers; browser glue mirrors them with JS
  string methods.
- **Loop-condition fast path**: `while`/`for` exit tests over a direct comparison
  emit the inverse comparison opcode, dropping the trailing `eqz` on every
  iteration; the interpreter's timeout check short-circuits when no deadline is
  armed. Measured (wall clock incl. process start, `BX_BENCH_RUNS=9`):
  fib ≈0.83×, loop ≈0.58×, calls ≈0.78× of the VM's time.
- **Explicit IO/map failures**: `read_file(missing)` aborts with a `cannot read`
  message (same shape as the VM's `read`), `write_file` returns `0`/`-1`
  (sign-extended via `i64.extend_i32_s`, now decoded as 0xac), and maps reject
  failures instead of degrading silently: `map is full (64 entries)`,
  `map allocation failed`, `map is not initialized`.
- **Indexing**: `arr[i]` supports negative indices (counted from the end) and
  float indices (truncated), matching the VM; `str[i]` returns the i-th Unicode
  character as a one-character string via host `env.str_char_at`; out-of-range
  reads abort (`index out of bounds`, documented divergence from the VM's null).
- **Bool ordering**: `true == false`, `false < true` and friends compile through
  the same i32 comparison opcodes; bool conditions and bool expressions share
  one emission path.
- **Empty string needles**: `.contains("")` / `.starts_with("")` /
  `.ends_with("")` are true even for an empty haystack; the host previously
  panicked on `str::windows(0)`.
- **Runtime boundary aborts**: array indexing is bounds-checked (`index out of
  bounds`), integer/float division by zero aborts with `division by zero` (same
  message as the VM), and modulo by zero aborts with `modulo by zero`. The VM
  yields `null` for out-of-bounds/missing-value reads and for `% 0`; the subset
  has no null, so it aborts instead — a documented divergence, not silent
  garbage. Host import `env.panic(ptr,len)` performs the abort; the browser glue
  throws.
- **Scoped loop variables in call arguments**: `print(k + ":" + str(v))` inside
  a record/map `for k, v in` loop now type-checks through the emitter's own
  `type_of` for builtin calls, instead of falling back to the checker (which
  cannot see loop-scoped locals during emission). Nested while/for-in inside a
  for-in, and `k + ":" + str(v)` forms are covered by unit/suite/verifier and
  the differential generator.
- **Loop `continue`**: for-in loops (array/map/record) and `for (…; …; step)`
  route `continue` through a dedicated block so the index increment / step still
  runs; previously `continue` branched to the loop header and skipped it
  (infinite loop on array iteration). Covered by unit, suite, verifier and the
  differential generator.
- **Numeric range divergence (documented)**: the VM promotes int arithmetic
  results to Float when the exact result is fractional or `|r| >= 9e15`; the
  wasm subset keeps i64. Values below that threshold agree; the differential
  generator deliberately bounds magnitudes so it never asserts past it.
- **Function literals**: `let f = fn(x) { return x * 2 }` compiles to a lifted
  wasm function; the value is a dummy i32 and every call site resolves the
  concrete literal statically (`Ty::Func(id)`). Literals can be passed to
  parameters (`fn apply(f, x) { return f(x) }`) and returned, because the type
  carries the identity. Capturing outer locals is rejected with a clear error
  (pass a parameter instead); nested named functions remain unsupported.
- **Dynamic maps** (wasm-target extension, not in the VM): `map_new()`,
  `map_set(map, key, int)` (statement), `map_get`, `map_has`, `map_len`.
  String keys, int values, fixed capacity of 64 entries in a dedicated memory
  region; `for k, v in map` walks entries in insertion order (single-variable
  `for k in map` yields keys); the browser glue backs them with JS `Map`.
- **Process utilities** (native + browser stubs): `now()`, `sleep(ms)`,
  `cwd()`, `env(name)`, `mkdir(path)`, `exists(path)` (Bool, matching the VM),
  plus the wasm-only `file_exists(path)` (Int, kept for the earlier rounds).
- **Conversions and methods**: `str(number)` / `int(string|number)` (host
  `int_to_str`/`f64_to_str`/`str_to_int`, VM-identical formatting), `.len()` on
  strings and arrays, and statement-level `arr.pop()` (length shrink; the removed
  value is not exposed yet because the subset has no null).
- **Function parameters**: a light call-site inference pass types function
  parameters and results as int, float, string, array or record. Strings travel
  as `(ptr, len)` pairs and string results use wasm multi-value results; int
  arguments promote to float parameters; conflicting call sites produce clear
  errors. Slots are laid out per function (a string parameter owns two locals),
  so helpers such as `fn tag(s) { return s + "?" }` compile and match the VM.
- **Execution budgets**: `bx run --max-steps N --timeout MS script.wasm` applies
  the same budgets as the native VM path to the wasm interpreter (instruction
  counter plus a periodic deadline check), so untrusted modules cannot spin
  forever. The normal-path hot loop adds only a counter increment per
  instruction; the deadline is sampled every 1024 instructions.
- **Browser demo**: `bx build --wasm --html script.bxs -o demo.html` writes a
  self-contained page (module embedded as base64) plus a sibling `demo.js`. The
  glue implements the same import namespaces as the native host (`env.print_*`,
  `bx.argc/arg`; file/process imports are stubbed out) and prints into `<pre>`
  in a browser or stdout under node. The suite and verifier run `node demo.js`
  when node is available and compare against the native module output.
- **Performance** (`bash bx/scripts/bench.sh --wasm`, wall clock, min of 3, this
  machine): fib 108 ms (VM 131 ms), 1e6-iteration loop 205 ms (VM 358 ms), 200k
  helper calls 128 ms (VM 184 ms) — the wasm interpreter is now faster than the
  VM on all three. Two fixes made this possible: function frames keep their own
  label base (a `return` inside a block previously discarded the caller's labels,
  which also silently skipped code after the block — caught by the benchmarks),
  and wasm calls now reuse a locals pool and leave results on the operand stack
  instead of allocating a `Vec` per call and per return.

## Non-goals

- No WASI: the ABI is web-shaped on purpose.
- No GC, exceptions, threads, SIMD or reference types in v0.
- No third-party wasm runtime or encoder.
- Not claiming the full language before M2/M3 land; the current target rejects
  unsupported constructs with a clear message.

## Milestones

| Milestone | Content | Status |
|---|---|---|
| M0 | this spec + decision record | `[v]` |
| M1 | numeric vertical slice: emit → run → `check`, node cross-check | `[v]` |
| M2 | data model: strings, arrays, objects in linear memory | `[-]` (strings incl. runtime concat, int arrays, fixed-layout records, in-module bump heap done; dynamic keys, mixed-type fields and growable arrays next) |
| M3 | functions, closures, control flow compiled to wasm | `[-]` (control flow, user functions/recursion and float values done; closures next) |
| M4 | builtins + native process ABI (`bx.*`), default target flip | `[-]` (argv, file and process imports done; default target flip next) |
| M5 | `web.*` + `webgl.*` ABI, software backend, browser demo | `[-]` (demo, `web.console_log*`/`web.performance_now`, and a `webgl.*` clear/read/PNG slice done; shader/buffer surface next) |
| M6 | remove `.bxb` + legacy code paths, six-cell wasm evidence | `[_]` |
