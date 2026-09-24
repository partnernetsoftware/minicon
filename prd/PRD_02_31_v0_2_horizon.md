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
│   ├── outcome: script-driven tab control, tmux-shaped
│   ├── surface ->m1 [ ]
│   │   ├── list/select tabs, read pane content, send text/paste
│   │   ├── reuses PRD_02_26's process-lifetime --control endpoint
│   │   └── #decision not a new daemon; no session store outside the process
│   ├── non-goals
│   │   ├── [-] attach across machines / network transport
│   │   ├── [-] persistent session independent of the MiniCon process
│   │   └── [-] a general multiplexing config language (tmux.conf-equivalent)
│   ├── safe failure: an invalid tab handle is a bounded CLI error, never a
│   │   crash or a silently created tab
│   └── dependency: PRD_02_26_con_control_cli.md (protocol, tab addressing)
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
process-bound lifetime. Design questions still open, to resolve before
implementation starts:

- [ ] Command surface: a `tmux`-flavored subcommand set (`list-tabs`,
  `select-tab`, `split-view` if MiniCon ever supports one) vs. reusing the
  existing `capture-pane`/`send-*` verbs under a `mux` alias. Reusing existing
  verbs is the default unless a concrete script need shows they are
  insufficient.
- [ ] Whether `mux` is a new binary subcommand or a mode of the existing
  control CLI. Default: a mode, since it needs no new endpoint.

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
