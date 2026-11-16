# Report the composition of a PE binary: sections, imports, totals.
# usage: pwsh -File bx/tools/pe-report.ps1 <path-to-exe>
param([Parameter(Mandatory = $true)][string]$Path)

$b = [System.IO.File]::ReadAllBytes((Resolve-Path $Path))
function U16($o) { [BitConverter]::ToUInt16($b, $o) }
function U32($o) { [BitConverter]::ToUInt32($b, $o) }

$pe = [BitConverter]::ToInt32($b, 0x3C)
if ([BitConverter]::ToUInt32($b, $pe) -ne 0x00004550) { throw "not a PE file" }

$machine = U16 ($pe + 4)
$nsec = U16 ($pe + 6)
$stamp = [BitConverter]::ToUInt32($b, $pe + 8)
$optSize = U16 ($pe + 20)
$opt = $pe + 24
$magic = U16 $opt
$is64 = ($magic -eq 0x20B)
$ddOff = $opt + $(if ($is64) { 112 } else { 96 })

"file        : $Path"
"bytes       : {0:N0} ({1:N1} KiB)" -f $b.Length, ($b.Length / 1KB)
"machine     : 0x{0:X4} ({1})" -f $machine, $(if ($machine -eq 0x8664) { "x86-64" } else { "?" })
"format      : {0}" -f $(if ($is64) { "PE32+" } else { "PE32" })
"build time  : {0:u}" -f ([DateTimeOffset]::FromUnixTimeSeconds($stamp).UtcDateTime)
""

$secStart = $opt + $optSize
$totalVirtual = 0
$totalRaw = 0
$rows = @()
for ($i = 0; $i -lt $nsec; $i++) {
    $s = $secStart + $i * 40
    $name = ([Text.Encoding]::ASCII.GetString($b, $s, 8)).TrimEnd([char]0)
    $vsize = U32 ($s + 8)
    $vaddr = U32 ($s + 12)
    $rawsize = U32 ($s + 16)
    $rawptr = U32 ($s + 20)
    $chars = U32 ($s + 36)
    $totalVirtual += $vsize
    $totalRaw += $rawsize
    $flags = @()
    if ($chars -band 0x20000000) { $flags += "X" }
    if ($chars -band 0x40000000) { $flags += "R" }
    if ($chars -band 0x80000000) { $flags += "W" }
    $rows += [pscustomobject]@{
        Section = $name; Virtual = $vsize; Raw = $rawsize
        Share   = if ($b.Length) { $rawsize * 100.0 / $b.Length } else { 0 }
        Flags   = ($flags -join "")
        RawPtr  = $rawptr; VAddr = $vaddr
    }
}

$rows | Format-Table -AutoSize @(
    @{ Label = "section"; Expression = { $_.Section } },
    @{ Label = "raw bytes"; Expression = { "{0,9:N0}" -f $_.Raw } },
    @{ Label = "share"; Expression = { "{0,5:N1}%" -f $_.Share } },
    @{ Label = "virt"; Expression = { "{0,9:N0}" -f $_.Virtual } },
    @{ Label = "flags"; Expression = { $_.Flags } }
) | Out-String | Write-Output

"section raw total : {0:N0} bytes (file has {1:N0} bytes of headers/slack)" -f $totalRaw, ($b.Length - $totalRaw)
""

# imports
$impRva = U32 ($ddOff + 8)
$impSize = U32 ($ddOff + 12)
if ($impRva -ne 0) {
    $impSec = $rows | Where-Object { $impRva -ge $_.VAddr -and $impRva -lt ($_.VAddr + $_.Virtual) } | Select-Object -First 1
    if ($impSec) {
        function RvaToOff($rva) {
            $s = $rows | Where-Object { $rva -ge $_.VAddr -and $rva -lt ($_.VAddr + $_.Virtual) } | Select-Object -First 1
            if (-not $s) { return -1 }
            return $rva - $s.VAddr + $s.RawPtr
        }
        $off = RvaToOff $impRva
        $dlls = @()
        for ($j = 0; $j -lt 64; $j++) {
            $d = $off + $j * 20
            $nameRva = U32 ($d + 12)
            if ($nameRva -eq 0) { break }
            $nameOff = RvaToOff $nameRva
            if ($nameOff -lt 0) { break }
            $end = $nameOff
            while ($end -lt $b.Length -and $b[$end] -ne 0) { $end++ }
            $dlls += [Text.Encoding]::ASCII.GetString($b, $nameOff, $end - $nameOff)
        }
        "imports     : $($dlls -join ', ')"
    }
}
""
