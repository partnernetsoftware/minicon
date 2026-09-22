# Sample live owners vs heap/WS for one PID. Research-only; no product change.
# Usage: powershell -File probe-gdi-objects.ps1 -TargetPid 1234 -Phase idle
param(
    [Parameter(Mandatory = $true)]
    [int]$TargetPid,
    [string]$Phase = "sample"
)

$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class LiveOwners {
    public const int GR_GDIOBJECTS = 0;
    public const int GR_USEROBJECTS = 1;
    public const uint THREAD_QUERY_LIMITED_INFORMATION = 0x0800;
    [DllImport("user32.dll")]
    public static extern uint GetGuiResources(IntPtr hProcess, int flags);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr OpenThread(uint access, bool inherit, uint tid);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", CharSet = CharSet.Unicode, EntryPoint = "GetThreadDescription")]
    public static extern int GetThreadDescription(IntPtr thread, out IntPtr description);
    [DllImport("kernel32.dll")]
    public static extern IntPtr LocalFree(IntPtr memory);
    public static string ThreadName(uint tid) {
        IntPtr thread = OpenThread(THREAD_QUERY_LIMITED_INFORMATION, false, tid);
        if (thread == IntPtr.Zero) { return ""; }
        try {
            IntPtr description;
            if (GetThreadDescription(thread, out description) < 0 || description == IntPtr.Zero) {
                return "";
            }
            try { return Marshal.PtrToStringUni(description) ?? ""; }
            finally { LocalFree(description); }
        } finally { CloseHandle(thread); }
    }
}
"@

$p = Get-Process -Id $TargetPid
$gdi = [LiveOwners]::GetGuiResources($p.Handle, [LiveOwners]::GR_GDIOBJECTS)
$user = [LiveOwners]::GetGuiResources($p.Handle, [LiveOwners]::GR_USEROBJECTS)
$names = @{}
foreach ($t in $p.Threads) {
    $n = [LiveOwners]::ThreadName([uint32]$t.Id)
    if ([string]::IsNullOrEmpty($n)) { $n = "(unnamed)" }
    if (-not $names.ContainsKey($n)) { $names[$n] = 0 }
    $names[$n] = [int]$names[$n] + 1
}
$reader = 0; if ($names.ContainsKey("minicon-reader")) { $reader = [int]$names["minicon-reader"] }
$waiter = 0; if ($names.ContainsKey("minicon-waiter")) { $waiter = [int]$names["minicon-waiter"] }
$reaper = 0; if ($names.ContainsKey("agenterm-pty-reaper")) { $reaper = [int]$names["agenterm-pty-reaper"] }
$overflow = 0; if ($names.ContainsKey("agenterm-pty-reaper-overflow")) { $overflow = [int]$names["agenterm-pty-reaper-overflow"] }
$nameText = (($names.GetEnumerator() | Sort-Object Name | ForEach-Object { "{0}={1}" -f $_.Key, $_.Value }) -join ",")
Write-Output ("phase={0} pid={1} name={2} ws={3} private={4} peak_ws={5} handles={6} gdi={7} user={8} threads={9} pty_reader={10} pty_waiter={11} pty_reaper={12} pty_reaper_overflow={13} thread_names={14}" -f `
    $Phase, $p.Id, $p.Name, $p.WorkingSet64, $p.PrivateMemorySize64, $p.PeakWorkingSet64, `
    $p.HandleCount, $gdi, $user, $p.Threads.Count, $reader, $waiter, $reaper, $overflow, $nameText)
