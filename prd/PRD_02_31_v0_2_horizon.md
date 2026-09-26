# 0.2.x horizon: mux and harness

Owner decision, 2026-09-24: `AGENTS.md`'s product-boundary exclusion of "mux"
and "script runtime, plugin host or Agent permission policy" is narrowed (not
removed) to admit exactly the two subcommands below. This module is their
scope statement; it does not yet contain implementation evidence, because
neither has shipped. Legend: `[v]` shipped, `[-]` partial, `[_]` planned,
`[-]` explicit non-goal.

## Product outcome

A MiniCon user can drive their own already-running tabs the way a `tmux` user
drives panes, from a script or another program, and can hand a narrowly
scoped local task — read/write these files, run this command — to a model
without leaving the terminal or installing a separate agent CLI. Neither
capability turns MiniCon into a server: both still live and die with the one
process, exactly like the existing `--control` endpoint.

```text
0.2.x horizon — mux and harness
├── mux {m}
│   ├── outcome: tmux-CLI-compatible tab control -- same verbs and flag
│   │   shape as `tmux`'s own client, so a script or agent that already
│   │   drives tmux can drive a MiniCon instance without a second code path
│   │   @method=tmux-verb-compat #decision (owner, 2026-09-25: cross-tool
│   │   agent interop is the concrete need the old "reuse existing verbs by
│   │   default" ruling was waiting for)
│   │   #evidence (owner, 2026-09-26): a concrete external consumer of this
│   │   pattern already exists and runs in production -- `mgttt/moltbaby`'s
│   │   `skills/mux/` builds an agent bus (`register`/`send`/`envelope`/
│   │   `inbox`/`drive`/`wait`/`dfleet` busy-idle+model probing) entirely on
│   │   top of tmux verb semantics (session/window/pane, send-keys,
│   │   capture-pane). Minicon-mux's tmux-verb compatibility is what would
│   │   let that same agent-bus tooling address a MiniCon tab the way it
│   │   already addresses a tmux pane, with no second code path in the
│   │   consumer. Not a dependency (MiniCon does not consume moltbaby's
│   │   code or evidence, per AGENTS.md); a design-intent reference only.
│   ├── surface ->m1 [v]
│   │   ├── verb subset: `list-windows`, `select-window`, `new-window`,
│   │   │     `kill-window`, `send-keys`, `capture-pane` -- named and flagged
│   │   │     the way tmux names and flags them, not MiniCon-native verbs
│   │   ├── model mapping: the running MiniCon instance IS the one tmux
│   │   │     session (a name to compare against, not a lookup -- there is
│   │   │     exactly one, and it is this process); a tmux "window" is a
│   │   │     MiniCon tab; a tmux "pane" is that tab's own terminal content
│   │   │     area -- pane 0 always exists and IS the tab's PTY view, but
│   │   │     `split-window` (a second pane inside one window) is not
│   │   │     implemented, so addressing tops out at pane 0
│   │   ├── reuses PRD_02_26's process-lifetime --control endpoint
│   │   └── #decision not a new daemon; no session store outside the
│   │         process; not a tmux control-mode (`-C`) server
│   ├── non-goals
│   │   ├── [-] attach across machines / network transport
│   │   ├── [-] persistent session independent of the MiniCon process
│   │   ├── [-] a general multiplexing config language (tmux.conf-equivalent)
│   │   ├── [-] pane splits/layout verbs (`split-window`, `select-pane`,
│   │   │     resize) -- each tab has exactly one pane (its own terminal
│   │   │     area, index 0); a target naming a nonzero pane is a bounded
│   │   │     error, not silently remapped
│   │   ├── [-] multiple named sessions -- a `-t` target's session component
│   │   │     must match the one implicit session name or is a bounded error
│   │   └── [-] tmux's full `-F` format-string language -- only the
│   │         substitution variables a concrete script need names get
│   │         implemented, same discipline as the verb subset itself
│   ├── safe failure: an invalid tab handle, a nonzero pane index, or a
│   │   session-name mismatch is a bounded CLI error naming which tmux
│   │   assumption MiniCon does not implement -- never a crash, a silently
│   │   created tab, or a silently accepted no-op
│   └── dependency: PRD_02_26_con_control_cli.md (protocol, tab addressing);
│         `agenterm_platform::input::NamedKey` (send-keys' base key names)
│         #correction 2026-09-25: this dependency was written as
│         `minicon_core::keymap`, which is wrong -- that module encodes the
│         *composer* text box's editing chords, not terminal key injection.
│         The control CLI's `send-keys` resolves names through
│         `NamedKey::from_name` plus a `ctrl+`/`alt+`/`shift+` spec
│         (`main.rs`'s `parse_control_key`), so mux translates tmux's
│         `C-`/`M-`/`S-` prefixes and its own spellings (`BSpace`, `DC`,
│         `NPage`) into that spec instead of growing a second key table
├── harness {h}
│   ├── #decision (owner, 2026-09-26, resolves {h-orch} below by splitting
│   │   the role instead of growing this one): "harness" is two distinct
│   │   capability owners, not one that grows a second job --
│   │   ├── `harness` (this node, unchanged): a WORKER -- the same shape as
│   │   │   Claude Code / grok-build / ChatGPT Codex's own harness, one
│   │   │   bounded task, two tools, no orchestration, ever, as this node's
│   │   │   own scope, not "not yet"
│   │   └── `harness-manage` {hm} (new, separate, not started): a MANAGER --
│   │         workflow/project-management role that dispatches across tabs
│   │         via `mux`, in the shape moltbaby's `dfleet`/`drive`/`wait`
│   │         already prove out in practice (see mux's #evidence node above)
│   │   The owner named this split from real practice, not a clean-room
│   │   design; treat both nodes as real requirements to detail, not a
│   │   naming exercise.
│   ├── outcome: minimal agent loop, two tools only
│   ├── tools [v]
│   │   ├── file: read/write within a bounded root, no plugin interface
│   │   ├── exec: run one command, capture output, no shell plugin chain
│   │   └── #decision exactly these two; a third tool is a new decision
│   ├── model backends [-]
│   │   ├── DeepSeek official API + key, flash model first ->h1 @method=first
│   │   ├── opencode-go-compatible key as the second accepted credential
│   │   └── both adapters are built with their OWN codec and dispatched per
│   │       backend; both are `[-]` not `[v]` because neither has run against
│   │       its real endpoint -- see "harness — what is built and what is
│   │       owed" below. #assumption
│   ├── policy #decision
│   │   ├── no separate "Agent permission policy" framework
│   │   └── the two tools' own bounds (root path, command) are the policy
│   ├── non-goals (this node's own permanent scope, not a 0.2.x-only limit)
│   │   ├── [-] a plugin/tool-registration system
│   │   ├── [-] inbound network service or remote task queue
│   │   └── [-] multi-agent orchestration or a scripting runtime -- owned by
│   │         `harness-manage` {hm} instead, never grown here; see #decision
│   │         above (this is the resolution of the 2026-09-26 open question
│   │         this line used to carry, replaced by the role split, not by
│   │         narrowing this node's own boundary)
│   ├── safe failure: a bad key or unreachable model is a bounded CLI error;
│   │   a tool call outside its bound (root/command) is refused, not widened
│   └── dependency: none new; a subprocess/file-IO capability MiniCon's
│       platform crate already has for other features
│       #correction 2026-09-25: half right. File IO is there and used
│       (`filesystem_read::read_bounded`, `filesystem_publish::write_file_atomic`).
│       Contained *spawning* is not: `contained_process` sits behind the
│       platform crate's `contained-process-spawn` feature, which MiniCon's
│       dependency does not enable, so `exec` currently spawns through
│       `std::process::Command`. That still gives the argv-vector/no-shell
│       guarantee the invariant is about, but not resource containment; see
│       H3's carried-debt line in `plan/archive/plan-v0.2.0.md`
├── harness-manage {hm} [_] not started -- horizon not assigned yet (not
│   │ 0.2.x; likely 0.3.x alongside the GUI workbench, but not decided --
│   │ see the harness GUI section below, which is a separate leaf: a
│   │ renderer question, this is an orchestration-logic question, and the
│   │ two need not land in the same release)
│   ├── outcome: a workflow/project-management role that coordinates the
│   │   agents already running in each MiniCon tab -- a "manager", not a
│   │   "worker"; drafted from real practice, not speculative ->h ->m
│   ├── real-world precedent, already running: moltbaby's
│   │   `dfleet`/`dstatus` (busy/idle+model+ctx probing across tabs),
│   │   `drive`/`wait` (send a task, block until real busy-then-idle,
│   │   return result), `envelope` (identity-checked cross-agent messages) --
│   │   this node's job is to decide MiniCon's own minimal version of that
│   │   shape, not to import moltbaby's code (AGENTS.md: no dependency on
│   │   another product's evidence)
│   ├── open design questions, none answered yet #assumption
│   │   ├── context concept: what does `harness-manage` track about each
│   │   │   tab's agent between dispatches -- own words from this thread:
│   │   │   "调度众多agents不是容易的事，要有上下文概念要有工作流概念"
│   │   ├── workflow concept: a fixed verb set (moltbaby-style
│   │   │   send/drive/wait) vs. a general scripting/workflow language --
│   │   │   the latter re-collides with the plugin/scripting-runtime
│   │   │   non-goal recorded for `harness` {h} above, so this choice needs
│   │   │   its own explicit decision, not an accidental default
│   │   ├── tool surface: does `harness-manage` reuse mux's CLI verbs as-is,
│   │   │   or need its own -- `mux` {m} is scoped to tmux-verb-compat, so a
│   │   │   MiniCon-native manager verb is new surface either way
│   │   └── relationship to `harness` {h}: does `harness-manage` dispatch to
│   │         a per-tab `minicon harness` worker specifically, or to
│   │         whatever agent a tab happens to be running (any TUI, per
│   │         moltbaby's comm-based detection) -- these are different scope
│   │         sizes and the difference matters for what "coordinate" means
│   └── #decision no implementation, no tool/verb naming, until the design
│         questions above get an actual tree DAG + memory palace pass per
│         AGENTS.md's planning method; this is `BLOCKED` on design work, not
│         skipped, and is not part of 0.2.x's own closing scope
└── Shared constraints
    ├── one file, no bundled runtime, no installer
    ├── every other AGENTS.md exclusion still applies (no server, no
    │   persistent workspace, no Fleet, no MCP, no general plugin host)
    └── black-box CLI evidence only, per AGENTS.md's test discipline
```

## mux — detail

`PRD_02_26_con_control_cli.md` already owns the bounded local control
protocol and tab addressing (`@ID` handles, `send-text`, `send-paste`,
`capture-pane`, snapshots). `mux` is a second, richer CLI surface over that
same endpoint — it does not open a new transport or change the endpoint's
process-bound lifetime.

**Resolved, 2026-09-25** (supersedes the two open questions this section
used to carry): the command surface is tmux-CLI-compatible, not a MiniCon-
native verb set. The concrete need the earlier "reuse existing verbs by
default" ruling was waiting for is agent interop — a script or agent that
already knows how to drive `tmux` should be able to drive MiniCon through
the identical verb/flag vocabulary, so the same automation works against
either implementation without a MiniCon-specific branch. `mux` is a mode of
the existing control CLI (`minicon mux <tmux-verb> [tmux-flags]`), not a new
binary subcommand or endpoint — that half of the original question is
unchanged.

Verb-to-primitive mapping (each verb is a thin translation over
`PRD_02_26`'s existing protocol, not a new capability):

| tmux verb | flags supported | maps to |
| --- | --- | --- |
| `list-windows` | `-F <format>` (subset of substitutions, grown on concrete need) | enumerate open tabs |
| `select-window` | `-t <target>` | switch focused tab |
| `new-window` | `-t <target>` (name only; no `-c`/layout flags) | open a tab |
| `kill-window` | `-t <target>` | close a tab |
| `send-keys` | `-t <target>`, `-l` (literal), tmux key names (`Enter`, `C-c`, ...) | `send-keys` (or `send-text` under `-l`), tmux names translated into the control CLI's existing `ctrl+`/`alt+`/`shift+` spec |
| `capture-pane` | `-t <target>`, `-p` (print to stdout) | `capture-pane`; `-S`/`-E` history range is `BLOCKED` on carried-debt item C1 (`plan/plan-carried-debt.md`) — MiniCon's scrollback semantics are not yet decided, so this flag is refused, not approximated |

Target parsing: a `-t` value is `[session:]window[.pane]`. `session`, if
given, must equal the name MiniCon reports for its own running instance —
this is a comparison against the one real session (this process), not a
lookup among several, since MiniCon runs as exactly one instance per
`--control` endpoint. `window` accepts either a MiniCon `@ID` handle or a
plain integer treated as positional index into the current tab list, for
agents that expect tmux's own numeric window indexing — `@ID` stays the
canonical form internally since tmux-style indices renumber on close and
MiniCon's own addressing deliberately does not. `pane` accepts `0` or is
omitted, and always resolves to that window's one terminal content area —
pane 0 is not a placeholder or a stand-in, it IS the tab's PTY view; any
other pane index is a bounded error naming that MiniCon has no
`split-window` (a window still has at most one pane, it just always has
that one).

Explicit non-goals this resolution does not reopen: no `split-window`/
multi-pane-per-window verbs, no multiple sessions, no tmux control-mode
(`-C`) protocol, no `tmux.conf` equivalent — see the tree above.

**Settled during implementation, 2026-09-25.** Three points the resolution
above left to whoever wrote the code, recorded here because each is a
visible part of the compatibility surface:

- **The session's name is `minicon`, and `0` is accepted as an alias.** A
  `-t` target's session component must be one of those two or it is a bounded
  error. `0` is accepted because that is what tmux calls its own first
  session, so a script written against tmux's default keeps working; the
  instance still IS the session, and neither name is looked up.
- **An unknown key name is refused, not sent as text.** tmux sends a name it
  does not recognize as literal characters (`send-keys foo` types `f`, `o`,
  `o`). MiniCon refuses it instead, for the same reason `-F` refuses an
  unknown substitution: a mistyped key name silently becoming keystrokes is
  the failure a script cannot see. `-l` remains the way to send text
  literally, and it routes to `send-text`, which resolves no names at all.
  This is a deliberate divergence from tmux's own behavior, not an omission.
- **`new-window -n NAME` is refused.** MiniCon's `new-tab` takes no name, and
  accepting `-n` while dropping the name would leave a script believing it had
  set one. `new-window -t TARGET` maps the target to the new tab's **parent**,
  which is the only placement MiniCon's tab tree has; tmux's "insert at this
  index" meaning has no equivalent and is not approximated.
- **The mutating verbs print JSON where tmux is silent.** `select-window`,
  `new-window`, `kill-window` and `send-keys` print the control protocol's own
  pretty-printed reply (`{"closed": "@2"}`, `{"sent_keys": 1}`); only
  `capture-pane -p` prints plain text. A script that parses tmux's silence as
  success still sees exit code 0, but a script that asserts empty stdout does
  not. Kept deliberately: the reply names which handle was actually affected,
  which is the one thing an agent driving tabs it did not open needs.
- **A stale `@ID` is refused by the server, in the server's words.** Every
  other bounded refusal names the tmux assumption MiniCon does not implement,
  because `mux` resolves the target itself. `@ID` is deliberately passed
  through instead, so the refusal is the control server's own
  (`minicon mux: terminal @2 does not exist`) and names no tmux assumption.
  The alternative -- pre-checking the handle against `list-tabs` -- would add
  a second round trip and a race window for no gain.

## harness — detail

**Resolved, 2026-09-25** (supersedes the four open questions this section
used to carry):

- **Invocation and scope.** `minicon harness --root <path> --task "<text>"
  [--backend deepseek|opencode-go] [--allow-cmd <name>]...`. `--root` and
  `--task` are required with no default; a missing `--root` is a bounded CLI
  error, never an implicit cwd, per MiniCon's existing "explicit flag over a
  silent default that could widen scope" preference. `--allow-cmd` is
  repeatable and names permitted executable basenames for the `exec` tool;
  giving none does not disable the tool (a script must always see the same
  two tools advertised) but makes every `exec` call refused with a bounded
  error naming the missing allow-list, so a model cannot tell "not allowed
  yet" apart from "tool absent" by probing.
- **Run shape.** One invocation runs one bounded task to completion (or to a
  bounded turn/tool-call limit) and exits, printing the result to stdout —
  no interactive loop inside a tab, no conversation persisted across
  invocations. A `--continue`-style resumed conversation is deliberately
  out of scope for v0.2.0, not designed here.
- **Credential storage.** Environment variable only:
  `MINICON_DEEPSEEK_API_KEY` for `--backend deepseek`,
  `MINICON_OPENCODE_API_KEY` for `--backend opencode-go`. A config-file
  credential store is deferred so MiniCon does not grow a general
  secrets-management feature for this one CLI mode.
**Decided by the owner, 2026-09-25 (was open during H4).** MiniCon could not
make an HTTPS request at all, and neither could its platform crate: the
`agenterm-platform` features it enabled were `network-dns`,
`network-interfaces` and `network-routes` and nothing more. The harness
dependency line above ("none new") was wrong a second time.

The owner chose option 1, the house route: **add the capability to
`agenterm-platform`**, reviving the `ureq` dependency that crate already
declared but no longer used, with the target-specific TLS trees that
AgenTerm's own native-platform PRD module (`PRD_02_20`, in that repository,
not this one) specifies (Unix Rustls/WebPKI, Windows NativeTls). It ships as
that crate's `network-http` feature -- a neutral contract plus a validating
facade, with no per-OS adapter, because `ureq` is portable and the only per-OS
difference is the TLS provider, which those Cargo feature trees already
express. MiniCon enables the feature and moved its pinned revision.

Shelling out to `curl` was rejected outright: it would put an unbounded
external command inside the one feature whose whole point is bounded tools.
Adding a TLS crate to MiniCon itself was rejected in favour of the shared
crate, per AGENTS.md's "cross-platform mechanisms in the shared platform
crates".

What this does NOT settle, and is recorded as `BLOCKED` rather than skipped:
certificate verification has no end-to-end proof anywhere -- the capability's
own courts reach `127.0.0.1` only -- and the Windows/macOS TLS arm has not
been compiled in any court that ran. A live HTTPS round trip against the
DeepSeek API additionally needs a working key, which the implementing
environment does not have (the one it carries is rejected by the API).

- **Wire adapters.** Each backend gets its own small, explicit adapter
  translating the two-tool (`file`, `exec`) loop into that backend's own
  tool-call wire shape. DeepSeek's flash model is wired first, verified end
  to end (H4); the opencode-go-compatible adapter is built and verified
  second (H5), independently — no "any OpenAI-compatible endpoint" claim is
  made from DeepSeek's adapter alone, since the two may not actually share a
  wire format and MiniCon does not assert compatibility it has not run.

## Evidence

**mux is `[v]` on named black-box evidence.** `tests/minicon_mux.rs` drives
the shipped binary's CLI against a live instance under a display server; the
four tests, the real effect each asserts, and the single-line break that each
one catches are named in `plan/archive/plan-v0.2.0.md`'s M2/M3/M3b nodes. In short:
`list-windows` renders the host's own handles with the active marker following
a real `select-tab`; index and `@ID` targets provably reach the same tab;
`new-window`/`select-window`/`kill-window` are read back from `cli list-tabs`
rather than from mux's stdout, and a bare `kill-window` closes the active tab;
`send-keys -l` plus the named key `Enter` makes a real child run a command
whose output `capture-pane -p` returns; and five bounded refusals each exit
non-zero with an empty stdout while leaving the tab list and active tab
byte-identical.

**The display-server `BLOCKED` recorded here was wrong and is withdrawn.**
The `control endpoint did not become ready` failures that produced it were a
missing display server, not a limit of the host. With `xvfb` installed, the
full gate passes on the implementing container: unit 338, `minicon_mux` 4,
`minicon_blackbox` 28, `minicon_control` 12, `minicon_alignment` 15, under the
same `xvfb-run -s "-screen 0 1280x900x24"` invocation
`scripts/linux-runtime-qualify.sh` uses in CI. What had actually been missing
was coverage, and it has since been written. Note that `cargo test --test
minicon_mux` does not rebuild the binary it drives, so `cargo build --bin
minicon` must precede it or a stale binary is what gets tested.

### harness — what is built and what is owed

**Built, with named evidence.** The two tools are `[v]`. Both backends are
`[v]`, wired and dispatched from `run_harness` on `--backend`, each running
its own codec: `harness_wire` speaks DeepSeek's wire format, `harness_opencode`
the OpenAI-compatible shape opencode-go's real hosted service presents, and
the only thing they share is the widened `Transport` seam (an
`extra_headers` parameter, needed for opencode-go's mandatory
`x-opencode-session`). Both codecs are covered by fixture tests plus, now,
live black-box calls against the real endpoints (see below).

That the wiring itself is real is falsifiable rather than asserted: reverting
`run_harness`'s dispatch makes the entire opencode-go adapter dead code, which
the gate's `dead_code` denial rejects with eight `never used` errors. No
module in the harness carries a `cfg_attr(not(test), allow(dead_code))` any
more; each allowance was removed as its stated reason expired.

**No-live-call BLOCKERs withdrawn, 2026-09-26.** Both backends now have live
evidence, each `BLOCKED` only when this environment lacks the credential
(never silently skipped): `deepseek_backend_runs_a_real_bounded_task_and_
writes_the_file` / `..._through_the_exec_tool` and
`opencode_backend_runs_a_real_bounded_task_and_writes_the_file` in
`tests/minicon_harness.rs`.

opencode-go's wire shape was not documented anywhere in this repository, so
rather than invent it, it was researched live (WebSearch/WebFetch plus direct
`curl` against the real endpoint with the real credential) and the adapter
was corrected to match what was found: the real service is a **hosted
subscription API** (`https://opencode.ai/zen/go`), not a loopback local
server; it is HTTPS-only; it requires a mandatory `x-opencode-session` header
(`harness_opencode::OPENCODE_SESSION_ID`, sent unconditionally -- its absence
is refused with `MissingSessionID` even with a valid bearer key, and a query
parameter does not substitute); and it has no model named `"opencode"` -- its
real catalog (`GET /v1/models`) includes `deepseek-flash`, `deepseek-v4-pro`,
`glm-5.3`, `grok-4.7`, `kimi-k3` among others, selectable via the new
`MINICON_OPENCODE_MODEL` (`harness_opencode::OPENCODE_MODEL_VAR`). The shared
`Transport::post_json` seam was widened with an `extra_headers` parameter to
carry this, proved by a new loopback test in `harness_wire.rs` asserting a
real socket receives an arbitrary extra header.

**Owed, and BLOCKED rather than skipped.**

1. **TLS is proven only for Unix Rustls/WebPKI.** The live DeepSeek and
   opencode-go calls above are real evidence for that provider, but the
   Windows/macOS `native-tls` arm is not compiled in any evidence on record.
   The TLS provider selection there is proved only by the feature graph
   compiling.

Windows and macOS coverage for both branches is owed to their courts, not
claimed here.

Two process notes worth keeping, because each cost real time:

- A guard test can pass with its guard removed and still look like evidence.
  `harness_wire`'s Authorization-header assertion matched
  `Authorization: Bearer secret` as a substring, which the broken version's
  `x-not-authorization: Bearer secret` contains — so the break failed to fail.
  It now matches the whole folded header line. A break that does not fail is
  the finding, not a nuisance.
- The opencode-go fixtures deadlocked two complete gate runs: a `Drop` joined
  a listener thread parked in blocking `accept()`, so any test that finished
  before consuming its scripted replies — including every refusal test, once
  the transport validates client-side and never connects — hung forever. It
  was diagnosed from the processes' kernel wait states
  (2x `inet_csk_accept` plus `futex_do_wait`), not from the test output, which
  showed nothing at all. Fixtures must terminate on their own merits; a
  wrapper timeout would have hidden this.

### harness GUI workbench — direction settled, work not started

**Owner intent (2026-09-26).** The motivation is real and MiniCon-owned, not
a re-run of AgenTerm's CC: a GUI shell over `harness` for e-commerce users who
want a customized workbench, not a bare CLI. Recorded so this does not get
re-argued from scratch later.

**Owner decision (2026-09-26): GUI mode is explicitly 0.3.x, not 0.2.x.**
This horizon's own title and "Product outcome" above already scope 0.2.x to
`mux` and `harness` only; this decision makes that boundary explicit for the
workbench idea specifically, so it is not mistaken for open 0.2.x scope. What
0.2.x does instead: deepen and harden `mux` + `harness` themselves (the
statefulness leaf below, plus closing any other `[_]`/`[-]` items in the two
subsections above) and continue stabilizing the existing terminal UI/UX and
underlying platform foundation (`terminal.rs`, `theme.rs`, `host_ui.rs`,
`host_paint.rs`, `raster_surface.rs` and the `agenterm-platform`/
`agenterm-ui-core` pins they consume) — not new-feature work, hardening of
what already ships. A `PRD_02_3x` module for the 0.3.x GUI horizon gets
written when 0.2.x's own scope is closed, not before; this section is that
horizon's placeholder, not its start.

**[_] Not started. Real blocker is statefulness, not the renderer.** `harness`
today is single-shot per "Run shape" above — one bounded task, no `--continue`,
no persisted conversation. A workbench needs multi-turn session persistence
and streaming output; a window around a one-shot CLI call does not deliver
that. This is the work to do first, independent of any GUI decision, and is
its own leaf.

**Renderer choice, decided in advance so it is not re-litigated per session:**

- **[✓] If/when a renderer is built: reuse `agenterm-platform`'s webview
  adapter and bridge-v1 security state machine (origin binding, 64 KiB
  message cap, 8 concurrent requests, replay protection), per
  `PRD_02_28_shared_core.md`'s "consume at the git pin, no mandatory
  dependency inversion".** Do not pull `wry`/`tao` directly into MiniCon.
  `/home/user/agenterm/research/agenterm-webview/` already ran the
  direct-WRY-vs-Tauri comparison end to end (direct-WRY 520,704 B vs Tauri
  8,763,392 B on Windows) and decided `prefer-direct-wry-if-webview`; MiniCon
  gets that evidence for free by consuming the shared crate instead of
  re-deriving it.
- **Rejected: driving a user-downloaded portable Chrome via CDP.** Contradicts
  this repo's own precedent twice over — the harness `exec` tool line rejected
  shelling out to `curl` specifically because it "would put an unbounded
  external command inside the one feature whose whole point is bounded tools"
  (see "harness — detail" above), and the agenterm-webview experiment's own
  design rule is "never downloads a runtime and the workspace contains no
  fixed browser runtime". CDP control of an externally-sourced browser
  process is a strictly larger, less bounded surface than either.
- **Rejected: bundling Electron.** Electron ships Node.js plus a full Chromium
  and a parallel Node/npm build toolchain. `mux`+`harness` are already at the
  9 MiB Candidate ceiling and the Tauri reference (~8.4 MiB) was already ruled
  out on size in the agenterm comparison; Electron is heavier than Tauri by a
  further margin, and its own build toolchain is exactly what the direct-WRY
  approach and this repo's asset model (`assets/`: static HTML/CSS/JS,
  embedded with `include_bytes!`, no Node/npm required) were chosen to avoid.
- **Size boundary, decided in advance:** any future GUI ships as a separate
  subcommand/process (as AgenTerm's own plan keeps CC's webview dependency out
  of `agenterm-cc`/`agenterm.exe`'s 4 MiB budget), never linked into the core
  `minicon` binary, so a missing/broken WebView runtime on one platform cannot
  take down the CLI.
- **Deferred, not rejected: a from-scratch `minicon-surf` browser engine.**
  Raised in an earlier discussion, not previously written down anywhere in
  this repository (checked: no `minicon-surf` hits before this entry). A
  self-built rendering/network/HTML-CSS-JS stack is a different order of
  scope than consuming an existing WebView (CDP and Electron at least reuse
  someone else's engine) — it competes with browser vendors, not with a
  packaging choice, and has no path back to MiniCon's current "file+exec
  bounded tools + tmux-shaped mux" product boundary in this horizon. No work
  starts on it until a materially broader product mandate exists; recorded
  here only so it is not re-proposed from a blank slate.

**Non-goal for now:** designing the workbench's actual screens or task
vocabulary (which e-commerce tasks it surfaces) is out of scope until the
statefulness leaf above is done and the concrete task set is written up
separately. Do not start renderer code before that.

### 0.2.0 — what the version number stands for, and what it does not

The version is bumped in `Cargo.toml` and `release-policy.json` at the SHA
whose evidence reads:

- `./scripts/build.sh test` green under a display server: 517 tests, zero
  failures, across the host unit suite, alignment, the mux black boxes, the
  harness codecs and the 28 GUI black boxes. Those 28 fail as a block when no X
  server is present, which is an environment report and not a product signal;
  they must be run under `xvfb-run -a -s "-screen 0 1280x900x24"`, the same
  invocation `scripts/linux-runtime-qualify.sh` uses.
- `./scripts/six-cell-qualify.sh` with FAIL 0 / PASS 23 / BLOCKED 18 on the
  repinned `agenterm`: `common` fmt, build-fanout and both Windows-cell
  boundary checks, plus all-target-link, throughput-link and artifact
  inspection for `win-x86_64`, `win-aarch64`, `lnx-x86_64` and `lnx-aarch64`,
  and — with `MINICON_APPLE_SDK_ROOT` set to a macOS 15.5 SDK — `clippy`,
  `test-link` and artifact inspection for `osx-aarch64` and `osx-x86_64`. This
  is the only evidence that the repin's Windows console-agent code, the
  `native-tls` arm of `network-http` and the macOS process adapter compile and
  link at all.

Two whole classes of evidence are BLOCKED at this SHA, and the version number
claims neither:

- **No runtime evidence.** Six-cell on a non-macOS host links but never
  executes: all four runtime courts are BLOCKED on unconfigured runners, and
  the Apple cells' `test` and `throughput` stages are BLOCKED because a Linux
  kernel answers `Exec format error` to a Mach-O binary. Nothing here has run a
  MiniCon binary on Windows or macOS. Cross-compilation is not execution, and
  the eighteen BLOCKED stages say so by name rather than by omission.
- **No public release.** The sealed Candidate to Promotion chain is owned by
  the `run-reputation-and-release` skill, with `sign-macos-artifacts` and
  `sign-windows-artifacts` for the signing courts. None of the three is
  registered under `~/.claude/skills/` in the container this version was built
  in, and `AGENTS.md` forbids reconstructing such a procedure from the
  `.github/workflows/` files or hand-rolling a local equivalent. So 0.2.0 is a
  development-complete version, not a released one; it becomes releasable when
  those skills are registered, and public Promotion still needs explicit human
  version and publish authority on top of that.

  **Resolved 2026-09-26.** The owner ran the three skills locally (they were
  registered in that container), and 0.2.0 published as `v0.2.0` (release.yml
  run `36230348611`, Candidate run `36223506557`, source SHA
  `c0b5ed7a2289cbe18c25f92dcae3769101424f7a`) — this repo's own
  `six-grid-cloud-build`/`defender-ci-scan.yml` CI path, not `utm-court`. Two
  real defects surfaced and were fixed on the way, not glossed over: (1)
  `scripts/product-source-hash.sh` needed `MSYS_NO_PATHCONV=1` for the
  `windows-2025` runner's bash step (see its own history); (2) `minicon.com`
  outgrew the 9 MiB Candidate ceiling from `mux`+`harness`, and the owner
  raised it to 11 MiB by explicit dated decision (see "Artifact budget" in
  `PRD_02_27_con_delivery.md`) rather than trimming 0.2.0's scope. This repo
  now also vendors redacted copies of the three skills under
  [`.claude/skills/`](../.claude/skills/) so a future cloud/Linux-only agent
  does not get stuck the same way — see `AGENTS.md`'s "Skills: look them up
  BEFORE acting". **Still not resolved:** the "No runtime evidence" gap above
  is unchanged — 0.2.0 shipped on cross-compile + static-signing + static-scan
  evidence only, no real execution of the Windows/macOS bytes.
