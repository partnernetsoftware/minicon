# Misalignment fixture v3 (ASCII-only source; probe chars built from code
# points so PowerShell 5's codepage-sensitive parser cannot mangle them).
$sig = @'
using System;
using System.Runtime.InteropServices;
public static class VT {
  [DllImport("kernel32.dll")] public static extern IntPtr GetStdHandle(int n);
  [DllImport("kernel32.dll")] public static extern bool GetConsoleMode(IntPtr h, out uint m);
  [DllImport("kernel32.dll")] public static extern bool SetConsoleMode(IntPtr h, uint m);
  [DllImport("kernel32.dll")] public static extern bool SetConsoleOutputCP(uint cp);
  [DllImport("kernel32.dll")] public static extern bool SetConsoleCP(uint cp);
}
'@
Add-Type -TypeDefinition $sig
[void][VT]::SetConsoleOutputCP(65001)
[void][VT]::SetConsoleCP(65001)
[Console]::OutputEncoding = [Text.Encoding]::UTF8
$out = [VT]::GetStdHandle(-11)
$mode = 0
[void][VT]::GetConsoleMode($out, [ref]$mode)
[void][VT]::SetConsoleMode($out, $mode -bor 0x0004)

$w = [Console]::Out
$e = [char]27
function W([string]$s) { $w.Write($s); $w.Flush() }
function C([int]$cp) { [char]::ConvertFromUtf32($cp) }

$BOXH = C 0x2500   # EAW=A
$BOXV = C 0x2502   # EAW=A
$ELIP = C 0x2026   # EAW=A
$ARRW = C 0x2192   # EAW=A
$CYRT = C 0x0442   # EAW=A
$CJK1 = C 0x8BFB   # EAW=W (du2)
$CJK2 = C 0x53D6   # EAW=W (qu3)
$BULL = C 0x25CF   # EAW=A

W "$e[2J$e[H"

$probes = @(
  @{ label='boxh  '; run=($BOXH * 6) },
  @{ label='boxv  '; run=($BOXV + 'ab' + $BOXV) },
  @{ label='ellip '; run=($ELIP + 'xy') },
  @{ label='arrow '; run=($ARRW + 'xy') },
  @{ label='cyril '; run=($CYRT + 'ail') },
  @{ label='cjk   '; run=($CJK1 + $CJK2 + 'ab') },
  @{ label='bullet'; run=($BULL + 'xy') }
)
$row = 2
foreach ($p in $probes) {
  W "$e[$row;1H$e[K"
  W ($p.label + ':' + $p.run)
  W 'Z|'
  $row += 1
}

W "$e[10;1H$e[K"
W ("ow-box :" + ($BOXH * 6) + "rest")
W "$e[10;1H########------"
W "$e[11;1H$e[K"
W ("ow-cjk :" + $CJK1 + $CJK2 + "rest")
W "$e[11;1H#########"
W "$e[13;1HFIXTURE_DONE"
Start-Sleep -Seconds 3600
