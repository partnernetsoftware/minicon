# `bx` binary-scripting surface

Owner of the `bx` runtime vendored at `bx/`, its per-OS `.com` delivery through
the Rust launcher lane, and the evidence that the packed artifact still behaves
like bx. MiniCon's own packaging stays with [delivery](PRD_02_27_con_delivery.md);
bx is a sibling product that ships from this repository.

## Product outcome

An agent writes a short JS-like script; bx compiles it to bytecode (`.bxb`) and
runs it in a stack VM whose primitives are native processes (`exec`/`sh`). It
ships as one file per platform, next to MiniCon, with no Node install and no
runtime dependency. A failed runtime operation is either a value (`attempt`) or
an exact nonzero exit, never a mangled status.

## Tree DAG

```text
bx — binary scripting surface
├── language and VM → bx/README.md
│   ├── [v] compile-to-bytecode + `.bxcache`
│   ├── [v] iteration protocol: array | string | object keys, plus `for k, v in`
│   ├── [v] `attempt(fn)` error capture
│   ├── [v] closures share mutable locals at any nesting depth (per-call cells; per-iteration loop captures)
│   ├── [v] `c ? a : b` and `"${expr}"` interpolation (`\$` escapes a literal dollar)
│   └── [no] closures, try/catch, regex (roadmap, not claimed)
├── delivery → ../loader/bx/README.md
│   ├── [v] `MINICOM1` launcher lane (Rust; no cosmocc, no C)
│   ├── [v] four cells packed locally: {win,lnx} × {x86_64,aarch64}
│   └── [-] six-cell execution evidence (CI `bx_lane`)
└── qualification
    ├── [v] 108-check suite passes through the packed win-x86_64 artifact
    └── [-] the same suite on the five remaining runners
```

## Memory palace

```mermaid
flowchart LR
    S["author writes a script"] --> C["bx compiler → .bxb bytecode"]
    C --> V["stack VM"]
    V --> P["native processes: exec / sh"]
    B["bx cells: win/lnx × x86_64/aarch64"] --> L["Rust launcher (MINICOM1)"]
    L --> D["minicon-bx-<cell>.com"]
    D -->|"extract + spawn, stdio inherited"| B
    Q["bx 51-check suite"] --> D
    CI["six-cell matrix"] -.->|"bx_lane evidence"| Q
```

## Boundary

- bx is its own workspace (`bx/Cargo.toml` carries an empty `[workspace]`): it
  does not join MiniCon's workspace members and no MiniCon crate is linked into
  it. The terminal binary never contains bx code.
- Delivered artifacts are **per-OS** (`minicon-bx-<cell>.com`). The single
  six-OS polyglot stays MiniCon's `.com` concern
  ([APE join](PRD_02_27_con_delivery.md)); a compiled launcher cannot be one
  file for six kernels.
- No package manager, no runtime install, no background service, no bundled
  agent policy.

## Evidence

- 2026-11-16, Windows x86_64 host:
  `minicon-bx-{win-x86_64,win-aarch64,lnx-x86_64,lnx-aarch64}.com` packed from
  the vendored source; `com-pack verify` matches PE `0x8664`/`0xaa64` and ELF
  `0x3e`/`0xb7` for all four.
- `minicon-bx-win-x86_64.com`: `version` → `bx 0.2.0`; `eval "6*7"` → `42`;
  `exit(3)` → `3` (verbatim, not `(128+3)<<8`); a parent `exec` captures
  `[0,"42\n"]`.
- `BX_BIN=minicon-bx-win-x86_64.com bx run bx/tests/suite.bxs` → **PASS 108
  checks** (ternary, interpolation and its escape, CLI flags, stdin, argv with
  spaces/unicode, and include-aware cache invalidation).
- 2026-11-16 review pass: the container now carries `__meta__` metadata
  (`com-pack info` → product/cell/version/source_sha), publishes atomically,
  extracts into a private 0700 directory on Unix, cleans up on spawn failure,
  and validates the launcher's executable magic before appending. Measured cost
  of the model on Windows x86_64: native cell 18.6 ms min / 20.3 ms median vs
  packed artifact 44.3 / 54.9 — one extra process creation, so a packed `.com`
  is the distribution form and a native cell is the hot-path form.
- 2026-11-16 round 1: container reads now validate every entry against the table
  and footer bounds (a truncated file is refused, not read out of range), a
  killed launcher's extract is swept at most hourly, and `com-pack inspect`
  reports launcher format/subsystem, cells and metadata. Re-measured with 20
  interleaved runs: native cell p50 20.0 ms, packed p50 48.3 ms, launcher-only
  about 21 ms — the rest is the second process creation plus the 825 KB write.
- 2026-11-16 round 2: the `.bxcache` key now covers every recursively included
  file, so editing a `use` dependency rebuilds the program (a stale-cache bug:
  the key used to hash only the main script). The suite grew to 65 checks and
  covers `--version`/`--help`, `eval -` stdin, and argv with spaces/unicode.
- 2026-11-16 round 3: nested functions capture the enclosing function's visible
  locals by value (`GET_UPVALUE`/`MAKE_CLOSURE`); captured names are read-only,
  which is enforced at compile time. Factories and callback captures now work
  (`adder(10)(5)`, `map` with a captured base). Suite: 70 checks.
- 2026-11-16 round 4: `for k, v in` binds key/value for objects and index/value
  for arrays and strings through `ITER_KEYS`; a pre-existing stack residue after
  a loop used as the last statement is fixed and covered. Suite: 74 checks.
- 2026-11-16 round 5: function locals and parameters are boxed per call, so
  closures share mutable state (counters, two closures over one binding), writes
  reach the creator, and loop-created closures keep their own iteration value.
  Suite: 77 checks.
- 2026-11-16 round 6: a closure now sees every name visible at its creation
  point, forwarding further-out locals through the creator's own upvalues
  (`GET_UPVALUE_RAW`), so reads and writes work two or three levels up. Suite:
  80 checks.
- 2026-11-16 round 7: runtime errors now name the failing function
  (`script(line) in fn(): message`) and no longer duplicate the location prefix
  when they cross a call boundary; both are covered by suite cases. Suite: 82
  checks.
- 2026-11-16 round 8: only locals that a nested function references are boxed
  (`captured_names` analysis), so ordinary function calls no longer allocate
  cells; suite covers three-level writes and a local captured only by a
  grandchild. Suite: 84 checks.
- 2026-11-16 round 9: a reproducible baseline lives in `bx/scripts/bench.sh`
  (internal timing, packed-artifact mode); bytecode for `fib`/`loop`/`calls` is
  identical to the pre-closure build, and the remaining call-path cost
  (8-15%) is VM-implementation level, documented in the bx README.
- 2026-11-16 round 10: `bx check <script>` compiles (and caches) without
  executing, so an agent can gate a run on syntax/compile errors; covered for
  good, broken and missing files. Suite: 87 checks.
- 2026-11-16 round 11: `bx run`/`eval` accept `--max-steps N` and `--timeout MS`
  (wall clock checked every 4096 instructions); flags after the script stay in
  `argv`. A runaway script is stopped with a clear message and exit 1. Suite: 90
  checks.
- 2026-11-16 round 12: `read("-")` drains stdin to EOF, so `git log | bx run
  summarize.bxs` works; the packed-artifact selftest now also covers `check`,
  `--max-steps` and the stdin filter. Suite: 93 checks.
- 2026-11-16 round 13: compile errors inside `use` include the include file
  (nested includes do not stack prefixes), a missing include names the file
  that referenced it, and `bx check --json` prints one machine-readable line in
  both outcomes. Suite: 98 checks.
- 2026-11-16 round 15: chunk names and source files are shared via `Rc<String>`,
  so a call frame clones pointers instead of allocating two strings; `fib(24)`
  went from ~137 ms back to ~80 ms with bytecode unchanged. Suite: 108 checks.
- 2026-11-16 round 16: frame slot buffers are recycled through a pool capped at
  256 (max depth 220), removing the per-call `vec!` allocation; `fib(24)` is now
  ~71 ms vs ~137 ms at round 21 start. Suite: 108 checks.
- Open: execution on Linux x86_64/aarch64 and Windows ARM64 belongs to the
  six-runner matrix (`minicon-com.yml`, `bx_lane`), not the Windows host.

## Build

`bash bx/scripts/build-cells.sh` builds the cells this host can produce and then
calls the shared `loader/pack-rust-all.sh`; CI runs the same script in the build
job and each runner executes only its own artifact.

## Non-goals

- Replacing MiniCon's polyglot asset or merging bx into the terminal binary.
- A Node compatibility layer or an npm-style package registry.
- Producing macOS or ARM64 cells from a Windows host (the pack host or CI does).
