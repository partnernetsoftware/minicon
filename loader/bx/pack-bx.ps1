# Pack minicon-bx.com the same way loader/pack.sh packs minicon.com:
# one cosmocc trampoline with a ZIP overlay of per-host cells.
#
# Differences from loader/pack.sh, and why:
#   * No C compiler here: the trampoline is copied byte-for-byte from an
#     already-packed minicon.com (the released asset on disk). Its payload leaf
#     names are fixed ("minicon.exe" / "minicon"), so bx ships under those names.
#   * No `zip` binary: the overlay is built with .NET and its offsets are
#     rebased onto the trampoline, which is exactly what `zip -A` does.
#
# usage:  pwsh -File loader/bx/pack-bx.ps1 [-Source <packed minicon.com>] [-Bx <bx.exe>] [-Out <path>]
param(
    [string]$Source = "",
    [string]$Bx = "",
    [string]$Out = ""
)

$ErrorActionPreference = "Stop"
$here = Split-Path -Parent $MyInvocation.MyCommand.Path
$repo = (Resolve-Path (Join-Path $here "..\..")).Path

function Find-PackedCom {
    $candidates = Get-ChildItem (Join-Path $repo "target-six") -Recurse -File -Filter "minicon.com" -ErrorAction SilentlyContinue |
        Sort-Object Length
    if ($candidates.Count -eq 0) { throw "no packed minicon.com under target-six to borrow a trampoline from" }
    return $candidates[0].FullName
}

if ($Source -eq "") { $Source = Find-PackedCom }
if ($Bx -eq "") { $Bx = Join-Path $repo "..\..\deepseek-harness\default-workspace\bx\target\release\bx.exe" }
if ($Out -eq "") {
    $dist = Join-Path $here "dist"
    New-Item -ItemType Directory -Path $dist -Force | Out-Null
    $Out = Join-Path $dist "minicon-bx.com"
}
if (-not (Test-Path $Source)) { throw "trampoline source not found: $Source" }
if (-not (Test-Path $Bx)) { throw "bx payload not found: $Bx (build it: cargo build --release)" }

$src = [IO.File]::ReadAllBytes((Resolve-Path $Source))
if ([Text.Encoding]::ASCII.GetString($src, 0, 7) -ne "MZqFpD=") { throw "not an APE: $Source" }

# --- 1. Locate the ZIP overlay inside the trampoline -----------------------
$eocd = -1
for ($i = $src.Length - 22; $i -ge [Math]::Max(0, $src.Length - 70000); $i--) {
    if ($src[$i] -eq 0x50 -and $src[$i + 1] -eq 0x4b -and $src[$i + 2] -eq 0x05 -and $src[$i + 3] -eq 0x06) { $eocd = $i; break }
}
if ($eocd -lt 0) { throw "no ZIP end-of-central-directory in $Source" }
$entryCount = [BitConverter]::ToUInt16($src, $eocd + 10)
$cdOff = [BitConverter]::ToUInt32($src, $eocd + 16)
$p = [int]$cdOff
$minLocal = [int]::MaxValue
for ($k = 0; $k -lt $entryCount; $k++) {
    if ($src[$p] -ne 0x50 -or $src[$p + 1] -ne 0x4b -or $src[$p + 2] -ne 0x01 -or $src[$p + 3] -ne 0x02) {
        throw "bad central-directory signature at $p"
    }
    $nameLen = [BitConverter]::ToUInt16($src, $p + 28)
    $extraLen = [BitConverter]::ToUInt16($src, $p + 30)
    $cmtLen = [BitConverter]::ToUInt16($src, $p + 32)
    $localOff = [BitConverter]::ToUInt32($src, $p + 42)
    if ($localOff -lt $minLocal) { $minLocal = [int]$localOff }
    $p += 46 + $nameLen + $extraLen + $cmtLen
}
if ($minLocal -le 0 -or $minLocal -ge $src.Length) { throw "implausible first ZIP entry offset $minLocal" }

$trampoline = New-Object byte[] $minLocal
[Array]::Copy($src, 0, $trampoline, 0, $minLocal)
Write-Output ("[bx-pack] trampoline {0:N0} B from {1}" -f $minLocal, (Split-Path -Leaf $Source))

# --- 2. Build the cells overlay -------------------------------------------
$cells = @(
    @{ Path = "cells/win-x86_64/minicon.exe"; File = $Bx }
)
# A Linux cell is packed too when a static build is available; the trampoline
# picks it up on lnx-x86_64 and fails with "missing payload" elsewhere.
$linux = Join-Path (Split-Path -Parent (Split-Path -Parent $Bx)) "x86_64-unknown-linux-musl\release\bx"
if (Test-Path $linux) {
    $cells += @{ Path = "cells/lnx-x86_64/minicon"; File = $linux }
}

$ms = New-Object IO.MemoryStream
$zip = New-Object IO.Compression.ZipArchive($ms, [IO.Compression.ZipArchiveMode]::Create, $true)
foreach ($dir in @("cells/", "cells/win-x86_64/", "cells/lnx-x86_64/")) {
    $zip.CreateEntry($dir) | Out-Null
}
foreach ($cell in $cells) {
    $entry = $zip.CreateEntry($cell.Path, [IO.Compression.CompressionLevel]::Optimal)
    $entry.LastWriteTime = [DateTimeOffset]::Now
    $s = $entry.Open()
    $bytes = [IO.File]::ReadAllBytes($cell.File)
    $s.Write($bytes, 0, $bytes.Length)
    $s.Dispose()
    Write-Output ("[bx-pack] cell {0} {1:N0} B <- {2}" -f $cell.Path, $bytes.Length, (Split-Path -Leaf $cell.File))
}
$zip.Dispose()
$overlay = $ms.ToArray()
$ms.Dispose()

# --- 3. Rebase the overlay onto the trampoline (zip -A) --------------------
$base = [uint32]$minLocal
$eocd2 = -1
for ($i = $overlay.Length - 22; $i -ge 0; $i--) {
    if ($overlay[$i] -eq 0x50 -and $overlay[$i + 1] -eq 0x4b -and $overlay[$i + 2] -eq 0x05 -and $overlay[$i + 3] -eq 0x06) { $eocd2 = $i; break }
}
if ($eocd2 -lt 0) { throw "built overlay has no EOCD" }
$n2 = [BitConverter]::ToUInt16($overlay, $eocd2 + 10)
$cd2 = [BitConverter]::ToUInt32($overlay, $eocd2 + 16)
$p = [int]$cd2
for ($k = 0; $k -lt $n2; $k++) {
    if ($overlay[$p] -ne 0x50 -or $overlay[$p + 1] -ne 0x4b -or $overlay[$p + 2] -ne 0x01 -or $overlay[$p + 3] -ne 0x02) {
        throw "built overlay has a bad central-directory signature at $p"
    }
    $nameLen = [BitConverter]::ToUInt16($overlay, $p + 28)
    $extraLen = [BitConverter]::ToUInt16($overlay, $p + 30)
    $cmtLen = [BitConverter]::ToUInt16($overlay, $p + 32)
    $localOff = [BitConverter]::ToUInt32($overlay, $p + 42)
    $newLocal = [BitConverter]::GetBytes($localOff + $base)
    [Array]::Copy($newLocal, 0, $overlay, $p + 42, 4)
    $p += 46 + $nameLen + $extraLen + $cmtLen
}
$newCd = [BitConverter]::GetBytes($cd2 + $base)
[Array]::Copy($newCd, 0, $overlay, $eocd2 + 16, 4)

# --- 4. Emit --------------------------------------------------------------
$outBytes = New-Object byte[] ($trampoline.Length + $overlay.Length)
[Array]::Copy($trampoline, 0, $outBytes, 0, $trampoline.Length)
[Array]::Copy($overlay, 0, $outBytes, $trampoline.Length, $overlay.Length)
[IO.File]::WriteAllBytes($Out, $outBytes)
Write-Output ("[bx-pack] wrote {0} ({1:N0} B)" -f $Out, $outBytes.Length)
Write-Output ("[bx-pack] run it: {0} eval `"1+1`"" -f $Out)
