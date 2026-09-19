# macOS Developer ID signing — status and remaining work

Status: **SHIPPED in v0.1.9 (2026-09-13).** macOS Developer ID signing + notarization is live in the first dual-signed MiniCon release; the sections below are kept as the record of how it was built. As of 2026-09-13
the two Apple credentials exist, are verified, and are wired into CI. The signed
macOS release itself (workflow, policy gate, packaging, receipts) is scheduled
for **v0.1.9 / v0.1.10**, after the Windows signing switch ships alone first.

The reusable procedure and the redacted setup evidence live in the company hub
skill `sign-macos-artifacts` (`skills/sign-macos-artifacts/SKILL.md` and
`references/apple-signing-setup.md`). This file tracks only the minicon-specific
remainder.

## Done (2026-09-13)

- **Developer ID Application certificate** created (portal route, G2 Sub-CA),
  CN `Developer ID Application: PARTNERNET SOFTWARE PTY LTD (L2N7M5M544)`,
  expires 2031. Verified with `security find-identity` and a real
  `codesign --options runtime --timestamp` that chains to Apple Root CA and
  passes `--verify --strict`.
- **App Store Connect API notary key** (Team Key, Developer role) created and
  validated (`notarytool store-credentials --validate` → Success).
- Both stored as `release-signing` GitHub Environment secrets/vars
  (`MACOS_CERT_P12_BASE64`, `MACOS_CERT_P12_PASSWORD`, `ASC_API_KEY_P8_BASE64`,
  `ASC_API_KEY_ID`, `ASC_API_ISSUER_ID`, var `MACOS_SIGN_IDENTITY`) and in the
  local vault `~/.private_keys/` (0600). No key material is in Git.

## Signing court (built + CI-verified 2026-09-13)

`.github/workflows/macos-signing.yml` — the macOS counterpart of the Windows
`company-signing.yml`. It assembles the universal binary from the verified osx
cells, signs it (hardened runtime + timestamp) in a throwaway keychain,
notarizes it, and builds a signed+notarized+**stapled** `.dmg`, then writes a
`macos-signing-receipt.json` and uploads it. Proven in CI (qualification run:
preflight+sign green; dmg `spctl` = accepted; both notarizations Accepted).
`release-policy.json` carries an independent `signing.macos.mode` (default off).

## Remaining (the engineering, delegable)

Deliberately deferred to the v0.1.9 cut (done + tested together with the version
bump and the Windows switch), because this changes the working release chain and
its `required` path can only be exercised by a real signed release — exactly as
the Windows integration in candidate.yml/release.yml has never run until the
first signed release either. Turnkey steps:

1. **Packaging decision — confirmed:** ship the signed+notarized binary in the
   existing `tar.gz` AND add a stapled `.dmg` asset. No Developer ID Installer
   cert needed.

2. **candidate.yml preflight** — after the Windows `signing_mode` block, read the
   independent `macos_signing_mode = jq -r '.signing.macos.mode // "off"'`. When
   `required`: require a successful `macos-signing.yml` run for `source_sha`
   (event workflow_dispatch, head_sha match), download its `macos-signed-minicon`
   artifact, and assert `.release_eligible == true` in its
   `macos-signing-receipt.json`. Emit a `macos_signing_mode` output. Recommended
   model is **side-input** (candidate keeps its existing upstream and additionally
   pulls the macOS court's signed binary + dmg), so Windows and macOS signing can
   compose; do not make macOS a whole-upstream replacement like Windows unless
   only one signing line is ever on at a time.

3. **candidate.yml macos-universal packaging** — when `macos_signing_mode ==
   required`, instead of `lipo`-ing the unsigned cells, use the court's signed
   universal binary (verify its sha256 == receipt `macos-universal.after_sha256`)
   as `package-bin/minicon`; and stage the court's stapled
   `minicon-<version>-macos-universal.dmg` (verify sha256 == receipt
   `macos-universal-dmg.sha256`) as an additional dist asset.

4. **candidate manifest / receipts** — record the macОS signing receipt sha in the
   candidate manifest under `receipts.macos_signing.sha256` (mirror the Windows
   `receipts.signing`), so release.yml can bind it.

5. **release.yml verify** — mirror the Windows check (`release.yml:117-123`): when
   `macos_signing_mode == required`, assert the shipped `.dmg` and signed binary
   sha256 match the manifest, and that `spctl` in the receipt is `accepted`.
   Publish the `.dmg` alongside the `tar.gz`.

6. **Policy + docs** — flip `signing.macos.mode` to `required` in the same commit
   that bumps to the target version; `CODE_SIGNING_POLICY.md` already has the
   macOS section. Keep the two signing switches independent.

7. **Test at cut** — run the full chain once (six-cell → minicon-com →
   company-signing if Windows on → macos-signing → candidate → reputation →
   release, dry-run first) to exercise both the off and required paths before the
   real publish.

### Also open
