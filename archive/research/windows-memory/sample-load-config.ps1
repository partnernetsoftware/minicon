# Candidate 1: skip SHGetFolderPathW vs keep. Activated first-frame steady.
# No --no-activate. No research skip-Focus. No APPDATA stand-in. No screenshot.
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
        [bool]$SkipConfigDir
    )
    $localPipe = 'pipe:\\.\pipe\minicon-loadcfg-' + [guid]::NewGuid().ToString('N')
    $localTrace = "$Trace.$Tag"
    if (Test-Path -LiteralPath $localTrace) { Remove-Item -LiteralPath $localTrace -Force }
    Remove-Item Env:MINICON_INIT_SKIP_FOCUS -ErrorAction SilentlyContinue
    Remove-Item Env:AGENTERM_NO_ACTIVATE -ErrorAction SilentlyContinue
    $env:MINICON_INIT_ACTIVATE_AFTER_PRESENT = '1'
    $env:MINICON_INIT_SKIP_ENGLISH = '1'
    if ($SkipConfigDir) { $env:MINICON_SKIP_USER_CONFIG_DIRECTORY = '1' } else { Remove-Item Env:MINICON_SKIP_USER_CONFIG_DIRECTORY -ErrorAction SilentlyContinue }
    $env:MINICON_INIT_TRACE = $localTrace
    Write-Log ("run tag={0} skip_user_config_directory={1} no_activate=false activate_after_present=true skip_english=true ime_allowed=true no_screenshot no_appdata_standin" -f $Tag, $SkipConfigDir)
    $proc = Start-Process -FilePath $Exe -ArgumentList @(
        '--cols', '80', '--rows', '24',
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
    Start-Sleep -Milliseconds 600
    $p = Get-Process -Id $proc.Id -ErrorAction SilentlyContinue
    $ws = if ($null -ne $p) { [int64]$p.WorkingSet64 } else { 0 }
    Write-Log ("guest_ws_after_activate_settle tag={0} ws={1}" -f $Tag, $ws)
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
    } else {
        Write-Log "missing trace $localTrace"
    }
}

try {
    Set-Content -LiteralPath $Log -Value "start load-config causal activated_steady ime_on no_screenshot no_no-activate no_appdata_standin" -Encoding utf8
    Run-One -Tag 'keep_shgetfolderpath' $false
    Run-One -Tag 'skip_shgetfolderpath' $true
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
