---
name: sign-macos-artifacts
description: Sign and notarize PARTNERNET SOFTWARE PTY LTD macOS deliverables (universal Mach-O binaries, .pkg, .dmg) with the company Developer ID Application certificate, submit them to Apple notarization, and verify Gatekeeper acceptance from GitHub Actions. Use for enabling macOS signing on a product, wiring a macOS release workflow, and diagnosing codesign, notarization, stapling or Gatekeeper problems. Do not place private keys, .p12/.p8 files, passwords or App Store Connect identifiers in a repository, receipt or log.
---

# Sign macOS Artifacts

One company Apple team, one Developer ID Application certificate whose private
key is exportable and therefore secret, one notarization identity per pipeline,
exact-byte receipts. The certificate says `PARTNERNET SOFTWARE PTY LTD`; nothing
else may be called a company macOS signature.

**Status: SHIPPED in MiniCon v0.1.9 (2026-09-13)** — the first dual-signed
release (Windows Authenticode + macOS Developer ID). The universal Mach-O is
Developer ID signed (team `L2N7M5M544`), hardened-runtime, Apple-notarized; the
`tar.gz` carries the signed binary and a signed+notarized+stapled `.dmg` ships
alongside. CI courts `macos-signing.yml` (sign/notarize/staple) and the
`candidate_bundle` + candidate/release integration are live and gated behind
`signing.macos.mode`. This page records how it was built.

## Start with authority and state

1. Read the target repository's `AGENTS.md`, its release policy file and its
   release workflow; treat them as authoritative for gates and receipts.
2. Read `../../docs/current-state.md` for whether the Apple team, Developer ID
   certificate and notarization credential currently exist for automation.
3. Never write the private key, `.p12` password, `.p8` contents, Key ID, Issuer
   ID or keychain password into a repository, log, receipt, screenshot or
   prompt. Real values live in the GitHub `release-signing` Environment and the
   company vault. The certificate's public identity — publisher name and team
   ID — is public provenance, like the Windows publisher name.

## Route the work

- Company certificate or notarization credential does not exist, or must be
  renewed: follow
  [apple-signing-setup.md](references/apple-signing-setup.md).
- Windows deliverables are a different provider with a different key model:
  route them to `../sign-windows-artifacts/` instead. Do not share secrets or
  policy gates between the two.
- A product ships both Windows and macOS: keep two independent switches. Do not
  turn both on in the same release.

## What differs from the Windows line

The Windows key is non-exportable inside Azure, so there is nothing to leak and
nothing to protect on a developer machine. The Apple key is the opposite: it is
an exportable `.p12` that can sign anything attributed to the company until it
is revoked. Consequences:

- the `.p12` and its password are the company's most sensitive signing secret;
- rotation and revocation are real owner actions with a documented trigger, not
  a service-managed detail;
- a developer machine may hold the certificate for rehearsal only, and the
  rehearsal must be removable.

## Local toolchain

Xcode provides everything: `codesign`, `notarytool`, `stapler`, `lipo`,
`security`, `pkgbuild`, `productbuild`, `hdiutil`. Verified present on the dev
Mac (Xcode 26.6). Nothing needs to be downloaded to sign macOS artifacts; the
only thing that must be obtained is the certificate, and only the account
holder can obtain it.

## Handoff evidence

Return only:

- repository-relative implementation and policy paths;
- exact source SHA plus unsigned/signing run ids and attempts;
- public artifact hashes, sizes, `TeamIdentifier`, certificate common name,
  notarization submission id and status;
- signing policy mode and whether the receipt is release-eligible;
- remaining human authority gate.

Never return private key material, `.p12` passwords, or App Store Connect key
identifiers.
