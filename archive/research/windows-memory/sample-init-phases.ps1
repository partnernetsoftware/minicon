# One process, init-phase RSS. IME stays on. No screenshot. Not a counting court.
param(
    [Parameter(Mandatory = $true)]
    [string]$Exe,
    [Parameter(Mandatory = $true)]
    [string]$RegionsProbe,
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
$pipe = 'pipe:\\.\pipe\minicon-init-' + [guid]::NewGuid().ToString('N')

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

function Write-Probe([string]$Phase) {
    if ($null -eq $gui -or $gui.HasExited) { throw "process gone at $Phase" }
    $gui.Refresh()
    Write-Log ("phase={0} t={1} pid={2} hwnd=0x{3:x} ws={4} private_commit={5}" -f `
        $Phase, [datetime]::UtcNow.ToString('yyyy-MM-ddTHH:mm:ss.fffZ'), $gui.Id, [int64]$gui.MainWindowHandle, $gui.WorkingSet64, $gui.PrivateMemorySize64)
    $regions = & powershell.exe -NoProfile -File $RegionsProbe -TargetPid $gui.Id
    $regions | ForEach-Object { Write-Log ("qws phase={0} {1}" -f $Phase, $_) }
}

try {
    Set-Content -LiteralPath $Log -Value "start init-phases ime_allowed=true chinese_input_preserved no_screenshot no_count_court privatized_image_not_all_TIF" -Encoding utf8
    $gui = Start-Process -FilePath $Exe -ArgumentList @(
        '--no-activate', '--cols', '80', '--rows', '24',
        '--control', $pipe, '-e', 'cmd.exe', '/Q', '/K'
    ) -PassThru -WindowStyle Normal
    $null = $gui.Handle
    Start-Sleep -Milliseconds 30
    Write-Probe 'spawn'

    $deadline = [datetime]::UtcNow.AddMilliseconds(15000)
    while ([datetime]::UtcNow -lt $deadline) {
        $gui.Refresh()
        if ($gui.MainWindowHandle -ne [IntPtr]::Zero) { break }
        Start-Sleep -Milliseconds 20
    }
    Write-Probe 'hwnd'

    $deadline = [datetime]::UtcNow.AddMilliseconds(15000)
    $ready = $false
    while ([datetime]::UtcNow -lt $deadline) {
        $r = Invoke-Cli @('list-tabs')
        if ($r.Code -eq 0 -and $r.Text) { $ready = $true; break }
        Start-Sleep -Milliseconds 40
    }
    if (-not $ready) { throw "control not ready" }
    Write-Probe 'control'

    $deadline = [datetime]::UtcNow.AddMilliseconds(10000)
    $framed = $false
    $perf = ""
    while ([datetime]::UtcNow -lt $deadline) {
        $r = Invoke-Cli @('perf-stats')
        $perf = $r.Text
        if ($r.Code -eq 0 -and $r.Text) {
            $j = $r.Text | ConvertFrom-Json
            $present = 0; $direct = 0; $frames = 0
            if ($null -ne $j.present_success) { $present = [int64]$j.present_success }
            if ($null -ne $j.host_direct_frames) { $direct = [int64]$j.host_direct_frames }
            if ($null -ne $j.frames) { $frames = [int64]$j.frames }
            if ($present -ge 1 -or $direct -ge 1 -or $frames -ge 1) {
                Write-Log ("first_frame present_success={0} host_direct_frames={1} frames={2}" -f $present, $direct, $frames)
                $framed = $true
                break
            }
        }
        Start-Sleep -Milliseconds 40
    }
    if (-not $framed) { throw "first frame not observed: $perf" }
    Write-Probe 'first_frame'

    $null = Invoke-Cli @('close-window')
    if (-not $gui.WaitForExit(20000)) {
        Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
        throw "WaitForExit timeout"
    }
    $gui.Refresh()
    if ($null -eq $gui.ExitCode) { throw "wrapper failure: ExitCode null" }
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
