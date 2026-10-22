# Mouse court fixture v2: request SGR mouse reporting, then read RAW console
# input records (ReadConsoleInputW) so the court sees mouse delivery whether
# the backend presents it as MOUSE_EVENT records or as VT key characters.
$sig = @'
using System;
using System.Runtime.InteropServices;
public static class VT {
  [StructLayout(LayoutKind.Explicit)]
  public struct INPUT_RECORD {
    [FieldOffset(0)] public ushort EventType;
    // KEY_EVENT_RECORD
    [FieldOffset(4)] public int bKeyDown;
    [FieldOffset(14)] public char UnicodeChar;
    // MOUSE_EVENT_RECORD
    [FieldOffset(4)] public short MouseX;
    [FieldOffset(6)] public short MouseY;
    [FieldOffset(8)] public uint ButtonState;
    [FieldOffset(16)] public uint EventFlags;
  }
  [DllImport("kernel32.dll")] public static extern IntPtr GetStdHandle(int n);
  [DllImport("kernel32.dll")] public static extern bool GetConsoleMode(IntPtr h, out uint m);
  [DllImport("kernel32.dll")] public static extern bool SetConsoleMode(IntPtr h, uint m);
  [DllImport("kernel32.dll")] public static extern bool SetConsoleOutputCP(uint cp);
  [DllImport("kernel32.dll", CharSet=CharSet.Unicode)]
  public static extern bool ReadConsoleInputW(IntPtr h, [Out] INPUT_RECORD[] buf, uint n, out uint read);
}
'@
Add-Type -TypeDefinition $sig
[void][VT]::SetConsoleOutputCP(65001)
$out = [VT]::GetStdHandle(-11)
$inh = [VT]::GetStdHandle(-10)
$m = 0
[void][VT]::GetConsoleMode($out, [ref]$m)
[void][VT]::SetConsoleMode($out, $m -bor 0x0004)
# VT input (0x200) + mouse input (0x10) + extended flags (0x80): what real
# mouse-aware TUIs set (quick-edit off so the console does not eat clicks).
[void][VT]::SetConsoleMode($inh, 0x0290)

$w = [Console]::Out
$e = [char]27
$w.Write("$e[2J$e[H")
$w.Write("$e[?1000h$e[?1006h")
$w.Write("MOUSE_COURT_READY`r`n")
$w.Flush()

$recs = New-Object VT+INPUT_RECORD[] 32
$keys = ""
while ($true) {
  $n = 0
  if (-not [VT]::ReadConsoleInputW($inh, $recs, 32, [ref]$n)) { break }
  for ($i = 0; $i -lt $n; $i++) {
    $r = $recs[$i]
    if ($r.EventType -eq 2) {
      $w.Write(("RX:MOUSE x={0} y={1} buttons=0x{2:x} flags=0x{3:x}`r`n" -f $r.MouseX, $r.MouseY, $r.ButtonState, $r.EventFlags))
    } elseif ($r.EventType -eq 1 -and $r.bKeyDown -ne 0) {
      $c = $r.UnicodeChar
      if ([int]$c -eq 27) { $keys += "<ESC>" }
      elseif ([int]$c -ge 32 -and [int]$c -lt 127) { $keys += $c }
      elseif ([int]$c -ne 0) { $keys += ("<{0}>" -f [int]$c) }
      if ($c -eq 'q') {
        $w.Write("RX:KEYS " + $keys + "`r`nMOUSE_COURT_END`r`n"); $w.Flush()
        Start-Sleep -Seconds 3600
      }
    }
  }
  if ($keys.Length -gt 0 -and $keys.Contains('M') -or $keys.Contains('m')) {
    $w.Write("RX:KEYS " + $keys + "`r`n"); $keys = ""
  }
  $w.Flush()
}
