# 0.1.9 dual-signed release runbook (Windows + macOS)

The exact sequence to cut minicon 0.1.9 with BOTH company signing lines on.
Every court's `required` path binds source_sha == origin/main HEAD, so do NOT
commit to main between the version-bump commit and `release.yml`.

## 0. Preconditions (all verified 2026-09-13)
- Windows: Azure Trusted Signing wired; `company-signing.yml` CI-verified.
- macOS: Developer ID cert + notary key in `release-signing`; `macos-signing.yml`
  CI-verified. Integration in `candidate_bundle.py` + `candidate.yml` +
  `release.yml` (gated behind `signing.macos.mode`), self-tested + actionlint.

## 1. Version bump + turn on both switches (one commit)
- `Cargo.toml`, `crates/minicon-core/Cargo.toml`: `version = "0.1.9"`.
- `release-policy.json`: `version` = `0.1.9`; `signing.mode` = `required`;
  `signing.macos.mode` = `required`.
- Commit + push. This commit's SHA is the release `source_sha` for the whole
  chain. Note it.

## 2. Local cross-build
- `./scripts/six-cell-qualify.sh` on this Apple-silicon Mac (30-90 min; refuses a
  dirty tree; receipt `target-six/receipt.json`).

## 3. One-pack (unsigned upstream both courts consume)
- `gh workflow run minicon-com.yml --ref main`; wait; note `MINICON_COM_RUN_ID`.

## 4. Both signing courts (both bind the SAME minicon-com run)
- Windows: `gh workflow run company-signing.yml -f source_sha=<SHA>
  -f minicon_com_run_id=<MINICON_COM_RUN_ID> -f qualification_only=false`; wait;
  note `COMPANY_SIGNING_RUN_ID`.
- macOS: `gh workflow run macos-signing.yml -f source_sha=<SHA>
  -f minicon_com_run_id=<MINICON_COM_RUN_ID> -f qualification_only=false`; wait;
  note `MACOS_SIGNING_RUN_ID`.

## 5. Candidate (Windows = upstream replacement; macOS = side input)
- `gh workflow run candidate.yml -f source_sha=<SHA>
  -f upstream_run_id=<COMPANY_SIGNING_RUN_ID>
  -f macos_signing_run_id=<MACOS_SIGNING_RUN_ID>`; wait; note `CANDIDATE_RUN_ID`.
  (When `signing.mode=required`, candidate's upstream is the company-signing run,
  not minicon-com.)

## 6. Reputation (Defender scans the SIGNED Windows bytes)
- Local UTM Windows Defender court on the sealed candidate:
  `MINICON_DEFENDER_COURT=win-aarch64-desktop
   ./research/minicon-com-loader/utm-win-defender-court.sh <candidate-dir> /tmp/defender-receipt.json`
  then `reputation_court.py qualify ...` → `reputation-qualification.json`.
- `gh workflow run reputation.yml -f candidate_run_id=<CANDIDATE_RUN_ID>
  -f source_sha=<SHA> -f qualification_base64=<base64>`; wait; note
  `REPUTATION_RUN_ID`.

## 7. Release (dry-run first, then publish — confirm with owner before publish)
- Dry run: `gh workflow run release.yml -f candidate_run_id=<CANDIDATE_RUN_ID>
  -f source_sha=<SHA> -f reputation_run_id=<REPUTATION_RUN_ID> -f version=0.1.9
  -f confirmation=dry-run-publish-v0.1.9 -f dry_run=true`.
- Publish (OUTWARD, owner-gated): same with `confirmation=publish-v0.1.9
  -f dry_run=false`.

## 8. After publish
- Backfill `prd/archive/v0.1.9-release-history.md`, commit only that, push.
- The release ships: 6 platform bundles (macOS tar.gz carries the signed binary)
  + the stapled `minicon-0.1.9-macos-universal.dmg` + `minicon.com` + sidecars +
  both signing receipts + reputation qualification.
