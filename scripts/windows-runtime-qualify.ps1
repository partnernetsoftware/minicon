param(
    [Parameter(Mandatory = $true)][string]$TargetDir,
    [Parameter(Mandatory = $true)]
    [ValidateSet("status", "logic", "test", "throughput")]
    [string]$Mode
)

$ErrorActionPreference = "Stop"
$env:AGENTERM_NO_ACTIVATE = "1"
$depsDir = Join-Path $TargetDir "debug\deps"
$manifestPath = Join-Path $TargetDir "test-manifest.json"
if (-not (Test-Path -LiteralPath $manifestPath)) {
    throw "missing exact-artifact test manifest: $manifestPath"
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$product = Join-Path $TargetDir "debug\$($manifest.product)"
$env:MINICON_TEST_BINARY = $product

function Invoke-NativeWait([string]$Path, [string[]]$Arguments, [switch]$Quiet) {
    $token = [Guid]::NewGuid().ToString("N")
    $stdout = Join-Path $env:TEMP "minicon-$token.out"
    $stderr = Join-Path $env:TEMP "minicon-$token.err"
    try {
        $process = Start-Process -FilePath $Path -ArgumentList $Arguments -Wait -PassThru -RedirectStandardOutput $stdout -RedirectStandardError $stderr
        if (-not $Quiet) {
            if (Test-Path -LiteralPath $stdout) {
                Get-Content -LiteralPath $stdout | Write-Host
            }
            if (Test-Path -LiteralPath $stderr) {
                Get-Content -LiteralPath $stderr | Write-Host
            }
        }
        return $process.ExitCode
    } finally {
        Remove-Item -LiteralPath $stdout, $stderr -Force -ErrorAction SilentlyContinue
    }
}

function Find-TestBinary([string]$Prefix) {
    $name = $manifest.tests.$Prefix
    if ([string]::IsNullOrWhiteSpace($name)) {
        throw "exact-artifact manifest has no harness for $Prefix"
    }
    $candidatePath = Join-Path $depsDir $name
    if (-not (Test-Path -LiteralPath $candidatePath)) {
        throw "manifest-selected Rust test harness is missing: $candidatePath"
    }
    $probeExit = Invoke-NativeWait $candidatePath @("--list") -Quiet
    if ($probeExit -ne 0) {
        throw "manifest-selected Rust test harness is not runnable: $candidatePath"
    }
    return $candidatePath
}

function Invoke-Test([string]$Prefix, [switch]$Ignored) {
    $testBinary = Find-TestBinary $Prefix
    Write-Host "[windows-runtime] RUN $([IO.Path]::GetFileName($testBinary))"
    $arguments = @("--test-threads=1", "--nocapture")
    if ($Ignored) {
        $arguments = @("--ignored") + $arguments
    }
    $testExit = Invoke-NativeWait $testBinary $arguments
    if ($testExit -ne 0) {
        throw "$Prefix failed with exit code $testExit"
    }
}

function Invoke-Status {
    $statusExit = Invoke-NativeWait $product @("--status")
    if ($statusExit -ne 0) {
        throw "minicon --status failed with exit code $statusExit"
    }
}

try {
    switch ($Mode) {
        "status" {
            Invoke-Status
        }
        "logic" {
            Invoke-Status
            Invoke-Test "minicon"
            Invoke-Test "minicon_core"
            # Alignment is a source/PRD registry court owned on the build host; it
            # intentionally does not require a source checkout in runtime guests.
            Invoke-Test "minicon_load_portability"
        }
        "test" {
            Invoke-Status
            Invoke-Test "minicon"
            Invoke-Test "minicon_core"
            # Alignment is a source/PRD registry court owned on the build host; it
            # intentionally does not require a source checkout in runtime guests.
            Invoke-Test "minicon_load_portability"
            Invoke-Test "minicon_console_agent"
            Invoke-Test "minicon_control"
            Invoke-Test "minicon_blackbox"
        }
        "throughput" {
            Invoke-Test "minicon_throughput" -Ignored
        }
    }
} catch {
    [Console]::Error.WriteLine(($_ | Out-String))
    exit 1
}

# Start-Process exposes its code through the Process object and does not own
# PowerShell's ambient $LASTEXITCODE. Publish an explicit success so the UTM
# job wrapper cannot inherit a stale code from an earlier native probe.
exit 0
