---
name: sign-windows-artifacts
description: Sign PARTNERNET SOFTWARE PTY LTD Windows deliverables (PE .exe/.dll, MSI, and Cosmopolitan APE .com files) with the company Azure Artifact Signing Public Trust certificate from GitHub Actions over OIDC, or rehearse a signature locally. Use for enabling signing on a new product, wiring a release workflow, diagnosing identity-validation, profile, RBAC, login, or SmartScreen problems, and reading signing receipts. Do not export keys, widen roles, create a second publisher identity, or flip a product's signing policy without the owner's release decision.
---

# Sign Windows Artifacts

One company publisher identity, one non-exportable key inside Azure Artifact
Signing, one least-privilege GitHub identity per repository, exact-byte
receipts. The certificate says `PARTNERNET SOFTWARE PTY LTD`; nothing else may
be called a company signature.

## Start with authority and state

1. Read the target repository's `AGENTS.md`, its release policy file and its
   signing workflow; treat them as authoritative for gates and receipts.
2. Read `../../docs/current-state.md` for whether the company signing account,
   identity validation and Public Trust profile currently exist.
3. Never write the tenant, subscription, validation, app, mailbox or resource
   names into a repository, log, receipt, screenshot or prompt. Placeholders
   only; real values live in the GitHub `release-signing` Environment and the
   company vault.

## Route the work

- A new repository/product needs end-to-end signing: start with
  [new-product-checklist.md](references/new-product-checklist.md), then follow
  its links into provider setup and workflow gates.
- Company account, identity validation or profile does not exist, or a
  product needs its own GitHub identity: follow
  [azure-artifact-signing-setup.md](references/azure-artifact-signing-setup.md).
- A workflow needs to sign, or a signed run must be judged: follow
  [signing-gates.md](references/signing-gates.md).
- The deliverable also ships for macOS: that is a different provider with a
  different key model. Route it to `../sign-macos-artifacts/`; do not share
  secrets or policy gates between the two lines.
- For certificate lifetime, monthly quota/cost review, signing-transaction
  audit, routine access review, or a suspected signing-identity incident:
  follow [operations.md](references/operations.md).
- To inspect a local result, run `scripts/inspect-authenticode.ps1` on Windows
  (authoritative OS trust result plus VERSIONINFO) or `scripts/inspect-authenticode.sh` on
  macOS/Linux (portable certificate/timestamp inspection). Read
  [signing-gates.md](references/signing-gates.md#inspect-a-local-file) for exit
  codes and interpretation. If the portable host lacks Microsoft's trust
  chain, create the pinned two-certificate PEM with
  `scripts/fetch-microsoft-trust-bundle.sh OUTPUT.pem`; never trust certificates
  extracted from the artifact under inspection.
- When onboarding or reviewing a product, keep both product-local inspector
  copies and the trust-bundle fetcher byte-identical to this skill's canonical
  `pns-authenticode-inspector/v3` tools. Run
  `scripts/check-product-inspectors.sh <PRODUCT_REPOSITORY_ROOT>` to detect
  drift before trusting a product-local report.
- Before dispatching Candidate or release-eligible signing, run
  `scripts/check-product-signing-readiness.sh <PRODUCT_REPOSITORY_ROOT>`.
  It is read-only and rejects a dirty tree, stale/non-main HEAD, reused version,
  policy/version drift, missing workflow, or inspector drift before provider
  time and quota are spent. It does not replace protected Environment or live
  provider checks.
  `--qualification` permits an already published version only for a product
  workflow that explicitly emits `release_eligible=false`; it never authorizes
  Candidate or Promotion. When a product already has an unexpired successful
  Candidate whose source is an immutable `vX.Y.Z` tag, use
  `scripts/check-historical-signing-qualification.sh PRODUCT_ROOT OWNER/REPOSITORY SOURCE_SHA CANDIDATE_RUN_ID`.
  That stricter court binds the tag, source policy, Candidate run, required
  artifacts, current controller workflow, and canonical inspectors. It permits
  only a non-promotable signing qualification; the controller may use current
  signing machinery, but every byte being signed still comes from the named
  historical Candidate.
  This proves identity and retention readiness, not PE content eligibility;
  the product workflow must still reject any legacy byte that lacks the current
  VERSIONINFO, empty-Security-Directory, allowlist, or receipt prerequisites
  before provider login.
- To verify GitHub wiring without exposing configuration values, run
  `scripts/check-github-signing-environment.sh OWNER/REPOSITORY`. It requires
  the three Azure Environment secret names and three Azure variable names and
  prints names only. Other providers may keep their own names in the same
  protected Environment; their presence must not make the Azure subset fail.
  The check deliberately does not prove values, OIDC subject, Azure role scope,
  or provider access.
- To audit the shared Azure control plane without displaying identifiers or
  consuming a signing transaction, run
  `scripts/check-azure-signing-state.sh`. It requires the one company
  East-US/Basic account and Active profile, profile-scoped signer roles held
  only by service principals, and exactly one immutable GitHub Environment
  subject per signer identity.
- Before publishing or consuming a signing receipt, run
  `scripts/audit-public-signing-receipt.py`; it checks public signature facts
  and rejects protected provider/OIDC coordinate keys.
- After changing this skill's scripts, run `scripts/self-test.sh`. It uses only
  synthetic local repositories and a mock GitHub client: no Azure login,
  signing transaction, repository mutation or provider quota is involved.
- A local proof that a file format (for example a Cosmopolitan APE) survives
  Authenticode with the real certificate: the rehearsal branch in
  [azure-artifact-signing-setup.md](references/azure-artifact-signing-setup.md).
  Rehearsal output is mechanism evidence only and is never a release.

## Fixed decisions

- Provider: Azure Artifact Signing (formerly Trusted Signing), Public Trust,
  East US endpoint, Basic tier. SignPath Foundation declined the open-source
  application in September 2026; do not reapply for it as a substitute.
- Authentication from CI: GitHub OIDC through `azure/login`, federated to one
  Entra app registration per repository. New GitHub repositories use an
  immutable subject containing both owner and repository numeric IDs; derive
  it from GitHub's APIs exactly as described in the setup reference. Never
  shorten it to the legacy name-only form. No client secrets.
- RBAC: the federated identity holds only `Artifact Signing Certificate
  Profile Signer`, scoped to one certificate profile. Humans hold that role
  only for a bounded rehearsal and lose it afterwards.
- Signing inputs: SHA-256 file digest, RFC 3161 timestamp from
  `http://timestamp.acs.microsoft.com` with SHA-256, an explicit file catalog,
  and `ProductName`/`ProductVersion` resources on every PE.
- Receipt: before SHA-256, after SHA-256, byte count, signer subject/issuer/
  thumbprint/validity, timestamp subject/issuer, provider class, source commit
  and run identity. Azure endpoint/account/profile and OIDC identifiers are
  protected configuration and are forbidden in public receipts. Validators
  must fail closed on a foreign publisher or timestamp policy.
- Policy: signing mode is checked-in source (`off` or `required`). Missing
  credentials in `required` mode fail the run; they never downgrade to
  unsigned. Enabling `required` is an owner release decision.
- Qualification: prove a new product's real OIDC/provider path with an explicit
  non-promotable run while policy remains `off`. Its receipt must say it is not
  release-eligible, and Candidate validation must reject it. Never change
  release policy merely to test credentials.

## Human gates

Owner at the exact point for: Azure password/MFA, accepting Artifact Signing
terms, submitting identity validation, clicking the vetting email link (it is a
legal attestation), deleting identities/profiles, revoking a certificate,
and switching a product to `signing.mode=required`. Under an explicit product-signing setup request,
creating a repository-specific OIDC identity and its profile-scoped role is a
normal implementation step; continue automatically and verify the resulting
state.
