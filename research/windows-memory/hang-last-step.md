# win-aarch64 release RSS hang: last live step

Job kept until the host runner’s 20 min poll timed out and released the VM.
QGA reads were taken while the guest was still `started`.

## Identity

- Host runner PID 50131, ~20 min
- Job id `win_aarch64_rss_release_50131_17471`
- Guest files under `C:\minicon-six\`
- Product PE: `win-aarch64\target\debug\minicon-release-rss.exe` (release bytes)
- Harness PE: `win-aarch64\target\debug\minicon-control-release-rss.exe`

## What ran vs what did not

| check | result |
|---|---|
| QGA / interactive-ready | ready (lease JSON earlier) |
| `job.pending.ps1` | gone (agent consumed it) |
| `job.ready` | present (`ready`) |
| `job-*.exit` / `.exit.tmp` | **missing** — finally block never finished |
| `job-*.log` pull | **sharing violation** — another process still had the log open |
| `agent-v2\job.log` | missing (agent only writes that after `& job.running.ps1` returns) |
| `minicon-release-rss.exe` | **alive** PID 9852, Session 1, WS 33,620 K, CPU 0:00:07, window title N/A |
| `minicon-control-release-rss.exe` | **not** in `tasklist /V` |
| powershell | two Console processes (10060, 9704) |
| cmd + conhost | present beside the product EXE |

Raw tasklist: `hang-diag/tasklist-v-20260906T101138Z.txt`.
Pipe dump did not complete: guest already `虚拟机未运行` after host timeout/release.

## Named last step

Not “never started”. Not “waiting to write exit as the only remaining work”.

**Last live step: product GUI (`minicon-release-rss.exe`) is up (~33 MiB working set) and holding `job-*.log`. The interactive agent is still inside `& harness *> job.log` (or stuck because that redirection’s handles remain open). Control-endpoint ready / named-pipe round-trip never produced an exit file.**

Discriminators:

1. Host EXE **did start**.
2. Harness image **not listed** 20 min later — either it never stayed running under that name, or it exited while the GUI child kept the redirected log (PowerShell `*>` does not reach `finally` until those handles close).
3. Named-pipe / `wait_until_ready` **not observed** (no pipe listing before release).
4. Exit file **not written** because the job script never left `& harness`.

This is a research-runner hang (redirect + GUI child), not a MiniCon RSS FAIL and not “waited 20 min therefore BLOCKED” without a step name.

## Next research runner change (not production)

Host poll later hit “exceeded 20 minutes” / `WIN_RELEASE_RSS_EXIT` absent / process EXIT 1; trap released the VM.

`run-release-rss.sh` now uses guest `Start-Process -Wait -PassThru` with explicit stdout/stderr files instead of `*> job.log`, so a live GUI child cannot pin the agent inside `& harness`. Not re-leased until that script is used; production `windows-utm-runner.sh` unchanged.
