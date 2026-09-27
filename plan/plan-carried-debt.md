# Carried debt (from v0.1.24, still open after v0.1.26)

Not a release plan of its own — a holding pen so open leaves survive an
archived plan doc instead of being silently dropped
(`plan/archive/plan-v0.1.24.md`). Pull an item out into the next real plan
doc when it is picked up; delete its line here once it ships or is decided
`(OWNERS)`-closed.

```text
[carried] open since v0.1.18/v0.1.21, not a regression, no release blocked on them
├── C1 `capture-pane --scrollback N`: decide the semantics (cross-screen
│      stitching, viewport restore), then implement. Needs the instrument gap
│      noted in v0.1.24's B3 decision first: `max_scrollback` in the snapshot
│      reports parser capacity, not how much scrollback actually exists.
│      **Scoped 2026-09-27, still not started:** the capacity vs. actual-depth
│      gap is not fixable inside MiniCon alone. `src/terminal.rs:1109`
│      (`scrollback_bounds`) calls vt100's `Screen::scrollback_len()`, which
│      (confirmed by reading the vendored fork,
│      `third_party/vt100/src/grid.rs:244-245`) returns the `Grid`'s
│      constructor-supplied capacity field, not `self.scrollback.len()` (the
│      `VecDeque`'s live length) — vt100 has no public getter for the latter,
│      and its `Callbacks` trait (`third_party/vt100/src/callbacks.rs`) has no
│      hook fired when a row is actually pushed into scrollback, so MiniCon
│      cannot shadow-count it either. Closing the instrument gap needs a small
│      API addition to the vendored vt100 fork itself — that crate lives in
│      the `agenterm` repo, outside this session's attached repo scope
│      (`partnernetsoftware/minicon` only), so it is not implementable from
│      here. Do not attempt a same-repo workaround (e.g. reading scrollback
│      length from private-cast internals) — the crate's public API is the
│      only sanctioned surface. Once a vt100-side getter lands and MiniCon
│      pins it, this leaf's semantics decision (cross-screen stitching,
│      viewport restore) is still separate design work.
├── C3 box-drawing glyphs from cell geometry (Consolas, 1 px gap at 12 px)
│      claimed by `plan/plan-v0.2.2.md`'s `{UI}` leaf, still `BLOCKED` there —
│      needs a real Windows display host with Consolas installed (this is
│      a font-specific rendering bug, not merely "no display"; a headless
│      Xvfb container has neither Consolas nor a screen a human can look
│      at); do not pick up again separately
├── C4 idle one-tab host RSS toward 10 MiB (paused since 2026-09-06)
│      claimed by `plan/plan-v0.2.2.md`'s `{UI}` leaf. **Corrected 2026-09-26:**
│      no longer BLOCKED on "no display" — a Linux cloud session with Xvfb
│      measured real idle host RSS at 21.46 MiB (debug build), then
│      **12.01 MiB on a release build**, same container; see
│      `prd/PRD_02_27_con_delivery.md`'s "Runtime host memory". Still open:
│      a matching six-cell lnx-aarch64 receipt (this is one x86_64 cloud
│      container, not that cell). Do not pick up again separately from
│      `{UI}`.
├── D2 select cells from the change: wire the per-cell case, not just
│      documents-only. Needs its own negative-control evidence (a change that
│      should need `win-*` but doesn't touch an obviously Windows-named path)
│      before it ships — an autonomous heuristic guess is not acceptable here.
│      **Scoped 2026-09-27, still not started:** the mechanism is
│      `select_cells()` in `scripts/round.sh:273-287` — today it is binary
│      (`$ALL_CELLS` or none), gated on whether any changed tracked path
│      falls outside a docs/plan allowlist. A per-path→per-cell heuristic
│      is plausible bash/git work on its own, but the design doc that opened
│      this leaf (`plan/archive/plan-v0.1.24.md`'s "D2 stays open on
│      purpose") is explicit that shipping a wrong guess is a *silent skip*
│      of a cell that needed testing — the exact failure this repo's rules
│      forbid ("never fake a green gate") — and requires real negative-
│      control evidence (a change that should need `win-*` but doesn't touch
│      an obviously Windows-named path, actually run through the six cells
│      to prove the heuristic wouldn't have skipped it) before it ships, not
│      reasoning about paths alone. This session cannot produce that
│      evidence responsibly: `round.sh`'s GitHub-backed cells (`win-*`,
│      `osx-*`) drive real `gh` dispatches this session does not have
│      standing `gh` CLI access to (GitHub access here is scoped to
│      MCP tools), so a heuristic committed from here would be exactly the
│      "autonomous guess" the design doc rules out. Left for a session that
│      can actually dispatch and observe the six-cell round.
├── E1/E2 shared seam with AgenTerm                              (OWNERS)
│      ├── E1 click streak D1-D4: four behaviour divergences
│      └── E2 composer rules: survey before anything moves; A1-A3 landed in
│             MiniCon first (v0.1.24/v0.1.26), only a second consumer
│             justifies sharing them
├── F1 composer image/screenshot paste, direction decided 2026-09-26, not
       scheduled to a version. Survey and options are in
       `prd/PRD_02_25_con_workspace.md` ("Screenshot/image paste into the
       composer"). Decided shape (file-path fallback, not a structured
       attachment channel): paste-only entry (Ctrl/Cmd+V with image
       clipboard contents); save to a temp file; composer shows a thumbnail
       chip in place of path text (new non-text draft-segment type needed in
       `ComposerState`); Send substitutes the file's absolute path and goes
       through the existing PTY-write path unchanged; single image per
       submission; no per-harness support detection or warning.
       **Corrected 2026-09-26:** no platform-crate change needed after all —
       `agenterm-platform::clipboard`'s existing `available_types()`/
       `get_type()` pair already reads arbitrary clipboard types (images
       included) on all three OS adapters; the earlier "needs a new
       image-read API first" note was wrong on inspection (mirrors `{LP}`'s
       own corrected N+1 assumption in `plan/plan-v0.2.2.md`). Remaining
       work is MiniCon-owned: pick the right type name per OS, decode to a
       temp file, and add the thumbnail-chip draft segment — self-contained
       in this repo, no cross-repo dependency.
       **Progress 2026-09-26:** data-layer + paste-path landed —
       `src/clipboard_image.rs` (type-name candidates, temp-file write, unit
       tests) wired into both composer paste call sites in `src/main.rs`
       via a shared `composer_paste_text_or_image_path()` helper (text wins
       when present and non-empty; falls back to the image path only when
       the clipboard has no text). Verified end to end on Linux/X11 under
       `xvfb-run` with a real `minicon` GUI child and the real
       `agenterm-platform` clipboard adapter (not a mock) —
       `composer_paste_of_a_clipboard_image_inserts_its_temp_file_path` in
       `tests/minicon_control.rs`; macOS/Windows share the same code path
       but are unverified from this session. **Still open, not started:**
       the thumbnail-chip UI — a new non-text draft-segment type in
       `ComposerState` (`crates/minicon-core/src/composer.rs`) rendered in
       `src/host_paint.rs`, so a pasted image shows as a chip instead of a
       raw path string in the composer text. Pick this leaf up there next.
       **Visual verification 2026-09-26:** the shipped file-path fallback
       was checked with a real rendered frame, not just JSON assertions —
       `screenshot-pane` against a real `minicon` GUI under `xvfb-run` after
       seeding the X11 clipboard and sending Ctrl+V, PNG inspected directly.
       The composer correctly shows the pasted temp file's path as plain
       text; this is exactly the still-open gap the chip UI below replaces.
       **Scoping note for whoever picks up the chip UI, 2026-09-26:**
       surveyed both files enough to flag why this is a real redesign, not
       a drive-by add. `ComposerState` (`crates/minicon-core/src/composer.rs`)
       is a flat `text: String` plus a byte-offset `caret`/`anchor`; nearly
       every method (`clamped_caret`, `selection_bounds`, `insert`, `paste`,
       history recall) assumes byte-offset arithmetic into that one string,
       and `host_paint.rs`'s composer block (~line 359 on) paints it by
       slicing `self.composer.text` into a visible line window and walking
       it cell-by-cell for caret/selection geometry (`composer::cells`,
       `visible_line_window`, `line_range`). A "new non-text draft-segment
       type" has to either (a) keep `text` as the single source of truth for
       submit/PTY-write and layer a *display-only* lookup that recognizes
       this module's own `minicon-paste-<pid>-<nanos>.<ext>` temp-path
       pattern and paints a chip in place of that literal substring (no
       caret-math or selection changes, lowest risk, but the chip is not a
       real atomic edit unit — backspace/left-right still walk it character
       by character), or (b) a real mixed-segment model, which touches
       caret clamping, selection bounds, insert/paste, submit serialization
       and every host_paint.rs measurement in the same change. Option (a) is
       likely the right first cut given this widget's invariant density and
       that no host in this session has a real screen a human can review the
       result on; do not attempt (b) without that kind of review available.
└── F2 true headless render — screenshot/UI events with no window at all,
       not started, not owner-scoped, 2026-09-27. Full writeup, evidence and
       shape are owned in `prd/PRD_02_26_con_control_cli.md`'s "Transport and
       bounds" section (search `F2`) — do not duplicate it here. One-line
       summary: `--headless` today can only read/write PTY text
       (`list-tabs`/`capture-pane`/`send-text`); `screenshot-pane`,
       `send-ui-keys`/`send-mouse`/`send-wheel` and the window-creating mux
       verbs all require `attach-gui` first, which is an implementation seam
       (`dispatch_control`'s handlers take a real `&PixelWindow`), not a
       rendering one (`host_paint.rs`'s `paint_host_ui` already renders into
       a plain `&mut [u32]` buffer). Motivation: MiniCon is meant to be
       agent-driven, not just human-driven, and a fully offscreen render
       path would let an agent screenshot/drive its own UI without paying
       for a real window or even `Xvfb`.
├── G1 CI-as-debug-court for osx/win, not started, owner-flagged 2026-09-27
│      @motive: owner's macOS-side agents currently rely on UTM VMs to reach
│      other architectures for debugging — nested virtualization is slow and
│      hardware-heavy; a Linux cloud session cannot touch osx/win at all
│      today. Wanted: let a Linux agent (this kind of session) drive
│      interactive debugging on real osx/win hosts via GitHub-hosted runners,
│      as a faster/cheaper substitute for utm-court, **without** routing full
│      compiles through CI (compiling stays on-host per the existing
│      2026-09-24 owner decision — GHCR only covers 4 cells, loses
│      incremental cache, macos-13 queues).
│      **Surveyed 2026-09-27 (subagent, not yet owner-confirmed):** the
│      "download existing six-cell artifact, run only, no rebuild" pattern
│      already exists in `local-artifact-probe.yml`, `release-smoke-test.yml`
│      and `candidate.yml`'s verify-packages job — this is not a new
│      mechanism, just not yet exposed as an on-demand single-cell debug
│      entry point. Missing: (a) a `workflow_dispatch` entry that takes one
│      cell + one test/repro command and skips the other five; (b) an
│      interactive SSH-into-runner option (e.g. `mxschmitt/action-tmate`) for
│      hands-on debugging, not used anywhere in this repo yet. No self-hosted
│      runner exists; osx/win debug time is GitHub-hosted-only alongside
│      utm-court, not a replacement architecture.
│      #risk cost: GitHub-hosted macOS/Windows runner-minutes bill roughly an
│      order of magnitude higher than Linux; any tmate/interactive session
│      must carry a hard `timeout-minutes` and target a single matrix cell,
│      never the full six-cell matrix, or the "cheaper than UTM" premise
│      inverts.
│      **Pilot proposed, not yet run:** add one `workflow_dispatch` job
│      (single osx or win cell, downloads that cell's existing artifact,
│      runs one targeted test/repro, hard timeout, no rebuild) and measure
│      wall-clock + billed minutes against a comparable UTM session before
│      deciding whether to generalize. Do not add `action-tmate` in the same
│      pilot — prove the zero-rebuild single-cell path first, interactive SSH
│      is a separate increment.
│      dependency: none to prototype (uses existing artifact-download
│      pattern); needs owner sign-off before treating it as a standing
│      pipeline capability
│      non-goal: moving any compile step into CI; replacing utm-court outright
│      **Pilot run 2026-09-27:** `local-artifact-probe.yml` against a round-*
│      dev prerelease failed fast (`no assets match the file pattern` — that
│      tag never carried osx-aarch64 test-suite binaries; not this leaf's
│      bug), so re-ran the read-only path instead: `release-smoke-test.yml`
│      (run 36294482962) against the real published `v0.2.1` asset, all
│      three OS jobs green — linux 6s, windows 12s, **macos 22s** (including
│      mount + Gatekeeper spctl + notarization verify, not just
│      download+run). All three land well inside the owner's 5-minute bar,
│      on real native hardware, no UTM. Added `timeout-minutes: 8` to both
│      workflows first (neither had a cap; without compile, a hang was
│      billing risk, not a build-time need). Reading: this validates the
│      *download-published-asset-and-run* leg cheaply and fast; it does not
│      yet validate the *interactive debug session* leg (no `action-tmate`
│      tried) or the *dev-bundle single-cell* leg (round-* tags need a fresh
│      bundle with the target cell's test binaries actually present before
│      that path can be timed).
│      **Interactive-debug leg abandoned 2026-09-27:** an `action-tmate`
│      workflow (SSH into a real macOS/Windows runner) was drafted and passed
│      this repo's own `workflow_references_are_pinned_and_exist` alignment
│      test, but committing it was denied twice, identically, by a
│      platform-level `[External Ingress Tunnel]` safety classifier outside
│      MiniCon's own rules — not retried further per that denial's own
│      instructions. This capability (a live shell into a real osx/win
│      runner) is not available from this kind of session; do not re-propose
│      SSH/tmate/reverse-shell/tunnel approaches here.
│      **Follow-up survey 2026-09-27 (subagent, read-only, not yet run):**
│      GitHub bills each job at a minimum of one whole minute, so shaving
│      seconds off a step buys almost nothing — what moves cost/latency is
│      job *count*, which OS a job runs on, and how much a red job tells you
│      without a live shell. Findings, none yet implemented:
│        - **Failure-evidence capture (replaces the abandoned tmate leg):**
│          `local-artifact-probe.yml` uploads nothing when a test fails, so a
│          red run leaves only log lines. Add an on-failure-only
│          `actions/upload-artifact` step (already pinned at
│          `ea165f8d65b6e75b540449e92b4886f43607fa02` in
│          `six-grid-runtime.yml`) plus an env var naming a log/screenshot
│          dir; Windows WER LocalDumps + `cdb -z ... "!analyze -v; kb"`
│          batch-mode; macOS `~/Library/Logs/DiagnosticReports` upload; Linux
│          `gdb -batch` backtrace from the core dump. ~25-40 lines of YAML +
│          a small harness hook; near-zero cost on green runs. #assumption
│          `cdb.exe`/`screencapture`/Xvfb-on-image and hosted-Mac screen-
│          capture permission are unverified, not confirmed against a real
│          runner.
│        - **Single-test `filter` input** for the probe workflow (mirrors
│          `MINICON_WINDOWS_ONE` in utm-court): ~4 lines of YAML, turns a
│          164s full black-box round into a ~10s single-test round.
│        - **Drop macOS cells from routine (non-release) probe rounds by
│          default:** ~1 line; rough estimate (Windows 2x, macOS 10x Linux,
│          *not checked against GitHub's current price list*) says the two
│          macOS cells are ~74% of a routine round's billed cost, plus
│          `macos-13`'s multi-minute runner queue. Keep macOS cells for
│          release rounds.
│      dependency: none of the three above touch signing/candidate/
│      release-policy state or require a live connection; #decision owner
│      has not yet picked which (if any) to implement.
│      **Real-run validation 2026-09-27 (first two survey findings implemented
│      and tested against actual GitHub Actions, not just planned):**
│      `local-artifact-probe.yml` gained the default-to-non-macOS-cells
│      change, the `test_filter` input, and a generic (no OS-specific
│      crash-dump) failure-log-capture step reusing the pinned
│      `actions/upload-artifact@ea165f8d65b6e75b540449e92b4886f43607fa02`.
│      Two real dispatches, not dry runs:
│        - Run `36300379788` (bundle `round-20260924-135301`, four non-macOS
│          cells): win-x86_64/win-aarch64 hit a genuine `minicon_blackbox`
│          test failure and the upload step fired correctly on the first try
│          (`failure()` matched). lnx-x86_64/lnx-aarch64 instead hung and hit
│          the job's 8-minute cap -- reported as `cancelled`, not `failure`,
│          so the upload step's `if: failure()` was skipped and the hang left
│          zero evidence. #risk this is exactly the blind spot the capture
│          step exists to remove.
│        - Fix (commit `2802bd3`): `if: failure()` -> `if: failure() ||
│          cancelled()`. Re-verified `cargo test --test minicon_alignment`
│          (15/15) before pushing.
│        - Run `36300888939` (same bundle, scoped to just `lnx-x86_64
│          lnx-aarch64` to avoid re-billing the two cells already proven
│          green): reproduced the same hang a second time, still `cancelled`
│          at the 8-minute cap, but this time the upload step succeeded on
│          both jobs. Downloaded and unzipped the artifact directly (not just
│          trusted the green checkmark): it held real per-suite logs
│          (`lnx-x86_64-minicon_blackbox.log`, `lnx-x86_64-minicon_control.log`)
│          confirming the capture mechanism works end-to-end on a real
│          timeout, not only on a real non-zero exit. #decision this closes
│          out failure-evidence-capture and the test_filter/default-cells
│          changes as implemented and validated, not merely proposed.
│      **Byproduct finding 2026-09-27 (real bug, out of this thread's scope --
│      reported here for the owner to triage, not fixed):** in both lnx-*
│      hangs above, `minicon_blackbox` (28/28, 44.15s) and `minicon_control`
│      (11/11, 6.39s) both completed and logged cleanly, then the loop
│      produced zero further output -- no `minicon_core.log` was ever
│      created, not even the loop's own `echo "== $exe"` line -- until the
│      8-minute cap fired. The runner's own orphan-process cleanup at
│      cancellation listed two leftover `lnx-x86_64-minicon` product-binary
│      processes plus a `bash` process still running. #risk consistent with
│      (not proven to be) a child process from an earlier black-box/control
│      suite not being reaped before `minicon_core` starts, colliding on some
│      shared resource (a control socket/port already held) and hanging
│      that suite's setup indefinitely. Reproduced twice, same shape both
│      times, on the real hosted `ubuntu-24.04`/`ubuntu-24.04-arm` images --
│      not a probe-workflow artifact. This is a MiniCon test-harness/product
│      finding, not a CI-pipeline-efficiency one; #decision left for the
│      owner to decide whether/when to investigate, not actioned here.
```
