param(
    [Parameter(Mandatory = $true)][string]$GuestRoot,
    [Parameter(Mandatory = $true)]
    [ValidateSet("status", "test", "throughput")]
    [string]$Mode,
    [switch]$Child
)

$ErrorActionPreference = "Stop"
$runner = Join-Path $GuestRoot "windows-runtime-qualify.ps1"
$target = Join-Path $GuestRoot "target"
$log = Join-Path $GuestRoot "$Mode.log"
$result = Join-Path $GuestRoot "$Mode.exit"

if ($Child) {
    $exitCode = 1
    try {
        & $runner -TargetDir $target -Mode $Mode *> $log
        $exitCode = $LASTEXITCODE
        if ($null -eq $exitCode) {
            $exitCode = 0
        }
    } catch {
        $_ | Out-String | Add-Content -Path $log
        $exitCode = 1
    } finally {
        Set-Content -Path $result -Value $exitCode -NoNewline
    }
    exit $exitCode
}

$interactiveUser = (Get-CimInstance Win32_ComputerSystem).UserName
if ([string]::IsNullOrWhiteSpace($interactiveUser)) {
    throw "no interactive Windows user is logged on"
}

Remove-Item -LiteralPath $log, $result -Force -ErrorAction SilentlyContinue
$cell = Split-Path $GuestRoot -Leaf
$taskName = "MiniConSix-$cell-$Mode"
$powershell = "$env:SystemRoot\System32\WindowsPowerShell\v1.0\powershell.exe"
$arguments = "-NoLogo -NoProfile -NonInteractive -ExecutionPolicy Bypass -File `"$PSCommandPath`" -GuestRoot `"$GuestRoot`" -Mode $Mode -Child"
$action = New-ScheduledTaskAction -Execute $powershell -Argument $arguments
$principal = New-ScheduledTaskPrincipal -UserId $interactiveUser -LogonType Interactive -RunLevel Limited
$task = New-ScheduledTask -Action $action -Principal $principal

try {
    Register-ScheduledTask -TaskName $taskName -InputObject $task -Force | Out-Null
    Start-ScheduledTask -TaskName $taskName
    $deadline = [DateTime]::UtcNow.AddMinutes(20)
    while (-not (Test-Path -LiteralPath $result)) {
        if ([DateTime]::UtcNow -ge $deadline) {
            throw "interactive Windows test task exceeded its 20-minute deadline"
        }
        Start-Sleep -Milliseconds 250
    }
    if (Test-Path -LiteralPath $log) {
        Get-Content -LiteralPath $log
    }
    $exitCode = [int](Get-Content -LiteralPath $result -Raw)
    exit $exitCode
} finally {
    Unregister-ScheduledTask -TaskName $taskName -Confirm:$false -ErrorAction SilentlyContinue
}
