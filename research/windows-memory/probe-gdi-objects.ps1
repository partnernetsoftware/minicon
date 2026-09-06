# Sample GDI/USER/handle counts for one PID. Research-only; no product change.
# Usage: powershell -File probe-gdi-objects.ps1 -TargetPid 1234
param(
    [Parameter(Mandatory = $true)]
    [int]$TargetPid
)

$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class GuiRes {
    public const int GR_GDIOBJECTS = 0;
    public const int GR_USEROBJECTS = 1;
    [DllImport("user32.dll")]
    public static extern uint GetGuiResources(IntPtr hProcess, int flags);
}
"@

$p = Get-Process -Id $TargetPid
$gdi = [GuiRes]::GetGuiResources($p.Handle, [GuiRes]::GR_GDIOBJECTS)
$user = [GuiRes]::GetGuiResources($p.Handle, [GuiRes]::GR_USEROBJECTS)
Write-Output ("pid={0} name={1} ws={2} vm={3} handles={4} gdi={5} user={6} threads={7}" -f `
    $p.Id, $p.Name, $p.WorkingSet64, $p.VirtualMemorySize64, $p.HandleCount, $gdi, $user, $p.Threads.Count)
