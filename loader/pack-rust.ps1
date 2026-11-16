# Pack MiniCon's cells with the Rust launcher instead of the cosmocc trampoline.
#
# This is the cosmocc-free path: loader/com-rs builds `com-trampoline` for the
# host and `com-pack` appends the matching cell plus a MINICOM1 table. The child
# inherits stdio and its exit code is passed through verbatim, so a packed file
# behaves like the cell for redirection, pipes and `$LASTEXITCODE`.
#
# Trade-off, stated once: cosmocc's trampoline is a polyglot, so ONE asset runs on
# six OSes. A compiled launcher is per-OS, so this script produces one file per
# platform (that is what `-Platforms` selects). The six-OS `minicon.com` asset
# still needs the polyglot trampoline (loader/pack.sh) or the APE! loader in
# systems/ape.
#
# usage:
#   pwsh -File loader/pack-rust.ps1 -Cells <dir> [-Platforms win-x86_64,lnx-x86_64] [-OutDir loader/dist]
param(
    [Parameter(Mandatory = $true)][string]$Cells,
    [string[]]$Platforms = @("win-x86_64", "lnx-x86_64"),
    [string]$OutDir = "",
    [string]$Name = "minicon"
)

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = (Resolve-Path (Join-Path $here "..")).Path
$comRs = Join-Path $here "com-rs"
if ($OutDir -eq "") { $OutDir = Join-Path $here "dist" }
$OutDir = [IO.Path]::GetFullPath($OutDir)
New-Item -ItemType Directory -Path $OutDir -Force | Out-Null

if (-not (Test-Path $Cells)) { throw "cells directory not found: $Cells" }

# cell os prefix -> payload leaf name (loader.c's rule, kept)
function Get-Leaf([string]$platform) {
    if ($platform.StartsWith("win-")) { return "minicon.exe" }
    return "minicon"
}

# platform -> cargo target for the launcher; $null means "host"
function Get-Target([string]$platform) {
    switch ($platform) {
        "win-x86_64" { return $null }
        "lnx-x86_64" { return "x86_64-unknown-linux-musl" }
        "lnx-aarch64" { return "aarch64-unknown-linux-musl" }
        default { return "" }   # "" = unsupported here
    }
}

Push-Location $comRs
try {
    $packer = Join-Path $comRs "target\release\com-pack.exe"
    if (-not (Test-Path $packer)) {
        cargo build --release --bin com-pack
        if ($LASTEXITCODE -ne 0) { throw "com-pack build failed" }
    }

    $built = @{}
    $made = 0
    foreach ($platform in $Platforms) {
        $target = Get-Target $platform
        if ($target -eq "") {
            Write-Output "[pack-rust] skip $platform (no launcher target configured for this host)"
            continue
        }
        $cellDir = Join-Path $Cells $platform
        $leaf = Get-Leaf $platform
        $cell = Join-Path $cellDir $leaf
        if (-not (Test-Path $cell)) {
            Write-Output "[pack-rust] skip $platform (missing $cell)"
            continue
        }

        $key = if ($null -eq $target) { "host" } else { $target }
        if (-not $built.ContainsKey($key)) {
            if ($null -eq $target) {
                Write-Output "[pack-rust] building launcher (host)"
                $env:RUSTFLAGS = $null
                cargo build --release --bin com-trampoline
                $built[$key] = Join-Path $comRs "target\release\com-trampoline.exe"
            } else {
                Write-Output "[pack-rust] building launcher for $target"
                $env:RUSTFLAGS = "-C linker=rust-lld -C link-self-contained=yes"
                cargo build --release --bin com-trampoline --target $target
                $built[$key] = Join-Path $comRs "target\$target\release\com-trampoline"
                $env:RUSTFLAGS = $null
            }
            if ($LASTEXITCODE -ne 0) { throw "launcher build failed for $platform" }
        }

        # One platform keeps the clean name (minicon-bx.com); a matrix run carries the platform.
        $suffix = if ($Platforms.Count -eq 1) { "" } else { "-$platform" }
        $out = Join-Path $OutDir ("{0}{1}.com" -f $Name, $suffix)
        & $packer pack --trampoline $built[$key] --out $out "$platform=$cell"
        if ($LASTEXITCODE -ne 0) { throw "pack failed for $platform" }
        $made++
    }
    Write-Output "[pack-rust] wrote $made artifact(s) to $OutDir"
} finally {
    $env:RUSTFLAGS = $null
    Pop-Location
}
