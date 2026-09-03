# Code signing policy

MiniCon's accepted Windows signing provider is a publicly trusted Authenticode
certificate issued to PARTNERNET SOFTWARE PTY LTD through Azure Artifact
Signing (Public Trust). No public MiniCon release has used it yet. The SignPath Foundation open-source application was
declined, so no SignPath signature will ever appear on a MiniCon release. A
release is signed only when its downloadable bytes have a valid Authenticode
signature and its release receipt says so; repository metadata, this policy,
an identity validation, a certificate profile, or a test certificate is not a
signature.

The committed `release-policy.json` decides whether a version requires signing.
MiniCon v0.1.4 and v0.1.5 deliberately published unsigned artifacts; their
checksums, six-cell runtime courts and Defender evidence remain mandatory.
Company signing of `minicon.com` and the native Windows executables is a later
explicit policy switch for a new version. When policy says
`signing.mode=required`, missing provider configuration blocks the release; it
never falls back to unsigned output.

Signed Windows release artifacts (`minicon.com` and both native
`minicon.exe`) carry the publisher identity `PARTNERNET SOFTWARE PTY LTD`
(Sydney, New South Wales, AU) on a short-lived certificate chained to the
Microsoft ID Verified Code Signing PCA, with an RFC 3161 SHA-256 timestamp
from `timestamp.acs.microsoft.com` so signatures stay valid after the
certificate expires. The private key is non-exportable and lives only inside
the Azure Artifact Signing service; no PFX, token or key file exists anywhere.
Signing is performed by a GitHub Actions job: `azure/login` exchanges GitHub's
OpenID Connect token for a short-lived Azure CLI session, and the signing
action consumes that session as a federated identity whose only permission is
the Artifact Signing Certificate Profile Signer role on one certificate
profile.

## Team roles

- Authors, committers, and reviewers: PartnerNet Software organization members
  who have repository write/maintain authority. Public membership and project
  contribution history are visible through the
  [organization](https://github.com/orgs/partnernetsoftware/people) and
  [repository contributors](https://github.com/partnernetsoftware/minicon/graphs/contributors).
- Signing authority: the
  [PartnerNet Software organization owners](https://github.com/orgs/partnernetsoftware/people?query=role%3Aowner).
  Only the `release-signing` GitHub Environment can obtain the OIDC token that
  the signing identity accepts; only a manually dispatched Trusted Signing
  Court run against current `main` uses that Environment.
- Public Release authority is separate: no tag or Release is created until a
  human names the exact version and explicitly says `promote` after all
  Candidate courts pass.

All team members participating in repository or Azure signing authority must
use multi-factor authentication. External contributions require review by a team
member before merge.

## Build and signing boundary

- Source: <https://github.com/partnernetsoftware/minicon>
- Licence: MIT OR Apache-2.0; there is no proprietary component or commercial
  dual-licensing branch.
- The release workflow binds one clean source SHA to one build/pack run.
- Signing transforms only the exact unsigned artifact produced by that run and
  records the before and after SHA-256 values.
- Six native Windows/Linux/macOS x86_64/aarch64 courts execute the exact
  policy-selected bytes. In signing-required mode they execute the signed
  `minicon.com` after-SHA and verify signed native Windows executables. In
  signing-off mode the APE is absent and both unsigned Windows executables are
  bound to the Candidate and Defender receipt without claiming a publisher.
- Promotion downloads and publishes the sealed Candidate bytes without
  rebuilding or re-signing them.
- A test/self-signed certificate is mechanism evidence only and can never
  satisfy release qualification.
- A Trusted Signing Court run with `qualification_only=true` exercises the real
  company certificate and all six runtime cells without changing release
  policy. Its receipt records `release_eligible=false`; Candidate validation
  rejects it even if a later caller supplies that run id. This is the supported
  way to prove OIDC/provider wiring before switching a new version to
  `signing.mode=required`.
- Live qualification `33737286265` proved that path for all three signing
  inputs and all six execute-only cells. It did not sign a public Release and
  did not change the checked-in policy from `off`.

The signing job fails closed unless every input PE reports
`ProductName=MiniCon` and the one release version, and it signs only the three
artifacts produced from this repository by the approved GitHub workflow run it
was given.

Azure endpoint, signing-account, certificate-profile, tenant, subscription and
client identifiers are workflow configuration, not public provenance. They
must not be copied into receipts, artifacts, logs or screenshots. Public
evidence names the provider, publisher certificate facts, timestamp policy,
run identity and exact before/after hashes instead.

## Privacy and system changes

This program will not transfer any information to other networked systems
unless specifically requested by the user or the person installing or
operating it.

MiniCon has no telemetry, updater, account, remote control service, default
listener, or background daemon. Its optional control endpoint is a local Unix
socket or Windows named pipe created only when the user passes `--control`, and
it ends with that GUI process. Programs intentionally launched inside the
terminal can use the network under their own behavior and authority; they are
not MiniCon telemetry.

MiniCon has no installer and changes no system configuration. Removing the
downloaded executable removes MiniCon. It stores only user-local configuration
and failure diagnostics described by `minicon --status`.

## Verification and incident response

Maintainers and users can inspect a downloaded file without reading workflow
logs:

```powershell
.\scripts\inspect-authenticode.ps1 .\minicon.com `
  -ExpectedProductName MiniCon -ExpectedProductVersion '<VERSION>'
```

This Windows court emits structured basename/SHA-256/size,
signer/timestamp-certificate and VERSIONINFO JSON without the expanded local
path. Exit
`0` means a valid signature from the expected company organization with a
timestamp and any requested product identity; `2` means unsigned, `3`
invalid/incomplete, `4` a foreign publisher, `5` no timestamp, `6` a requested
product/version mismatch, and `69` that the Windows Authenticode cmdlet is unavailable. The
portable `scripts/inspect-authenticode.sh` prints hash, size and
`osslsigncode` evidence on macOS/Linux. It returns `2` when no embedded
signature can be extracted and `3` when a signature exists but portable trust
verification fails (including an unavailable local CA chain); neither replaces
the Windows trust verdict.

Every release publishes SHA-256 sidecars and exact-source receipts. A signing
or antivirus failure blocks that artifact; capabilities are not hidden or
removed to manufacture a green result. Reports concerning signed artifacts can
be filed through the repository's
[issue tracker](https://github.com/partnernetsoftware/minicon/issues). The
maintainers will revoke the certificate profile through Azure Artifact Signing
and disable the federated signing identity if signing authority or signed
bytes are compromised, and will record the incident in the release history.
