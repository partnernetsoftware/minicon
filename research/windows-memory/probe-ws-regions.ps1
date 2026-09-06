# Sum committed VirtualQueryEx regions for one PID. Research-only.
# Usage: powershell -File probe-ws-regions.ps1 -TargetPid 1234
param(
    [Parameter(Mandatory = $true)]
    [int]$TargetPid
)

$ErrorActionPreference = "Stop"
Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;
using System.Text;
public static class WsRegions {
    public const uint PROCESS_QUERY_INFORMATION = 0x0400;
    public const uint PROCESS_VM_READ = 0x0010;
    public const uint MEM_COMMIT = 0x1000;
    public const uint MEM_RESERVE = 0x2000;
    public const uint MEM_FREE = 0x10000;
    public const uint MEM_PRIVATE = 0x20000;
    public const uint MEM_MAPPED = 0x40000;
    public const uint MEM_IMAGE = 0x1000000;
    [StructLayout(LayoutKind.Sequential)]
    public struct MEMORY_BASIC_INFORMATION {
        public UIntPtr BaseAddress;
        public UIntPtr AllocationBase;
        public uint AllocationProtect;
        public UIntPtr RegionSize;
        public uint State;
        public uint Protect;
        public uint Type;
    }
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern UIntPtr VirtualQueryEx(IntPtr process, UIntPtr address, out MEMORY_BASIC_INFORMATION info, UIntPtr length);
    [DllImport("psapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern uint GetMappedFileNameW(IntPtr process, UIntPtr address, StringBuilder filename, uint size);
    [DllImport("psapi.dll", SetLastError = true)]
    public static extern bool QueryWorkingSet(IntPtr process, IntPtr buffer, uint size);
    [DllImport("kernel32.dll")]
    public static extern void GetSystemInfo(out SYSTEM_INFO info);
    [StructLayout(LayoutKind.Sequential)]
    public struct SYSTEM_INFO {
        public ushort wProcessorArchitecture;
        public ushort wReserved;
        public uint dwPageSize;
        public IntPtr lpMinimumApplicationAddress;
        public IntPtr lpMaximumApplicationAddress;
        public UIntPtr dwActiveProcessorMask;
        public uint dwNumberOfProcessors;
        public uint dwProcessorType;
        public uint dwAllocationGranularity;
        public ushort wProcessorLevel;
        public ushort wProcessorRevision;
    }
    public static string TypeName(uint type) {
        if ((type & MEM_IMAGE) != 0) return "image";
        if ((type & MEM_MAPPED) != 0) return "mapped";
        if ((type & MEM_PRIVATE) != 0) return "private";
        return "other";
    }
}
"@

$access = [uint32]([WsRegions]::PROCESS_QUERY_INFORMATION -bor [WsRegions]::PROCESS_VM_READ)
$h = [WsRegions]::OpenProcess($access, $false, $TargetPid)
if ($h -eq [IntPtr]::Zero) { throw "OpenProcess failed for $TargetPid" }
try {
    $infoSize = [uint32][Runtime.InteropServices.Marshal]::SizeOf([type][WsRegions+MEMORY_BASIC_INFORMATION])
    $addr = [UIntPtr]::Zero
    $sums = @{}
    $files = @{}
    $lastAlloc = [uint64]0
    $lastPath = ""
    $guard = 0
    while ($guard -lt 200000) {
        $guard++
        $info = New-Object WsRegions+MEMORY_BASIC_INFORMATION
        $got = [WsRegions]::VirtualQueryEx($h, $addr, [ref]$info, [UIntPtr]$infoSize)
        if ([uint64]$got -eq 0) { break }
        $size = [uint64]$info.RegionSize
        if ($size -eq 0) { break }
        if ($info.State -eq [WsRegions]::MEM_COMMIT) {
            $kind = [WsRegions]::TypeName($info.Type)
            if (-not $sums.ContainsKey($kind)) { $sums[$kind] = [uint64]0 }
            $sums[$kind] = [uint64]$sums[$kind] + $size
            $alloc = [uint64]$info.AllocationBase
            if ($kind -ne 'private') {
                if ($alloc -ne $lastAlloc) {
                    $sb = New-Object System.Text.StringBuilder 512
                    $n = [WsRegions]::GetMappedFileNameW($h, $info.BaseAddress, $sb, 512)
                    $lastAlloc = $alloc
                    $lastPath = $(if ($n -gt 0) { $sb.ToString() } else { "" })
                }
                if ($lastPath -ne "") {
                    if (-not $files.ContainsKey($lastPath)) { $files[$lastPath] = [uint64]0 }
                    $files[$lastPath] = [uint64]$files[$lastPath] + $size
                }
            }
        }
        $next = [uint64]$info.BaseAddress + $size
        if ($next -le [uint64]$addr) { break }
        $addr = [UIntPtr]$next
    }
    $p = Get-Process -Id $TargetPid
    $img = 0; $map = 0; $priv = 0; $oth = 0
    if ($sums.ContainsKey('image')) { $img = [uint64]$sums['image'] }
    if ($sums.ContainsKey('mapped')) { $map = [uint64]$sums['mapped'] }
    if ($sums.ContainsKey('private')) { $priv = [uint64]$sums['private'] }
    if ($sums.ContainsKey('other')) { $oth = [uint64]$sums['other'] }
    Write-Output ("pid={0} ws={1} private_counter={2} commit_image={3} commit_mapped={4} commit_private={5} commit_other={6}" -f `
        $p.Id, $p.WorkingSet64, $p.PrivateMemorySize64, $img, $map, $priv, $oth)
    $files.GetEnumerator() | Sort-Object Value -Descending | Select-Object -First 20 | ForEach-Object {
        Write-Output ("mapped_va {0} {1}" -f $_.Value, $_.Key)
    }
    $sys = New-Object WsRegions+SYSTEM_INFO
    [WsRegions]::GetSystemInfo([ref]$sys)
    $page = [uint64]$sys.dwPageSize
    if ($page -eq 0) { $page = 4096 }
    $pagesGuess = [Math]::Max(1024, [int](($p.WorkingSet64 / $page) + 512))
    $bytes = [uint32](8 + ($pagesGuess * 8))
    $buf = [Runtime.InteropServices.Marshal]::AllocHGlobal([int]$bytes)
    try {
        if (-not [WsRegions]::QueryWorkingSet($h, $buf, $bytes)) {
            $pagesGuess = $pagesGuess * 4
            [Runtime.InteropServices.Marshal]::FreeHGlobal($buf)
            $bytes = [uint32](8 + ($pagesGuess * 8))
            $buf = [Runtime.InteropServices.Marshal]::AllocHGlobal([int]$bytes)
            if (-not [WsRegions]::QueryWorkingSet($h, $buf, $bytes)) {
                throw "QueryWorkingSet failed"
            }
        }
        $n = [Runtime.InteropServices.Marshal]::ReadInt64($buf)
        $sharedPages = [uint64]0
        $privatePages = [uint64]0
        for ($i = 0; $i -lt $n; $i++) {
            $block = [uint64][Runtime.InteropServices.Marshal]::ReadInt64($buf, [int](8 + ($i * 8)))
            $shared = ($block -band ([uint64]1 -shl 8)) -ne 0
            if ($shared) { $sharedPages++ } else { $privatePages++ }
        }
        Write-Output ("resident_pages={0} page={1} resident_shared={2} resident_private={3}" -f `
            $n, $page, ($sharedPages * $page), ($privatePages * $page))
    } finally {
        [Runtime.InteropServices.Marshal]::FreeHGlobal($buf)
    }
} finally {
    [void][WsRegions]::CloseHandle($h)
}
