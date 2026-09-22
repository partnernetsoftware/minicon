# v0.1.23 — every loop fast, CI only seals

Owner's brief (2026-09-22): trial and error only pays when each iteration is
short. Compress every stage of the cycle, at least incrementally. The CI
release chain shrinks to the last step, signing and stamping bytes that were
already verified. All testing runs locally and is made fast by incremental
work.

## Baseline (measured 2026-09-22, v0.1.22)

| stage | time | what dominates |
| --- | --- | --- |
| local six-cell, full | ~20 min | osx-x86_64 tests under Rosetta; full rebuild when the build dir changes |
| Windows court `test` stage | 227 s -> **72 s** | QGA file copy at ~240 KB/s; now HTTP (utm-court `b87196c`) |
| Windows court rebuild after a one-line change | full build -> **4 s** | APFS clone of the previous fingerprint dir + incremental `cargo xwin` |
| court cold start | ~150 s, flaky | disposable boot + session agent; a stale `ping.response` poisoned boots |
| CI release chain | ~45 min per round | minicon-com -> signing x2 -> candidate -> reputation -> release |

## 1. Local loop

1. **Fingerprint dir by clone, not rebuild.** six-cell seeds
   `target-six/builds/<new fp>` with `cp -c -R` from the newest previous
   fingerprint dir, then builds incrementally. Integration tests bake
   `CARGO_BIN_EXE_*` absolute paths, so host-run test cells must rebuild those
   binaries (touch their crate) after the clone; Windows cells push exes and
   are unaffected.
2. **Select cells from the change.** Docs/plan-only changes skip build cells;
   a change confined to one target's `cfg` runs that target's cells. The
   receipt names what was skipped and why; nothing is skipped silently.
3. **Pre-flight the environment (15 s).** Before any cell: free disk, a
   one-line x86_64 binary under Rosetta, court agent nonce. A hung Rosetta
   cost 20 minutes today.
4. **nextest archive for courts.** Build test binaries once, archive, run the
   archive in each court. Partition long suites.

## 2. Courts

1. HTTP transfer (done for Windows; do the same for Linux courts).
2. Keep the court warm across stages of one qualification; release it at the
   end. Push only binaries whose hash changed since the last push.
3. Snapshot a booted, agent-ready court and resume it instead of cold boot.

## 3. CI is only the seal

CI stops building. Local qualification produces the exact bytes plus a
receipt binding source SHA, source fingerprint, artifact digests and the
six-cell/court results. CI verifies that binding (attestation), signs,
notarizes, seals the Candidate, and promotes it. Rebuild-to-promote stays
forbidden; the change is where the unsigned bytes come from. Needs an owner
decision on how locally built bytes are trusted (build provenance), before
any workflow changes.

## 4. Pre-existing Windows court gaps (found by 0.1.22's first full run)

- `minicon_control`: `a_host_whose_program_cannot_be_spawned_dies_and_says_why`,
  `a_new_tab_that_cannot_start_is_a_notice_not_an_exit`,
  `gui_control_surface_isolated_multitab_black_box`.
- `minicon_throughput`: `pty_drained_bytes` a few KB against the 32 MiB
  payload, while `THROUGHPUT_DONE_32M` is seen.

Each gets its own diagnosis now that a court round is ~1 minute.

## Verification

Every speed change is measured before and after, N runs, same machine; a
single A/B is not evidence.
