# Same-snapshot QueryWorkingSet VirtualPage -> VirtualQueryEx / module range.
# Shared = sharable (PSAPI_WORKING_SET_BLOCK), not "already shared".
# ShareCount = process count (max 7). Protection 5/7 family = copy-on-write.
# https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-psapi_working_set_block
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

public static class WsResident {
    public const uint PROCESS_QUERY_INFORMATION = 0x0400;
    public const uint PROCESS_VM_READ = 0x0010;
    public const uint LIST_MODULES_ALL = 0x03;
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

    [StructLayout(LayoutKind.Sequential)]
    public struct MODULEINFO {
        public IntPtr lpBaseOfDll;
        public uint SizeOfImage;
        public IntPtr EntryPoint;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern UIntPtr VirtualQueryEx(IntPtr process, UIntPtr address, out MEMORY_BASIC_INFORMATION info, UIntPtr length);
    [DllImport("kernel32.dll")]
    public static extern void GetSystemInfo(out SYSTEM_INFO info);
    [DllImport("psapi.dll", SetLastError = true)]
    public static extern bool QueryWorkingSet(IntPtr process, IntPtr buffer, uint size);
    [DllImport("psapi.dll", SetLastError = true)]
    public static extern bool EnumProcessModulesEx(IntPtr process, IntPtr[] modules, uint cb, out uint needed, uint filter);
    [DllImport("psapi.dll", SetLastError = true)]
    public static extern bool GetModuleInformation(IntPtr process, IntPtr module, out MODULEINFO info, uint size);
    [DllImport("psapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern uint GetModuleFileNameExW(IntPtr process, IntPtr module, StringBuilder name, uint size);
    [DllImport("psapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern uint GetMappedFileNameW(IntPtr process, UIntPtr address, StringBuilder filename, uint size);

    public static string TypeName(uint type) {
        if ((type & MEM_IMAGE) != 0) return "image";
        if ((type & MEM_MAPPED) != 0) return "mapped";
        if ((type & MEM_PRIVATE) != 0) return "private";
        return "other";
    }

    public static string Leaf(string path) {
        int slash = Math.Max(path.LastIndexOf('\\'), path.LastIndexOf('/'));
        return slash >= 0 ? path.Substring(slash + 1) : path;
    }

    public static bool IsCow(ulong protection) {
        ulong low = protection & 7UL;
        return low == 5UL || low == 7UL;
    }

    struct ModRange {
        public ulong Start;
        public ulong End;
        public string Leaf;
    }

    public static List<string> Dump(int pid, long wsBefore) {
        var lines = new List<string>();
        IntPtr h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid);
        if (h == IntPtr.Zero) throw new InvalidOperationException("OpenProcess failed");
        try {
            SYSTEM_INFO sys;
            GetSystemInfo(out sys);
            ulong page = sys.dwPageSize == 0 ? 4096UL : sys.dwPageSize;
            int guess = Math.Max(1024, (int)(wsBefore / (long)page) + 1024);
            uint bytes = (uint)(8 + guess * 8);
            IntPtr buf = Marshal.AllocHGlobal((int)bytes);
            try {
                if (!QueryWorkingSet(h, buf, bytes)) {
                    Marshal.FreeHGlobal(buf);
                    guess *= 4;
                    bytes = (uint)(8 + guess * 8);
                    buf = Marshal.AllocHGlobal((int)bytes);
                    if (!QueryWorkingSet(h, buf, bytes)) throw new InvalidOperationException("QueryWorkingSet failed");
                }
                long n = Marshal.ReadInt64(buf);
                int count = n > int.MaxValue ? 0 : (int)n;

                var mods = new List<ModRange>();
                uint needed;
                var slots = new IntPtr[512];
                if (EnumProcessModulesEx(h, slots, (uint)(IntPtr.Size * slots.Length), out needed, LIST_MODULES_ALL)) {
                    int nmod = (int)(needed / (uint)IntPtr.Size);
                    if (nmod > slots.Length) nmod = slots.Length;
                    var name = new StringBuilder(512);
                    for (int m = 0; m < nmod; m++) {
                        MODULEINFO mi;
                        if (!GetModuleInformation(h, slots[m], out mi, (uint)Marshal.SizeOf(typeof(MODULEINFO)))) continue;
                        name.Clear();
                        GetModuleFileNameExW(h, slots[m], name, 512);
                        ulong start = (ulong)mi.lpBaseOfDll.ToInt64();
                        mods.Add(new ModRange { Start = start, End = start + mi.SizeOfImage, Leaf = Leaf(name.ToString()) });
                    }
                }

                ulong sharable = 0, notSharable = 0, shareGe2 = 0, shareEq1 = 0, shareEq0 = 0;
                ulong cow = 0, cowPrivate = 0, unknown = 0;
                ulong resImage = 0, resMapped = 0, resPrivType = 0, resOther = 0;
                var byMod = new Dictionary<string, ulong>(StringComparer.OrdinalIgnoreCase);
                var byMapped = new Dictionary<string, ulong>(StringComparer.OrdinalIgnoreCase);
                var nameByAlloc = new Dictionary<ulong, string>();
                uint mbiSize = (uint)Marshal.SizeOf(typeof(MEMORY_BASIC_INFORMATION));
                var sb = new StringBuilder(512);

                for (int i = 0; i < count; i++) {
                    ulong block = unchecked((ulong)Marshal.ReadInt64(buf, 8 + i * 8));
                    ulong protection = block & 0x1FUL;
                    ulong shareCount = (block >> 5) & 0x7UL;
                    bool sharedBit = ((block >> 8) & 1UL) != 0;
                    ulong va = block & ~(page - 1UL);
                    if (sharedBit) {
                        sharable += page;
                        if (shareCount >= 2) shareGe2 += page;
                        else if (shareCount == 1) shareEq1 += page;
                        else shareEq0 += page;
                    } else {
                        notSharable += page;
                    }
                    bool cowPage = IsCow(protection);
                    if (cowPage) cow += page;

                    string owner = null;
                    for (int m = 0; m < mods.Count; m++) {
                        if (va >= mods[m].Start && va < mods[m].End) { owner = mods[m].Leaf; break; }
                    }
                    if (owner != null) {
                        ulong cur;
                        byMod.TryGetValue(owner, out cur);
                        byMod[owner] = cur + page;
                        resImage += page;
                        continue;
                    }

                    MEMORY_BASIC_INFORMATION mbi;
                    if (VirtualQueryEx(h, (UIntPtr)va, out mbi, (UIntPtr)mbiSize) == UIntPtr.Zero) {
                        unknown += page;
                        resOther += page;
                        continue;
                    }
                    string kind = TypeName(mbi.Type);
                    if (kind == "image") {
                        resImage += page;
                        ulong alloc = mbi.AllocationBase.ToUInt64();
                        string path;
                        if (!nameByAlloc.TryGetValue(alloc, out path)) {
                            sb.Clear();
                            uint got = GetMappedFileNameW(h, (UIntPtr)va, sb, 512);
                            path = got > 0 ? Leaf(sb.ToString()) : "unknown-image";
                            nameByAlloc[alloc] = path;
                        }
                        if (path == "unknown-image") unknown += page;
                        ulong cur;
                        byMod.TryGetValue(path, out cur);
                        byMod[path] = cur + page;
                    } else if (kind == "mapped") {
                        resMapped += page;
                        ulong alloc = mbi.AllocationBase.ToUInt64();
                        string path;
                        if (!nameByAlloc.TryGetValue(alloc, out path)) {
                            sb.Clear();
                            uint got = GetMappedFileNameW(h, (UIntPtr)va, sb, 512);
                            path = got > 0 ? Leaf(sb.ToString()) : "unknown-mapped";
                            nameByAlloc[alloc] = path;
                        }
                        if (path.StartsWith("unknown")) unknown += page;
                        ulong cur;
                        byMapped.TryGetValue(path, out cur);
                        byMapped[path] = cur + page;
                    } else if (kind == "private") {
                        resPrivType += page;
                        if (cowPage) cowPrivate += page;
                    } else {
                        unknown += page;
                        resOther += page;
                    }
                }

                lines.Add("fields Shared=sharable_not_already_shared ShareCount=process_count_max7 Protection5or7=COW VirtualPage=page_va https://learn.microsoft.com/en-us/windows/win32/api/psapi/ns-psapi-psapi_working_set_block");
                lines.Add(string.Format(
                    "pid={0} ws_before={1} page={2} ws_pages={3} walk_bytes={4} sharable={5} not_sharable={6} sharecount_ge2={7} sharecount_eq1={8} sharecount_eq0={9} cow={10} cow_private={11} unknown={12} resident_image={13} resident_mapped={14} resident_private_type={15} resident_other={16}",
                    pid, wsBefore, page, count, (ulong)count * page, sharable, notSharable, shareGe2, shareEq1, shareEq0, cow, cowPrivate, unknown, resImage, resMapped, resPrivType, resOther));
                var items = new List<KeyValuePair<string, ulong>>(byMod);
                items.Sort((a, b) => b.Value.CompareTo(a.Value));
                int shown = 0;
                foreach (var item in items) {
                    if (shown++ >= 25) break;
                    lines.Add(string.Format("module {0} {1}", item.Value, item.Key));
                }
                var mapped = new List<KeyValuePair<string, ulong>>(byMapped);
                mapped.Sort((a, b) => b.Value.CompareTo(a.Value));
                shown = 0;
                foreach (var item in mapped) {
                    if (shown++ >= 15) break;
                    lines.Add(string.Format("mapped {0} {1}", item.Value, item.Key));
                }
            } finally {
                Marshal.FreeHGlobal(buf);
            }
        } finally {
            CloseHandle(h);
        }
        return lines;
    }
}
"@

$p = Get-Process -Id $TargetPid
$wsBefore = $p.WorkingSet64
$lines = [WsResident]::Dump($TargetPid, $wsBefore)
$p.Refresh()
$wsAfter = $p.WorkingSet64
$walk = $null
foreach ($line in $lines) {
    if ($line -match 'walk_bytes=(\d+)') { $walk = [int64]$Matches[1] }
    $_ = $line
    Write-Output $line
}
$driftBefore = $wsBefore - $walk
$driftAfter = $wsAfter - $walk
Write-Output ("ws_after={0} drift_before={1} drift_after={2}" -f $wsAfter, $driftBefore, $driftAfter)
