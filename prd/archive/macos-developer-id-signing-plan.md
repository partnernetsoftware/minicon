# macOS Developer ID signing + notarization plan

Status: **not started; blocked on owner-gated Apple actions.** MiniCon's macOS
release is today a bare, unsigned, un-notarized universal Mach-O inside a
`tar.gz`. This is the plan to make it a Developer ID-signed, notarized,
Gatekeeper-clean download, distributed outside the App Store. Windows signing
(Azure Artifact Signing) is unrelated and already wired; this concerns only the
macOS artifact.

## What already exists (reusable)

- Paid Apple Developer Program membership under PARTNERNET SOFTWARE PTY LTD,
  Team ID `L2N7M5M544`, Xcode authenticated to that team (company hub
  `docs/current-state.md`).
- Local toolchain on the Mac mini: Xcode 26.6, `codesign`, `notarytool` 1.1.2,
  `stapler`, `lipo` — all present.
- A stable bundle identifier `com.partnernetsoftware.minicon`, embedded in the
  Mach-O via `build.rs` (`assets/macos-info.plist`; it carries a UI
  compatibility key, not signing keys).
- The universal binary is already produced by `lipo -create` in
  `candidate.yml`, so there is a single artifact to sign.

## What is missing

1. **Developer ID Application certificate** — none exists (0 code-signing
   identities in the local keychain; none in CI). This is a different cert type
   from the iOS Apple Development/Distribution certs and from Azure (Windows).
2. **Notary credential** — none exists. Needs either an App Store Connect API
   key (issuer id + key id + `.p8`) or an app-specific password, stored as a
   `notarytool store-credentials` profile locally and as CI secrets.
3. **Codesign step** (hardened runtime) in the release chain — none exists.
4. **Notarize + staple step** — none exists. A bare Mach-O cannot be stapled;
   the binary must be wrapped in a `.zip` (for notarization) and shipped as a
   stapled `.dmg`/`.pkg`, or shipped as the notarized `.zip`.
5. **Policy + receipts wiring** — `release-policy.json` and
   `CODE_SIGNING_POLICY.md` cover Windows only; both need a macOS Developer ID
   dimension and a `codesign --verify` / `spctl` release check.
6. **CI runner keychain setup** — the `macos-15` runner needs the Developer ID
   cert imported into a temporary keychain and the notary credential as secrets.

## Owner-gated steps (only the account owner can do these)

These require Apple portal sign-in and credential creation, which are owner
gates; an agent cannot perform them:

1. **Create a Developer ID Application certificate** in the Apple Developer
   portal (Certificates → Developer ID Application) under team `L2N7M5M544`.
   Export the certificate + private key as a `.p12` with a strong password.
2. **Create a notary credential**: preferably an App Store Connect API key
   (Users and Access → Integrations → App Store Connect API → generate a key
   with the Developer role). Download the `.p8`; record the issuer id and key
   id. (An app-specific password on the Apple ID is the fallback.)
3. Hand both to the signing operator as GitHub Actions secrets on a protected
   environment (mirroring the Windows `release-signing` environment): the base64
   `.p12` + its password, and the API key `.p8` + issuer id + key id.

## Agent-doable steps (once the credentials above exist)

1. Add a `signing.macos` dimension to `release-policy.json` (mode off/required),
   parallel to the Windows `signing.mode`, and document it in
   `CODE_SIGNING_POLICY.md`.
2. Add a macOS signing job to the release chain that, on the `macos-15` runner:
   imports the `.p12` into a temporary keychain; runs
   `codesign --force --options runtime --timestamp --sign "Developer ID
   Application: PARTNERNET SOFTWARE PTY LTD (L2N7M5M544)"` on the lipo'd binary;
   zips it; `xcrun notarytool submit --wait` with the stored credential;
   `xcrun stapler staple` the container; and records a signing receipt.
3. Change the macOS packaging to ship the notarized+stapled container (a
   `.dmg` or `.pkg` can be stapled; a loose binary cannot) instead of, or
   alongside, the current `tar.gz`.
4. Add a release verify step asserting `codesign --verify --deep --strict` and
   `spctl -a -t exec -vv` pass on the shipped artifact.

## Notes

- Because MiniCon is a single-file CLI Mach-O, not an `.app` bundle,
  notarization is simpler than an app (no framework deep-signing) but the
  stapling target must be a container, not the loose binary.
- A CLI Mach-O usually needs no entitlements file; add a minimal
  `.entitlements` only if the hardened runtime blocks something at runtime.
- The iOS `ship-ios-app` skill in the company hub shares the account/Team
  scaffolding and the owner-gate discipline, but has no Developer ID or
  notarization procedure to reuse directly.
