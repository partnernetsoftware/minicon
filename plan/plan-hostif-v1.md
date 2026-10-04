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
{"version":"0.2.4","hostif":"1.0","os":"windows","arch":"aarch64","asset":"windows-arm64"}
```

- `version`: `env!("CARGO_PKG_VERSION")`, same string `product_window_title()`
  already uses (`src/main.rs:2423`) — one source, no drift.
- `hostif`: the host-interface contract version (this document), independent
  of `version`. Starts at `"1.0"`. Bumped minor for additive capability,
  major for breaking change — this is the number a plugin/launcher actually
  checks, not `version`.
- `os`/`arch`: **raw** `std::env::consts::OS`/`ARCH` (`"windows"`/`"aarch64"`,
  etc.) — no renaming, per cc-agenterm review (2026-10-04): a launcher-side
  alias table is an extra place to drift, macOS universal already breaks the
  naive 1:1 mapping.
- `asset`: minicon's own asset-name suffix for this build — `"windows-arm64"`,
  `"macos-universal"`, etc. — exactly the substring used in
  `candidate-manifest.json`'s `assets[].name`
  (`minicon-<version>-<asset>.<ext>`). minicon computes this once, so the
  launcher never maps `os`/`arch` to an asset suffix itself.

Plain `--version` (no `--json`) keeps today's human-readable output
unchanged — this is additive, not a breaking change to existing scripts.

### Old-version safety (reviewed 2026-10-04)

Confirmed by reading `src/main.rs:377-386` + `src/cli.rs`'s `parse_args`: on
a pre-`{HOSTIF}` build (0.2.3 and earlier), `--version --json` and
`--hostif-handshake` are unrecognized by `offline_cli_exit` (returns `None`),
then fall into `parse_args`, whose catch-all arm
(`src/cli.rs:164-170`, `unknown => Err(...)`) rejects any unrecognized `--`
argument and exits **2**, writing to stderr — this happens before any
window, PTY, or GUI setup code runs. So an old binary never opens a window
when probed this way; a launcher can safely treat "non-JSON output or
non-zero exit" as `hostif=0` (pre-dates this contract, needs upgrade) with
no risk of an unexpected window popping up during version probing.

## 2. Minimal handshake call

```
minicon --hostif-handshake
{"hostif":"1.0","version":"0.2.4","capabilities":["exec","mux","pty"]}
```

Includes `version` alongside `hostif` (per cc-agenterm review) so a launcher
needs exactly one call, not two — `--version --json` remains available
separately for scripts that only want version info without the handshake
semantics.

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

### Known limitation (recorded 2026-10-04, accepted for v1)

The "latest version" discovery address is a hardcoded GitHub Releases URL:
`https://github.com/<org>/minicon/releases/latest/download/candidate-manifest.json`.
The manifest itself is **not signed** — its trust root for v1 is: TLS to
GitHub + each asset's `sha256` inside the manifest + the asset's own
platform signature (Authenticode on Windows, notarization on macOS). A
launcher must still verify the downloaded asset's signature/notarization
independently; the manifest's sha256 only proves "this is the byte-exact
file GitHub served," not "this file is trustworthy." Revisit if/when the
manifest itself needs a signature (v2+).

## Non-goals of this draft

- Does not touch plugin manifest/market format — that is agenterm's half.
- Does not implement wasm+gl sandboxing, webui, or native dynamic loading —
  those are agenterm-side runtime concerns; minicon's job ends at exposing
  `hostif`/`capabilities`/`candidate-manifest.json`.
- No code changes in this commit — spec first, reviewed with cc-agenterm,
  then implemented as a `{HOSTIF}` sub-step in `plan-v0.2.4.md`.
