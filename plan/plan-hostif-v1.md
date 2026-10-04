# {HOSTIF} v1 draft — minicon host-interface for the agenterm launcher

Owner-decided 2026-10-04: agenterm restarts at 0.2.0.0 as a launcher +
plugin/app market built on minicon. This is minicon's half of the draft
(`plan/plan-v0.2.4.md`'s `{HOSTIF}` leaf), answering cc-agenterm's three
concrete asks over mux the same day. Not yet implemented — this is the spec
to implement against, reviewed with agenterm before code.

## 1. `minicon --version --json`

Extends the existing offline, no-window CLI path (`cli::offline_cli_exit`,
already handles bare `--version`/`--help`/`--status` — see
`src/main.rs:4445-4454`). `--json` is a new modifier on `--version` only:

```
minicon --version --json
{"version":"0.2.4","hostif":"1.0","os":"windows","arch":"arm64"}
```

- `version`: `env!("CARGO_PKG_VERSION")`, same string `product_window_title()`
  already uses (`src/main.rs:2423`) — one source, no drift.
- `hostif`: the host-interface contract version (this document), independent
  of `version`. Starts at `"1.0"`. Bumped minor for additive capability,
  major for breaking change — this is the number a plugin/launcher actually
  checks, not `version`.
- `os`/`arch`: `std::env::consts::OS`/`ARCH`, Rust's own normalized strings
  (`"windows"`/`"linux"`/`"macos"`, `"x86_64"`/`"aarch64"`) — matches the
  asset-naming convention already used in `candidate-manifest.json` (see
  below) once `aarch64`→`arm64` is aliased for the asset-name match.

Plain `--version` (no `--json`) keeps today's human-readable output
unchanged — this is additive, not a breaking change to existing scripts.

## 2. Minimal handshake call

```
minicon --hostif-handshake
{"hostif":"1.0","capabilities":["exec","mux","pty"]}
```

Same offline/no-window tier as `--version`/`--status` — must not spawn a
GUI window or touch a PTY to answer. `capabilities` is a flat string list;
a launcher checks membership, never parses version numbers out of it. New
capabilities get appended (minor hostif bump); removing one is a breaking
change (major bump) and must ship with a `hostif` major increment in the
same release.

This is deliberately the same call shape as `--version --json` (both
offline, both JSON, both zero side effects) rather than a separate
subsystem — one new branch in `offline_cli_exit`, not a new binary mode.

## 3. `candidate-manifest.json` stability contract

Already the authoritative source agenterm asked for — confirming the
existing shape as frozen, not proposing a new one:

- `version` (top-level string) — current release version.
- `assets[]` — each entry: `name`, `bytes`, `sha256`, `sidecar.{name,sha256}`.
  Confirmed live shape (`target-six/candidate-0.1.23/candidate-manifest.json`):
  native per-arch archives named `minicon-<version>-<os>-<arch>.<ext>`
  (`windows-x86_64.zip`, `windows-arm64.zip`, `linux-x86_64.tar.gz`,
  `linux-arm64.tar.gz`, `macos-universal.tar.gz`, `macos-universal.dmg`)
  plus one architecture-independent `minicon.com` entry — exactly the two
  tiers agenterm's launcher needs (native-first, `minicon.com` fallback).
- `schema` (top-level int) — bumps only on a breaking field change to this
  contract. Launchers should refuse an unrecognized `schema` rather than
  guess.

No field renames planned. If a future release needs a new field, it is
additive under the same `schema`; a rename or removal requires a `schema`
bump, same rule as `hostif`.

## Non-goals of this draft

- Does not touch plugin manifest/market format — that is agenterm's half.
- Does not implement wasm+gl sandboxing, webui, or native dynamic loading —
  those are agenterm-side runtime concerns; minicon's job ends at exposing
  `hostif`/`capabilities`/`candidate-manifest.json`.
- No code changes in this commit — spec first, reviewed with cc-agenterm,
  then implemented as a `{HOSTIF}` sub-step in `plan-v0.2.4.md`.
