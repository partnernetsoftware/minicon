# Start research PE, wait first control+frame, no screenshot. Trace is in-process.
param(
    [Parameter(Mandatory = $true)]
    [string]$Exe,
    [Parameter(Mandatory = $true)]
    [string]$Trace,
    [Parameter(Mandatory = $true)]
    [string]$Log,
    [Parameter(Mandatory = $true)]
    [string]$Result
)

$ErrorActionPreference = "Stop"
$PSDefaultParameterValues["Out-File:Encoding"] = "utf8"
$env:AGENTERM_NO_ACTIVATE = "1"
$exitCode = 1
$gui = $null
$pipe = 'pipe:\\.\pipe\minicon-inithooks-' + [guid]::NewGuid().ToString('N')

function Write-Log([string]$line) {
    Add-Content -LiteralPath $Log -Value $line -Encoding utf8
}

function Invoke-Cli([string[]]$Arguments) {
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $Exe @('cli', '--control', $pipe) @Arguments 2>&1 | ForEach-Object { $_.ToString() } | Out-String
        return @{ Code = $LASTEXITCODE; Text = $out.Trim() }
    } finally {
        $ErrorActionPreference = $prev
    }
}

function Run-One([string]$Tag, [bool]$SkipFocus) {
    $localPipe = 'pipe:\\.\pipe\minicon-inithooks-' + [guid]::NewGuid().ToString('N')
    $localTrace = "$Trace.$Tag"
    if (Test-Path -LiteralPath $localTrace) { Remove-Item -LiteralPath $localTrace -Force }
    if ($SkipFocus) { $env:MINICON_INIT_SKIP_FOCUS = '1' } else { Remove-Item Env:MINICON_INIT_SKIP_FOCUS -ErrorAction SilentlyContinue }
    $env:MINICON_INIT_TRACE = $localTrace
    Write-Log "run tag=$Tag skip_focus=$SkipFocus ime_allowed=true visible_show=true no_screenshot"
    $proc = Start-Process -FilePath $Exe -ArgumentList @(
        '--no-activate', '--cols', '80', '--rows', '24',
        '--control', $localPipe, '-e', 'cmd.exe', '/Q', '/K'
    ) -PassThru -WindowStyle Normal
    $null = $proc.Handle
    $deadline = [datetime]::UtcNow.AddMilliseconds(15000)
    $ready = $false
    while ([datetime]::UtcNow -lt $deadline) {
        $prev = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $out = & $Exe @('cli', '--control', $localPipe, 'list-tabs') 2>&1 | ForEach-Object { $_.ToString() } | Out-String
        $code = $LASTEXITCODE
        $ErrorActionPreference = $prev
        if ($code -eq 0 -and $out.Trim()) { $ready = $true; break }
        Start-Sleep -Milliseconds 40
    }
    if (-not $ready) { throw "control not ready $Tag" }
    $deadline = [datetime]::UtcNow.AddMilliseconds(10000)
    $framed = $false
    while ([datetime]::UtcNow -lt $deadline) {
        $prev = $ErrorActionPreference
        $ErrorActionPreference = 'Continue'
        $out = & $Exe @('cli', '--control', $localPipe, 'perf-stats') 2>&1 | ForEach-Object { $_.ToString() } | Out-String
        $code = $LASTEXITCODE
        $ErrorActionPreference = $prev
        if ($code -eq 0 -and $out.Trim()) {
            $j = $out | ConvertFrom-Json
            $present = 0; $frames = 0
            if ($null -ne $j.present_success) { $present = [int64]$j.present_success }
            if ($null -ne $j.frames) { $frames = [int64]$j.frames }
            if ($present -ge 1 -or $frames -ge 1) { $framed = $true; break }
        }
        Start-Sleep -Milliseconds 40
    }
    if (-not $framed) { throw "first frame not observed $Tag" }
    Start-Sleep -Milliseconds 200
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    $null = & $Exe @('cli', '--control', $localPipe, 'close-window') 2>&1
    $ErrorActionPreference = $prev
    if (-not $proc.WaitForExit(20000)) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        throw "WaitForExit timeout $Tag"
    }
    $proc.Refresh()
    if ($null -eq $proc.ExitCode) { throw "wrapper failure $Tag" }
    Write-Log "==== trace $Tag ===="
    if (Test-Path -LiteralPath $localTrace) {
        Get-Content -LiteralPath $localTrace | ForEach-Object { Write-Log $_ }
        $edge = Get-Content -LiteralPath $localTrace | Where-Object { $_ -match 'tif_edge=1' }
        Write-Log ("tif_edge_lines tag={0} {1}" -f $Tag, (($edge | Measure-Object).Count))
    } else {
        Write-Log "missing trace $localTrace"
    }
}

try {
    Set-Content -LiteralPath $Log -Value "start init-hooks ime_allowed=true visible_window=true no_screenshot no_delay_first_frame_as_idle" -Encoding utf8
    Run-One 'baseline_focus' $false
    Run-One 'skip_focus' $true
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
