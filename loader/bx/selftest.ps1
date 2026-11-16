# Selftest for the packed minicon-bx product: structure, metadata, and real runs.
# Also proves Authenticode coexistence: a signed copy must stay readable and
# runnable, and a tampered signed copy must fail signature verification.
# Signing uses an ephemeral untrusted certificate via signtool when available;
# that stage is skipped (not failed) when signtool/openssl are absent.
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

# The footer need not be the last 24 bytes: an Authenticode certificate overlay
# may follow it. What must hold is that the packer still finds the table.
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
# CLI surface for agents: compile gate, execution budgets, stdin filter
$checkScript = Join-Path $env:TEMP "minicon-bx-check.bxs"
Set-Content -Path $checkScript -Value 'print("ran")' -Encoding ascii
$checkOut = (& $com check $checkScript 2>&1 | Out-String).Trim()
Check "check compiles without running" ($checkOut -match '^ok ' -and $checkOut -notmatch 'ran') "out=<$checkOut>"
Remove-Item $checkScript -Force -ErrorAction SilentlyContinue

$spin = Join-Path $env:TEMP "minicon-bx-spin.bxs"
Set-Content -Path $spin -Value 'while (true) { }' -Encoding ascii
& $com run --max-steps 200000 $spin 2>&1 | Out-Null
Check "run --max-steps stops a runaway script" ($LASTEXITCODE -eq 1) "exit=$LASTEXITCODE"
Remove-Item $spin -Force -ErrorAction SilentlyContinue

$pipedOut = ("abc" | & $com eval 'read("-")' 2>&1 | Out-String).Trim()
Check "read drains stdin through the packed file" ($pipedOut -eq "abc") "got <$pipedOut>"

$helpOut = (& $com help 2>&1 | Out-String)
Check "help lists the check command" ($helpOut -match 'bx check') "help is missing check"

foreach ($expected in @(0, 1, 3, 42)) {
    & $com eval "exit($expected)" 2>$null | Out-Null
    Check "exit $expected passes through" ($LASTEXITCODE -eq $expected) "got $LASTEXITCODE"
}

# --- Authenticode coexistence (L27): sign a copy, prove readability, run,
# --- and reject a tampered copy. Mechanism-only evidence, never G6.
$signtool = Get-Command signtool.exe -ErrorAction SilentlyContinue
if (-not $signtool) {
    $kits = Join-Path ${env:ProgramFiles(x86)} "Windows Kits\10\bin"
    if (Test-Path $kits) {
        $signtool = Get-ChildItem $kits -Recurse -Filter signtool.exe -ErrorAction SilentlyContinue |
            Where-Object { $_.FullName -match 'x64' } | Sort-Object FullName -Descending | Select-Object -First 1
    }
}
$openssl = Get-Command openssl -ErrorAction SilentlyContinue
if ($signtool -and $openssl) {
    $signtoolPath = if ($signtool.Source) { $signtool.Source } else { $signtool.FullName }
    $opensslPath = $openssl.Source
    $lab = Join-Path $tempDir "minicon-sign-selftest"
    New-Item -ItemType Directory -Force -Path $lab | Out-Null
    try {
        # openssl writes progress dots to stderr; under $ErrorActionPreference
        # =Stop PowerShell turns the first stderr line into a terminating
        # NativeCommandError, so relax to Continue around these two calls.
        $ErrorActionPreference = 'Continue'
        & $opensslPath req -x509 -newkey rsa:2048 -nodes -days 1 -sha256 `
            -subj "/O=MINICON_SELFTEST/CN=UNTRUSTED_TEST" `
            -addext 'extendedKeyUsage=codeSigning' `
            -keyout "$lab\key.pem" -out "$lab\cert.pem" 2>$null | Out-Null
        $ErrorActionPreference = 'Stop'
        if (-not (Test-Path "$lab\cert.pem")) { throw "openssl req produced no certificate" }
        $ErrorActionPreference = 'Continue'
        & $opensslPath pkcs12 -export -inkey "$lab\key.pem" -in "$lab\cert.pem" `
            -passout pass:test -out "$lab\cert.pfx" 2>$null | Out-Null
        $ErrorActionPreference = 'Stop'
        if (-not (Test-Path "$lab\cert.pfx")) { throw "openssl pkcs12 produced no keystore" }
        $signed = "$lab\signed.com"
        Copy-Item $com $signed -Force
        $signOut = & $signtoolPath sign /fd sha256 /f "$lab\cert.pfx" /p test $signed 2>&1 |
            Out-String
        Check "test signature applied" ($LASTEXITCODE -eq 0) "signtool rc=$LASTEXITCODE $signOut"

        $signedList = & $packer list $signed 2>&1 | Out-String
        Check "signed copy still readable" ($signedList -match 'win-x86_64') "list: $signedList"
        $signedEval = (& $signed eval "6*7" 2>&1 | Out-String).Trim()
        Check "signed copy still runs" ($signedEval -eq "42") "got <$signedEval>"

        $tampered = "$lab\tampered.com"
        $tamperedBytes = [IO.File]::ReadAllBytes($signed)
        # Flip one byte inside the embedded cell, not inside the signature.
        $tamperedBytes[1200] = $tamperedBytes[1200] -bxor 1
        [IO.File]::WriteAllBytes($tampered, $tamperedBytes)
        & $signtoolPath verify /pa $tampered *> $null
        Check "tampered signed copy fails verify" ($LASTEXITCODE -ne 0) "tampered verify rc=0"
    } finally {
        Remove-Item $lab -Recurse -Force -ErrorAction SilentlyContinue
    }
} else {
    Write-Output "  skip Authenticode coexistence (signtool/openssl not found)"
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
