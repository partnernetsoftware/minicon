# `minicon-bx` absorbing the `tinyvm` core

Owner of the decision to replace the `bx` wasm runtime with the `tinyvm`
engine, and of the migration that lets `tinyvm` be retired with `minicon-bx`
carrying its needs. Status: `[-]` decision accepted; P0 completed 2026-11-17; **P1 completed
2026-11-17** — the vendored core is the only wasm engine in the product. P2
(uniform value ABI) is next.

Related: [bx surface](PRD_02_41_bx.md) · [wasm target](PRD_02_42_bx_wasm_target.md) ·
[wasm migration](PRD_02_43_bx_wasm_migration.md). `tinyvm` facts below are owned
by the `tinyvm` repo (`prd/PRD.md` there); this module owns only the absorption
plan and its gates.

## Decision

Absorb `tinyvm`'s **interpreter core and host boundary**, not its product skin:

- Execution format becomes **standard WebAssembly**, executed by the absorbed
  `tinyvm` core (decode / validate / instantiate / interpret, step and memory
  budgets, host import table, replay, C ABI).
- `bx` becomes a **language skin over wasm** (it already emits standard modules);
  the in-tree interpreter `bx/src/wasm.rs` is retired in favor of the core.
- The `.com` CLI stays the product face for agents; the **embeddable core**
  (static lib / XCFramework / C ABI) is what replaces `tinyvm` for iOS and
  `agenterm`.
- The typed-subset matrix in `bx/src/wasm_target.rs` (one `Ty` variant and one
  host import per element kind) is replaced by a **uniform tagged value ABI**;
  new container kinds stop costing a matrix.

This is a product-sentence change: from "a scripting tool with an experimental
wasm backend" to "a portable programmable core (standard wasm) with `bx` as one
language skin and a `.com` CLI". It corrects the road that the 2026-11-17
review exposed: a dynamic language compiled through a combinatorial static type
matrix costs a matrix per feature (float arrays alone: ~400 emitter lines, four
host imports, and an invalid-module bug our own interpreter did not catch).

## Evidence for feasibility (2026-11-17 inventory)

| Fact | Number |
| --- | --- |
| `tinyvm` core interpreter (`crates/tinyvm/src/wasm.rs`) | 11,513 lines |
| `tinyvm` crate test files | 54 |
| `tinyvm` suite reported in its PRD | 314 / 1 |
| default stripped static core | 101,256 B (< 100 KiB gate) |
| accepted standard feature families (independent fixtures + WABT/JSC oracles) | 10 |
| opt-in SIMD core | 117,800 B |
| iOS arm64 / x86_64 consumers (default) | 1,791,960 / 1,897,232 B |
| `tinyvm-qjs` (second language skin) | 29,940 lines |
| `minicon-bx` source today | 21,471 lines (wasm target 9,829; own interpreter 3,662) |
| `tinyvm` host boundary | `HostBackend` (WASI-shaped) **plus** arbitrary registered host functions (`Fn(&[i32], &mut [u8])` / typed `Val`), so `bx`'s `env.*` imports can be served without changing the ABI first |
| portability | `no_std` + `alloc`, no `fmt`, no `unsafe`, runs on iOS and `wasm32` |
| toolchain | `tinyvm` is Rust 2024 / 1.97; the `minicon` workspace is Rust 2024 / 1.97 |

The decisive compatibility question is answered: the core executes arbitrary
imports through a host registration table, so it can host `bx`'s `env.*`
surface; `bx` already emits standard modules (`\0asm`, version 1).

## Tree DAG

```text
minicon-bx owns a standard wasm core and one language skin      [-]
│
├── P0 decisive experiment                                      [v]
│   ├── core linked into an ignored harness (path crate)         [v]  not yet a workspace member
│   ├── `env.*` host profile over the core's host table         [~]  ~45 of 68 imports bound
│   ├── bx differential run on the core                          [v]  300 cases
│   ├── module accepted by an independent engine (node)          [v]  existing demo already proves the emitter
│   └── core-inside-wasm + `.com` size/startup measurement       [_]  linked std delta measured only
│
├── P1 engine swap                                              [v]
│   ├── `bx run --wasm` executes through the core                [v]  2026-11-17
│   ├── validation errors surface with function name/line        [v]  `bx check`/`bx run` reject with site
│   ├── `bx/src/wasm.rs` interpreter retired                     [v]  decoder/interpreter/tests deleted; encoder+host remain
│   ├── step/memory budgets owned by the core                    [v]  steps, interrupt, memory pages
│   └── core vendored as a workspace member                      [v]  `bx/vendor/tinyvm` (no crates.io wasm crate)
│
├── P2 language front end simplification                        [-]  hybrid tagged-value P2a/P2b shipped 2026-11-18; P2c/P2d pending
│   ├── uniform tagged value ABI (containers hold values)        [v]  16-byte cells; runtime parse, scalar ops, arrays/objects, iteration/map/filter/sort/join/concat (144 unit, 232 suite)
│   ├── element-kind matrix and its host imports deleted         [_]
│   ├── `.bxb` default flip and retirement (PRD_02_43)           [_]
│   └── float/nested/mixed containers from the uniform layout    [-]  scalar/array/object dynamic values done; record field kind 8 (dynamic cells in records) landed, nested static literal fields pending
│
├── P3 embeddable product face                                  [_]
│   ├── static lib + C ABI built in the minicon repo             [_]
│   ├── XCFramework / Swift package scripts moved over           [_]
│   ├── `tinyvm`-named compatibility shim for one release        [_]
│   ├── agenterm embedder switched                               [_]
│   └── TinyArcade app switched                                 [_]
│
└── P4 tinyvm retirement                                        [_]
    ├── downstream cutover evidence for both embedders           [_]
    ├── CI moved into minicon (wasm cells, iOS budgets)          [_]
    └── tinyvm repo archived, docs ownership transferred         [_]
```

## Memory palace

```mermaid
flowchart LR
  SRC[bx source] --> EMIT[bx wasm emitter]
  EMIT --> WASM[standard .wasm]
  WASM --> CORE[absorbed tinyvm core<br/>decode+validate+interpret+budgets]
  CORE --> HOST[host profile<br/>env.* / WASI / cartridge]
  HOST --> CLI[.com CLI for agents]
  HOST --> IOS[iOS XCFramework + C ABI]
  HOST --> EMB[agenterm embedder]
  EMIT -.->|P2| TAG[uniform tagged values]
  TAG -.-> EMIT
  CORE -.->|retires| OLD[bx/src/wasm.rs interpreter]
  CORE -.->|retires after cutover| TV[tinyvm repo]
```

## P0 results (2026-11-17)

Experiment shape: the `tinyvm` clone at `D:\dev\partnernetsoftware\tinyvm`
(`f476cd2`) was linked into an ignored harness
(`bx/target/p0/harness`, not committed). It registers the `env.*` imports the
probe programs call through `Module::bind_import_typed`, instantiates the
module, and invokes the exported `_start`. A `bx-shim` binary delegates
everything that is not `run <module>.wasm` to the real `bx`, so the existing
`bx/tools/wasm-diff.bxs` differential could run unchanged with the core as the
wasm runner.

| Evidence | Result |
| --- | --- |
| feature matrix (closures/records/strings/maps/arrays/floats/nested/sorts/control) | 7/7 modules byte-identical between the core and bx's interpreter |
| 300-case generated differential (`wasm-diff.bxs` via shim) | `PASS 300 differential cases`; no rejected module |
| `bx` gates after the fix below | unit 91, suite 180, verifier 76, native differential 300 |
| linked size (std harness) | 523,776 B vs 121,344 B control ⇒ ~402 KB delta on `.com` (published static core in tinyvm: 101,256 B) |

Findings that change the plan:

1. **The validator pays for itself immediately.** It rejected the differential's
   first case with `validation: type mismatch in function #68`. Delta-reducing
   the 183-chunk program found `grow.push(8)`: the int-array `push` path loaded
   the value into an `i64` local and then wrote it through an `i32` local
   (i32/i64 store mismatch). bx's own interpreter accepted it and printed the
   right answer; node had also been validating less strictly than tinyvm here
   for method-form pushes. Fixed in the emitter
   (`bx wasm: fix int push local types`), after which the 300-case differential
   ran clean.
2. **The host ABI was under-specified.** Porting the host surfaced two implicit
   contracts: `print_record`/`print_arr_obj` take `(table, count, pointer)` (not
   pointer-first), and `str_char_at` accepts a negative index meaning "count
   from the end" (bx's native host and the JS demo both normalize it; a naive
   host returns an empty string). Both are now written into
   [PRD_02_42_bx_wasm_target.md](PRD_02_42_bx_wasm_target.md) as the host
   contract every host must satisfy.
3. **Do not ship two engines.** The invalid module passed every existing bx gate
   because the in-tree interpreter does not type-check locals. P1's first
   safety rule is therefore: once the core executes the module, delete the
   in-tree interpreter rather than keeping it as a lenient fallback.
4. **Host coverage is the real P1 work.** The shim binds ~45 of 68 imports;
   `web.*`/`webgl.*`, `bx.run`/`bx.capture`, and some container printers were not
   exercised. P1 must port the whole surface as one Rust host profile and prove
   it with the differential plus the browser demo.
## P1 progress (2026-11-17)

- `bx/vendor/tinyvm` is the owned core crate (the `tinyvm` name is kept so a
  future upstream drift check can diff it), `std` feature only, provenance in
  its README. `cargo tree -p bx` now shows `bx → tinyvm → libm`; there is still
  no third-party WebAssembly runtime crate.
- `bx run <module.wasm>` and `bx run <source>` (wasm target) execute through the
  core by default. `BX_WASM_OWN=1` still reaches the in-tree interpreter for one
  release of engine differential; unit tests keep using it for now.
- The host surface is **not duplicated**: the core adapter (`bx/src/wasm_core.rs`)
  converts `tinyvm::Val` to bx's `WVal` and calls the existing `PrintHost` arms,
  so the ABI keeps one implementation. This answers the P0 ABI-drift finding
  structurally instead of by documentation.
- Validation is now a product surface: `bx check file.wasm` and `bx run
  file.wasm` reject the P0 invalid-module class with `wasm module rejected:
  validation: type mismatch in function #0`; budgets map to core limits
  (`--max-steps` → step budget, `--timeout` → interrupt).
- Evidence: unit 93 (91 + two core tests: reject-invalid, run-compiled), suite
  180, verifier 76, local differential 300 and packed-product differential 300
  all on the core; corpus (floats/nested/sortstr/maps) is byte-identical across
  core engine, own interpreter and VM. `.com` grew from ~1.92 MB to ~2.17 MB
  (win-x64; +243 KB with LTO+strip).
- Remaining P1 work: delete the interpreter and its decoder, move the unit tests
  that use `execute` onto the core, then remove `BX_WASM_OWN` and the
  `#[allow(dead_code)]` exceptions.
## P1 completion (2026-11-17, same day)

- The in-tree interpreter is gone: `bx/src/wasm.rs` keeps only the encoder
  (`Builder`/`Instr`) and the native host (`PrintHost`), 2,139 lines instead of
  3,704. The decoder, `Exec`, block/loop interpreter, compare/binop helpers and
  their unit tests were deleted; `BX_WASM_OWN` and the `#[allow(dead_code)]`
  exceptions went with them.
- `bx check --json` keeps its shape through a small section scanner
  (`imports`/`exports`/`defined_functions`) instead of the deleted decoder;
  function exports only, matching the old output.
- Evidence after deletion: unit 88 (the 91-unit figure included interpreter
  tests; end-to-end coverage moved to the core), suite 180, verifier 76,
  differential 300, corpus (floats/nested/sortstr/maps) identical to the VM,
  `bx check --json` lists all 68 imports and `_start`, clippy clean.
- Safe failure recorded: no second engine remains, so a semantic regression
  now shows up as a core/bx gate failure rather than as "the interpreter
  accepted it".
## Gates

| Gate | Meaning | Evidence required | Status |
| --- | --- | --- | --- |
| G0 | absorption is worth it | 300-case differential and a 7-case feature matrix run byte-identically through the core with a shim host; the core's validator caught one real emitter bug (int-array `push` local types) | `[v]` |
| G1 | one engine | `bx/src/wasm.rs` interpreter deleted; no dual semantics in the product | `[_]` |
| G2 | no matrix tax | a new container kind is added by one representation change, no per-operation host imports | `[_]` |
| G3 | embeddable parity | static lib + C ABI passes `tinyvm`'s C-ABI smoke shapes; XCFramework + Swift smoke exit 0 in CI | `[_]` |
| G4 | downstream cutover | `agenterm` and the TinyArcade app consume `minicon-bx` artifacts, both suites green | `[_]` |
| G5 | retirement | `tinyvm` repo archived; any remaining feature is explicitly dropped with a recorded reason | `[_]` |

Safe failure: P0 is a decisive experiment, not a promise. If the core cannot
host the `env.*` profile or the wasm32/`.com` shape regresses badly, the fallback
is to absorb only the **validator + budgets + replay discipline** into the
existing interpreter and stop the plan, with the measurements recorded here.

## P2 design: uniform tagged values, hybrid (2026-11-18)

The static `Ty` matrix now covers a large but finite surface (int/float/string/
bool scalars, arrays of each scalar, record arrays, nested int/text rows, maps),
each new kind costing emitter arms, host imports and a checker/element mapping.
Everything the VM does dynamically still fails: `parse(exec(...).out)`, `null`,
heterogeneous containers, record fields that hold containers, callbacks whose
result type depends on input. P2 removes that ceiling with a **hybrid value
model** rather than a rewrite, so the static fast path and its performance stay.

### Representation

One dynamic cell is 16 bytes:

```
[tag u32][aux u32][payload 8 bytes]
```

| tag | meaning | aux | payload |
| --- | --- | --- | --- |
| 0 | null | 0 | 0 |
| 1 | int | 0 | i64 |
| 2 | float | 0 | f64 bits |
| 3 | bool | 0 | 0/1 |
| 4 | string | byte length | ptr to UTF-8 |
| 5 | array | element count | ptr to `[16-byte cell]*N` |
| 6 | map | entry count | ptr to insertion-ordered `(key, cell)` pairs |
| 7 | record | shape id | ptr to `[8-byte slot]*fields` (static layout) |
| 8 | function | literal id | env ptr |

Records keep the existing 8-byte static field slots; a field holding a container
is kind `8` and stores a pointer to a 16-byte dynamic cell. Arrays of dynamic
cells are contiguous, so `for`/`map`/`filter`/`sort` loops stay guest-side.

### Boundary rules

- Static code stays static. `Ty::Dyn` appears only at dynamic boundaries:
  `parse` over runtime text, `exec`/`sh` record fields if widened, `get` on
  heterogeneous records, and any value whose kind the checker cannot prove.
- `to_dyn(static)` boxes a static value into a cell; it is a no-op for values
  already stored as cells.
- `from_static` helpers (`dyn_int`, `dyn_str`, `dyn_float`, `dyn_bool`,
  `dyn_array`) abort with `cannot use <type> as <type>` unless the tag matches
  (integers promote to floats), mirroring the VM's runtime errors; inside
  `attempt` those aborts become the usual caught errors.
- Static containers that receive a `Dyn` element widen to a dynamic array; the
  reverse narrowing never happens implicitly.
- Printing/`str`/`json` for dynamic values reuse the VM's formatter semantics
  (null -> `null`, strings quoted in JSON, insertion order for maps).

### Host ABI (additive; existing 100 imports stay)

```
env.dyn_alloc(cells) -> ptr
env.dyn_type(cell_ptr) -> tag
env.dyn_len(cell_ptr) -> i64
env.dyn_index(cell_ptr, i) -> block(16-byte cell)      // arrays/strings
env.dyn_member(cell_ptr, key_ptr, key_len) -> block    // records/maps
env.dyn_set_index / dyn_set_member                     // writes
env.dyn_eq(a, b) / dyn_cmp(a, b) -> i64
env.dyn_add(a, b) -> block                             // numbers, strings, concat
env.dyn_truthy(cell_ptr) -> i32
env.dyn_to_str(cell_ptr, mode) -> block                // str/json
env.dyn_to_int / dyn_to_float / dyn_to_bool
```

Cells are written/read by the emitter with plain `i32`/`i64` loads; the host is
only needed where the VM semantics are non-local (key lookup, formatting,
number/string coercion).

### Milestones and gates

| step | scope | gate |
| --- | --- | --- |
| P2a | `Ty::Dyn` scalars + `null`, `dyn_*` conversions, `parse(exec.out)` for 1/1.5/"a"/true | `[v]` 2026-11-18 — runtime `parse` of scalar JSON returns `Ty::Dyn`; print/`str`/`json`/`type`/interpolation, arithmetic, equality/ordering, `int`/`float`, `!`, truthiness, operand-returning `&&`/`||` all match the VM |
| P2b | dynamic arrays/maps/records, heterogeneous containers, `dyn_index`/`dyn_member`/`dyn_set_*` | `[v]` 2026-11-18 — arrays/objects parse to nested cells; len/index (negative, OOB→null)/member (missing→null)/nested chains, structural equality, `for`/`for k,v` iteration, `each`/`map`/`filter`, `sort`/comparator-sort/reverse/`sum`/`min`/`max`/`join` and dynamic↔static concatenation match the VM (162 unit, 249 suite; numeric/coll/method/attempt/dynamic differentials and benchmarks re-run). mixed static literal containers (`["a", 1]`, differing record shapes) also lower to dynamic cells (2026-11-18); remaining: record/nested-array concatenation and Dyn→static container conversion |
| P2c | record field kind 8 (pointer to cell) + `get`/`merge` on heterogeneous shapes | `[-]` 2026-11-18 — field kind 8 holds dynamic cell pointers: `{a: parse(...), b: 7}` constructs/prints/`json`s/reads/assigns/compares, and works in record arrays, `join` and `merge` (146 unit, 233 suite). nested static literal fields are boxed through `env.dyn_from_record`/`dyn_from_array` (2026-11-18), so `{a:{b:1}}`, `{xs:[1,2]}` round-trip too; dynamic `get`/`has`/`contains`/`index_of`/`slice` and dynamic mutation (index/member assignment with key insertion, `push`/`pop`, compound assignment) also landed (2026-11-18) and `bx/tools/wasm-diff-dyn.bxs` randomly diffs the dynamic paths (scalars, arrays, objects, records with dynamic fields, nested values, dynamic strings; 5 seeds x 40 plus 4 seeds x 200 cases PASS (mixed record fields, heterogeneous arrays, arrays of parsed objects included); 12 cases run in `test.sh`); remaining: Dyn→static container conversion |
| P2d | front-end simplification: delete element-kind matrix arms/imports that Dyn makes redundant, flip `.bxb` default (PRD_02_43) — `[_]` not started | one shared value path; import count and emitter arm count drop; suite/verify/both differentials green |

Rollout (updated 2026-11-18): no feature flag was needed — `Ty::Dyn` only
appears for values whose kind the checker cannot prove statically (runtime
`parse` results), so the static fast path keeps its exact code shapes and the
dynamic path is opt-in by data, not by a build switch. P2a/P2b are shipped and
`verify`/the four differential tools stay green (numeric/coll/method diffs
3 seeds × 20 cases each, attempt 3 × 12, re-run 2026-11-18); P2c (record-field
containers) and P2d (delete the now-redundant element-kind matrix, flip `.bxb`
default) are still open.
Performance budget: static benchmarks (`fib`, `loop`, `calls`, `coll`, `recs`)
must not regress more than 5%; dynamic-heavy workloads (parse/exec output
pipelines) target at least 0.5x VM time. The hybrid design is what keeps that
promise: the hot static path never touches a tagged cell.

Risks and rollback: the 16-byte cell doubles container memory for dynamic data
(acceptable: dynamic data is I/O-sized, not hot loops); shape ids must stay
stable across the checker and host; mixing static and dynamic records needs a
conversion audit (P2c). If P2a/P2b fail their gates the Dyn arm remains behind
`BX_WASM_DYN`, and the static matrix stays the shipped path exactly as today.

## Open decision

"Replace `tinyvm`'s needs" has two possible scopes:

- **A — engine + embedding** (recommended first): the core, host profile,
  budgets, replay, C ABI, iOS build scripts. TinyArcade-specific modules
  (cartridge, catalog, media, game runtime) stay in a feature layer or remain
  owned by the downstream app.
- **B — full platform**: also absorb cartridge/catalog/trust/media/game runtime
  and `tinyvm-qjs`. This is a product-boundary change ("minicon-bx becomes the
  game/agent platform") and roughly doubles the absorbed surface (game.rs 1,458
  lines, ios_c_api.rs 3,661 lines, qjs 29,940 lines).

P0 does not depend on the answer; G3 onward does.

## Non-goals

- No JIT/AOT. The core stays an interpreter; iOS forbids generated code.
- No third-party wasm runtime crate. The core is owned source either way.
- No new language. `bx` remains the language; `qjs` is a migration question
  (compatibility shim vs rewrite) decided before P4, not silently dropped.
- No byte-budget regression of the `.com` product beyond the measured core cost.
