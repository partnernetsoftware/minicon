# G2 Windows control black-box against packed minicon.com.
# The GitHub Windows runner is the disposable court. Minicon resolves its
# config through CSIDL_APPDATA, so environment-variable isolation is not
# claimed: the real config is hashed before and after instead.
param(
    [Parameter(Mandatory = $true)][string]$Binary
)
$ErrorActionPreference = "Stop"
$Binary = (Resolve-Path -LiteralPath $Binary).Path

function Get-ConfigState([string]$Path) {
    if (-not (Test-Path -LiteralPath $Path)) { return "ABSENT" }
    return (Get-FileHash -LiteralPath $Path -Algorithm SHA256).Hash.ToLowerInvariant()
}

function Invoke-Cli([string[]]$Arguments) {
    $lines = @(& $Binary cli --control $script:Endpoint @Arguments 2>&1)
    $rc = $LASTEXITCODE
    $text = ($lines | ForEach-Object { $_.ToString() }) -join "`n"
    if ($rc -ne 0) {
        throw "cli $($Arguments -join ' ') exit $rc`n$text"
    }
    return $text
}

function Get-ExtractDirs([int]$LoaderPid) {
    $roots = @([IO.Path]::GetTempPath(), "C:\tmp") | Select-Object -Unique
    foreach ($root in $roots) {
        if (-not (Test-Path -LiteralPath $root -PathType Container)) { continue }
        Get-ChildItem -LiteralPath $root -Directory -Force -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like "minicon.com.$LoaderPid.*" }
    }
}

if (-not (Test-Path -LiteralPath $Binary -PathType Leaf)) { throw "missing binary" }
$appData = [Environment]::GetFolderPath([Environment+SpecialFolder]::ApplicationData)
if (-not $appData) { throw "CSIDL_APPDATA resolved empty" }
$config = Join-Path $appData "minicon.json"
$configBefore = Get-ConfigState $config

$root = Join-Path $env:RUNNER_TEMP ("minicon-g2-" + [guid]::NewGuid().ToString("N"))
$work = Join-Path $root "work"
$stdout = Join-Path $root "host.stdout.log"
$stderr = Join-Path $root "host.stderr.log"
$script:Endpoint = "pipe:\\.\pipe\minicon-g2-$PID-$([guid]::NewGuid().ToString('N'))"
$hostProcess = $null
$finished = $false

try {
    New-Item -ItemType Directory -Path $work -Force | Out-Null
    $hostProcess = Start-Process -FilePath $Binary -ArgumentList @(
        "--no-activate", "--control", $script:Endpoint
    ) -WorkingDirectory $work -RedirectStandardOutput $stdout `
      -RedirectStandardError $stderr -PassThru
    Write-Host "loader_pid=$($hostProcess.Id)"

    $tabsText = $null
    $deadline = [DateTime]::UtcNow.AddSeconds(30)
    while ([DateTime]::UtcNow -lt $deadline) {
        $hostProcess.Refresh()
        if ($hostProcess.HasExited) {
            $hostErr = if (Test-Path -LiteralPath $stderr) { Get-Content -LiteralPath $stderr -Raw } else { "" }
            throw "host died before list-tabs rc=$($hostProcess.ExitCode)`n$hostErr"
        }
        try {
            $tabsText = Invoke-Cli @("list-tabs")
            break
        } catch {
            Start-Sleep -Milliseconds 200
        }
    }
    if (-not $tabsText) { throw "list-tabs never ready" }
    Write-Host "list-tabs=$tabsText"
    $tabs = $tabsText | ConvertFrom-Json
    $activeTabs = @($tabs.tabs | Where-Object { $_.active -eq $true -and $_.id })
    if ($activeTabs.Count -ne 1) { throw "list-tabs needs exactly one active tab with id" }
    $tab = [string]$activeTabs[0].id

    # The typed command contains the two halves but not RESULT_TOKEN verbatim.
    # Seeing RESULT_TOKEN therefore proves cmd.exe executed the command rather
    # than the terminal merely echoing input.
    $tokenPart = "G2TOK$PID$([guid]::NewGuid().ToString('N').Substring(0, 8))"
    $resultToken = "G2RESULT$tokenPart"
    $command = "for %A in (G2RESULT) do @echo %A$tokenPart`r"
    [void](Invoke-Cli @("send-text", "--target", $tab, $command))
    [void](Invoke-Cli @("wait-text", "--target", $tab, "--timeout-ms", "15000", $resultToken))

    $snapshotText = Invoke-Cli @("ui-snapshot")
    $snapshot = $snapshotText | ConvertFrom-Json
    if ([string]$snapshot.active -ne $tab) {
        throw "ui-snapshot active '$($snapshot.active)' != '$tab'"
    }
    Write-Host "ui-snapshot-active=$tab"

    $pane = Invoke-Cli @("capture-pane", "--max-bytes", "8000")
    if (-not $pane.Contains($resultToken)) { throw "capture-pane missing result token" }
    Write-Host "capture-pane contains $resultToken"

    [void](Invoke-Cli @("close-window"))
    if (-not $hostProcess.WaitForExit(5000)) { throw "loader still alive after close-window" }
    if ($hostProcess.ExitCode -ne 0) { throw "loader rc=$($hostProcess.ExitCode) want 0" }

    $left = @(Get-ExtractDirs $hostProcess.Id)
    if ($left.Count -ne 0) {
        throw "leftover extract dirs for loader pid $($hostProcess.Id): $($left.Name -join ', ')"
    }
    Write-Host "extract_dirs_for_$($hostProcess.Id)=0"

    $configAfter = Get-ConfigState $config
    Write-Host "config_baseline=$configBefore"
    Write-Host "config_after=$configAfter"
    if ($configBefore -ne $configAfter) { throw "CSIDL_APPDATA minicon.json changed" }

    $finished = $true
    Write-Host "PASS g2-control windows endpoint=unique token=$resultToken"
} finally {
    if ($hostProcess -and -not $hostProcess.HasExited) {
        try { [void](Invoke-Cli @("close-window")) } catch {}
        if (-not $hostProcess.WaitForExit(2000)) { Stop-Process -Id $hostProcess.Id -Force }
    }
    if (Test-Path -LiteralPath $root) { Remove-Item -LiteralPath $root -Recurse -Force }
}
