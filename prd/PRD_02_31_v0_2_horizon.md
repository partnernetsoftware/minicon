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
│   ├── surface ->m1 [ ]
│   │   ├── verb subset: `list-windows`, `select-window`, `new-window`,
│   │   │     `kill-window`, `send-keys`, `capture-pane` -- named and flagged
│   │   │     the way tmux names and flags them, not MiniCon-native verbs
│   │   ├── model mapping: one implicit MiniCon session per running
│   │   │     instance; a tmux "window" is a MiniCon tab; MiniCon has no
│   │   │     pane splits, so only pane index 0 of a `session:window.pane`
│   │   │     target is accepted
│   │   ├── reuses PRD_02_26's process-lifetime --control endpoint
│   │   └── #decision not a new daemon; no session store outside the
│   │         process; not a tmux control-mode (`-C`) server
│   ├── non-goals
│   │   ├── [-] attach across machines / network transport
│   │   ├── [-] persistent session independent of the MiniCon process
│   │   ├── [-] a general multiplexing config language (tmux.conf-equivalent)
│   │   ├── [-] pane splits/layout verbs (`split-window`, `select-pane`,
│   │   │     resize) -- MiniCon tabs do not split; a target naming a
│   │   │     nonzero pane is a bounded error, not silently remapped
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
│         `minicon_core::keymap` (send-keys' tmux key-name table, e.g.
│         `Enter`/`C-c`, reuses the same key encoder v0.1.26 unified)
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
| `send-keys` | `-t <target>`, `-l` (literal), tmux key names (`Enter`, `C-c`, ...) | `send-text`/`send-paste`, keys resolved through `minicon_core::keymap`'s existing encoder |
| `capture-pane` | `-t <target>`, `-p` (print to stdout) | `capture-pane`; `-S`/`-E` history range is `BLOCKED` on carried-debt item C1 (`plan/plan-carried-debt.md`) — MiniCon's scrollback semantics are not yet decided, so this flag is refused, not approximated |

Target parsing: a `-t` value is `[session:]window[.pane]`. `session`, if
given, must equal the one fixed pseudo-name MiniCon reports for its own
running instance (there is exactly one; naming a different session is a
bounded error, not a lookup). `window` accepts either a MiniCon `@ID` handle
or a plain integer treated as positional index into the current tab list, for
agents that expect tmux's own numeric window indexing — `@ID` stays the
canonical form internally since tmux-style indices renumber on close and
MiniCon's own addressing deliberately does not. `pane` must be `0` or absent;
any other value is a bounded error naming that MiniCon has no pane splits.

Explicit non-goals this resolution does not reopen: no pane/split verbs, no
multiple sessions, no tmux control-mode (`-C`) protocol, no `tmux.conf`
equivalent — see the tree above.

## harness — detail

Design questions still open, to resolve before implementation starts:

- [ ] Exact request/response shape MiniCon sends to DeepSeek's flash model
  and to an opencode-go-compatible endpoint — these may not share a wire
  format, so the harness needs an explicit small adapter per backend, not a
  generic "any OpenAI-compatible endpoint" claim until a second real backend
  is verified end to end.
- [ ] Where the bounded root for `file` and the allowed command shape for
  `exec` are configured (CLI flag, `minicon.json`, or both) and what the
  default is when neither is given — MiniCon's existing preference is an
  explicit flag over a silent default that could widen scope unexpectedly.
- [ ] Credential storage: how a DeepSeek or opencode-go key reaches the
  harness (environment variable vs. a config file) without MiniCon
  accumulating a general secrets-management feature.
- [ ] Whether `harness` runs one bounded task and exits, or holds an
  interactive loop inside a tab — the outcome statement above assumes the
  former (a bounded task) unless a concrete use case requires the latter.

## Evidence

None yet — `[ ]` for every item above. Per AGENTS.md's own rule, no line here
may move to `[x]` without a named black-box test, and this module gets
upserted with real status once a 0.2.x plan document picks up either branch
for implementation.
