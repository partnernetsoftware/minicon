# `bx` WebAssembly target (`.wasm`)

Owner of the experimental `.wasm` backend and its host ABI. The goal is to
replace the private `.bxb` container as the long-term execution format; `.bxb`
stays as the legacy path until parity is proven.

## Product outcome

bx source compiles to a **standard WebAssembly core module** that any engine can
run. bx itself executes those modules with the owned vendored core (vendored
`tinyvm`, PRD_02_44 P1), so the product keeps its "one native binary, no
runtime install" shape. The same module runs in a browser through the JS host
glue.

## Decision

- Encoder and the old interpreter are **in-tree** (`bx/src/wasm.rs`,
  `bx/src/wasm_target.rs`). Execution and validation now run on the owned
  vendored core (`bx/vendor/tinyvm`, PRD_02_44 P1); `cargo tree -p bx` shows
  `bx → tinyvm → libm`. It is owned source in this repository, not a crates.io
  or third-party WebAssembly runtime.
- The emitted artifact is a standard module (`\0asm`, version 1), not a
  bx-private container. Provenance moves to a sidecar/metadata only if a later
  milestone needs it.
- `.bxb` is now the **legacy** path. The default flip, its parity gates and the
  `.bxb` retirement are owned by
  [PRD_02_43_bx_wasm_migration.md](PRD_02_43_bx_wasm_migration.md); this module
  only supplies the backend and its feature evidence.
- Host imports are **web-shaped**, not WASI: the ABI mirrors browser surface
  names so one module can serve both native and browser hosts.

## Engine performance (measured 2026-11-17)

`bx/scripts/bench.sh` (`BX_BENCH_RUNS=5`, wall clock, process start included):

| benchmark | VM (`.bxb` source) | wasm target (core) |
| --- | --- | --- |
| fib | 134 ms | 223 ms |
| loop | 332 ms | 243 ms |
| calls | 166 ms | 260 ms |

Split for `fib(24)` on the same machine: `bx run source.bxs` (compile+VM) 56 ms,
`bx run prebuilt.wasm` 153 ms, `bx build --wasm` 20 ms. So the wasm path's cost
is dominated by the generic core interpreter (per-instruction budget/stack
accounting), and it is faster on tight numeric loops while slower on call-heavy
code. This is an accepted trade-off for validation, budgets, and one engine;
candidates for later optimization (not scheduled): skip step accounting while
no budget is set, and avoid per-instruction live-slot checks in the vendored
core.
## Host ABI contract

Every host (the native `PrintHost` in `bx/src/wasm.rs`, the browser/node demo glue, and the
`tinyvm`-based host profile) must implement these exact shapes; they are not
documented by the module beyond its import types:

- Strings are passed as `(ptr, len)` pairs. Results that are strings or string
  arrays are returned as a block pointer `[len: u32][utf8 bytes]` allocated by
  the host; multi-string results are `[count][ptr, len]*N`.
- **String lengths and offsets are Unicode scalar values, not bytes**:
  `str_len` (length), `str_char_at`, `str_slice` and `str_index_of` all count
  characters, and negative indexes/slices count from the end. Byte offsets
  diverge from the VM for any non-ASCII text (found and fixed 2026-11-17).
- `for ch in text { .. }` iterates scalar values; the two-variable form
  (`for i, ch in text`) yields the character index.
- `print_record(table, count, rec_ptr)` and `print_arr_obj(table, count, arr_ptr)`
  take the shape table first; `table` holds `u32` offsets to length-prefixed key
  blocks, and record fields are `i64` at `rec + 8*index`.
- `print_arr_i64`/`print_arr_str`/`print_arr_f64`/`print_arr_arr`/
  `print_map`/`print_arr_obj` must render the VM's compact JSON shape so the
  products stay interchangeable.
- Maps are a wasm-target extension: `map_new()` returns an opaque id whose
  entries are `[count][ptr,len,value i64]*N`; `map_*` order is insertion order.
- `print_i64` returns the value it printed (`i64 -> i64`); the other print
  imports return nothing.

Portability note: the ABI is intentionally small and web-shaped, but it is
*bx's* ABI, not WASI. A standard-wasm engine can still run the module because
`env.*` is just an import namespace; only the host has to provide it.
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
  and the verifier 8; a 100-case local run passes. The generator also appends
  fixed edge cases (unicode strings, empty containers, negative indexes, text
  rows) and accepts `[cases] [seed]`; on 2026-11-17 seeds 1/7/42/1234567/999999937
  at 100 cases each all passed, which is how the byte-vs-character string bugs
  above were found. `bx/tools/wasm-diff-coll.bxs` (same `[cases] [seed]` form)
  covers collections instead: int/float/string arrays, text rows, int rows,
  records, map/filter/each/sort/reverse/slice/reduce/push, nested indexes,
  negative indexes, element writes, two-variable loops and closure captures.
  Sweep on 2026-11-17: six seeds at 50 cases each, all pass; the suite runs 12
  collection cases and 15 numeric cases. This is what caught the VM/floats
  divergence: bx's `/` is float division, so the wasm target now rejects `/` with a
  clear message instead of silently producing integer results.
- `bx build --wasm` output starts with `\0asm` and is instantiated by **node v24**
  (an independent engine), which proves the bytes are standard WebAssembly rather
  than a bx-private format. String scripts are executed by node too, reading the
  exported linear memory (`Buffer.from(instance.exports.memory.buffer, ptr, len)`).
- 100 in-tree unit tests (plus the self-hosted differential tools) cover the encoder and the core host; the interpreter tests retired with the engine
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
- **Typed record fields**: record fields may hold int, string, float or bool
  values. Each field still occupies one 8-byte slot (a string is `ptr, len`);
  `print`/record-array printing use the typed hosts
  `env.print_record_typed(kinds, table, count, rec)` and
  `env.print_arr_obj_typed(kinds, table, count, arr)`, where `kinds` is one byte
  per field (0 int, 1 string, 2 float, 3 bool). Field reads, writes, record
  arrays, `map`/`filter`/`each`/comparator `sort` over record arrays and closure
  captures see the field's real type. `for k, v in record` stays int-only
  (mixed value types would need a dynamic tag); nested records are still
  rejected explicitly.
- **File and lookup aliases**: `read(path)` and `write(path, data)` mirror the VM
  names for `read_file`/`write_file`, and `read("-")` drains stdin to EOF;
  `which(name)` resolves a command and returns its path (an empty string when
  missing, where the VM returns null). `write` returns `0` on success (the VM
  returns null). `exec`/`sh` are still absent because their result record mixes
  int/string/bool fields, which the fixed-shape record model does not carry yet.
- **Collection builtins**: `push` works in expression position (returns the
  array) and `pop(arr)` returns the removed element for int, float, string,
  text-row, int-row and record arrays (the empty case aborts; the VM returns
  null). `contains(arr, value)` covers int, string and float arrays.
  `float()` accepts ints and strings (`float("1.5")`), `int()` accepts bools,
  and unparsable numeric strings abort with the VM's wording (`cannot convert
  str to int` / `cannot convert str to float`).
- **Exit, errors and assertions**: `exit(code)` ends the module through the host
  and the CLI returns that code (stdout before it is kept); `error(msg)` and
  `assert(cond, msg?)` abort with the message on stderr and exit 1. The VM
  prefixes those messages with `file(line):`; the wasm host has no source
  location, so the message text is exactly `msg` (documented divergence). Both
  statement and expression positions are handled; the browser demo throws a JS
  error instead of terminating the page.
- **Control flow**: `break`/`continue` work inside every loop form (`while`,
  `for` over int/float/string arrays, rows, records and strings, including
  two-variable forms and nested `if`s); `break` leaves the loop's own exit
  block instead of the per-iteration continue block (fixed 2026-11-17), and
  string-branch ternaries (`cond ? "a" : "b"`, nested) emit a `(ptr, len)` pair
  from both arms.
- **Text rows**: arrays of string arrays (`[["a","b"],["c"]]`, `Ty::ArrStrArr`)
  type, index (including `rows[1][0]`), iterate, `len`, `push`, element writes,
  `map`/`filter`/`each`, comparator `sort`, `reverse`/`slice`, closure capture
  and VM-style printing (`print_arr_strarr`). This is the text-table shape agent
  scripts reach for (CSV/rows) without a uniform-value rewrite.
- **Nested rows and generic `reduce`**: arrays of int arrays participate in
  `map`/`filter`/`each`/comparator `sort`/`reverse`/`slice` (a row is a value
  slot, so `map(rows, fn(r) { return sum(r) })` returns an int array and a
  callback returning an array yields another row array). `reduce` now takes any
  array kind with a number/string accumulator: `(acc, elem) -> acc`, seed or
  first-element start, and the empty-with-seed case returns the seed.
- **Record-array collections**: `map` over record arrays returns int, float,
  string or record arrays (the callback result picks the element kind),
  `filter` keeps the record shape, `each` iterates records, comparator `sort`
  works for int/float/string/record arrays, and `reverse`/`slice` accept record
  arrays too. Fixing this exposed a real slot-allocation bug: function-literal
  scratch locals were numbered as if the capture-environment was not a wasm
  parameter, so building an array/record literal inside a closure could
  overwrite its own parameters (`map(xs, fn(r) { return {b: r.a + 1} })` read
  `r.a` from the freshly allocated record instead of the argument).
- **Kind-generic array helpers**: default `sort` (int, float, string arrays),
  `reverse` (in place for every array kind), `slice` (int/float/string arrays)
  and `join` (one or two arguments; int and float arrays stringify their
  elements) now dispatch on the array kind instead of failing on non-int arrays.
  A call whose value is discarded is accepted in statement position
  (`reverse(a)`, `sort(a)`), matching the VM.
- **Array search and min/max**: `index_of(array, value)` works on int and string
  arrays (function and method form, `-1` when absent), `.contains(value)` on
  arrays is `index_of(...) != -1`, and `min`/`max` work on int arrays (int) and
  string arrays (lexicographic string); the search/min-max helpers are host
  imports (`arr_index_of`, `arr_index_of_str`, `arr_minmax`, `arr_minmax_str`)
  so no guest loops are needed. `min([])`/`max([])` abort (the VM returns null).
- **Map ergonomics + printing**: maps are a wasm-target extension over the VM.
  `keys(map)` -> string array, `values(map)` -> int array, `has(map, key)`,
  `get(map, key[, default])` (missing without a default aborts), and `len(map)`
  all work; `print(map)` renders `{"k":1,"k2":2}` in insertion order via the
  `print_map` host import (browser glue reads its JS map table instead of guest
  memory). Maps still cannot be compared/sorted.
- **Float arrays**: mixed numeric literals promote to `Ty::ArrF` (`[1.5, 2]`);
  `print` renders VM-style (`[1.5,2,3.25]`), and read/write, `len`, `for` (one or
  two variables), `push`, `sum`, `min`/`max`, `index_of`, `.contains` work with
  f64 slots. `map`/`filter`/`reduce` callbacks, comparator `sort` and nested
  float containers are still rejected; each new element kind costs a matrix of
  emitter arms plus host imports (the reason a uniform value model is the next
  design step, see `prd/PRD_02_44_bx_tinyvm_absorption.md`).
- **Nested arrays**: `[[1,2],[3,4]]` types as `Ty::ArrArr` (entries hold inner
  array pointers); `len`, element read (including `m[1][0]`), element write,
  single/two-variable `for row in m`, `push`, negative indexes, passing rows to
  functions and printing (`[[1,2],[3,4]]`) all work. Deeper nesting and arrays
  mixing element kinds remain unsupported.
- **Container printing**: `print` accepts int arrays, string arrays, records and
  record arrays and formats them exactly like the VM (compact JSON style:
  `[1,2,3]`, `["a","b"]`, `{"a":1,"b":2}`, `[{"a":1}]`). The guest calls one of
  four host imports (`print_arr_i64`, `print_arr_str`, `print_record`,
  `print_arr_obj`) with the container pointer plus, for records, the shape key
  table; the host walks memory and prints. Maps/functions still cannot print and
  `console_log` stays scalar-only.
- **push/pop on every array kind**: both the method form (`xs.push(v)`) and the
  call form (`push(xs, v)`) append a value for int, string and record arrays;
  `pop` shrinks the length (the removed value is still not returned in an
  expression). Growth copies the header and all 8-byte entries, then writes the
  element (int64 / ptr+len pair / record pointer).
- **Empty record literals**: `{}` is now a valid zero-field record, so
  `merge({}, {x: 1})`, `keys({})`, `values({})` and `type({})` work; printing a
  record is still rejected.
- **Comparator sort**: `sort(arr, fn(a, b) { return a - b })` runs an in-place
  insertion sort in wasm calling the literal comparator (environment included);
  the comparator receives two ints and returns an int whose sign decides the
  order, the array is mutated and returned like the VM. String arrays take a
  comparator over `(str, str) -> int` the same way (insertion sort swapping
  ptr/len pairs); int and string arrays are the supported element kinds.
- **Arrays of records**: `[{a: 1}, {b: 2}]` with one consistent shape types as
  `Ty::ArrObj(shape)`; entries hold record pointers in the usual 8-byte slots, so
  `len`, element read (negative indexes included), element write, `for r in xs`
  (single variable = record) and `for i, r in xs` all work, as do record arrays
  passed to functions. Object lists nested inside other array literals keep the
  array pointer in a dedicated local because each record literal allocates
  storage.
- **Record merge**: `merge(a, b)` on records with statically known shapes builds
  the union shape at compile time (keys of `a` in order, then new keys of `b`);
  overlapping keys take `b`'s value, matching the VM. Chained merges work.
  Merging with an empty-object literal (`{}`) remains unsupported.
- **Record builtins**: `keys` -> string array, `values` -> int array, `has`, and
  `get(record, key[, default])` walk the per-shape key table at runtime with
  `str_compare`; a missing key without a default aborts (`get() missing key with
  no default`) instead of returning the VM's null. `merge` remains unsupported
  (it would need a dynamic object shape).
- **Capturing closures**: each function literal gets a leading environment
  pointer parameter; captured outer bindings live in heap cells referenced from
  that environment. Locals/parameters captured by an inner literal are boxed
  (a cell is allocated at declaration/entry), so closures share mutable state
  exactly like the VM (verified with counters). Capture analysis runs before
  literal bodies are checked and is transitive across nested literals; it also
  feeds inference pass B so callback results that read captures are typed.
  Named functions and top-level globals are read directly (no capture needed).
  Loop variables over every iterable (int arrays, string arrays, records, maps)
  are capturable: each loop allocates one cell before the loop and refreshes it
  every iteration, matching the VM's single-shared-binding semantics.
  `map`/`filter`/`reduce`/`each` callbacks may capture outer locals: the callback
  value (its environment) is evaluated once and passed into every callback
  invocation. Remaining limits: a variable cannot hold function literals from
  two different syntactic sites (`requires stable types`), and arrays of records
  are not supported.
- **Higher-order builtins over string arrays**: `map`/`filter` accept string
  arrays; `map` returns a string array when the callback returns a string (an
  int array when it returns an int), `filter` keeps the input element kind.
  Soft inference now distinguishes string-array literals, walks `let`
  initialisers and `for-in` iterators, and types `.len()`/mixed `+` inside
  callbacks.
- **Higher-order array builtins**: `map`, `filter`, `reduce` and statement-level
  `each` accept non-capturing function literals; the emitter inlines a loop and
  calls the lifted literal directly (no indirect dispatch). `map` results are
  int arrays, `filter` predicates must return bool, `reduce` callbacks take
  `(acc, x)` (an optional int seed; without a seed an empty array aborts).
- **Int-array builtins**: `range(stop)` / `range(start, stop)` (the VM ignores a
  third step argument; we match), `sum`, `reverse`, `sort` and `slice`. The VM
  mutates the input array for `reverse`/`sort`, so the host mutates the same
  block in place and returns the same pointer (array identity preserved).
- **String arrays**: `[len][ptr,len]*N` layout; literals of all-string elements
  type as a string array, element read/write (with negative indexing), `len`,
  `for i, s in arr`, `split(str, sep)` -> string array, `join(arr, sep)` -> str,
  and `slice` over strings, int arrays and string arrays (negative bounds clamp
  like the VM). Allocates from the host string region; browser glue mirrors the
  same block layout.
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
