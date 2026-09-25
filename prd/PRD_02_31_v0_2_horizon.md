# 0.2.x horizon: mux and harness

Owner decision, 2026-09-24: `AGENTS.md`'s product-boundary exclusion of "mux"
and "script runtime, plugin host or Agent permission policy" is narrowed (not
removed) to admit exactly the two subcommands below. This module is their
scope statement; it does not yet contain implementation evidence, because
neither has shipped. Legend: `[x]` shipped, `[~]` partial, `[ ]` planned,
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
│   ├── surface ->m1 [x]
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
│   ├── outcome: minimal agent loop, two tools only
│   ├── tools [ ]
│   │   ├── file: read/write within a bounded root, no plugin interface
│   │   ├── exec: run one command, capture output, no shell plugin chain
│   │   └── #decision exactly these two; a third tool is a new decision
│   ├── model backends [ ]
│   │   ├── DeepSeek official API + key, flash model first ->h1 @method=first
│   │   └── opencode-go-compatible key as the second accepted credential
│   ├── policy #decision
│   │   ├── no separate "Agent permission policy" framework
│   │   └── the two tools' own bounds (root path, command) are the policy
│   ├── non-goals
│   │   ├── [-] a plugin/tool-registration system
│   │   ├── [-] inbound network service or remote task queue
│   │   └── [-] multi-agent orchestration or a scripting runtime
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
│       H3's carried-debt line in `plan/plan-v0.2.0.md`
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

**mux is `[x]` on named black-box evidence.** `tests/minicon_mux.rs` drives
the shipped binary's CLI against a live instance under a display server; the
four tests, the real effect each asserts, and the single-line break that each
one catches are named in `plan/plan-v0.2.0.md`'s M2/M3/M3b nodes. In short:
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

**harness stays `[ ]`.** Its two tools and its transport-independent half
(wire codec, tool dispatch, bounded turn loop) have named unit evidence in the
plan's H2/H3/H4 nodes, and the transport now exists, but the live end-to-end
assertion the `model backends` line requires has not run: the environment's
DeepSeek key is rejected by the API. That is `BLOCKED`, not skipped, and no
"it compiles" claim substitutes for it. Windows and macOS coverage for both
branches is likewise owed to their courts, not claimed here.
