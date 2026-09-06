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
$exitCode = 1
$gui = $null

function Write-Log([string]$line) {
    Add-Content -LiteralPath $Log -Value $line -Encoding utf8
}

function Run-One {
    param(
        [string]$Tag,
        [bool]$SkipFocus,
        [bool]$NoActivate,
        [bool]$ActivateAfterPresent
    )
    $localPipe = 'pipe:\\.\pipe\minicon-inithooks-' + [guid]::NewGuid().ToString('N')
    $localTrace = "$Trace.$Tag"
    if (Test-Path -LiteralPath $localTrace) { Remove-Item -LiteralPath $localTrace -Force }
    if ($SkipFocus) { $env:MINICON_INIT_SKIP_FOCUS = '1' } else { Remove-Item Env:MINICON_INIT_SKIP_FOCUS -ErrorAction SilentlyContinue }
    if ($ActivateAfterPresent) { $env:MINICON_INIT_ACTIVATE_AFTER_PRESENT = '1' } else { Remove-Item Env:MINICON_INIT_ACTIVATE_AFTER_PRESENT -ErrorAction SilentlyContinue }
    if ($NoActivate) { $env:AGENTERM_NO_ACTIVATE = '1' } else { Remove-Item Env:AGENTERM_NO_ACTIVATE -ErrorAction SilentlyContinue }
    $env:MINICON_INIT_TRACE = $localTrace
    Write-Log ("run tag={0} skip_focus={1} no_activate={2} activate_after_present={3} ime_allowed=true visible_show=true no_screenshot no_pty_control_write" -f $Tag, $SkipFocus, $NoActivate, $ActivateAfterPresent)
    $argList = New-Object System.Collections.Generic.List[string]
    if ($NoActivate) { $argList.Add('--no-activate') }
    $argList.AddRange([string[]]@('--cols', '80', '--rows', '24', '--control', $localPipe, '-e', 'cmd.exe', '/Q', '/K'))
    $proc = Start-Process -FilePath $Exe -ArgumentList $argList.ToArray() -PassThru -WindowStyle Normal
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
    if ($ActivateAfterPresent) { Start-Sleep -Milliseconds 800 } else { Start-Sleep -Milliseconds 250 }
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
        $blocked = Get-Content -LiteralPath $localTrace | Where-Object { $_ -match 'chinese_ime_BLOCKED' }
        Write-Log ("chinese_ime_blocked_lines tag={0} {1}" -f $Tag, (($blocked | Measure-Object).Count))
        $keys = Get-Content -LiteralPath $localTrace | Where-Object { $_ -match 'label=WM_KEYDOWN' }
        Write-Log ("wm_keydown_lines tag={0} {1}" -f $Tag, (($keys | Measure-Object).Count))
    } else {
        Write-Log "missing trace $localTrace"
    }
}

try {
    Set-Content -LiteralPath $Log -Value "start init-hooks ime_allowed=true visible_window=true no_screenshot no_delay_first_frame_as_idle no_pty_control_write_as_ime" -Encoding utf8
    # (1) current baseline: no-activate flag + opened() still Focus
    Run-One -Tag 'noact_opened_focus' -SkipFocus $false -NoActivate $true -ActivateAfterPresent $false
    # (2) skip opened Focus, then in-process real activate after first present + English SendInput
    Run-One -Tag 'noact_skip_then_activate' -SkipFocus $true -NoActivate $true -ActivateAfterPresent $true
    # (3) normal start control: no --no-activate, no AGENTERM_NO_ACTIVATE
    Run-One -Tag 'normal_activate' -SkipFocus $false -NoActivate $false -ActivateAfterPresent $false
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
