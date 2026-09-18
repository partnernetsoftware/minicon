# Plan 0.1.16 — macOS `.app` packaging + clarity & test fortification

Status: **Theme M shipped as v0.1.16; Themes A/B/C carry to 0.1.17.** The macOS
distribution fix (the Gatekeeper warning hit on 0.1.15) was the only
user-facing regression-class issue open, so 0.1.16 was deliberately scoped to it
plus the `install-cli` requirement — bundling the large `main.rs` refactor into
the same release would have put unrelated risk in front of a user-facing fix.

Owner directives feeding this plan:
- "把 UI/UX 做到极致,别老让我一个个发现 bug" → proactive invariant tests
  ([[minicon-ux-invariant-testing]] in memory).
- macOS 0.1.15 double-click → "Apple 无法验证…恶意软件": ship a real `.app`.
- **Besides `.app`, the user must also be able to find `minicon` on the command
  line** (minicon is also a control CLI — terminal access is first-class).

---

## Theme M (lead) — macOS `.app` bundle + CLI on PATH — **SHIPPED in v0.1.16**

Delivered: `macos-signing.yml` now builds `MiniCon.app`, signs it inner-out with
a hardened runtime, notarizes and **staples the bundle**, asserts
`spctl -a -t exec` is `accepted / source=Notarized Developer ID` (the gate a bare
Mach-O can never pass), and ships it inside the `.dmg`. Asset *names* are
unchanged, so `candidate_bundle.py`, `release.yml` and the policy needed no
edits; the signing receipt gained a `macos-universal-app` entry. `install-cli`
/ `uninstall-cli` link the executable onto `PATH` and refuse to touch anything
that is not a symlink they own (covered by
`tests/minicon_control.rs`).

### The problem (verified on 0.1.15 bytes)
minicon ships as a bare Mach-O, not a `.app`. Consequences, measured:
- Bare Mach-O **cannot be stapled** (Apple Error 73) — `stapled=False` in the
  signing receipt for `macos-universal`; the `.dmg` container is stapled but the
  binary inside is not.
- Gatekeeper's GUI-launch path only trusts `.app` bundles: `spctl -a -t exec` on
  the bare binary → *"the code is valid but does not seem to be an app"*
  (rejected), even from inside the mounted stapled DMG. So double-click shows the
  scary "cannot verify malware" dialog although the binary IS Developer-ID signed
  AND notarized (Apple accepted it; ticket exists on their servers).

### Goal
1. **`MiniCon.app`** — a proper bundle: `Contents/MacOS/minicon`,
   `Contents/Info.plist` (CFBundleIdentifier `com.partnernetsoftware.minicon`,
   name, version, `LSMinimumSystemVersion`, high-DPI/`NSHighResolutionCapable`),
   `Contents/Resources/` (icon). Codesign the **bundle** with hardened runtime,
   notarize, and **staple the `.app`** (bundles can be stapled) → double-click
   opens cleanly, offline, no warning. `spctl -a -t exec MiniCon.app` must be
   `accepted / source=Notarized Developer ID`.
2. **`minicon` on the command line** — because minicon is also a control CLI, a
   terminal user must be able to run `minicon` and `minicon cli …`. Design
   options (decide during build):
   - A small **`minicon install-cli`** subcommand that symlinks the bundle's
     `Contents/MacOS/minicon` into `/usr/local/bin/minicon` (VS Code's `code`
     model), with an uninstall counterpart; print the target and PATH note.
   - And/or keep shipping the **standalone signed+notarized binary** (today's
     `.tar.gz`) for users who only want the CLI, documenting the
     `xattr -d com.apple.quarantine` / right-click-Open first-run step.
   - Document both in the release notes and `old-windows`-style help page.
3. **Pipeline wiring** (after a local proof): teach the macOS build/pack to emit
   the `.app` (in the `.dmg`, and a zipped `.app`), extend `macos-signing.yml`
   to sign+notarize+staple the bundle, `candidate.yml` to seal the new asset(s),
   `release.yml` + `release-policy.json` to publish them. Keep the bare binary
   asset for CLI-only use unless we fold it into the bundle install flow.

### Execution order (de-risk before touching release CI)
1. **Local proof** (no CI): build the universal binary, wrap `MiniCon.app`,
   sign+notarize+staple locally (Developer ID cert + notary key already in the
   vault; see the `sign-macos-artifacts` hub skill), verify `spctl` accepted and
   a real double-click opens with no warning. Prove `install-cli` symlink works
   and `minicon cli` runs from a plain shell.
2. Only then wire the workflows and cut the asset through candidate/release.

---

## Theme A — code clarity (the long-standing "让 minicon 更清晰")
Now safe to refactor behind the new invariant tests. Behavior-preserving, each
step green through six-cell:
- Split `src/main.rs` (~8800 lines) into `cli.rs` (arg parse + control client),
  `app.rs` (host event loop), `terminal.rs` (single session).
- Extract `paint_host_ui` (~472 lines) → `host_ui.rs` (skeleton exists).
- Extract `build_ui_snapshot` out of `dispatch_control`, then thin
  `dispatch_control`.
- Drop no-information `Con` prefixes where they don't disambiguate.

## Theme B — UX test net + real interactive macOS/Windows courts
- The session-0 Windows court can't present frames (no GUI/GPU) so it can't do
  real mouse-click or pixel-screenshot tests. Get a **real interactive
  logged-in desktop court** (Windows, and macOS) so double-click / Gatekeeper /
  real-click / screenshot behaviour is automatable — this would have caught both
  the click bug and the `.app` Gatekeeper issue.
- Extend invariants to scrollbar/terminal seam, sidebar-drag grip, settings
  panel containment, IME preedit placement, tab-tree row hit precision.
- One proactive UX audit pass (focus behaviour, resize jitter, high-scale
  render, extreme sizes).

## Theme C — doc debt
- README signing version range (v0.1.4–v0.1.6) contradicts
  `CODE_SIGNING_POLICY.md`; correct it.
- Archive completed plans (`plan-0.1.9-dual-signed-release.md`,
  `plan-macos-developer-id-signing.md`, `plan-0.1.12-review.md`).

## Theme D — feature candidates (owner to decide)
Open: remember window size/position, etc. No hard requirement.

---

## Non-goals / guardrails
- No `foundry` repo yet (entry gate unmet; see [[minicon-foundry-and-0116]]).
- Signing identifiers never in receipts/logs/docs; never relax the Defender
  scan; publish promotes the sealed candidate (no rebuild-to-promote).
