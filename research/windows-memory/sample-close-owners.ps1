# Live-owner sample across extra-tab open/close. Research-only.
# Does not squeeze the PTY ring. Does not run the RSS harness.
param(
    [Parameter(Mandatory = $true)]
    [string]$Exe,
    [Parameter(Mandatory = $true)]
    [string]$Probe,
    [Parameter(Mandatory = $true)]
    [string]$Log,
    [Parameter(Mandatory = $true)]
    [string]$Result
)

$ErrorActionPreference = "Stop"
$PSDefaultParameterValues["Out-File:Encoding"] = "utf8"
$env:AGENTERM_NO_ACTIVATE = "1"
$pipe = 'pipe:\\.\pipe\minicon-owners-' + [guid]::NewGuid().ToString('N')
$exitCode = 1
$gui = $null

function Write-Log([string]$line) {
    Add-Content -LiteralPath $Log -Value $line -Encoding utf8
}

function Invoke-Cli([string[]]$Arguments) {
    $out = & $Exe @('cli', '--control', $pipe) @Arguments 2>&1 | Out-String
    return @{ Code = $LASTEXITCODE; Text = $out.Trim() }
}

function Wait-Ready([int]$TimeoutMs) {
    $deadline = [datetime]::UtcNow.AddMilliseconds($TimeoutMs)
    while ([datetime]::UtcNow -lt $deadline) {
        $r = Invoke-Cli @('list-tabs')
        if ($r.Code -eq 0 -and $r.Text) { return $r.Text }
        Start-Sleep -Milliseconds 50
    }
    throw "control endpoint not ready: $pipe"
}

function Get-TabIds([string]$ListJson) {
    $obj = $ListJson | ConvertFrom-Json
    return @($obj.tabs | ForEach-Object { $_.id })
}

function Write-Sample([string]$Phase, [int]$ProcessId, [int]$TabCount) {
    $probeOut = & powershell.exe -NoProfile -File $Probe -TargetPid $ProcessId -Phase $Phase
    Write-Log ("tabs={0} {1}" -f $TabCount, $probeOut)
}

try {
    Set-Content -LiteralPath $Log -Value ("start pipe=" + $pipe) -Encoding utf8
    $gui = Start-Process -FilePath $Exe -ArgumentList @(
        '--no-activate', '--cols', '80', '--rows', '24',
        '--control', $pipe, '-e', 'cmd.exe', '/Q', '/K'
    ) -PassThru -WindowStyle Normal
    $null = $gui.Handle
    $listed = Wait-Ready 15000
    $ids = @(Get-TabIds $listed)
    if ($ids.Count -ne 1) { throw "expected one tab, got $($ids.Count): $listed" }
    $root = [string]$ids[0]
    Write-Sample 'idle_one_tab' $gui.Id $ids.Count
    $loadCmd = "for /L %i in (1,1,2000) do @echo LOAD_%i & echo LOAD_DONE`r"
    $load = Invoke-Cli @('send-text', '--target', $root, $loadCmd)
    if ($load.Code -ne 0) { throw "send-text: $($load.Text)" }
    $waited = Invoke-Cli @('wait-text', '--target', $root, '--timeout-ms', '10000', 'LOAD_DONE')
    if ($waited.Code -ne 0) { throw "wait-text: $($waited.Text)" }
    Start-Sleep -Milliseconds 400
    Write-Sample 'after_load' $gui.Id 1
    for ($cycle = 1; $cycle -le 4; $cycle++) {
        $created = Invoke-Cli @('new-tab')
        if ($created.Code -ne 0) { throw "new-tab: $($created.Text)" }
        $two = Invoke-Cli @('list-tabs')
        $twoIds = @(Get-TabIds $two.Text)
        Write-Sample ("cycle{0}_two_tabs" -f $cycle) $gui.Id $twoIds.Count
        $extra = $twoIds | Where-Object { $_ -ne $root } | Select-Object -First 1
        if (-not $extra) { throw "no extra tab id in $($two.Text)" }
        $closed = Invoke-Cli @('close-tab', '--target', [string]$extra)
        if ($closed.Code -ne 0) { throw "close-tab: $($closed.Text)" }
        Start-Sleep -Milliseconds 400
        $after = Invoke-Cli @('list-tabs')
        $afterIds = @(Get-TabIds $after.Text)
        Write-Sample ("cycle{0}_after_close" -f $cycle) $gui.Id $afterIds.Count
        if ($afterIds.Count -ne 1) { throw "after close expected 1 tab, got $($afterIds.Count)" }
        $root = [string]$afterIds[0]
    }
    $null = Invoke-Cli @('close-window')
    if (-not $gui.WaitForExit(20000)) {
        Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
        throw "GUI WaitForExit timeout"
    }
    $gui.Refresh()
    if ($null -eq $gui.ExitCode) { throw "wrapper failure: ExitCode null after WaitForExit" }
    $exitCode = 0
} catch {
    $_ | Out-String | Add-Content -LiteralPath $Log -Encoding utf8
    $exitCode = 1
    if ($null -ne $gui -and -not $gui.HasExited) {
        Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
    }
} finally {
    [IO.File]::WriteAllText($Result + '.tmp', [string]$exitCode)
    Move-Item -LiteralPath ($Result + '.tmp') -Destination $Result -Force
}
exit $exitCode
