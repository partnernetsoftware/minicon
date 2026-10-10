# PRD archive

This directory preserves superseded decisions and historical qualification
evidence. Archived material explains how a decision was reached; it is not the
current product contract. Current truth starts at `PRD.md`, then follows the
owning `prd/PRD_*.md` module and machine-readable policy.

Rules:

- Never use an archived run as evidence for a later source identity.
- A current module may link here instead of repeating an experiment diary.
- Contradicted plans remain readable, but their status and replacement must be
  explicit at the top of the archive entry.
- Release assets, manifests and GitHub receipts remain the authoritative byte
  evidence; this directory is an index, not a second manifest.

This index is not exhaustive — every `vX.Y.Z-release-history.md` file in this
directory is a real entry even when it is not (yet) listed below; treat a
missing listing as a backlog gap in this README, not as the file not
existing. Recorded here so it is a real backlog item, not silently the whole
directory's future TODO.

Entries:

- `v0.1.3-release-history.md` — native-six-cell release decision, superseded
  signed-APE rehearsals, and exact Promotion evidence.
- `v0.2.0-release-history.md` — `mux`+`harness`, published without runtime
  execution evidence (cross-compile + static-signing + static-scan only);
  full narrative in `prd/PRD_02_31_v0_2_horizon.md`.
- `v0.2.1-release-history.md` — harness statefulness/containment hardening;
  independently smoke-tested post-publish (see its own "Independent
  post-publish smoke test" section).
- `v0.2.3-release-history.md` — multi-line composer editing, Authenticode +
  notarization signing, header settings panel, three themes, grid crosshair.
- `v0.2.4-release-history.md` — composer Send timing (trim + held Enter),
  terminal-scoped Ctrl+V paste review, paste-review content preview; first
  release whose chain was driven from a Windows host.

Complete file index (added 2026-10-10), so a reader can tell "listed" from
"missing" at a glance — the per-file text is the authority, this is only a map:

- `v0.1.3` · `v0.1.4` · `v0.1.5` · `v0.1.6` · `v0.1.7` · `v0.1.9` · `v0.1.10`
  · `v0.1.11` · `v0.1.12` · `v0.1.13` · `v0.1.14` · `v0.1.15` · `v0.1.16`
  · `v0.1.17` · `v0.1.18` · `v0.1.19` · `v0.1.20` · `v0.1.21` · `v0.1.22`
  · `v0.1.23` · `v0.1.26` · `v0.2.0` · `v0.2.1` · `v0.2.3` · `v0.2.4`
- Also in this directory: `v0.1.7-mac-handoff.md` (§ handoff, not a release),
  `azure-work-tenant-signing-enroll.md`.
- No ledger exists for `v0.1.8`, `v0.1.24`, `v0.1.25` or `v0.2.2` — recorded
  here as a known gap rather than left as a silent omission.
