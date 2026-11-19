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

- Encoder and host live **in-tree** (`bx/src/wasm.rs`, `bx/src/wasm_target.rs`).
  Execution and validation run on the owned vendored core (`bx/vendor/tinyvm`,
  PRD_02_44 P1); `cargo tree -p bx` shows
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

## Engine performance (measured 2026-11-18)

`bx/scripts/bench.sh` (`BX_BENCH_RUNS=5`, wall clock, process start included,
re-measured 2026-11-18 after the array/bool/null feature rounds):

| benchmark | VM (`.bxb` source) | wasm target (core) |
| --- | --- | --- |
| fib | 131 ms | 227 ms |
| loop | 327 ms | 243 ms |
| calls | 166 ms | 258 ms |
| coll (`map`/`filter`/`reduce`/`sort` over 2000 ints) | 94 ms | 100 ms |
| recs (`map`/comparator-`sort`/`reduce` over 500 records) | 96 ms | 100 ms |
| boolarr (`map`/`filter`/`sort`/`reverse`/`contains`/`index_of`/`sum` over bool arrays) | 93 ms | 100 ms |

So the absorbed core is ~1.6–1.7× slower on call-heavy code, ~0.75× on tight
numeric loops and about equal on collection/record-heavy work; the table is the
price of validation, budgets and one engine, not a regression from the feature
work. The machine is noisy run to run (the `calls` VM figure moved between 164
and 199 ms across sessions), so compare within one row rather than across runs.

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
│   ├── [v] interpreter (retired by PRD_02_44 P1; the core owns this now)
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
- **Errors and `attempt`**: an uncaught trap aborts the module.
  `attempt(fn() { ... })` is compiled by inlining the zero-parameter literal
  body inside a labelled block: guest-detected fatal guards (index bounds,
  division/modulo by zero, `min`/`max`/`reduce`/`pop` on an empty container,
  `get` missing key without a default, explicit `error()`) store the message in
  the `{error, ok, value}` record and branch to the block end instead of
  trapping. Inside `attempt`, `exec`/`sh` use a soft host import
  (`bx.exec_record_soft`) that reports spawn failures ("cannot run ...") in the
  result block instead of trapping, so those become `ok: false` like the VM.
  `mkdir` is an expression too (returns `0`, where the VM returns `null`) and a
  failed `mkdir` is fatal outside `attempt` and catchable inside it through
  `env.mkdir_soft`. `read`/`write` do the same through
  `bx.read_file_soft`/`bx.write_file_soft`
  (`[status][len][data]` blocks): missing files and write failures become
  attempt errors, while the direct (non-attempt) paths keep their existing
  behaviour (fatal read error, `-1` write). Other host-side traps still abort.
  Divergences from the VM: `error` is `""` on success where the VM has `null`;
  `value` is `0` for callbacks without a return value where the VM has `null`;
  messages omit the VM's `file(line) in <lambda>()` prefix; out-of-range reads
  and `% 0`, which the VM turns into `null`, are fatal here, so `attempt`
  reports them as errors. `bx/tools/wasm-diff-attempt.bxs` seeds random
  `attempt` programs (div-by-zero, explicit `error()`, missing `read`, `exec`
  spawn failures, `mkdir` on a file, attempt inside loops and nested attempts)
  and compares `ok`/error-flag output with the VM; the suite runs 12 cases and a
  local sweep of twelve seeds at 40 cases each passes.

## Evidence

- `cargo test`: the wasm module has unit tests for encoder round-trip, operator
  precedence and rejection of unsupported constructs; the whole crate suite is
  part of `bx/scripts/test.sh`.
- **Differential testing**: `bx/tools/wasm-diff.bxs` generates seeded random programs
  from the shared numeric subset (including float literals, `/`, user functions,
  recursion and rich local variables in functions) and runs each one through both the bx VM and the wasm
  backend; stdout and exit codes must match byte-for-byte. The suite runs 15 cases
  and the verifier 8; a 100-case local run passes. It now also emits random
  ternaries (numeric and string branches), `&&`/`||`/`!`, C-style `for` loops
  with `continue`, and a helper function whose body returns a ternary; twelve
  seeds at 80 cases each (960 programs) pass. The generator also appends
  fixed edge cases (unicode strings, empty containers, negative indexes, text
  rows) and accepts `[cases] [seed]`; on 2026-11-17 seeds 1/7/42/1234567/999999937
  at 100 cases each all passed, which is how the byte-vs-character string bugs
  above were found. `bx/tools/wasm-diff-coll.bxs` (same `[cases] [seed]` form)
  covers collections instead: int/float/string arrays, text rows, int rows,
  records, map/filter/each/sort/reverse/slice/reduce/push, nested indexes,
  negative indexes, element writes, two-variable loops and closure captures.
  `bx/tools/wasm-diff-dyn.bxs` (same `[cases] [seed]` form) also generates
member-callback `reduce` cases (`map(data["items"], fn(it){ it["score"] })`
then numeric and dynamic-identity reducers), dynamic-object method cases
(`keys`/`values`/`get`/`has`/`len`), dynamic and
dyn-init `reduce`, mixed-return `map`/truthy `filter`, and mixed literal
containers. It covers the P2 dynamic
mixed dynamic/static record shapes (`{xs: <dynamic>, list: [4,5], o: <dynamic>}`
with writes back into the dynamic field), heterogeneous parsed arrays and arrays
of parsed objects. It covers the P2 dynamic
paths: each case writes a random JSON payload, parses it at runtime and runs
dynamic print/str/json/type/len, iteration and key/value loops, indexing and
member access, get/has/contains/index_of/slice, map/filter/each,
sort/reverse/sum/min/max/join, equality and dynamic/static concatenation; the
VM and wasm outputs must match. `bx/scripts/test.sh` runs 12 of its cases with a
fixed seed after the suite, so CI keeps this parity checked. `bx/tools/wasm-diff-methods.bxs` (same `[cases] [seed]` form) targets the
  method surface and null: `.join([sep])`, `.sort()`, `.sort(cmp)`,
  `.reverse()`, `.pop()`, `slice`, `contains`/`index_of`, `map`/`filter`/
  `reduce`/`each`, `sum`/`min`/`max` over int/float/string/bool arrays, plus
  record `join`/`.join`/`.sort(cmp)` and nested-row joins plus record/nested
  `contains`/`index_of`, and the method forms `.slice`/`.sum`/`.min`/`.max`/
  `.map`/`.filter`/`.reduce`, plus the full null scalar set. Sweep on 2026-11-18
  (higher-order methods added): three seeds at 40 cases each and the 12-case
  smoke, all pass.
Sweep on 2026-11-17: six seeds at 50 cases each, all pass; the suite runs 12
  collection cases and 15 numeric cases. The generator also emits random string
  expressions (trim/upper/lower/replace/repeat/slice/concat/split-join,
  comparisons, interpolation via `"${var}"`) inside array/record literals and
  `map` callbacks; eight seeds at 40 cases each pass. This exposed a soft
  inference gap (`trim`/`upper`/`lower`/`replace`/`repeat` were not typed as
  strings for array-literal inference, so `map([lower(s)], fn(x) { return
  len(x) })` failed) which is fixed. This is what caught the VM/floats
  divergence: bx's `/` is float division, so the wasm target now rejects `/` with a
  clear message instead of silently producing integer results.
- Review sweep 2026-11-18 after the P2a/P2b rounds: `wasm-diff` (numeric) and `wasm-diff-coll` three seeds x 20 cases each, `wasm-diff-methods` three seeds x 20, `wasm-diff-attempt` three seeds x 12 - all PASS; `bench.sh --wasm` re-run (`BX_BENCH_RUNS=3`) keeps the static benchmarks flat (fib 127/226, loop 330/240, calls 162/266, coll 94/99, recs 94/96, boolarr 95/99 ms), so the dynamic-value additions did not slow the static path.

- `bx build --wasm` output starts with `\0asm` and is instantiated by **node v24**
  (an independent engine), which proves the bytes are standard WebAssembly rather
  than a bx-private format. String scripts are executed by node too, reading the
  exported linear memory (`Buffer.from(instance.exports.memory.buffer, ptr, len)`).
- 144 in-tree unit tests (plus the self-hosted differential tools) cover the encoder and the core host; the interpreter tests retired with the engine
  round-trip, memory/data decoding, UTF-8
  imports, operator precedence, control flow, user functions/recursion, argv,
  file I/O, process execution, operator spec mappings and rejection of
  unsupported constructs. `i64.div_s`/`i64.rem_s` and the six comparison opcodes
  are asserted against the wasm spec directly, after node cross-checks exposed an
  off-by-one in the retired interpreter's `rem`/`le`/`ge` mappings.
- CLI round-trip (`build --wasm` → `run` → `check`) is covered by the self-hosted
  suite and therefore by the packed six-cell CI lane.
- `bx check <module> --json` reports the decoded import/export names and defined
  function count, so agents can introspect a module without a JS toolchain.
- **Bools**: a real `bool` type (`i32` 0/1) so comparisons are expressions
  (`let ok = a < b`, `return x % 2 == 0`), plus `true`/`false`, short-circuit
  `&&`/`||`/`!` through if/else locals, ternary with int/float/bool branches as
  well as string branches (both arms produce a `(ptr, len)` pair), and
  `str(bool)`/`print(bool)` via host `env.bool_to_str`.
- **Interpolation and mixed concatenation**: the parser lowers `"x=${e} end"` to
  string `+`; the emitter stringifies int/float/bool operands (host
  `int_to_str`/`f64_to_str`/`bool_to_str`) so interpolation and mixed forms like
  `1 + "x"` match the VM. Float negation (`-x` for floats) maps to `f64.neg`
  (0x9a) and the core decodes the f64 unary range 0x99–0x9f.
- **Expression statements**: any supported call (for example `sort(xs)` or
  `write(path, text)`) may stand alone as a statement; its value is dropped,
  matching the VM.
- **Record/array writes**: `r.a = v`, `r.a += v`, `xs[i] = v`, `xs[-1] = v` and
  `xs[i] += v` are supported with the same negative-index normalisation and
  bounds checks as reads (`+ - * %` for compound).
- **String concatenation safety**: each operand of a runtime `+` now lives in its
  own local pair, so a string operand that itself reads a record field (which
  clobbers the shared pointer/length temporaries) can no longer corrupt the
  other side; interpolation like `"v=${o.a}-${o.b}"` and `o.s + "!"` match the
  VM. Record fields still hold scalars only (`int`/`string`/`float`/`bool`);
  array- or record-valued fields are rejected explicitly (the VM allows them).
- **Type inference across statements**: both fixpoint passes now keep one
  locals map across top-level statements instead of a fresh clone per
  statement, and a provisional `int` guess no longer overrides a later,
  more specific type. That fixes record-array chains such as
  `let ordered = sort(rows, cmp)` followed by `map`/`reduce` over `ordered`
  (previously "can read fields of objects only, got int").
- **Dynamic record keys**: `get(record, key)` accepts a non-literal key when every
  field of the record has the same type (int/string/float/bool); the key search
  runs in the guest and returns that field type, falling back to the default or
  aborting when missing. Heterogeneous records still need a literal key (their
  fields would need a tagged union). `has(record, key)` always accepts dynamic
  keys.
- **Structural equality**: `==`/`!=` compare records and arrays by value
  (field-by-field for records with int/string/float/bool fields; length plus
  element-by-element for int/float/string/record/nested arrays). `contains` and
  `index_of` extend to record arrays, int-array rows and text rows. `get(record,
  "key")` needs a literal key (fields are statically shaped; a dynamic key is
  rejected explicitly) and returns the field's real type; `has(record, key)`
  works with dynamic keys. The VM supports dynamic keys on records, this subset
  does not.
- **`exec` / `sh` (VM shape)**: both return the VM's record
  `{code, err, ms, ok, out, timeout}` in that key order (so printing it matches
  the VM). `exec` runs the command without a shell by default (a string command
  is split on whitespace, the VM's quoting rules are not reproduced yet) and
  `sh` always goes through the platform shell. Supported options:
  `cwd`, `stdin`, `timeout`, `shell` and `env` (an object literal; the emitter
  sends each pair through `bx.exec_env_clear`/`bx.exec_env_add` and the host
  applies them additively, coercing scalar values to text like the VM); other
  VM options (`max`, `stdout`, `stderr`) are rejected explicitly. A missing program aborts with
  `cannot run '<name>': program not found` (the VM adds its `file(line):`
  prefix; documented divergence). `ms` is wall-clock time, so two runs of the
  same program may print different values. The host import is
  `bx.exec_record(mode, cmd_ptr, cmd_len, cwd_ptr, cwd_len, stdin_ptr, stdin_len,
  timeout_ms, shell, 0) -> block`, where the block is
  `[code i32][out_ptr][out_len][err_ptr][err_len][ms][flags]` and `flags & 1`
  marks a timeout.
- **`exec_all(commands)`**: runs each command spec (a string array per command
  or a plain command string, matching the VM's two accepted forms) and collects
  the records into a `Ty::ArrObj(exec shape)` array, so `map`/`filter` over the
  results and closures work. Options are rejected for now (the VM accepts
  `{cwd, ...}` per batch); call `exec` when per-command options are needed.
  Because the array holds record pointers, `map` callbacks returning bools still
  hit the missing bool-array kind (a separate limitation).
- **String methods**: `trim`, `upper`, `lower`, `split`, `slice`, `replace`,
  `repeat` and `index_of` work as methods as well as functions (VM parity).
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
- **`__dir__`**: resolves to the directory of the executed module through
  `bx.script_dir` (canonicalised, verbatim Windows prefixes stripped), matching
  the VM's `__dir__` when a script is run from the same directory as the module.
  A module moved elsewhere reports its own directory, which is the useful
  deployment behaviour (the VM has no equivalent).
- **`argv` and `env(name, default)`**: the VM's `argv` global works as a
  materialised `Ty::ArrStr` (built on each access from `argc()`/`arg(i)`), so
  `len(argv)`, `argv[i]`, `for` loops, `join(argv, ...)`, captures and `map`
  callbacks all work; `env(name, default)` returns the default when the variable
  is unset (the one-argument form still returns `""` where the VM returns
  `null`).
- **stderr**: `eprint(text)` writes `text` plus a newline to the process stderr
  through a third host sink (`env.eprint`); `print(eprint(...))` yields `0` where
  the VM yields `null`. The browser/node demo glue routes it to `console.error`.
- **File helpers**: `append(path, text)` opens the file in append mode (returns
  `0`, the VM returns `null`), `remove(path)` deletes a file, and
  `list(dir)` returns the sorted entry names as a string array, all matching the
  VM apart from the null-vs-0 return of `append`. They use
  `env.append_file`/`env.remove_file`/`env.dir_count`/`env.dir_entry` (the
  entry imports re-read the directory per index; fine for the small
  directories scripts list).
- **Path helpers**: `basename(path)` and `dirname(path)` follow the VM's rules
  (both `/` and `\` separate, trailing separators trimmed, `dirname("/a") == "/"`,
  `dirname("file.txt") == ""`), served by `env.basename`/`env.dirname` host
  imports that write `[len][utf8]` blocks like `which`.
- **File and lookup aliases**: `read(path)` and `write(path, data)` mirror the VM
  names for `read_file`/`write_file`, and `read("-")` drains stdin to EOF;
  `which(name)` resolves a command and returns its path (an empty string when
  missing, where the VM returns null). `write` returns `0` on success (the VM
  returns null). `exec`/`sh` return the VM-shaped typed record
  `{code, err, ms, ok, out, timeout}` through the `bx.exec_record` host import
  (see the `exec`/`sh` bullet above; `ms` is wall clock and the command-string
  split does not reproduce the VM's quoting rules yet).
- **Collection builtins**: `push` works in expression position (returns the
  array) and `pop(arr)` returns the removed element for int, float, string,
  text-row, int-row and record arrays (the empty case aborts; the VM returns
  null). `contains(arr, value)` and `index_of(arr, value)` cover int, string,
  float, record, int-row and text-row arrays (structural comparison for the
  container kinds).
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
  f64 slots; `map`/`filter`/`reduce` callbacks and comparator `sort` also run
  over float arrays. Nested float containers (an array of float arrays) are
  still rejected; each new element kind costs a matrix of emitter arms plus host
  imports (the reason a uniform value model is the next design step, see
  `prd/PRD_02_44_bx_tinyvm_absorption.md`).
- **Nested arrays**: `[[1,2],[3,4]]` types as `Ty::ArrArr` (entries hold inner
  array pointers); `len`, element read (including `m[1][0]`), element write,
  single/two-variable `for row in m`, `push`, negative indexes, passing rows to
  functions and printing (`[[1,2],[3,4]]`) all work. Deeper nesting and arrays
  mixing element kinds remain unsupported.
- **Container printing**: `print` accepts int arrays, string arrays, records and
  record arrays and formats them exactly like the VM (compact JSON style:
  `[1,2,3]`, `["a","b"]`, `{"a":1,"b":2}`, `[{"a":1}]`). The guest calls one of
  host imports (`print_arr_i64`, `print_arr_str`, `print_arr_f64`,
  `print_arr_arr`, `print_arr_strarr`, `print_map`, plus the typed
  `print_record_typed`/`print_arr_obj_typed` that take a per-field kind byte
  table) with the container pointer; the host walks memory and prints.
  Functions and map keys still cannot print, and `console_log` stays
  scalar-only.
- **push/pop on every array kind**: both the method form (`xs.push(v)`) and the
  call form (`push(xs, v)`) append a value for int, string and record arrays;
  `pop` shrinks the length and (in expression position) returns the removed
  value for every array kind; an empty array aborts where the VM returns null.
  Growth copies the header and all 8-byte entries, then writes the element
  (int64 / ptr+len pair / record pointer).
- **Empty record literals**: `{}` is now a valid zero-field record, so
  `merge({}, {x: 1})`, `keys({})`, `values({})`, `type({})` and `print({})`
  work (printed as `{}`).
- **Comparator sort**: `sort(arr, fn(a, b) { return a - b })` runs a stable
  bottom-up merge sort in wasm calling the literal comparator (environment
  included); the comparator returns an int whose sign decides the order, the
  array is mutated and returned like the VM. One shared emitter handles every
  element kind: int arrays, string arrays, float arrays, record arrays and
  nested arrays (raw 8-byte slot copies for the merge, per-kind argument
  conversion for the callback). This is O(n log n) callback calls instead of
  insertion sort's O(n²), which brought the 500-record benchmark from 178 ms to
  103 ms (VM 95 ms). Equal keys keep their input order (the merge takes the
  left run on `<= 0`); the collection differential covers ties, both directions,
  empty/single arrays and random sizes. Default `sort` on record, nested and
  text-row arrays is a **no-op** returning the array unchanged, matching the VM
  (it can only default-sort numbers and strings); `reverse` still reorders every
  array kind.
- **Arrays of records**: `[{a: 1}, {b: 2}]` with one consistent shape types as
  `Ty::ArrObj(shape)`; entries hold record pointers in the usual 8-byte slots, so
  `len`, element read (negative indexes included), element write, `for r in xs`
  (single variable = record) and `for i, r in xs` all work, as do record arrays
  passed to functions. Object lists nested inside other array literals keep the
  array pointer in a dedicated local because each record literal allocates
  storage.
- **`parse()` of JSON literals**: `parse("<json literal>")` is folded at compile
  time with the same parser the VM uses and emitted as a typed literal, so
  `parse("{\"x\":9}").x`, `parse("[1,2]")[1]` and
  `parse("[{\"a\":1}]")[0].a` match the VM. Runtime strings (for example
  `exec(...).out`) are rejected with a hint because the static model has no
  dynamic value type; JSON containing `null` or mixed array kinds is rejected
  for the same reason.
- **String predicates**: `starts_with(text, prefix)`, `ends_with(text, suffix)`
  and `contains(text, needle)` work in function form as well as method form
  (`"abc".starts_with("ab")`), all through the `env.str_test` host import;
  `contains` keeps its array meanings as well.
- **Container stringification**: `str(value)` and `json(value)` produce the VM's
  compact JSON for int/string/float/record/nested/text-row arrays and records,
  and `json("a")` quotes/escapes strings like the VM (`json(1)` is `"1"`).
  Interpolation and string `+` stringify containers the same way
  (`"arr=${[1, 2]}"` -> `arr=[1,2]`). One host import
  (`env.format_container(kind, ptr, aux)`) serves every kind, with a 12-byte
  emitter-written descriptor `(kinds_ptr, keys_table_ptr, field_count)` for
  records and record arrays; `env.json_str` escapes strings.
- **Growable empty arrays**: `let xs = []` takes its element kind from the first
  syntactic `push` (`push(xs, "s")` or `xs.push("s")`) of a string, float, array
  or string array, so the VM's accumulator pattern works
  (`let failures = []` then `failures.push("...")`); int stays the default. The
  hint pass is purely syntactic (nested calls and unknown expressions fall back
  to int), and local (function-scope) empty arrays keep the old int default.
- **Method `.pop()` in expressions**: `xs.pop()` works for every array kind in
  expression position (the removed element is the value, the array shrinks),
  matching the VM; `join` also accepts bool arrays (`join([true, false], ",")`
  via `env.join_bool`, default separator empty).
- **Array method forms**: `.join([sep])`, `.sort()`, `.sort(cmp)`,
  `.slice([start[, end]])`, `.sum()`, `.min()`, `.max()`, `.map(cb)`,
  `.filter(cb)`, `.reduce(cb[, init])` and `.reverse()` work in expression
  position for int/float/string/bool arrays
  (bool arrays also gained `slice`). `join` and the method form `.sort(cmp)` now
  also cover record arrays and nested rows: each record/row is stringified
  through `env.format_container` into a temporary string array, then joined, which
  matches the VM's `{"a":1}|{"a":2}` and `[1,2];[3]` output. Record and nested-row
  arrays also support `contains`/`index_of` in both call and method form
  (structural equality, VM ordering preserved).
- **Runtime `parse` (P2a first slice)**: `parse(<runtime string>)` no longer
  needs a compile-time literal — it returns a dynamic value (`Ty::Dyn`, a
  16-byte `[tag][aux][payload]` cell from `env.dyn_parse`) for every scalar JSON
  kind. `print`, `str`, `json`, `type`, interpolation and `let` bindings of
  such values match the VM (`parse(env("X", "2.5"))` prints `2.5`, `json` of a
  parsed string keeps the quotes). Dynamic values also support the VM's scalar
  operations: `+ - * / %` (string concat when either side is text, `null` as 0,
  `%` on integers), ordering and equality (`null == 0`/`false`, numeric
  cross-type), `int()`/`float()`, unary `-`, `!`, if/ternary truthiness and the
  operand-returning `&&`/`||`. P2b containers are also live: runtime JSON
  arrays/objects become nested cell trees, so `parse(env("X", "[1,2,3]"))`
  prints/`str`/`json`/interpolates like the VM, `len()`, indexing (negative
  indexes, out-of-range -> `null`), member access (missing key -> `null`) and
  nested `obj.a[1]` chains work, and `type()` reports `array`/`object`.
  Dynamic containers compare structurally (`==`/`!=` for arrays/objects/nested
  trees) and `for v in <dynamic>` iterates array elements or object keys, like
  the VM; `map`/`filter` over dynamic arrays also work (callback receives a
  dynamic element; results are boxed back into a fresh dynamic array, so
  `arr.map(fn(v) { return v + 1 })` and record fields like `o.a` compose).
  `for k, v in <dynamic>` binds both slots as dynamic cells (array index ints
  and object keys boxed, values via `dyn_value_at`), and `each(dynArray, cb)`
  works. Dynamic arrays also sort (`sort`/`.sort()`, in place, numeric and string
  ordering), reverse, `sum`/`min`/`max` and `join` (both call and method form).
  `+` concatenates dynamic arrays with each other and with static int/string/
  float/bool arrays (`dyn + [3]`, `[0] + dyn`, `dyn + dyn`; static sides are
  converted through `env.dyn_from_array`). Dynamic comparator sort works in both
  forms (`sort(dyn, cmp)`, `dyn.sort(cmp)`): a guest-side insertion sort orders
  cells through the callback (`env.dyn_cell_at`/`dyn_swap`), accepting int- or
  dynamic-returning comparators. Record/nested-array concatenation and
  converting a dynamic value back into a static container remain explicit errors.
  Record fields can also hold dynamic values (field kind 8 stores a dynamic cell
  pointer): `{a: parse("..."), b: 7}` constructs, prints/`json`s, reads (`r.a`
  is dynamic), assigns, compares and works inside record arrays, `join` and
  `merge`, all matching the VM; nested static record/array literal fields
  (`{a: {b: 1}}`) are boxed into dynamic cells too (static records go through
  `env.dyn_from_record`, arrays through `env.dyn_from_array`), so
  `{a: {b: 1}, xs: [1, 2], s: "x"}` prints/`json`s/reads/assigns/nests inside
  record arrays and `merge` exactly like the VM; record printing now sorts keys
  like the VM. Remaining P2c: `get` on dynamic fields and converting a dynamic
  value back into a static container. Dynamic `get`/`has` (numeric keys index
  arrays, string keys read object members, missing values are `null`, the
  optional default applies when the key is absent), `contains`/`index_of`
  (structural equality) and `slice` (negative/clamped bounds) work in function
  and method forms, matching the VM. Dynamic strings behave like the VM too:
  char-based `len`/index (negative/OOB -> `null`)/`slice`, substring
  `contains`/`index_of` (char index), while `get` on strings/arrays-with-string
  keys and `has` on strings stay errors/false exactly like the VM. Dynamic
  containers are mutable like the VM: `a[i] = v` (negative indexes), `o.k = v`
  and `o["k"] = v` (missing keys are appended), and `push`/`pop` in function and
  method form (empty `pop()` is `null`). Compound assignment works on dynamic
  variables, elements and members (`v += 2`, `a[i] += n`, `o.k += n`,
  `o["new"] += n`; `null` starts at 0 and strings concatenate). `print` and
  `eprint` accept any number of arguments like the VM: values are display-
  formatted and joined with single spaces (`print("a", 1, [2,3])` →
  `a 1 [2,3]`; zero arguments print an empty line), which also makes
  `bx check --target wasm` usable as a compile-only gate. `use "file"` is
  Functions whose `return` paths disagree become dynamically typed (each return is boxed, callers see `Dyn`), numeric `abs`/`floor`/`ceil`/`round`/`sqrt` and string `upper`/`lower`/`trim`/`replace`/`split`/`repeat`/`starts_with`/`ends_with` accept dynamic values (function and method form; text goes through `[len][utf8]` blocks) (via `env.dyn_math`/`dyn_text_block`; VM semantics: `has`/`get` with numeric keys are false/`null`), runtime-key **writes** to static records work when the value matches the field kind and the key already exists (inserting a new field cannot grow the fixed record storage, so it fails loudly with `cannot insert a new field in the wasm target`; the VM grows the record) (`r[k] = v` via an object round-trip and `env.dyn_write_record`; changing a field's type stays VM-only), runtime indices work on static records (`r[k]` -> `Dyn`) and dynamic arrays accept string/bool indices like the VM (`a["0"]`, and `it["score"]` reads an object member via `DYN_GET`); error messages carry the VM-style `file(line): ` prefix; VM "errors" (`assert`, `error()`, conversion failures) stay plain while VM panics (division by zero, out-of-bounds) add the frame descriptor ` in name() [path]` exactly like the VM (the path is the verbatim input path; static array indexing out of bounds still panics where the VM yields `null`, a documented divergence); literal-key record growth (`o.b = 2`, including an empty `let e = {}` record) reallocates the record, copies the existing slots and updates every tracked alias, and `len(record)` returns the compile-time field count; static values (records, arrays, strings, numbers) box into a `Dyn` parameter at the call site, and a function whose body returns a dynamic value widens its result to `Dyn`, so helpers like `fn pick(o, k)`, `fn twice(xs)` or `fn lineTotal(it)` work with both parsed and static arguments; `+=` also concatenates strings (`s += str(i)`) like the VM; an empty growable array pushed with a container or unknown-kind operand becomes a dynamic array (so `transposed.push(col)` and arrays of records work), and `.push` on a dynamic value now works even when the receiver is a record field (`rec.scores.push(9)`); compound assignments (`total += dyn`, `record.field += dyn`, `array[i] += dyn`) unbox the dynamic value through `DYN_TO_INT`/`DYN_TO_FLOAT` for numeric targets, so loops that accumulate dynamic elements work like the VM; writing or pushing a `Dyn` value into an int/float slot (static array element, int/float record field, `push`) unboxes it (`DYN_TO_INT`/`DYN_TO_FLOAT`); bool/string field assignments with a dynamic value promote the whole record variable to a dynamic object (VM replacement semantics: `r.b = dyn` stores the value), while writing a dynamic value into a bool/string array element (and int/float arrays too) promotes the array variable to a dynamic array (`dyn_from_array` + `dyn_set_at`), and `push` of a dynamic value promotes the same way (`dyn_from_array` + `dyn_push`, function or method form), matching the VM replacement behaviour (`[true,false]` -> `[7,false]`, `["a","b"]` -> `["a",7]`); indexing a dynamic value whose tag is null raises the VM panic "cannot index null (missing value?)" (checked with the new `env.dyn_tag` host, catchable by `attempt()`), so missing nested fields behave like the VM; assigning or passing a `Dyn` value where a static scalar is expected unboxes it (`DYN_TO_INT`/`DYN_TO_FLOAT`/`DYN_TRUTHY`/`DYN_STR`), so `for x in dynamicArray { total = total + x }`, dynamic method chains and mixed dynamic arguments work like the VM; runtime-key insertion (`o[k] = 3`) promotes the variable's static record to a dynamic object built through `dyn_from_record`, inserts the field with `dyn_set_member` and stores the cell back, so `tests/cases/loops.bxs` now runs end to end (only plain variables are promoted — aliased or captured records are rejected, and an in-place field update before the insertion must come after only growth writes, otherwise the compiler asks to reorder), while the bench scripts under `tests/bench-wasm/` are covered by the suite differential (the file basename plus the statement line; runtime messages are joined through the new `env.text_concat` host, e.g. `int("x")` reports `cannot convert 'x' to int`), `attempt()` also catches `assert` failures, division by zero and `int()`/`float()` parse failures (the host conversions report a sentinel so the guest can raise an attempt-aware error; the message omits the VM's `file(line): ` prefix), a generic dynamic `reduce()` boxes scalar callback results (int/float/bool/string/null) into cells before continuing, so callbacks whose parameters are dynamic but whose result is a scalar work instead of producing an invalid module; builtin method syntax (`a.sum()`, `s.split(",")`, `r.get("x")`, `a.sort()`, `a.reverse()`, `s.upper()`, ...) lowers to the same-named function call, while `push`/`pop` keep their array growth path and `sort(cmp)` keeps its dedicated comparator lowering; functions and lambdas without a `return <value>` are supported: calls yield `null`, a bare `return` is allowed, and `attempt()` callbacks that fall through store a null value (a function that returns values on some paths and falls through on others becomes `Dyn` and yields `null` on the fall-through paths exactly like the VM); record literals keep array/object fields as boxed dynamic cells (kind 8) when a function returns them, so `r.y` and `r.y[0]` work (previously such fields were mis-typed as ints); `merge(left, right)` resolves duplicate keys with the right record's field kind, so a dynamic-valued field overrides an int field (and vice versa) like the VM; a literal `parse("...")` argument is constant-folded only when the JSON has no nulls/unsupported shapes; otherwise it falls back to the runtime parser and yields a dynamic cell (so `parse("{\"k\":[1,null]}")` and mixed JSON work), conditions use VM truthiness: `if`/`while`/`for` conditions, ternaries and `&&`/`||` accept bools, numbers (non-zero), strings (non-empty) and dynamic cells (`DYN_TRUTHY`), preserving short-circuit side effects; array `push` grows in place when the array is still the bump-allocator top, so aliases (`let b = a`) see the growth like the VM; when it has to reallocate, every statically tracked `let` alias is updated too (aliases created through function parameters or stored containers are still not tracked); `attempt()` records now store both `value` and `error` as dynamic cells (VM shape): a successful call prints `null` for `error`, a failure boxes the host message into a text cell (the VM also prefixes the source location; the wasm message omits that prefix), and `attempt(fn() { return null })` is supported: the record value is a dynamic null cell (`value` prints `null` and `type(value)` is `null`), while the success `error` field remains an empty string rather than the VM's null; `write`/`write_file`/`append`/`mkdir`/`remove` return `null` like the VM (a plain write traps with the host message on failure, while `attempt()` captures the same message), and `remove(path)` deletes empty directories as well as files, `split(text, "")` splits into Unicode chars like the VM (both static and dynamic strings; the previous non-empty-separator restriction is gone); float arrays reduce with an int init by promoting the accumulator; `reduce` and `sort` over `map`-produced dynamic arrays with numeric callbacks work by unboxing cells (`dyn_len` is i64; the numeric accumulator/comparator is converted before the call), while array literals that contain a dynamic element soft-type as dynamic, so `map` results built from nested dynamic values (like `[it.score, it.name]`) become `Dyn` and nested-array comparators (`sort(byScore, fn(a, b) { return b[0] - a[0] })`) compile and run; `tests/cases/data.bxs` is now an end-to-end differential case compared with the VM in the suite; runtime-key **writes** to static records stay VM-only; mixed number/string dynamic arrays keep VM ordering (`sort` leaves them unchanged, `min`/`max` skip incomparable elements; the `<`/`>` operators still raise like the VM), `get(record, runtimeKey[, default])` views the record as a dynamic object, so heterogeneous shapes work (`get(o, k)` -> `Dyn`, literal keys keep the static fast path), record `values()` builds a `Dyn` array (so `values({a: [1,2], b: 7})` is `[[1,2],7]` and mixed scalar kinds work), dynamic objects expose `keys()`/`values()` (function and method form, `Dyn` arrays), `get`/`has` as methods, and `for k, v in record` boxes mixed-kind fields into dynamic cells (so a string or array field prints like the VM; all-int records keep the static fast path), mixed-return `reduce` over a static array promotes the accumulator to a dynamic cell (with an init argument it is boxed; without one the first element seeds it via `env.dyn_from_array` + `dyn_index`, and an empty array panics like the VM) (`reduce([1,2], fn(a,b){ if (b==1){ return a+"x" } return a+b }, "")` -> `x2`; without an init the accumulator stays the element type and mixed returns stay VM-only), and dynamic `reduce` works over parsed arrays and static arrays with a dynamic init value (accumulator and elements are cells; without an init argument the first element seeds the accumulator), and `filter` accepts any callback result with VM truthiness (bool, numbers non-zero, non-empty strings, dynamic cells via `DYN_TRUTHY`), and `map` over a static array with such a callback builds a dynamic array (`map([1,2], fn(v){ if (v>1){ return "s" } return v })`); mixed-return functions (each return is boxed, callers see `Dyn`), so `fn f(x) { if (x) { return 1 } return "s" }` works directly; callbacks with mixed returns inside `map` over static arrays remain a boundary. Mixed static literals (`["a", 1]`, `[1, "x", true, null]`, rows of different record shapes) lower to dynamic cells at construction, and an empty `let xs = []` whose pushes use different kinds becomes a dynamic growable array (top level and inside functions), so they print/`json`/index/iterate/compare exactly like runtime-parsed JSON; typed homogeneous literals keep their static fast path. `use "file"` is expanded before compilation for `bx build/run/check --target wasm` (paths
  resolve relative to the including file; cycles and >16 nesting levels are
  errors), so shared helper modules compile and run like the VM. Fixed a latent stack imbalance in string
  boxing that these paths exposed.
- **`null` (static marker)**: `null` is typed `Ty::Null` and covers the VM's
  scalar semantics: printing/`str`/`json`/`type`/interpolation give `null`,
  `null == null`, `null == 0`, `null == 0.0`, `null == false` are true while
  `null == ""` is false, ordering treats null as 0, arithmetic coerces it to
  0/`0.0` (`null + 1` -> `1`, `"a" + null` -> `"anull"`), `int(null)`/`float(null)`
  -> 0, `!null` -> true, and conditions/ternaries see it as falsy. Arrays and
  records still cannot hold null, and `parse` of runtime text is still P2a work
  owned by PRD_02_44; `tmp == null` style checks now work, which moved
  `verify.bxs` past the old "no null value" blocker to its `test()` boundary.
- **Bool arrays**: `[true, false]` types as `Ty::ArrBool` and supports the whole
  surface: printing (`[true,false]` via `env.print_arr_bool`), `len`, indexed
  read/write, `push`/`pop`, `contains`/`index_of`, default and comparator
  `sort`, `reverse`, concatenation, structural equality, `for` loops,
  `map`/`filter`/`reduce` (including bool results and bool accumulators),
  `min`/`max`/`sum` (`sum` counts trues like the VM), `str`/`json`/
  interpolation. Nested bool arrays (`[[true]]`) are still rejected.
- **Array concatenation**: `a + b` (and `a += b`) concatenates arrays of the same
  element kind — int, string, float, record, nested and text-row arrays — into a
  new array, matching the VM for same-kind inputs. Mixed-kind concatenation
  (`["a"] + [1]`, `"x" + [1]`) is rejected with a clear message because the VM
  produces a dynamic array/stringified value the static model cannot type.
- **Record merge**: `merge(a, b)` on records with statically known shapes builds
  the union shape at compile time (keys of `a` in order, then new keys of `b`);
  overlapping keys take `b`'s value, matching the VM. Chained merges work and
  merging with an empty-object literal (`{}`) works too.
- **Record builtins**: `keys` -> string array, `values` -> int array, `has`, and
  `get(record, key[, default])` walk the per-shape key table at runtime with
  `str_compare`; a missing key without a default aborts (`get() missing key with
  no default`) instead of returning the VM's null. `get` also accepts a dynamic
  key when the record's fields share one type, and `values()` is int-only.
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
  invocation. Remaining limit: a variable cannot hold function literals from two
  different syntactic sites (`requires stable types`).
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
  iteration. Measured with the absorbed core (wall clock incl. process start,
  `BX_BENCH_RUNS=9`): fib ≈1.73×, loop ≈0.72×, calls ≈1.55× of the VM's time.
- **Robust rejection**: malformed programs are rejected with a message, never a
  compiler panic. Two unit tests cover 30 hand-written wrong-type/unknown-call
  templates and 300 seeded random expression programs with `catch_unwind`;
  unknown or wrong-arity calls report
  `wasm target does not support <name>() with <n> argument(s)` instead of a
  generic "call" message.
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
  the same budgets as the native VM path to the wasm core (instruction
  counter plus a periodic deadline check), so untrusted modules cannot spin
  forever. The normal-path hot loop adds only a counter increment per
  instruction; the deadline is sampled every 1024 instructions.
- **Browser demo**: `bx build --wasm --html script.bxs -o demo.html` writes a
  self-contained page (module embedded as base64) plus a sibling `demo.js`. The
  glue implements the same import namespaces as the native host (`env.print_*`,
  `bx.argc/arg`; file/process imports are stubbed out) and prints into `<pre>`
  in a browser or stdout under node. The suite and verifier run `node demo.js`
  when node is available and compare against the native module output.
- **Performance, historical (in-tree interpreter era, before PRD_02_44 P1)**:
  fib 108 ms (VM 131 ms), 1e6-iteration loop 205 ms (VM 358 ms), 200k helper
  calls 128 ms (VM 184 ms). That engine was deleted when the vendored `tinyvm`
  core became the only engine; current numbers are in "Engine performance" at
  the top of this module. The two fixes that got the old interpreter there
  (a per-frame label base, and a call path that reused locals and left results
  on the operand stack) are recorded here because the label-base bug also
  silently skipped code after a block and is easy to reintroduce.

## Compatibility boundary

The wasm target models values statically; the VM is dynamically typed. Runtime
JSON and null are already handled through the P2 dynamic-cell layer
(`void`-free `Ty::Dyn` values over `parse()` results), so the remaining
boundaries are narrower and produce explicit errors (the CLI adds a
`BX_TARGET=bxb` hint), not silent behaviour changes:

| Boundary | Example | Workaround |
| --- | --- | --- |

| static typing across the dynamic boundary | passing a runtime-parsed `Dyn` container into a function whose parameter was inferred as a static array | keep such helpers dynamic-agnostic (`len`, `for`, dynamic methods) or use the VM |
| VM-only builtins | `test(...)`, `patch(...)`, `spawn(...)`/`spawn_*` handles | run with `--target bxb`; the wasm errors name the replacement (`bx test`, `write_file`/`exec`) |
| host traps | uncaught `read`/`exec` failures outside `attempt` | wrap in `attempt`, or use the VM |
| static typing across the dynamic boundary | passing a runtime-parsed `Dyn` array into a user function whose parameter was inferred as a static array | keep such helpers dynamic-agnostic (`len`, `for`, dynamic methods) or use the VM |

`bx/tools/wasm-diff*.bxs` deliberately avoid these boundaries so a differential
failure always means a real divergence; `bx/tools/verify.bxs` stops at the same
points when built for wasm. How these boundaries get removed is owned by
`prd/PRD_02_44_bx_tinyvm_absorption.md` ("P2 design: uniform tagged values,
hybrid"), which stages a 16-byte tagged cell behind static/dynamic boundary
conversions rather than rewriting the static fast path (`P2a`/`P2b` shipped
2026-11-18).
- **Inference convergence (fixed 2026-11-18)**: `InferState` now seeds the type
  of an empty-array `let` from the same push-hint map the checker uses, so a
  growable `[]` declared inside a loop or function can feed `map`/`filter`/
  `reduce` callbacks without the old `function returns both bool and int values`
  error. Both the hint walker and the inference pass descend into function
  bodies; unit 156 and suite 241 pin the loop and function-local forms.
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
| M2 | data model: strings, arrays, objects in linear memory | `[v]` — strings (incl. runtime concat/chars), int/float/bool/string/record/nested arrays, growable arrays, records with dynamic-cell fields (kind 8), and dynamic containers over runtime JSON |
| M3 | functions, closures, control flow compiled to wasm | `[v]` — control flow, recursion, literals with captures (boxed cells), `use` module expansion, higher-order callbacks and dynamic comparator sorts |
| M4 | builtins + native process ABI (`bx.*`), default target flip | `[-]` (argv/file/process/env/time/dir imports, multi-arg print/eprint and `bx check --target` done; default target flip next) |
| M5 | `web.*` + `webgl.*` ABI, software backend, browser demo | `[-]` (demo, `web.console_log*`/`web.performance_now`, and a `webgl.*` clear/read/PNG slice done; shader/buffer surface next) |
| M6 | remove `.bxb` + legacy code paths, six-cell wasm evidence | `[_]` |
