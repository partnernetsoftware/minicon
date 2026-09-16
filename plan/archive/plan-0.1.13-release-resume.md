# v0.1.13 release — resume runbook (paused at the reputation gate)

**Status: paused, not failed.** Everything through the sealed Candidate is done
and dual-signed. Only the local Defender/reputation gate + publish remain. The
Candidate is immutable and promotes to a published release with **no rebuild and
no re-sign**.

## What is done (all green, bound to one source SHA)

- Source SHA: **`6ee4e6ed4bdbd5bb2a9e2824cba660342d9437ec`** (main HEAD; version
  0.1.13 across `Cargo.toml`, `release-policy.json`, `crates/minicon-core`).
- agenterm fix committed + pushed: rev **`945ab8ec206f11bbb81dd6200cb994e9ac19cfe1`**
  (console-agent width-derived CJK continuation); minicon pin bumped to it.
- Local six-cell qualify: PASS 27 / FAIL 0 (BLOCKED = local runtime runners, CI-covered).

| Stage | Run ID | Result |
| --- | --- | --- |
| one-pack `minicon-com.yml` | `35066414457` | success (headSha 6ee4e6e) |
| `company-signing.yml` (Windows, qualification_only=false) | `35067068677` | success |
| `macos-signing.yml` (qualification_only=false) | `35067072176` | success |
| `candidate.yml` (upstream=company-signing, macos side) | `35067318465` | success — **sealed Candidate** |
| Defender court (local) | — | **BLOCKED (see below)** |
| `reputation.yml` | — | pending |
| `release.yml` dry-run then publish | — | pending |

Sealed candidate already downloaded to `target-six/candidate-0.1.13/`
(candidate-manifest.json + payload + release-policy.json).

## Why it paused: the Defender court (reputation.mode=defender)

The `win-x86_64-desktop` court is a QEMU-emulated x86 Windows VM. Two things bit:

1. **utm-court registry declares it not-ready.** `partnernetsoftware/utm-court`
   `courts/registry.json` has `win-x86_64-desktop` at `"automation_state":
   "planned"` with a hardcoded `blocked_reason` ("Guest Agent did not become
   ready within the 600-second emulated-x86 budget"). `bin/utm-court:468` refuses
   any lease when `automation_state != ready`. This is a repo-declared state, not
   session damage. The **guest agent actually works** though —
   `utmctl ip-address <uuid>` returns real IPs. To run the real scan you can
   temporarily flip that court to `"ready"` locally (guest agent proven live) and
   revert after.
2. **UTM automation bridge is unhealthy in a thrashed macOS session.** Heavy
   manual UTM use this session (a Win7 repro VM + repeated `open`/quit UTM +
   manual start/stop) left UTM's Apple-Event automation returning `-10004`
   (errAEEventNotHandled) and `-2700` on `start` and on `utmctl file push`, so
   the scan's guest file transfers produce empty/missing files
   (`C:\minicon-six\defender\.csize` not found) and no receipt. This is the
   env/code-coupling fragility to design out (see the memory note).

**Fix = a clean UTM/macOS state:** reboot (or fully quit UTM with no other VMs
running, verify System Settings → Privacy & Security → Automation lets the
terminal control UTM), then run the court once. Do NOT run exploratory VMs in the
same UTM instance the release court uses.

## Resume steps (clean state; promotes the ALREADY-sealed candidate)

There is a same-named duplicate UTM registration; the **real court VM UUID is
`6D345DE6-4503-4B6A-B56B-8EA8612A1461`** (23 GB image). Export it to bypass the
name ambiguity:

```sh
cd ~/repos/minicon
export UTM_COURT_VM=6D345DE6-4503-4B6A-B56B-8EA8612A1461

# (optional) prove the guest agent, then temporarily enable the court:
/Applications/UTM.app/Contents/MacOS/utmctl start 6D345DE6-4503-4B6A-B56B-8EA8612A1461
/Applications/UTM.app/Contents/MacOS/utmctl ip-address 6D345DE6-4503-4B6A-B56B-8EA8612A1461   # expect IPs
#   in ~/repos/utm-court/courts/registry.json flip win-x86_64-desktop automation_state -> "ready" (revert after)

# 1) Defender court on the sealed candidate -> reputation-qualification.json
bash research/minicon-com-loader/utm-win-defender-court.sh \
  "$PWD/target-six/candidate-0.1.13" "$PWD/target-six/reputation-qualification-0.1.13.json"

# 2) reputation.yml  (qualification_base64 = base64 of that receipt)
Q="$(base64 -i target-six/reputation-qualification-0.1.13.json)"
gh workflow run reputation.yml --ref main \
  -f candidate_run_id=35067318465 -f source_sha=6ee4e6ed4bdbd5bb2a9e2824cba660342d9437ec \
  -f qualification_base64="$Q"
#   -> note the reputation run id (call it REP)

# 3) release.yml — dry run first, then publish
gh workflow run release.yml --ref main \
  -f candidate_run_id=35067318465 -f source_sha=6ee4e6ed4bdbd5bb2a9e2824cba660342d9437ec \
  -f reputation_run_id=REP -f version=0.1.13 \
  -f confirmation=dry-run-publish-v0.1.13 -f dry_run=true
# verify green, then:
gh workflow run release.yml --ref main \
  -f candidate_run_id=35067318465 -f source_sha=6ee4e6ed4bdbd5bb2a9e2824cba660342d9437ec \
  -f reputation_run_id=REP -f version=0.1.13 \
  -f confirmation=publish-v0.1.13 -f dry_run=false
```

Then revert the utm-court registry flip, and the exe for the real-machine test is
`minicon-0.1.13-windows-x86_64.zip` from the published release.

## Do NOT
- Do not relax/skip the Defender scan or edit assertions to force a green — the
  scan must actually run. Flipping `automation_state` to `ready` is only
  legitimate because the guest agent is *proven* live (`ip-address`).
- Do not rebuild/re-sign — publish promotes the sealed Candidate `35067318465`.
