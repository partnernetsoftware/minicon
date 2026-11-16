# bx test suite launcher.
#
# The cases live in tests/suite.bxs and run on bx itself — that is the point of
# the suite: if bx cannot run its own tests, it is not the tool it claims to be.
# This file only exists so `pwsh -File tests/run.ps1` keeps working.
$root = Split-Path -Parent $PSScriptRoot
$bx = Join-Path $root "target\release\bx.exe"
if (-not (Test-Path $bx)) {
    throw "bx is not built yet; run: cargo build --release (in $root)"
}
& $bx run (Join-Path $PSScriptRoot "suite.bxs")
exit $LASTEXITCODE
