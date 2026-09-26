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

## The real root cause: env/code coupling

The whole delay in the 0.1.13 release traced to running a Win7 repro VM in the
same UTM instance the release court used. **Never run exploratory VMs in the
release UTM instance during a release.** Lease/reap courts cleanly. This is the
strongest argument for a release environment provisioned separately from any
product's code and from ad-hoc experimentation.
