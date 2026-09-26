---
name: run-reputation-and-release
description: Take a sealed, dual-signed release Candidate through the Microsoft Defender reputation court (prefer the CI-native `defender-ci-scan.yml`, no Mac/UTM required), the reputation qualification gate, and the release publish for a minicon-style gated release chain (candidate → reputation → release). Use after signing succeeds when you must run the Defender scan, produce the reputation qualification, and dispatch the release workflows. Do not relax or skip the Defender scan, edit assertions to force green, or publish without the owner's release decision.
---

# Run reputation and release

The last two gates after a Candidate is sealed and dual-signed: a real
Microsoft Defender scan of the policy-selected assets (the reputation court),
then the `reputation.yml` and `release.yml` workflow dispatches. Signing itself
lives in `../sign-windows-artifacts/` and `../sign-macos-artifacts/`; this skill
is everything after those receipts exist.

The Candidate is immutable — publishing PROMOTES it with no rebuild and no
re-sign. Never rebuild to "fix" a gate.

## Start with authority and state

1. Read the target repo's `AGENTS.md`, its `release-policy.json`
   (`reputation.mode`, `reputation.assets`, `signing.*`), and the
   `candidate.yml` / `reputation.yml` / `release.yml` workflows. They are
   authoritative for input names and gate order. Confirm each `-f` input name
   against the workflow file — do not trust memory.
2. Identify the sealed Candidate run id and the source SHA it is bound to. Every
   downstream gate binds to that exact SHA.
3. The publish dispatch is outward-facing and irreversible — confirm the
   owner's release decision before firing `confirmation=publish-v<version>`.

## Gate 0: the upstream chain that produces the Candidate

The signing skills cover *how* each signature is made; this is the **order and
the run-id wiring** between them, which no skill owned until it cost a failed
dispatch (minicon v0.1.18, 2026-09-18).

```
minicon-com.yml            (unsigned one-pack / six-cell)      -> <com>
  ├─ company-signing.yml   -f minicon_com_run_id=<com>          -> <win>   ┐ run these
  └─ macos-signing.yml     -f minicon_com_run_id=<com>          -> <mac>   ┘ in parallel
candidate.yml              -f upstream_run_id=???  -f macos_signing_run_id=<mac>  -> <cand>
defender-ci-scan.yml       -f candidate_run_id=<cand>                            -> <defci>  (Gate 1, CI-native)
reputation.yml             -f candidate_run_id=<cand> -f qualification_base64=…  -> <rep>    (Gate 3)
release.yml                 dry-run, then publish                                            (Gate 4)
```

**`upstream_run_id` is NOT always the `minicon-com` run.** `candidate.yml`
resolves it against `release-policy.json`:

| `signing.mode` | `upstream_run_id` must be | artifact it downloads |
| --- | --- | --- |
| `required` | the **`company-signing`** run `<win>` | `trusted-signed-minicon` |
| `off` | the **`minicon-com`** run `<com>` | `minicon-com` |

Passing `<com>` while `signing.mode=required` fails in `preflight` at
`[[ "$(jq -r .path <<<"$run")" == "$expected_workflow" ]]` — the message names
the workflow path mismatch, not the concept, so it reads as a mystery unless you
know this table. All dispatches carry the same `-f source_sha=<sha>`; every gate
asserts it, so `main` must not move while the chain runs.

## The four gates

Placeholders: `<sha>` = Candidate source SHA, `<cand>` = candidate run id,
`<v>` = version.

### 1. Defender reputation court — CI-NATIVE PATH NEEDS NO MAC, NO UTM

Prefer the product's `defender-ci-scan.yml` (minicon has one; port the pattern
to a product without it) if it exists: a `windows-2025` GitHub-hosted runner
runs real `MpCmdRun.exe` against the exact Candidate-manifest bytes and uploads
`reputation-qualification.json`/`.b64` as a run artifact. This needs only
`gh workflow run` and `gh run download`; there is no Mac, no UTM guest, and no
local Defender install involved, so a cloud/Linux-only agent can run the whole
signing→release chain end to end. Defender scans files **statically, without
executing them**, so a GitHub-hosted Windows runner (any ISA) is an equivalent
court to a local one — this is not a weaker substitute.

```sh
gh workflow run defender-ci-scan.yml --ref main \
  -f candidate_run_id=<cand> -f source_sha=<sha>
# poll to success (run id <defci>), then:
gh run download <defci> --dir <dir>
Q="$(cat <dir>/defender-ci-qualification-<defci>/reputation-qualification.b64)"
```
`Q` is ready for step 3 below; skip step 2 entirely — the CI job already ran
`reputation_court.py qualify`/`verify` for you.

Fall back to the local UTM path only for a product/runner combination that has
no GitHub-hosted native runner for the target ISA, or while diagnosing a
`defender-ci-scan.yml` failure. On an Apple-Silicon Mac the emulated x86
Windows court (QEMU-TCG) is slow and its guest agent times out under the
court's large-file transfer + scan load, so use the native ARM Windows court:

```sh
cd <product-repo>
export MINICON_DEFENDER_COURT=win-aarch64-desktop   # native Virtualization.framework, fast + stable
bash release/utm-win-defender-court.sh \
  "$PWD/target-six/candidate-<v>" "$PWD/target-six/reputation-qualification-<v>.json"
```

A clean run prints `CLEAN UTM Defender receipt assets=<n>` and writes a receipt
whose `verdict` is `clean`, with each asset's `sha256 == post_scan_sha256`
(proves the bytes were unchanged by the scan). See
[references/troubleshooting.md](references/troubleshooting.md) for the court
selection, VM identity, and UTM-bridge recovery details.

### 2. Qualify the receipt (this is what reputation.yml consumes)

Skip this step entirely when step 1 used `defender-ci-scan.yml` — its own
job already produced and verified the qualification file. Only the local UTM
path needs this manual conversion.

`reputation.yml` verifies a QUALIFICATION (kind `minicon-reputation-qualification`),
NOT the raw court receipt (kind `minicon-defender-court`). Convert locally first,
then base64:

```sh
python3 release/reputation_court.py qualify \
  --manifest target-six/candidate-<v>/candidate-manifest.json \
  --defender target-six/reputation-qualification-<v>.json \
  --output  target-six/qualification-<v>.json
# pre-check (saves a CI round-trip):
python3 release/reputation_court.py verify \
  --manifest target-six/candidate-<v>/candidate-manifest.json \
  --qualification target-six/qualification-<v>.json
```

### 3. reputation.yml — dispatch on the CANDIDATE SHA, not main

`reputation.yml` and `release.yml` assert `GITHUB_SHA == source_sha`. If `main`
has moved past the Candidate SHA (any commit since — even docs), `--ref main`
fails at "Resolve exact Candidate". GitHub dispatch `ref` only accepts a branch
or tag name, so pin a throwaway branch at the SHA:

```sh
git branch candidate-src-<v> <sha> && git push origin candidate-src-<v>
Q="$(base64 -i target-six/qualification-<v>.json | tr -d '\n')"
gh workflow run reputation.yml --ref candidate-src-<v> \
  -f candidate_run_id=<cand> -f source_sha=<sha> -f qualification_base64="$Q"
# poll to success; note the reputation run id as <rep>
```

### 4. release.yml — dry-run, then publish

```sh
gh workflow run release.yml --ref candidate-src-<v> \
  -f candidate_run_id=<cand> -f source_sha=<sha> -f reputation_run_id=<rep> \
  -f version=<v> -f confirmation=dry-run-publish-v<v> -f dry_run=true
# verify green, get the owner's release decision, then:
gh workflow run release.yml --ref candidate-src-<v> \
  -f candidate_run_id=<cand> -f source_sha=<sha> -f reputation_run_id=<rep> \
  -f version=<v> -f confirmation=publish-v<v> -f dry_run=false
```

After publish: delete the throwaway branch
(`git push origin --delete candidate-src-<v>`), confirm the public Release, and
record the run ids in the product's release-history archive.

## Hard rules

- Do NOT relax or skip the Defender scan, edit assertions, or flip a policy gate
  to force green. Flipping a court's `automation_state` to `ready` is only
  legitimate when the guest agent is *proven* live and you are using the native
  court for a real scan — prefer just using the native court, which needs no flip.
- Do NOT rebuild or re-sign to promote — publish promotes the sealed Candidate.
- Keep the release environment isolated: never run exploratory VMs in the UTM
  instance the release court uses (see troubleshooting).
- A `git rev-parse "$ref:$path"` (or any `<ref>:<path>` revision spec) run as a
  bash step on a `windows-2025` runner is silently mangled by Git-for-Windows'
  MSYS path auto-conversion (`origin/main:.cargo/config.toml` becomes
  `origin\main;.cargo\config.toml`); a shared helper script called from both a
  `ubuntu-latest` gate and a Windows one (minicon's
  `scripts/product-source-hash.sh`, first hit by `defender-ci-scan.yml`,
  2026-09-26) must `export MSYS_NO_PATHCONV=1`, a no-op elsewhere.
- A hand-written "stamped ceiling … must not be auto-raised" comment binds that
  specific number, not future owner-directed budget changes — raising it is a
  normal owner release decision when growth is a real feature cost, done as an
  explicit, dated, recorded change (minicon's `CANDIDATE_CEILING_BYTES`
  9→11 MiB for 0.2.0's `mux`+`harness`, 2026-09-26), never a silent or
  automatic bump.
