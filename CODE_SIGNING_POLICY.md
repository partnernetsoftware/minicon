# Code signing policy

MiniCon is applying to the SignPath Foundation open-source program. The
application is pending. A release is signed only when its downloadable bytes
have a valid Authenticode signature and its release receipt says so; repository
metadata, this policy, a signing request, or a test certificate is not a
signature.

The committed `release-policy.json` decides whether a version requires signing.
MiniCon v0.1.3 deliberately publishes only unsigned native archives; its
checksums, six-cell runtime courts and Defender evidence remain mandatory.
`minicon.com` and trusted signing begin with the v0.1.4 plan. When policy says
`signing.mode=required`, missing provider configuration blocks the release; it
never falls back to unsigned output.

When the application is accepted, Windows release artifacts covered by the
approved SignPath artifact configuration use:

> Free code signing provided by [SignPath.io](https://about.signpath.io/),
> certificate by [SignPath Foundation](https://signpath.org/).

The certificate publisher is SignPath Foundation. It is not a certificate
issued to PARTNERNET SOFTWARE PTY LTD. Company-owned publisher signing remains
a separate future delivery path and will never be represented by a SignPath
Foundation signature.

## Team roles

- Authors, committers, and reviewers: PartnerNet Software organization members
  who have repository write/maintain authority. Public membership and project
  contribution history are visible through the
  [organization](https://github.com/orgs/partnernetsoftware/people) and
  [repository contributors](https://github.com/partnernetsoftware/minicon/graphs/contributors).
- Signing-request approvers: the
  [PartnerNet Software organization owners](https://github.com/orgs/partnernetsoftware/people?query=role%3Aowner).
  SignPath requires a manual approval for every release signing request.
- Public Release authority is separate: no tag or Release is created until a
  human names the exact version and explicitly says `promote` after all
  Candidate courts pass.

All team members participating in repository or SignPath authority must use
multi-factor authentication. External contributions require review by a team
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

The requested SignPath configuration must restrict product name and product
version metadata to MiniCon and the one release version. It may sign only
artifacts produced from this repository by the approved GitHub workflow.

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

Every release publishes SHA-256 sidecars and exact-source receipts. A signing
or antivirus failure blocks that artifact; capabilities are not hidden or
removed to manufacture a green result. Reports concerning signed artifacts can
be filed through the repository's
[issue tracker](https://github.com/partnernetsoftware/minicon/issues). The
maintainers will assist SignPath Foundation with investigation and request
revocation if signing authority or signed bytes are compromised.
