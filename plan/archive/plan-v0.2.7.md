# plan-v0.2.7

**Shipped 2026-10-13** as [v0.2.7](https://github.com/partnernetsoftware/minicon/releases/tag/v0.2.7)
from `f52662a`; the exact-artifact chain is in
`prd/archive/v0.2.7-release-history.md`. This file is evidence history, not
current scope. The one leftover it names — the notes scratchpad (GitHub #5) —
is carried in `PRD.md`'s v0.2.6 entry.

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
├── {ICON} window icon on Windows and X11                          [v] @host=win,x11
│      owner: `prd/PRD_02_25_con_workspace.md` "Host UI"
│      Windows: load the PE icon group (`winresource` id 1) into the
│        window class `hIcon` and pin it with `WM_SETICON` (big + small)
│      X11: `build.rs` decodes the largest face of the same
│        `assets/minicon.ico` into RGBA; the product passes it through
│        `PixelWindowOptions::window_icon_rgba` and winit sets the window icon
│      invariant: one committed icon (`assets/minicon.ico`) is the source for
│        every host; no derived blob is committed, no PNG decoder ships
│      evidence: Windows verified against the live window (`WM_GETICON` and the
│        class icon non-null, dumped icon matches the artwork); X11 is
│        typechecked by `cargo zigbuild --target x86_64-unknown-linux-gnu` and
│        compiled by the CI cells, the strongest evidence available without a
│        display
│      #deferred macOS runtime icon — the shipped `.app` already carries
│        `CFBundleIconFile`; the bare `.tar.gz` binary still shows the system
│        default. Setting it needs `objc2` as a direct platform dependency,
│        which would also pull it into Linux builds. Named gap in
│        `prd/PRD_02_25_con_workspace.md`, not claimed here.
│      non-goal: shipping a second icon, a `.desktop` file, or a macOS `.icns`
│        runtime path
├── {UPDATE} settings "Check for updates" → verify → swap → restart  [v] @host=none
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
│        a separate opt-in (`--ignored`) test, not part of the default gate —
│        and that opt-in test is what caught the one defect the loopback gate
│        could not (see below)
│      fixed during 0.2.7: `ureq`'s runtime default TLS provider is Rustls, but
│        the Windows manifest enables only `native-tls`, so the first real
│        `https://` call panicked in `ureq`'s transport ("provider is Rustls
│        but feature is not enabled: rustls"). The updater is this product's
│        first `https` caller, so it shipped broken. `network_http` now names
│        its provider per host (Windows `NativeTls`, Unix `Rustls`), matching
│        the manifest feature tree exactly
└── {VERSION} 0.2.7                                                 [v] @host=none
       `Cargo.toml` version 0.2.7; `release-policy.json` is bumped by the
       release step, not here
```

## GATE status

The `{UPDATE}` state machine is proved against a loopback server, with real
GitHub as a separate opt-in path — not part of the default gate. Version-gate
runs per the `run-reputation-and-release` skill when a Candidate is cut.

### Decisions taken (were §Open, now recorded)

1. **Boundary wording** — accepted as written: the update check is manual and
   per-press, so the README's "no auto-update" line stays true with a
   qualifier. Landed in `README.zh.md`, `README.en.md`,
   `prd/PRD_02_23_minicon.md` "Explicit non-goals".
2. **Archive decode** — option **(a)**, a small in-product reader for the two
   shapes shipped (deflated `.zip` entry, gzipped tar member). Chosen over
   publishing an extra uncompressed asset because it needs **no change to the
   release chain**, which is the part of this system whose correctness the
   whole no-rebuild discipline depends on. `flate2` (already in the graph as
   `png`'s pure-Rust backend) and `sha2` are named directly instead of pulling
   an archive crate. **Measured cost**: the local `release` build grew
   1,713,152 → 1,868,800 bytes (+152 KB, +9%) for the two named crates plus the
   reader, the state machine and the panel section. Recorded as a considered
   price rather than hidden; the Windows release profile applies its own size
   settings on top.
3. **Backup retention** — the replaced executable stays beside the new one as
   `<name>.backup`. Kept, not removed after a launch: a file the user can see
   is a file the user can roll back to, and the "不留痕" rule is about the
   product's own residue, not an update's two files.
4. **A strictly older remote** — shown as "you are ahead of the latest
   release", and the Update control is not offered: a `main` build must never
   be nudged to downgrade silently.
5. **macOS path** — no in-process swap. The Update control opens the release's
   own `.dmg`; replacing one file inside a signed, notarized `.app` breaks the
   signature and Gatekeeper refuses the result.

### Known gap (named, not claimed)

Code-signature verification of the new binary is **not** implemented. The
in-tree inspectors sit behind `minicon-platform`'s `app-facts` feature, whose
app inventory the product does not otherwise carry; enabling it for one boolean
is a size decision. The checksum gate is in place. Recorded in
`prd/PRD_02_25_con_workspace.md` "Host UI".

### A defect the offline gate cannot see (fixed in 0.2.7)

The loopback tests all speak `http://`, so none of them touches the TLS layer.
The opt-in real-GitHub smoke ran for the first time while driving this release
and instantly panicked: `ureq` picks its TLS provider at *runtime* and defaults
to Rustls ("the highest chance of being enabled"), while the Windows manifest
enables only `native-tls`. On Windows — the one host we can actually run — the
shipped check-for-updates would have panicked on its first press.

`network_http::request` now pins the provider to the one the build enables
(`#[cfg(windows)] NativeTls`, `#[cfg(unix)] Rustls`), so the runtime choice and
the manifest's target-specific features can no longer disagree.

Consequence for the gate: the default suite can never prove an `https` path,
because it has no server. The real-GitHub opt-in is not a nice-to-have — it is
the only check that exercises the TLS provider selection, on any host. It stays
`#[ignore]`d (CI must not depend on GitHub being reachable), so it has to be run
by hand before a release. **The Unix half is still unexercised**: nothing here
can execute a Linux/macOS binary, so the `Rustls` arm rests on the manifest's
feature tree (`rustls` = `rustls-no-provider` + `_ring` + `rustls-webpki-roots`,
verified by reading `ureq`'s own `Cargo.toml`) rather than on a live call.

## Open decisions (owner) — all closed except one

The six items below were the live list while 0.2.7 was in flight. Items 1-5 are
answered by "Decisions taken" above and shipped; item 6 was answered by the
icon-parity scope. Kept in place rather than deleted so the reasoning is
auditable. **Still open: the notes scratchpad (GitHub #5)**, which 0.2.6
deferred into 0.2.7 and 0.2.7 did not pick up; it is carried in `PRD.md`'s
v0.2.6 entry, not here.

1. **Boundary wording** — accepted and landed (see "Decisions taken" #1).
2. **Archive decode** — option (a), the in-product reader (see #2).
3. **Backup retention** — kept as `<name>.backup` (see #3).
4. **A strictly older remote** — "you are ahead", no downgrade offered (see #4).
5. **macOS path** — hand off to the `.dmg` (see #5).
6. **0.2.7's other scope** — window-icon parity on Windows and X11.
