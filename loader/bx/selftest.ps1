# Selftest for the packed minicon-bx product: structure, metadata, and real runs.
param([string]$Bx = $env:BX_NATIVE)
$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = Join-Path $here "..\.."
$dist = Join-Path $repo "loader\dist\rust"
$com = Join-Path $dist "minicon-bx-win-x86_64.com"
$packer = Join-Path $here "..\com-rs\target\release\com-pack.exe"
$fail = 0

function Check($name, $ok, $detail) {
    if ($ok) { Write-Output "  ok   $name" }
    else { $script:fail++; Write-Output "  FAIL $name - $detail" }
}

if (-not (Test-Path $com)) { throw "missing $com (run bx/scripts/build-cells.sh first)" }
if (-not (Test-Path $packer)) { throw "missing com-pack (cargo build in loader/com-rs)" }
$bytes = [IO.File]::ReadAllBytes($com)

Check "starts with a PE (MZ)" ($bytes[0] -eq 0x4D -and $bytes[1] -eq 0x5A) "no MZ header"
Check "plausible size" ($bytes.Length -gt 512KB -and $bytes.Length -lt 20MB) "size $($bytes.Length)"

$footer = New-Object byte[] 8
[Array]::Copy($bytes, $bytes.Length - 24, $footer, 0, 8)
Check "MINICOM1 footer" ([Text.Encoding]::ASCII.GetString($footer) -eq "MINICOM1") "bad footer"

$list = & $packer list $com 2>&1 | Out-String
Check "table names the cell" ($list -match 'win-x86_64') "list: $list"
Check "table carries metadata" ($list -match '__meta__') "list: $list"
Check "verify passes" ((& $packer verify $com 2>&1 | Out-String) -match 'ok') "verify failed"
$info = & $packer info $com 2>&1 | Out-String
Check "info names the product" ($info -match '"product":"minicon-bx"') "info: $info"
Check "info records a source sha" ($info -match '"source_sha":"[0-9a-f]{7,}"') "info: $info"
$shape = & $packer inspect $com 2>&1 | Out-String
Check "inspect names the launcher" ($shape -match 'launcher : PE') "inspect: $shape"
Check "inspect lists the cell" ($shape -match 'cell\s+: win-x86_64') "inspect: $shape"
Check "inspect shows metadata" ($shape -match 'metadata : \{') "inspect: $shape"

# A killed launcher can leave an extract behind; the next run sweeps only old ones.
$tempDir = [IO.Path]::GetTempPath()
$stale = Join-Path $tempDir "minicon-com-990001.exe"
$fresh = Join-Path $tempDir "minicon-com-990002.exe"
Set-Content -Path $stale -Value "stale" -Encoding ascii
(Get-Item $stale).LastWriteTime = (Get-Date).AddDays(-2)
Set-Content -Path $fresh -Value "fresh" -Encoding ascii
# The sweep is hourly-throttled; drop the stamp so this run actually sweeps.
Remove-Item (Join-Path $tempDir "minicon-com-reap.stamp") -Force -ErrorAction SilentlyContinue
& $com version 2>$null | Out-Null
Check "reaper removes a stale extract" (-not (Test-Path $stale)) "still present: $stale"
Check "reaper keeps a fresh extract" (Test-Path $fresh) "wrongly removed: $fresh"
Remove-Item $stale, $fresh -Force -ErrorAction SilentlyContinue

# A truncated file must be refused by the reader, not half-read.
$corrupt = Join-Path $tempDir "minicon-bx-corrupt.com"
$raw = [IO.File]::ReadAllBytes($com)
[IO.File]::WriteAllBytes($corrupt, $raw[0..($raw.Length - 200)])
& $packer verify $corrupt *> $null
Check "truncated container is refused" ($LASTEXITCODE -ne 0) "verify accepted a truncated file"
Remove-Item $corrupt -Force -ErrorAction SilentlyContinue

$version = (& $com version 2>&1 | Out-String).Trim()
Check "runs and reports bx" ($version -match '^bx \d+\.\d+\.\d+') "got <$version>"
$eval = (& $com eval "6*7" 2>&1 | Out-String).Trim()
Check "evaluates" ($eval -eq "42") "got <$eval>"

foreach ($expected in @(0, 1, 3, 42)) {
    & $com eval "exit($expected)" 2>$null | Out-Null
    Check "exit $expected passes through" ($LASTEXITCODE -eq $expected) "got $LASTEXITCODE"
}

if ($Bx -and (Test-Path $Bx)) {
    $probe = 'let r = exec(["' + $com.Replace('\', '\\') + '", "eval", "6*7"]); json([r.code, r.out])'
    $captured = (& $Bx eval $probe 2>&1 | Out-String).Trim()
    Check "nested exec captures stdout" ($captured -eq '[0,"42\n"]') "got <$captured>"
} else {
    Write-Output "  skip nested exec capture (pass -Bx <native bx> or set BX_NATIVE)"
}

$leftover = Get-ChildItem (Get-Location) -Filter "minicon-com-*" -ErrorAction SilentlyContinue
Check "no leftover extract" ($null -eq $leftover) "found $($leftover.Count)"

if ($fail -eq 0) { Write-Output "PASS"; exit 0 }
Write-Output "FAIL $fail check(s)"
exit 1
