# SignPath Foundation application draft

This is a redacted project-facts packet. Account identity, email, MFA, API
tokens and acceptance of SignPath terms stay in SignPath/GitHub, never here.

## Public project facts

- Project: MiniCon
- Repository: <https://github.com/partnernetsoftware/minicon>
- Website: <https://minicon.agenterm.work/>
- Releases: <https://github.com/partnernetsoftware/minicon/releases>
- Existing release: v0.1.2 (unsigned baseline)
- Licence: MIT OR Apache-2.0, both OSI-approved; no proprietary component or
  commercial dual-licensing branch
- Function: one-file local terminal with native PTY, tabs, dedicated composer,
  IME and an explicitly enabled local GUI-control endpoint
- Privacy/system behavior: no telemetry, updater, account, remote service,
  default listener, installer or system-configuration mutation
- Public policy: [CODE_SIGNING_POLICY.md](../../CODE_SIGNING_POLICY.md)

## Requested signed artifacts

1. `minicon.com`: a Cosmopolitan APE polyglot. Its Windows face is PE32+ with a
   link-time-reserved Authenticode Security Directory and a standard
   `VERSIONINFO` resource (`ProductName=MiniCon`, `ProductVersion=0.1.3`). It
   also contains Mach-O and a ZIP overlay of six MiniCon payloads built by the
   same workflow.
2. `cells/win-x86_64/minicon.exe`: native Windows x86_64 MiniCon.
3. `cells/win-aarch64/minicon.exe`: native Windows arm64 MiniCon.

The application must explicitly ask SignPath to confirm that its artifact
configuration can sign the `.com` PE/APE while preserving the ZIP overlay. Do
not silently fall back to signing only the two `.exe` files. Cosmopolitan is an
open-source toolchain/runtime dependency; the six product payloads, loader,
pack scripts and release orchestration are built from this public repository.

## Verifiable build/sign flow

```text
clean exact source SHA
└── one GitHub macOS build/pack
    ├── six native product payloads
    └── unsigned signable minicon.com
        └── SignPath request from immutable GitHub artifact ID
            ├── signed minicon.com
            ├── signed Windows x86_64 minicon.exe
            ├── signed Windows arm64 minicon.exe
            └── before→after SHA receipt
                └── six native GUI/control execution + Defender
                    └── sealed Candidate
                        └── separate human exact-version Promotion
```

No test runner compiles. Promotion never rebuilds or re-signs. Each SignPath
request requires manual SignPath approval, and each public Release separately
requires the product's exact-version `promote` authority.

## Application questions to resolve

- Can SignPath's Authenticode worker sign this PE32+ `.com` APE and preserve its
  valid ZIP overlay? A local `osslsigncode` rehearsal proves the file layout,
  but it is not evidence about SignPath's parser.
- Which artifact-configuration rule expresses these exact three paths and
  rejects every extra file?
- Confirm metadata restrictions that require ProductName `MiniCon` and version
  `0.1.3` on all three PEs; the workflow independently rejects any mismatch.
- Does the embedded Cosmopolitan open-source APE runtime satisfy the Foundation
  policy for this self-built public-source artifact?
- Which GitHub App/trusted-build configuration binds the request to the
  repository, workflow and immutable artifact ID?
