# Backend cost court for the 0.2.20 default decision. Per backend, same host:
# (1) process count and total working set of the GUI plus all descendants with
#     three tabs, (2) input echo round-trip latency through send-text +
#     wait-text, (3) orphans left after a hard kill of the GUI (crash shape).
# Owner of the verdict: prd/PRD_02_24_con_terminal.md backend selection.
param(
    [Parameter(Mandatory)][ValidateSet('agent', 'conpty', 'rust-console')][string]$Backend,
    [string]$Exe = "$PSScriptRoot\..\..\..\target\debug\minicon.exe",
    [int]$Rounds = 20
)
$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path $Exe).Path
# A court run from inside an older MiniCon tab inherits that host's
# AGENTERM_FORCE_CONSOLE_AGENT=1, which silently turns the conpty cell into an
# agent cell. Clear it so each cell measures the backend it names.
Remove-Item Env:AGENTERM_FORCE_CONSOLE_AGENT -ErrorAction SilentlyContinue
Remove-Item Env:FORCE_CONSOLE_AGENT -ErrorAction SilentlyContinue
$ep = "pipe:\\.\pipe\minicon-cost-$Backend-$PID"

function Descendants([uint32]$root) {
    $all = Get-CimInstance Win32_Process
    $set = @($root); $frontier = @($root)
    while ($frontier.Count) {
        $next = @($all | Where-Object { $frontier -contains $_.ParentProcessId } | ForEach-Object { [uint32]$_.ProcessId })
        $set += $next; $frontier = $next
    }
    $all | Where-Object { $set -contains [uint32]$_.ProcessId }
}

$argv = @()
if ($Backend -ne 'agent') { $argv += @('--feature', $Backend) }
$argv += @('--no-activate', '--control', $ep, '-e', 'cmd.exe', '/d', '/k', 'echo COST_READY')
$gui = Start-Process -FilePath $Exe -ArgumentList $argv -PassThru
& $Exe cli --control $ep wait-text --target '@1' --timeout-ms 20000 COST_READY | Out-Null
& $Exe cli --control $ep new-tab | Out-Null
& $Exe cli --control $ep new-tab | Out-Null
Start-Sleep -Seconds 4

$tree = @(Descendants $gui.Id)
$ws = ($tree | ForEach-Object { (Get-Process -Id $_.ProcessId -ErrorAction SilentlyContinue).WorkingSet64 } | Measure-Object -Sum).Sum
$names = ($tree | Group-Object Name | ForEach-Object { "$($_.Name)x$($_.Count)" }) -join ' '

$lat = @()
for ($i = 0; $i -lt $Rounds; $i++) {
    $tok = "LAT$i" + "Q"
    $sw = [Diagnostics.Stopwatch]::StartNew()
    & $Exe cli --control $ep send-text --target '@1' "echo $tok`r" | Out-Null
    & $Exe cli --control $ep wait-text --target '@1' --timeout-ms 10000 $tok | Out-Null
    $lat += $sw.Elapsed.TotalMilliseconds
}
$lat = $lat | Sort-Object
$p50 = $lat[[int]($lat.Count * 0.5)]; $p90 = $lat[[int]($lat.Count * 0.9) - 1]

$pids = @($tree | Where-Object { $_.ProcessId -ne $gui.Id } | ForEach-Object { [uint32]$_.ProcessId })
Stop-Process -Id $gui.Id -Force
Start-Sleep -Seconds 5
$orphans = @(Get-CimInstance Win32_Process | Where-Object { $pids -contains [uint32]$_.ProcessId })
$orphanNames = ($orphans | ForEach-Object { $_.Name }) -join ' '
$orphans | ForEach-Object { Stop-Process -Id $_.ProcessId -Force -ErrorAction SilentlyContinue }

[pscustomobject]@{
    backend        = $Backend
    processes      = $tree.Count
    process_kinds  = $names
    working_set_mb = [math]::Round($ws / 1MB, 1)
    echo_p50_ms    = [math]::Round($p50, 1)
    echo_p90_ms    = [math]::Round($p90, 1)
    hard_kill_orphans = $orphans.Count
    orphan_kinds   = $orphanNames
}
