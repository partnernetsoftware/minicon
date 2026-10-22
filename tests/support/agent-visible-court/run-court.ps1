# Visible three-agent court for the 0.2.20 default decision: start each real
# agent CLI in a MiniCon tab on the named backend, let its TUI settle, resize
# the window narrow -> wide -> narrow, and capture parser state plus pixels at
# each step. No prompt is sent (no account action, no model cost); this
# qualifies rendering, layout and resize of the agents' own start screens.
# Verdict owner: prd/PRD_02_24_01_tui_compatibility.md.
param(
    [Parameter(Mandatory)][ValidateSet('console-agent', 'conpty', 'rust-console')][string]$Backend,
    [string]$Exe = "$PSScriptRoot\..\..\..\target\debug\minicon.exe",
    [string]$Out = (Join-Path $env:TEMP "minicon-agent-court")
)
$ErrorActionPreference = 'Stop'
$Exe = (Resolve-Path $Exe).Path
Remove-Item Env:AGENTERM_FORCE_CONSOLE_AGENT -ErrorAction SilentlyContinue
Remove-Item Env:FORCE_CONSOLE_AGENT -ErrorAction SilentlyContinue

$agents = [ordered]@{
    claude = @('cmd.exe', '/d', '/c', 'claude')
    codex  = @('cmd.exe', '/d', '/c', 'codex')
    grok   = @('cmd.exe', '/d', '/c', 'grok')
}
foreach ($name in $agents.Keys) {
    $dir = Join-Path $Out "$Backend-$name"
    New-Item -ItemType Directory -Force $dir | Out-Null
    $ep = "pipe:\\.\pipe\minicon-agentcourt-$Backend-$name-$PID"
    $argv = @('--no-activate', '--feature', $Backend, '--control', $ep, '-e') + $agents[$name]
    $gui = Start-Process -FilePath $Exe -ArgumentList $argv -PassThru
    Start-Sleep -Seconds 15
    $snap = & $Exe cli --control $ep ui-snapshot
    $snap | Set-Content "$dir\ui.json"
    $step = 0
    foreach ($size in @(@(1100, 700), @(700, 600), @(1400, 800), @(900, 650))) {
        & $Exe cli --control $ep resize-window --width $size[0] --height $size[1] | Out-Null
        Start-Sleep -Seconds 4
        & $Exe cli --control $ep capture-pane --target '@1' --output "$dir\pane-$step.txt" | Out-Null
        & $Exe cli --control $ep screenshot-pane --target '@1' --output "$dir\shot-$step.png" | Out-Null
        $step += 1
    }
    & $Exe cli --control $ep close-window | Out-Null
    Start-Sleep -Seconds 2
    if (-not $gui.HasExited) { Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue }
    $pane = Get-Content "$dir\pane-3.txt" -Raw
    $replacement = ([regex]::Matches($pane, [string][char]0xFFFD)).Count
    [pscustomobject]@{
        backend = $Backend
        agent = $name
        nonblank_lines = (($pane -split "`n") | Where-Object { $_.Trim() }).Count
        replacement_chars = $replacement
        backend_reported = if ((Get-Content "$dir\ui.json" -Raw) -match '"pty_backend"\s*:\s*"([^"]+)"') { $Matches[1] } else { 'unreported' }
        out = $dir
    }
}
