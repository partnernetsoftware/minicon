# macOS Developer ID signing — status and remaining work

Status: **credentials done; signed release still to build.** As of 2026-09-13
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

1. **Packaging decision.** The mac artifact ships as `tar.gz` today; a bare
   Mach-O cannot be stapled. Decide: keep `tar.gz` (Gatekeeper online check, no
   staple) or move to a stapleable `.dmg`/`.pkg` (a `.pkg` also needs a
   *Developer ID Installer* certificate). This decision gates the rest.
2. **Candidate/release integration** — consume the court's receipt, verify the
   signed macOS bytes, ship the signed binary + `.dmg`, and gate on
   `signing.macos.mode`. (The court itself — the `macos-15` signing job: temporary keychain import of the
   `.p12` + `set-key-partition-list`; `lipo` then `codesign --options runtime
   --timestamp`; package; `notarytool submit --wait`; `stapler staple` where
   supported; `spctl -a -t exec` verify.
3. **Policy + receipts.** Add an independent macOS switch to
   `release-policy.json` (parallel to the Windows `signing.mode`), document it in
   `CODE_SIGNING_POLICY.md`, and record before/after SHA, TeamIdentifier, cert
   CN, notarization submission id and `spctl` verdict.
4. **The APE `minicon.com` is likely unsignable on macOS** (PE/ZipOS hybrid, not
   a Mach-O; `codesign` signs Mach-O). Resolve before promising macOS coverage
   for that artifact; the native universal Mach-O is the safe target.
