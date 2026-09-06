# Idle one-tab VirtualQueryEx commit split. Research-only. Not an RSS wrapper.
param(
    [Parameter(Mandatory = $true)]
    [string]$Exe,
    [Parameter(Mandatory = $true)]
    [string]$OwnersProbe,
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
$pipe = 'pipe:\\.\pipe\minicon-regions-' + [guid]::NewGuid().ToString('N')
$exitCode = 1
$gui = $null

function Write-Log([string]$line) {
    Add-Content -LiteralPath $Log -Value $line -Encoding utf8
}

function Invoke-Cli([string[]]$Arguments) {
    $out = & $Exe @('cli', '--control', $pipe) @Arguments 2>&1 | Out-String
    return @{ Code = $LASTEXITCODE; Text = $out.Trim() }
}

try {
    Set-Content -LiteralPath $Log -Value ("start pipe=" + $pipe + " ime_allowed=true chinese_input_preserved no_ime_opt_out") -Encoding utf8
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
        Start-Sleep -Milliseconds 50
    }
    if (-not $ready) { throw "control endpoint not ready: $pipe" }
    Start-Sleep -Milliseconds 400
    $owners = & powershell.exe -NoProfile -File $OwnersProbe -TargetPid $gui.Id -Phase idle_one_tab
    Write-Log $owners
    $regions = & powershell.exe -NoProfile -File $RegionsProbe -TargetPid $gui.Id
    $regions | ForEach-Object { Write-Log $_ }
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
