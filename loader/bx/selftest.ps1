# Selftest for the packed minicon-bx.com: structure, container, and real runs.
param([string]$Bx = $env:BX_NATIVE)
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$bx = $Bx
$com = Join-Path $here "dist\minicon-bx.com"
$fail = 0

function Check($name, $ok, $detail) {
    if ($ok) { Write-Output "  ok   $name" }
    else { $script:fail++; Write-Output "  FAIL $name - $detail" }
}

if (-not (Test-Path $com)) { throw "missing $com (run pack-bx.ps1 first)" }
$bytes = [IO.File]::ReadAllBytes($com)

Check "starts with a PE (MZ)" ($bytes[0] -eq 0x4D -and $bytes[1] -eq 0x5A) "no MZ header"
Check "plausible size" ($bytes.Length -gt 512KB -and $bytes.Length -lt 20MB) "size $($bytes.Length)"

# Container table: MINICOM1 footer at EOF.
$footer = New-Object byte[] 8
[Array]::Copy($bytes, $bytes.Length - 24, $footer, 0, 8)
Check "MINICOM1 footer" ([Text.Encoding]::ASCII.GetString($footer) -eq "MINICOM1") "bad footer"

$packer = Join-Path $here "..\com-rs\target\release\com-pack.exe"
$list = (& $packer list $com 2>&1 | Out-String).Trim()
Check "reports its cell" ($list -match 'win-x86_64') "list said <$list>"

$verified = (& $packer verify $com 2>&1 | Out-String).Trim()
Check "cell architecture matches its key" ($LASTEXITCODE -eq 0) $verified

$version = (& $com version 2>&1 | Out-String).Trim()
Check "runs and reports bx" ($version -match '^bx \d+\.\d+\.\d+') "got <$version>"
$eval = (& $com eval "6*7" 2>&1 | Out-String).Trim()
Check "evaluates" ($eval -eq "42") "got <$eval>"

# Exit codes are the launcher's contract: exact, never shifted.
foreach ($expected in @(0, 1, 3, 42)) {
    & $com eval "exit($expected)" 2>$null | Out-Null
    Check "exit $expected passes through" ($LASTEXITCODE -eq $expected) "got $LASTEXITCODE"
}

# A parent that pipes us must still capture the payload's output.
if ($bx -and (Test-Path $bx)) {
    $probe = 'let r = exec(["' + $com.Replace('\', '\\') + '", "eval", "6*7"]); json([r.code, r.out])'
    $captured = (& $bx eval $probe 2>&1 | Out-String).Trim()
    Check "nested exec captures stdout" ($captured -eq '[0,"42\n"]') "got <$captured>"
} else {
    Write-Output "  skip nested exec capture (no native bx given via BX_NATIVE)"
}

# The launcher must not leave its extracted payload behind.
$leftover = Get-ChildItem (Get-Location) -Filter "minicon-com-*.exe" -ErrorAction SilentlyContinue
Check "no leftover extract" ($null -eq $leftover) "found $($leftover.Count)"

# Other packed artifacts in dist must verify too, when present.
foreach ($other in Get-ChildItem (Join-Path $here "dist") -Filter "*.com" -File -ErrorAction SilentlyContinue) {
    if ($other.FullName -eq $com) { continue }
    $out = (& $packer verify $other.FullName 2>&1 | Out-String).Trim()
    Check "verify $($other.Name)" ($LASTEXITCODE -eq 0) $out
}

if ($fail -eq 0) { Write-Output "PASS"; exit 0 }
Write-Output "FAIL $fail check(s)"
exit 1
