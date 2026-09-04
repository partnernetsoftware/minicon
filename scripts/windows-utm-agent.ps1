$ErrorActionPreference = "Continue"
$root = "C:\minicon-six"
$pending = Join-Path $root "job.pending.ps1"
$ready = Join-Path $root "job.ready"
$running = Join-Path $root "job.running.ps1"
$log = Join-Path $root "job.log"
$result = Join-Path $root "job.exit"
$logTemp = Join-Path $root "job.log.tmp"
$resultTemp = Join-Path $root "job.exit.tmp"

# UI Automation and GUI processes belong to an interactive desktop. A worker
# started through QGA is in session 0 and can race the logged-in worker for the
# same job files while being unable to provide valid desktop evidence.
$sessionId = (Get-Process -Id $PID).SessionId
if ($sessionId -eq 0) {
    Write-Error "windows-utm-agent requires an interactive session"
    exit 3
}

# Startup can be invoked more than once (login, manual recovery, provisioning).
# A named mutex makes that idempotent without killing unrelated PowerShell
# processes in the interactive test account.
$createdNew = $false
$mutex = [Threading.Mutex]::new($true, "Local\MiniConUtmAgent", [ref]$createdNew)
if (-not $createdNew) {
    exit 0
}

New-Item -ItemType Directory -Force -Path $root | Out-Null
foreach ($cell in @("win-aarch64", "win-x86_64")) {
    New-Item -ItemType Directory -Force -Path (Join-Path $root "$cell\target\debug\deps") | Out-Null
}

while ($true) {
    if ((Test-Path -LiteralPath $ready) -and
        (Test-Path -LiteralPath $pending)) {
        Remove-Item -LiteralPath $ready, $running, $log, $result, $logTemp, $resultTemp -Force -ErrorAction SilentlyContinue
        Move-Item -LiteralPath $pending -Destination $running -Force
        $exitCode = 1
        try {
            $output = & powershell.exe -NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File $running 2>&1
            $exitCode = $LASTEXITCODE
            if ($null -eq $exitCode) { $exitCode = 0 }
        } catch {
            $output = $_ | Out-String
            $exitCode = 1
        } finally {
            $output | Out-File -LiteralPath $logTemp -Encoding utf8
            Move-Item -LiteralPath $logTemp -Destination $log -Force
            [IO.File]::WriteAllText($resultTemp, [string]$exitCode)
            Move-Item -LiteralPath $resultTemp -Destination $result -Force
            Remove-Item -LiteralPath $running -Force -ErrorAction SilentlyContinue
        }
    }
    Start-Sleep -Milliseconds 250
}
