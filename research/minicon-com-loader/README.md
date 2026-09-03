# minicon.com — cosmocc launcher over six-cell payloads

Exploration only. Does **not** recompile MiniCon with cosmopolitan libc.
GUI/PTY/Win32/Cocoa/Wayland stay in the already-qualified Rust cells.

```
minicon.com          ← cosmocc fat APE (ISAx2 host trampoline)
 └── /zip/cells/     ← ZipOS overlay
      osx-aarch64/minicon
      osx-x86_64/minicon
      lnx-aarch64/minicon
      lnx-x86_64/minicon
      win-aarch64/minicon.exe
      win-x86_64/minicon.exe
```

On start the trampoline picks `{os}-{isa}`, copies the blob out of ZipOS
(or `MINICON_COM_CELLS`), and **exec**s it. In-process mmap (rust-ape style)
is the wrong join for a native GUI terminal.

Payload source: `payload-build/…` then `target-six/builds/current/…`.
Linux cells use `release/` (LTO), Darwin uses `release-fast/`, and Windows uses
the QVM-qualified `windows-release/` profile with remapped builder paths. Those
binaries remain the six-cell gate's job; this directory only packs and
dispatches.

## Two lanes (one pack, six executes)

Never compile MiniCon or cosmocc on a guest/runner matrix.

**Local accelerated** (this Mac packs; host/Lima/UTM only execute)

```bash
./research/minicon-com-loader/local-accelerated.sh
```

`local-qualify.sh` is a compatibility alias. Lima instances this script starts
are stopped on EXIT. Receipt lane is `local-accelerated` (`source_sha`,
`source_dirty`, `source_tree_digest`, com digest, per-cell runner + log digest).
A dirty tree must not be treated as `source_sha == HEAD` alone.

**GitHub CI** (`.github/workflows/minicon-com.yml`)

- `workflow_dispatch` only (no per-push six-runner bill).
- **one** `macos-15` job: payloads + cosmocc pack; upload `minicon.com` + sha256.
  Windows cells need `llvm-rc` (Homebrew `llvm`) for `winresource` icon embed.
- **six** native runners download that artifact and run `--version`, unknown
  flag (must be nonzero), and `--status`. No `cargo`, no cosmocc, no checkout.
- `aggregate` requires exactly six unique cells, the same `minicon_com_sha256`
  / `source_sha` / `source_tree_digest`, `source_dirty=false`, one `run_id`,
  all `job_status=success`.
- cosmocc zip SHA-256 and `bin/cosmocc` digest are pinned; install always
  builds a complete NEXT tree then `rename`s it over DEST (same parent).
  Failure before commit restores PREV; `install-cosmocc-swap-test.sh`
  injects a mid-swap abort and checks the old bin digest. Zig 0.16.0 is
  the same idea (verified tar → PREFIX, mv-fail restores). `--version`
  of cosmocc is GCC 14.1.0 and is not the release pin. Cache key hashes
  workflow + loader/pack/install scripts. GUI+control black-box is a later
  candidate gate, not this smoke.

Windows CI uses `Start-Process -Redirect*` (same as `release.yml`). Local
Windows UTM uses `utm-win-ape-status.sh` (job agent). Both consume the same
packed file.

## Preflight / pack

```bash
# repo root
./research/minicon-com-loader/preflight.sh
./research/minicon-com-loader/pack.sh          # host-cc dispatcher if no cosmocc
# after ~/cosmocc exists:
COSMOCC_DIR=~/cosmocc ./research/minicon-com-loader/pack.sh
./research/minicon-com-loader/dist/minicon.com --status
```

`--status` of one `minicon.com` after rebuilding payloads from this tree (`0.1.2`):

| cell | court | `--status` |
|------|-------|------------|
| osx-aarch64 | host | PASS `minicon 0.1.2` `unix-pty` |
| osx-x86_64 | Rosetta | PASS `minicon 0.1.2` `unix-pty` |
| lnx-aarch64 | Lima vz | PASS `minicon 0.1.2` `unix-pty` |
| lnx-x86_64 | Lima qemu | PASS `minicon 0.1.2` `unix-pty` |
| win-aarch64 | UTM + `.exe` + Start-Process | PASS `conpty` (0.1.0 pack; 0.1.2 not re-run) |
| win-x86_64 | UTM TCG disposable + `job.ready` agent | PASS `minicon 0.1.2` `conpty` build 26200 |

Loader: per-invocation private temp dir (`mkdtemp /tmp/minicon.com.<pid>.XXXXXX`,
mode 0700 + `.minicon-extract` owner marker), atomic `.payload.tmp` → rename,
`posix_spawn` + `waitpid` (EINTR retry), then loader-owned unlink/rmdir.
`reap_stale_extracts` does not treat name+dead-pid as delete authority: `lstat`
the path, reject symlink/non-dir, require `st_uid==geteuid` and mode 0700,
require matching marker, then `open(O_NOFOLLOW|O_DIRECTORY)` / `unlinkat`.
Four cases in `reaper-tests.sh`: fake-symlink, foreign-owner, active-pid,
stale-owned. Never `#ifdef _WIN32` in cosmocc fat.

v0.1.3 is released as five unsigned native archives covering all six cells;
`minicon.com` is intentionally absent. Its completed gate ledger is
`v0.1.3-candidate-plan.md`, and superseded APE experiments are indexed by
`prd/archive/v0.1.3-release-history.md`.

This directory now owns v0.1.4 APE research. G3/G4 remain
`loader-lifecycle-tests.sh`, `install-cosmocc-swap-test.sh`, and
`write-size-report.py` (`CANDIDATE_CEILING_BYTES=9437184`). G2 uses
`ci-control.sh` (`HOME` + unique `--control` + `list-tabs` polling).

The delivery chain is policy-selected, not hard-coded by version:
`minicon-com.yml` builds once; `signing.mode=off` sends its native payloads
directly to `candidate.yml`, while `signing.mode=required` requires a successful
`company-signing.yml` upstream and seals its trusted transformed bytes.
Candidate packages without rebuilding, Reputation qualifies the policy-selected
executables, and human-only Promotion publishes the sealed bytes. v0.1.4 must
set `assets.minicon_com=true` and `signing.mode=required`; missing provider
configuration fails closed.

`company-signing.yml` uses the dedicated GitHub Environment
`release-signing`. The adapter is Azure Artifact Signing Public Trust over
GitHub OIDC: `azure/login` exchanges the job's OIDC token for a federated
identity that holds only the Artifact Signing Certificate Profile Signer role
at one certificate-profile scope, then `Azure/artifact-signing-action` signs
exactly the three catalogued files with SHA-256 and an RFC 3161 timestamp from
`timestamp.acs.microsoft.com`. The Environment holds `AZURE_CLIENT_ID`,
`AZURE_TENANT_ID` and `AZURE_SUBSCRIPTION_ID` as secrets and
`ARTIFACT_SIGNING_ENDPOINT`, `ARTIFACT_SIGNING_ACCOUNT` and
`ARTIFACT_SIGNING_PROFILE` as variables. The signing key is non-exportable and
never enters GitHub. Back up account recovery and role assignments in the
company-controlled secrets vault as specified by
`prd/PRD_02_27_con_delivery.md`; never put a token, real resource name, or
credential in Git, an Actions artifact, logs, Downloads, or a cloud-drive
mount. SignPath Foundation declined the open-source application, so no
SignPath adapter remains.

Before spending a Public Trust signing operation, maintainers can exercise the
APE mechanism locally with an explicitly untrusted, one-day certificate:

```sh
bash research/minicon-com-loader/self-sign-rehearsal.sh path/to/minicon.com
```

The script requires `openssl` and `osslsigncode`, destroys its ephemeral key,
and proves empty→populated Security Directory, signature verification against
the ephemeral CA, tamper rejection, ZIP readability, Darwin execution and the
final 9 MiB ceiling. Its receipt says `mechanism-only-not-g6`, `trusted=false`
and `timestamped=false`; it can never satisfy trusted publisher identity, Defender or
Candidate qualification.

`pack.sh` also compiles `ape-version.rc` into the APE's Windows face.
`write-build-receipt.py` rejects a zero Resource Directory, wrong
ProductName/ProductVersion, or any stale/mixed payload version marker before a
pack can become evidence.

G6 keeps Defender judgment outside the immutable build receipt. GitHub's
standard Windows runner image deliberately disables Defender, so it is not a
scan court. Run `utm-win-defender-court.sh CANDIDATE_DIR OUTPUT_RECEIPT` against
an active-Defender Windows guest to produce `defender-receipt.json`. Then run:

```sh
python3 research/minicon-com-loader/reputation_court.py qualify \
  --manifest candidate-manifest.json --defender defender-receipt.json \
  --output reputation-qualification.json
```

The qualifier fails closed on a hit, missing engine metadata, a different
Candidate run, or a different executable SHA. Raw scan
evidence stays outside Git; only a redacted qualification summary may enter the
delivery record. Base64-encode that summary and dispatch `Reputation
Qualification` with the exact Candidate run and source SHA. Promotion requires
the resulting successful workflow run ID; `.github/workflows/release.yml`
cannot publish without downloading and revalidating that receipt.

If Defender reports a false positive, submit the **exact sealed** `minicon.com`
as a Software developer at Microsoft's official sample portal
<https://www.microsoft.com/wdsi/filesubmission>. Preserve the submission ID
outside Git and rerun the same exact-SHA court after Microsoft updates its
classification. Do not disable Defender, add exclusions, rebuild, or remove
terminal/control behavior to manufacture a green verdict.

## Non-goals

- Linking MiniCon crates against cosmocc
- Replacing `scripts/six-cell-qualify.sh`
- Byte-level compatibility with moltbaby zig-ape/rust-ape `APE!` tables
