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

try {
    Set-Content -LiteralPath $Log -Value "start init-hooks ime_allowed=true no_screenshot" -Encoding utf8
    if (Test-Path -LiteralPath $Trace) { Remove-Item -LiteralPath $Trace -Force }
    $gui = Start-Process -FilePath $Exe -ArgumentList @(
        '--no-activate', '--cols', '80', '--rows', '24',
        '--control', $pipe, '-e', 'cmd.exe', '/Q', '/K'
    ) -PassThru -WindowStyle Normal
    $null = $gui.Handle
    $deadline = [datetime]::UtcNow.AddMilliseconds(15000)
    $ready = $false
    while ([datetime]::UtcNow -lt $deadline) {
        $r = Invoke-Cli @('list-tabs')
        if ($r.Code -eq 0 -and $r.Text) { $ready = $true; break }
        Start-Sleep -Milliseconds 40
    }
    if (-not $ready) { throw "control not ready" }
    $deadline = [datetime]::UtcNow.AddMilliseconds(10000)
    $framed = $false
    while ([datetime]::UtcNow -lt $deadline) {
        $r = Invoke-Cli @('perf-stats')
        if ($r.Code -eq 0 -and $r.Text) {
            $j = $r.Text | ConvertFrom-Json
            $present = 0; $frames = 0
            if ($null -ne $j.present_success) { $present = [int64]$j.present_success }
            if ($null -ne $j.frames) { $frames = [int64]$j.frames }
            if ($present -ge 1 -or $frames -ge 1) { $framed = $true; break }
        }
        Start-Sleep -Milliseconds 40
    }
    if (-not $framed) { throw "first frame not observed" }
    Start-Sleep -Milliseconds 200
    $null = Invoke-Cli @('close-window')
    if (-not $gui.WaitForExit(20000)) {
        Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
        throw "WaitForExit timeout"
    }
    $gui.Refresh()
    if ($null -eq $gui.ExitCode) { throw "wrapper failure: ExitCode null" }
    if (Test-Path -LiteralPath $Trace) {
        Write-Log "==== guest trace ===="
        Get-Content -LiteralPath $Trace | ForEach-Object { Write-Log $_ }
    } else {
        Write-Log "missing trace $Trace"
    }
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
