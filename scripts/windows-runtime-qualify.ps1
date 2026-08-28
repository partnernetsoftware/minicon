param(
    [Parameter(Mandatory = $true)][string]$TargetDir,
    [Parameter(Mandatory = $true)]
    [ValidateSet("status", "logic", "test", "throughput", "console-agent")]
    [string]$Mode
)

$ErrorActionPreference = "Stop"
$env:AGENTERM_NO_ACTIVATE = "1"
$manifestPath = Join-Path $TargetDir "test-manifest.json"
if (-not (Test-Path -LiteralPath $manifestPath)) {
    throw "missing exact-artifact test manifest: $manifestPath"
}
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$profile = if ($Mode -eq "throughput") { "release-fast" } else { "debug" }
if ($manifest.schema -eq 2) {
    $profileManifest = $manifest.profiles.$profile
    if ($null -eq $profileManifest) {
        throw "exact-artifact manifest has no profile: $profile"
    }
} else {
    # Schema 1 is the compatibility contract used by existing UTM payloads.
    $profile = "debug"
    $profileManifest = $manifest
}
$depsDir = Join-Path $TargetDir "$profile\deps"
$product = (Resolve-Path -LiteralPath (Join-Path $TargetDir "$profile\$($profileManifest.product)")).Path
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
    $name = $profileManifest.tests.$Prefix
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
    if ($Prefix -eq "minicon_console_agent" -and -not [string]::IsNullOrWhiteSpace($env:MINICON_WINDOWS_CONSOLE_AGENT_FILTER)) {
        $arguments = @($env:MINICON_WINDOWS_CONSOLE_AGENT_FILTER, "--exact") + $arguments
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
    "console-agent" {
        Invoke-Status
        Invoke-Test "minicon_console_agent"
    }
}
