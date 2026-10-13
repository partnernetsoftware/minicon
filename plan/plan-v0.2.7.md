# plan-v0.2.7

## Scope decision

Owner (2026-10-12): the **"Check for updates"** requirement (GitHub #4, opened
2026-10-12) moves out of 0.2.6 into **0.2.7**. It was separated deliberately:
it introduces the product's **first outbound network call** and **first
self-modifying behaviour**, so it should not be bundled with the 0.2.6 host-UI
region work, and its boundary wording needs an owner decision first.

0.2.7's **other** scope is open. This leaf is carried here so the requirement has
one owner rather than dangling.

Owner (2026-10-13): the **window-icon parity** fix rides in 0.2.7. 0.2.6 shipped
a Windows window class registered with a null `hIcon`, so the caption and the
taskbar showed the generic system default instead of the executable's icon; the
same host-neutral gap left X11 with no `_NET_WM_ICON`. Fixing it is a platform
change, so it cannot be re-cut into the already-published 0.2.6.

## Tree DAG

```text
0.2.7: check-for-updates + window-icon parity; other scope open
├── {ICON} window icon on all three hosts                              [v] @host=all
│      owner: `prd/PRD_02_25_con_workspace.md` "Host UI"
│      Windows: load the PE icon group (`winresource` id 1) into the
│        window class `hIcon` and pin it with `WM_SETICON` (big + small)
│      X11/macOS: `build.rs` decodes the largest face of the same
│        `assets/minicon.ico` into RGBA; the product passes it through
│        `PixelWindowOptions::window_icon_rgba`; X11 uses it as the window
│        icon, macOS builds an `NSImage` and calls
│        `setApplicationIconImage:` (winit's icon is a no-op on macOS)
│      invariant: one committed icon (`assets/minicon.ico`) is the source for
│        every host; no derived blob is committed, no PNG decoder ships
│      evidence: Windows verified against the live window (`WM_GETICON` and the
│        class icon non-null, dumped icon matches the artwork); X11/macOS are
│        typechecked per target here and compiled by the CI cells, which is the
│        strongest evidence available without a display or an Apple host
│      non-goal: shipping a second icon, a `.desktop` file, or a macOS `.icns`
│        runtime path
├── {UPDATE} settings "Check for updates" → verify → swap → restart  [_] @host=none
│      source: GitHub #4 (owner, 2026-10-12, moved here 2026-10-12); owns
│        `prd/PRD_02_23_minicon.md` boundary + `prd/PRD_02_26_con_control_cli.md`
│      mechanism already present, reuse — do NOT add a stack:
│        `minicon_platform::network_http` (bounded outbound client, feature
│        already enabled) for the GitHub `releases/latest` GET + the asset
│        download; `filesystem_publish::publish_file`/`write_file_atomic`
│        (staging sibling + atomic replace, retained backup) for the swap;
│        `src/cli.rs` `current_exe` + the `install-cli` permission-hint for
│        resolving/refusing the destination
│      invariant: a manual, per-click check only — never background, never
│        silent; version compare is SEMANTIC (0.2.10 > 0.2.9), not lexical;
│        the panel section is a state machine (idle/checking/up-to-date/
│        available/downloading/failed/ready) that survives the settings panel
│        being closed; the network work and the swap run OFF the paint thread
│        (clipboard-worker pattern); a restart is always an explicit user
│        action (it drops every terminal and ends the `--control` endpoint)
│      integrity: verify the SHA-256 sidecar AND the code signature of the new
│        binary before swapping; refuse on any mismatch, old binary untouched
│      safe failure: network/DNS/proxy/rate-limit/truncation, sha or signature
│        mismatch, missing archive entry, disk full, permission denied,
│        rename-aside-succeeded-but-place-failed (roll the backup back) — each
│        leaves the running instance working with a specific message
│      #risk macOS cannot swap a single Mach-O (breaks signature/notarization):
│        a `.app`/`.dmg` path, a platform split in the flow; Windows can
│        rename a running exe but not overwrite it (hence rename-aside→restart)
│      #risk no archive decoder exists in the tree (no zip/tar/gz dep); pick
│        (a) a minimal pure-Rust decoder or (b) an extra uncompressed release
│        asset — a gating decision (§Open)
│      dependency: owner boundary-wording edit in `README*.md` + `prd/PRD_02_23`;
│        release assets/SHA sidecars per the release skill
│      non-goal: auto/background update; delta patches; rollback UI; updating
│        anything but the product binary; any telemetry
│      evidence: the whole state machine proven against a loopback server
│        (sha-mismatch, truncated response, rate-limit, permission-denied), the
│        swap tested against a temporary copy of the binary (never the running
│        one), `ui-snapshot` publishes the update state; a real-GitHub smoke is
│        a separate opt-in (`--ignored`) test, not part of the default gate
└── {VERSION} 0.2.7                                                 [_] @host=none
       `Cargo.toml` version 0.2.7; `release-policy.json` is bumped by the
       release step, not here
```

## GATE status

Not started. If `{UPDATE}` ships, its whole state machine must be provable
against a loopback server, with real-GitHub as a separate opt-in (`--ignored`)
test — not part of the default gate. Version-gate runs per the
`run-reputation-and-release` skill when a Candidate is cut.

## Open decisions (owner)

1. **Boundary wording** (gate): acceptance of the line "「不留痕」constrains the
   product's own residue, not a user-authored data file", and the edit to
   `README.zh.md`/`README.en.md` ("no auto-update" stays true because the check
   is manual/per-click) plus `prd/PRD_02_23_minicon.md`.
2. **Archive decode** (gate): minimal pure-Rust decoder vs an extra uncompressed
   release asset.
3. Backup retention: remove the previous binary after one successful launch, or
   keep it?
4. Behaviour when the remote version **differs but is older** than the local
   build (owner said "不一致", not "newer").
5. macOS path: whole-bundle update vs hand off to the `.dmg`/Releases page.
6. 0.2.7's other scope beyond `{UPDATE}`.
