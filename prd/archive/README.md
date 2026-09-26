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
