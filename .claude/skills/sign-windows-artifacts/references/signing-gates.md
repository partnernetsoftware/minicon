# Signing workflow gates

The cross-platform APE reference is `partnernetsoftware/minicon`
`.github/workflows/company-signing.yml`; the Windows-archive reference is
`partnernetsoftware/agenterm`
`.github/workflows/windows-signing-qualification.yml`. Reuse their shared
identity/receipt shape while matching post-sign courts to the files actually
transformed.

## Workflow shape

Before step 1, run the skill's read-only
`scripts/check-product-signing-readiness.sh <PRODUCT_REPOSITORY_ROOT>`. It
checks local/remote source identity, clean state, version freshness, checked-in
policy/workflow presence, and the canonical inspector copies. `READY` means the
source is eligible to enter its policy-selected flow; it does not prove Azure
configuration, unsigned Candidate success, signature validity, or Promotion
authority.
For a historical mechanism diagnosis, `--qualification` allows a published
version only when the product workflow itself is explicitly non-promotable and
its receipt validator requires `release_eligible=false`. The default remains
the release/Candidate court and rejects version reuse before provider cost.
If the bytes already live in an unexpired successful Candidate at an immutable
version tag, run
`scripts/check-historical-signing-qualification.sh PRODUCT_ROOT OWNER/REPOSITORY SOURCE_SHA CANDIDATE_RUN_ID`.
This is a two-identity court: current `main` is the reviewed workflow
controller, while the immutable tag and upstream Candidate identify every byte
being transformed. The controller may supply newer signing/receipt tooling,
but the signed output stays non-promotable and may never replace the published
version.
Historical status never grandfathers the payload. The normal pre-provider
court still requires current VERSIONINFO, empty Security Directories, exact PE
and companion allowlists, and matching provenance. If old bytes fail there,
record the incompatibility and wait for a new conforming Candidate; do not
rewrite the historical archive or relax the production contract.

1. `workflow_dispatch` only, inputs: exact source SHA, successful unsigned
   build run id, and an explicit qualification/release-eligible choice.
   Preflight binds source/run to current `main`, or for an explicitly historical
   non-promotable court to exactly one immutable version tag. A release-eligible run requires
   checked-in `signing.mode=required`; a qualification run may execute while
   policy is `off` but its receipt must carry `release_eligible=false`. Bind the
   upstream unsigned run id and attempt into qualification state, signing
   receipt, and aggregate receipt; signing must consume those artifacts without
   rebuilding. Reject an already existing `v<version>` tag before any
   release-eligible packaging or signing. Historical qualification may consume
   the original Candidate bytes for that tag only; it must not publish, replace
   assets, or claim release eligibility.
2. Sign job on `windows-2025`, `environment: release-signing`, permissions
   `id-token: write` plus read. Download the exact unsigned artifacts, verify
   their digests against the build receipt, verify every PE has
   `ProductName`/`ProductVersion` and an empty Security Directory.
   Any tracked `.sh` helper copied from that Windows checkout into the signed
   cross-platform bundle must be normalized to UTF-8 without BOM and LF before
   upload. Otherwise Linux reports an unexecutable interpreter and macOS shows
   `/bin/bash^M`, even though signing itself succeeded.
3. Write `signing-catalog.txt` next to the files, upload the immutable
   unsigned input artifact, then `azure/login` (client/tenant/subscription
   from Environment secrets) and `Azure/artifact-signing-action` with
   `files-catalog`, `file-digest: SHA256`, `timestamp-rfc3161:
   http://timestamp.acs.microsoft.com`, `timestamp-digest: SHA256`, and every
   non-OIDC credential excluded. Pin both actions to full commit SHAs.
4. Require exactly the expected output paths; copy into the signed tree.
5. `Get-AuthenticodeSignature` on each file: `Valid`, signer and timestamp
   certificates present, subject matches `O=PARTNERNET SOFTWARE PTY LTD`,
   VERSIONINFO unchanged, bytes changed. Write `signing-receipt.json`
   (`signing_provider: azure-artifact-signing`, release eligibility,
   per-asset before/after SHA-256 and public certificate facts) and run the
   receipt validator. Endpoint/account/profile and OIDC coordinates never enter
   the receipt.
6. Execute-only courts run every transformed after-SHA on each native ISA it
   targets. A cross-platform APE therefore needs all six OS/ISA cells; two
   Windows archives need both Windows ISAs after the unsigned Candidate's
   broader courts have passed. Aggregate only the current run attempt and
   require the exact expected cell set. Each runtime receipt records the
   archive/file hash it actually consumed; the aggregate matches that value to
   the signing receipt's after-SHA. Never claim a six-cell post-sign court
   when four unchanged Unix archives were merely inherited from upstream.

## Failure guards

- Missing Environment values in `required` mode: hard failure, never
  unsigned output.
- A signer subject without the company `O=`, a timestamp URL other than
  Microsoft's, or protected provider coordinates in a public receipt fails the
  receipt validator.
- When a provider path is declined or retired, remove it from every active
  signing-receipt, Candidate and reputation allowlist. Historical fixtures may
  remain in an archive, but must not stay accepted merely to keep old self-tests
  green.
- Candidate preflight and bundle verification both reject
  `release_eligible=false`; a successful qualification run is evidence of the
  provider path, never release authority.
- Treat step-local environment bindings as part of the receipt identity
  contract. A static test must isolate each PowerShell/Bash identity step and
  prove that every consumed source SHA, version and upstream run/attempt value
  is injected in that same step. `actionlint` validates expression shape but
  does not detect a missing runtime `$env:` binding.
- The action runs on x64 Windows runners only; ARM runners are for execution
  courts, not signing.
- Cosmopolitan APE: reserve 16 PE data directories and an empty Security
  Directory at link time (the header cannot grow later); signing then adds
  roughly 12 KiB and keeps ZipOS and the Unix loader intact.
- SmartScreen reputation attaches to the publisher identity and accumulates
  with downloads; the first signed releases may still prompt. Submit the
  signed file to Microsoft Security Intelligence if prompts persist.

## Reading a signed run

Bind claims to: source SHA, unsigned run id/attempt, signing run id/attempt,
per-asset before/after SHA-256, signer thumbprint and validity window,
timestamp-certificate subject/issuer, and the configured RFC 3161 policy.
PowerShell's `System.Management.Automation.Signature` exposes the timestamp
authority certificate, but not the exact signing instant. Claim an exact
timestamp instant only when a separately validated deterministic parser records
it in the receipt. Anything not in the receipt or the run logs is not evidence.
Run `scripts/audit-public-signing-receipt.py` before publishing or consuming a
receipt; it rejects protected Azure/OIDC coordinate keys and malformed public
signature facts.

## Inspect a local file

The canonical reusable inspector contract is
`pns-authenticode-inspector/v3`. Product repositories carry local copies of
both inspectors and the pinned trust-bundle fetcher so
their public build and tests remain self-contained; those copies must stay
byte-identical to this skill rather than accumulating product-specific logic.
From the skill directory, detect drift with:

```bash
scripts/check-product-inspectors.sh <PRODUCT_REPOSITORY_ROOT>
```

Pass expected ProductName/ProductVersion as PowerShell arguments. A contract
change requires updating the canonical scripts, their documented output/exit
semantics, every product copy, and the real Windows qualification evidence as
one reviewed migration.

For a quick human check on Windows, right-click the file, open
**Properties → Digital Signatures**, select the signature, then open
**Details → View Certificate**. The tab being absent means the file has no
embedded Authenticode signature. This view is convenient, but it is not the
machine-readable release gate.

Windows is authoritative because it evaluates the platform trust policy:

```powershell
.\skills\sign-windows-artifacts\scripts\inspect-authenticode.ps1 .\product.exe `
  -ExpectedProductName '<PRODUCT>' -ExpectedProductVersion '<VERSION>'
```

The script emits JSON. Exit `0` means `Valid`, the expected company `O=` is
present, and a timestamp certificate exists; `2` means unsigned, `3` means an
invalid/incomplete signature, `4` means a different publisher, and `5` means
the trusted timestamp is absent. Exit `6` means an explicitly requested
ProductName/ProductVersion does not match. Exit `69` means it was invoked
somewhere the Windows Authenticode cmdlet is unavailable. The JSON includes a
basename (never an expanded host path), SHA-256, byte count, ProductName,
ProductVersion, FileDescription, OriginalFilename, signer facts, and
timestamp-certificate facts. This makes byte and resource drift visible
without leaking the local account path when the report is shared.

On macOS/Linux:

```bash
skills/sign-windows-artifacts/scripts/inspect-authenticode.sh ./product.exe
```

This prints SHA-256, byte count and `osslsigncode`'s signer/timestamp report.
Exit `0` means portable verification passed; `2` means no extractable embedded
signature (or a missing input); `3` means an embedded signature exists but
portable verification failed. The last case includes a missing local CA chain,
but can also mean an integrity failure, so it is diagnostic rather than proof
that Windows accepts or rejects the file. Use the PowerShell court for the final
verdict. `osslsigncode` may display a parsed timestamp instant; treat it as
diagnostic unless the product receipt records and validates that field
explicitly.

If the portable host lacks Microsoft's Artifact Signing trust chain, create a
pinned PEM bundle containing both **Microsoft Identity Verification Root
Certificate Authority 2020** and **Microsoft Public RSA Timestamping CA 2020**.
The helper downloads only from the
[Microsoft PKI repository](https://www.microsoft.com/pkiops/docs/repository.htm),
requires their SHA-256 digests
`5367f20c7ade0e2bca790915056d086b720c33c1fa2a2661acf787e3292e1270`
and `36e731cfa9bfd69dafb643809f6dec500902f7197daeaad86ea0159a2268a2b8`,
converts DER to PEM, and publishes atomically:

```bash
skills/sign-windows-artifacts/scripts/fetch-microsoft-trust-bundle.sh \
  ./microsoft-artifact-signing-trust.pem
skills/sign-windows-artifacts/scripts/inspect-authenticode.sh \
  --ca-file ./microsoft-artifact-signing-trust.pem ./product.exe
```

The option supplies the same verified bundle to the signer and timestamp
certificate courts in
`osslsigncode`; it does not install the root, modify the system trust store, or
replace the authoritative Windows verdict. Never trust a root extracted from
the file being verified.

Before wiring a new product, inspect every target's real release bytes. Require
an empty PE Security Directory and nonempty `ProductName` / `ProductVersion`
for every file in the signing allowlist. Build scripts run on the host, so
`#[cfg(windows)]` in `build.rs` does not compile resources during a Linux/macOS
cross-build; branch on Cargo's target environment at runtime and pin the
resource compiler in the owning build lane. Repeat the resource check for
separate executable/library crates rather than assuming the root package's
resource reaches them.
