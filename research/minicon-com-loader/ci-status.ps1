# Windows runner smoke: --version, unknown-flag nonzero, --status. Never compiles.
param(
    [Parameter(Mandatory = $true)][string]$Binary,
    [string]$Receipt = ""
)
$ErrorActionPreference = "Stop"

function Invoke-Captured([string]$Path, [string[]]$Arguments) {
    $stem = Join-Path $env:TEMP ([guid]::NewGuid().ToString("N"))
    $stdout = "$stem.out"
    $stderr = "$stem.err"
    $process = Start-Process -FilePath $Path -ArgumentList $Arguments -Wait -PassThru `
        -RedirectStandardOutput $stdout -RedirectStandardError $stderr
    $out = if (Test-Path -LiteralPath $stdout) { Get-Content -LiteralPath $stdout -Raw } else { "" }
    $err = if (Test-Path -LiteralPath $stderr) { Get-Content -LiteralPath $stderr -Raw } else { "" }
    return [pscustomobject]@{ ExitCode = $process.ExitCode; StdOut = $out; StdErr = $err }
}

if (-not (Test-Path -LiteralPath $Binary)) { throw "missing $Binary" }

$want = $null
if ($Receipt -and (Test-Path -LiteralPath $Receipt)) {
    $want = (Get-Content -LiteralPath $Receipt -Raw | ConvertFrom-Json).product_version
}

$ver = Invoke-Captured $Binary @("--version")
if ($ver.ExitCode -ne 0) { throw "--version exit $($ver.ExitCode) $($ver.StdErr)" }
$verLine = ($ver.StdOut -split "`r?`n")[0].Trim()
Write-Host "version_line=$verLine"
if ($want -and $verLine -ne "minicon $want") { throw "version '$verLine' != minicon $want" }

$unk = Invoke-Captured $Binary @("--definitely-not-a-flag")
Write-Host "unknown_flag_exit=$($unk.ExitCode)"
if ($unk.ExitCode -eq 0) { throw "unknown flag must be nonzero" }
$unkText = "$($unk.StdOut)$($unk.StdErr)"
if ($unkText -notmatch "unknown argument") { throw "unknown flag did not mention unknown argument" }

$st = Invoke-Captured $Binary @("--status")
if ($st.StdErr) { Write-Host $st.StdErr }
if ($st.ExitCode -ne 0) { throw "minicon.com --status exit $($st.ExitCode)" }
Write-Host $st.StdOut
if ($st.StdOut -notmatch '(?m)^minicon ') { throw "missing minicon version line" }
if ($st.StdOut -notmatch 'pty backend') { throw "missing pty backend" }
if ($st.StdOut -notmatch 'conpty') { throw "expected conpty on Windows" }
Write-Host "PASS windows version+passthrough+status"
