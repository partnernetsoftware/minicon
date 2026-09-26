# Reputation court & release — troubleshooting

Field-tested on an Apple-Silicon Mac driving UTM 4.7 for a minicon-style release.

## Court selection: native ARM Windows, not emulated x86

`release/utm-win-defender-scan.sh` supports
`MINICON_DEFENDER_COURT=<court-id>` because Defender scans files statically
without executing them, so a native Windows court of any architecture is a valid
scanner for x86 and arm assets alike.

- `win-x86_64-desktop` — QEMU-TCG **pure emulation** on Apple Silicon. The
  utm-court registry marks it `automation_state: "planned"` with
  `blocked_reason` "Guest Agent did not become ready within the 600-second
  emulated-x86 budget". Under the court's large-file transfer + scan load its
  guest agent times out (`Timed out waiting for RPC`,
  `Invalid parameter type for 'command.return'`) or the UTM bridge returns
  `-2700`. Do not fight it.
- `win-aarch64-desktop` — **native Virtualization.framework**. Already `ready`
  in the registry. VM name `minicon-win-arm-64` (unique, so no `UTM_COURT_VM`
  override and no registry flip needed). Guest agent was ready in ~24 s (vs
  ~114 s for x86) and the court passed on the first try with a clean 3-asset
  receipt.

**Default to `win-aarch64-desktop`.**

## utm-court gotchas (only relevant if you must use a non-ready court)

- `bin/utm-court` refuses a lease when `automation_state != ready`, emitting the
  registry's hardcoded `blocked_reason` WITHOUT probing the live agent. Prove the
  agent with `utmctl ip-address <uuid>` (real IPs = live). Only then may you
  temporarily flip that court to `"ready"` in `courts/registry.json` (back it up,
  revert after). This is legitimate only for running a REAL scan on a proven-live
  agent — never to skip the scan.
- Duplicate/ambiguous VM names: `resolve()` honours `UTM_COURT_VM=<uuid>` to pick
  the exact VM.

## UTM automation-bridge recovery (no host reboot)

Heavy manual UTM use in the same session — a repro VM, repeated open/quit,
manual start/stop — poisons UTM's Apple-Event automation. Symptoms: `utmctl`
returns `-10004` (errAEEventNotHandled) / `-2700`; `utmctl file push` publishes
empty guest files; the scan finds no receipt.

Recovery, cheapest first (none of these is a host/computer reboot — do not tell
the owner to reboot anything):

1. **Cold-restart the guest**: `utmctl stop <uuid>` then `utmctl start <uuid>`,
   wait for `ip-address` to return, then re-run. Fixes stale file handles /
   leftover PowerShell locking an asset ("another program is using this file").
2. **Restart the UTM app** (fixes the Apple-Event layer):
   `osascript -e 'tell application "UTM" to quit'`, wait for the process to
   exit, `open -a UTM`, then start the guest. This is restarting the UTM
   *application*, not the machine.
3. **Verify Automation TCC**: System Settings → Privacy & Security → Automation
   lets the terminal control UTM.

Correct `utmctl` file syntax (push reads stdin, pull writes stdout):

```sh
echo "hi" | utmctl file push <uuid> 'C:\probe.txt'
utmctl file pull <uuid> 'C:\probe.txt'      # prints to stdout
```

## Post-publish: verify the actual downloaded bytes, not just the chain's own receipts

A green `release.yml` run proves the chain accepted its own artifacts; it does
not prove a user's `tar -xzf`/`unzip`/mounted `.dmg` actually runs. After a
publish, dispatch a read-only `workflow_dispatch` smoke test that does exactly
what an end user does: `gh release download` the real tag's assets, verify
their published checksum, extract/mount them, and run `--status`/`--version`
(plus `codesign --verify`/`spctl` for macOS). Keep it separate from any CI that
builds or signs — it must touch no signing/candidate/release-policy state, so
it's safe to run on `main` without the "Modify Shared Resources" scope concerns
that come with enabling a broad build-and-test workflow.

Two path assumptions bit on the very first v0.2.1 run of exactly this kind of
smoke test (repo: `.github/workflows/release-smoke-test.yml`), both fixed
before they were mistaken for a product defect:

- **Don't assume a tar/zip archive is flat.** `tar -xzf` extracting
  `minicon-<version>-linux-x86_64.tar.gz` did not drop `minicon` at the
  extraction root; `chmod +x ./minicon` failed with "No such file or
  directory". Fix: `find . -type f -name minicon` after extracting, not a
  hardcoded relative path.
- **`hdiutil attach -mountpoint DIR` mounts the volume's contents into `DIR`,
  not the `.dmg` file itself.** `spctl -a -t open ... "$MOUNT_DIR"/*.dmg`
  always failed ("No such file or directory") because the `.dmg` never lived
  inside its own mount point — it stayed wherever it was downloaded to. Fix:
  keep a reference to the original `.dmg` path (`DMG="$(ls ./*.dmg)"`) and run
  `spctl` against that, using `$MOUNT_DIR` only to find the `.app` bundle.

Both failures looked, at a glance, like the published release was broken —
codesign/checksum steps upstream of them had already passed, which is the
tell that the bug is in the smoke-test script's path handling, not the
shipped binary. Confirm which one it is (checksum + earlier steps green) before
escalating a smoke-test failure as a real release defect.

## The real root cause: env/code coupling

The whole delay in the 0.1.13 release traced to running a Win7 repro VM in the
same UTM instance the release court used. **Never run exploratory VMs in the
release UTM instance during a release.** Lease/reap courts cleanly. This is the
strongest argument for a release environment provisioned separately from any
product's code and from ad-hoc experimentation.

## Release handoff and avoidable latency (MiniCon 0.2.1)

- Establish one release owner before dispatch. Read the other agent's actual
  pane/logs, source SHA, staged diff and running processes; stale messages are
  not current state. An explicit owner instruction naming the version and
  publication already supplies release authority. Do not ask for it again.
- Inventory existing receipts before repeating gates. Reuse evidence only when
  its source, configuration and artifact identity match. Resume a failed stage
  where the gate supports it; source changes still require the applicable gates.
  Report local qualification time separately from the actual release chain.
- Finish version/docs edits before source-stability qualification. Use the
  standard ignored output directory, or verify a custom directory with
  `git check-ignore` before running. Generated output entering the source
  fingerprint and edits during qualification invalidate the receipt even when
  every build/test passes. Freeze source through sealing and Promotion; write
  final release-history documentation afterward.
- Diagnose inherited proxies before treating loopback transport failures as
  product regressions. In this handoff, eight local HTTP fixture tests failed
  with `Peer disconnected`, and cargo-xwin stalled downloading MSVC for over
  an hour. Removing inherited HTTP/HTTPS/ALL proxy variables for those specific
  commands restored both. Do not change the machine-wide proxy or assume
  direct access works for every destination. Bound downloads and retain logs.
- Distinguish Defender definition-update failure from a malware verdict. Run
  36250938765 attempt 1 failed `Update-MpSignature` before scanning; attempt 2
  scanned the same Candidate successfully. A transient setup failure permits a
  bounded retry, never a skipped update/scan or a fabricated clean receipt.
  Record the successful attempt as well as the failed one.
- Avoid extra artifact downloads on the critical path when the official
  Promotion workflow already downloads, rehashes and executes the public
  assets. Additional local downloads are diagnostic, not a new release gate.
  Preserve the existing parallel signing jobs and no-rebuild Promotion.
