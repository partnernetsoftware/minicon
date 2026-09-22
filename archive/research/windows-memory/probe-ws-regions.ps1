# Same-snapshot QueryWorkingSet VirtualPage -> module range / VirtualQueryEx.
# Classification walk is internally closed. A separate GetProcessMemoryInfo WS
# is not that product; record PMC WS + UTC before QWS, after QWS, after classify.
# Shared = sharable. ShareCount = process count (max 7). Keep unknown + COW.
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
using System.Globalization;
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

    [StructLayout(LayoutKind.Sequential)]
    public struct PROCESS_MEMORY_COUNTERS {
        public uint cb;
        public uint PageFaultCount;
        public UIntPtr PeakWorkingSetSize;
        public UIntPtr WorkingSetSize;
        public UIntPtr QuotaPeakPagedPoolUsage;
        public UIntPtr QuotaPagedPoolUsage;
        public UIntPtr QuotaPeakNonPagedPoolUsage;
        public UIntPtr QuotaNonPagedPoolUsage;
        public UIntPtr PagefileUsage;
        public UIntPtr PeakPagefileUsage;
    }

    class UnnamedMap {
        public ulong Alloc;
        public ulong Resident;
        public uint Protect;
        public uint VqType;
        public int LastError;
        public string Why;
        public ulong ShareGe2;
        public ulong ShareEq1;
        public ulong NotSharable;
    }

    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern IntPtr OpenProcess(uint access, bool inherit, int pid);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool CloseHandle(IntPtr handle);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern UIntPtr VirtualQueryEx(IntPtr process, UIntPtr address, out MEMORY_BASIC_INFORMATION info, UIntPtr length);
    [DllImport("kernel32.dll")]
    public static extern void GetSystemInfo(out SYSTEM_INFO info);
    [DllImport("kernel32.dll")]
    public static extern void SetLastError(uint error);
    [DllImport("kernel32.dll", SetLastError = true)]
    public static extern bool K32GetProcessMemoryInfo(IntPtr process, out PROCESS_MEMORY_COUNTERS counters, uint size);
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

    static string UtcNow() {
        return DateTime.UtcNow.ToString("yyyy-MM-ddTHH:mm:ss.fffZ", CultureInfo.InvariantCulture);
    }

    static ulong PmcWs(IntPtr h) {
        PROCESS_MEMORY_COUNTERS pmc = new PROCESS_MEMORY_COUNTERS();
        pmc.cb = (uint)Marshal.SizeOf(typeof(PROCESS_MEMORY_COUNTERS));
        if (!K32GetProcessMemoryInfo(h, out pmc, pmc.cb)) return 0;
        return pmc.WorkingSetSize.ToUInt64();
    }

    struct ModRange {
        public ulong Start;
        public ulong End;
        public string Leaf;
    }

    public static List<string> Dump(int pid) {
        var lines = new List<string>();
        IntPtr h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid);
        if (h == IntPtr.Zero) throw new InvalidOperationException("OpenProcess failed");
        try {
            SYSTEM_INFO sys;
            GetSystemInfo(out sys);
            ulong page = sys.dwPageSize == 0 ? 4096UL : sys.dwPageSize;
            string t0 = UtcNow();
            ulong pmc0 = PmcWs(h);
            int guess = Math.Max(1024, (int)(pmc0 / page) + 1024);
            uint bytes = (uint)(8 + guess * 8);
            int qwsRetry = 0;
            IntPtr buf = Marshal.AllocHGlobal((int)bytes);
            try {
                if (!QueryWorkingSet(h, buf, bytes)) {
                    qwsRetry = 1;
                    Marshal.FreeHGlobal(buf);
                    guess *= 4;
                    bytes = (uint)(8 + guess * 8);
                    buf = Marshal.AllocHGlobal((int)bytes);
                    if (!QueryWorkingSet(h, buf, bytes)) {
                        lines.Add("count_fail qws last_error=" + Marshal.GetLastWin32Error());
                        throw new InvalidOperationException("QueryWorkingSet failed");
                    }
                }
                string t1 = UtcNow();
                ulong pmc1 = PmcWs(h);
                long n = Marshal.ReadInt64(buf);
                int count = n > int.MaxValue ? 0 : (int)n;
                ulong walk = (ulong)count * page;

                var mods = new List<ModRange>();
                uint needed = 0;
                var slots = new IntPtr[512];
                SetLastError(0);
                bool enumOk = EnumProcessModulesEx(h, slots, (uint)(IntPtr.Size * slots.Length), out needed, LIST_MODULES_ALL);
                int enumErr = enumOk ? -1 : Marshal.GetLastWin32Error();
                int nmodAvail = enumOk ? (int)(needed / (uint)IntPtr.Size) : 0;
                int nmod = nmodAvail > slots.Length ? slots.Length : nmodAvail;
                int enumTruncated = nmodAvail > slots.Length ? 1 : 0;
                int getModFail = 0;
                if (enumOk) {
                    var name = new StringBuilder(512);
                    for (int m = 0; m < nmod; m++) {
                        MODULEINFO mi;
                        if (!GetModuleInformation(h, slots[m], out mi, (uint)Marshal.SizeOf(typeof(MODULEINFO)))) {
                            getModFail++;
                            continue;
                        }
                        name.Clear();
                        GetModuleFileNameExW(h, slots[m], name, 512);
                        ulong start = (ulong)mi.lpBaseOfDll.ToInt64();
                        mods.Add(new ModRange { Start = start, End = start + mi.SizeOfImage, Leaf = Leaf(name.ToString()) });
                    }
                }

                ulong sharable = 0, notSharable = 0, shareGe2 = 0, shareEq1 = 0, shareEq0 = 0;
                ulong cowProtect = 0, unknown = 0;
                ulong resImage = 0, resMapped = 0, resPrivType = 0, resOther = 0, vqFail = 0;
                ulong privatizedImage = 0, privatizedMapped = 0;
                ulong imgS0 = 0, imgS1 = 0, mapS0 = 0, mapS1 = 0, privS0 = 0, privS1 = 0, othS0 = 0, othS1 = 0;
                var byMod = new Dictionary<string, ulong>(StringComparer.OrdinalIgnoreCase);
                var byModS0 = new Dictionary<string, ulong>(StringComparer.OrdinalIgnoreCase);
                var byModS1 = new Dictionary<string, ulong>(StringComparer.OrdinalIgnoreCase);
                var byMapped = new Dictionary<string, ulong>(StringComparer.OrdinalIgnoreCase);
                var nameByAlloc = new Dictionary<ulong, string>();
                var unnamed = new Dictionary<ulong, UnnamedMap>();
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
                    if (cowPage) cowProtect += page;

                    string owner = null;
                    for (int m = 0; m < mods.Count; m++) {
                        if (va >= mods[m].Start && va < mods[m].End) { owner = mods[m].Leaf; break; }
                    }
                    if (owner != null) {
                        ulong cur;
                        byMod.TryGetValue(owner, out cur);
                        byMod[owner] = cur + page;
                        if (sharedBit) {
                            byModS1.TryGetValue(owner, out cur);
                            byModS1[owner] = cur + page;
                        } else {
                            byModS0.TryGetValue(owner, out cur);
                            byModS0[owner] = cur + page;
                        }
                        resImage += page;
                        if (!sharedBit) { privatizedImage += page; imgS0 += page; } else imgS1 += page;
                        continue;
                    }

                    MEMORY_BASIC_INFORMATION mbi;
                    if (VirtualQueryEx(h, (UIntPtr)va, out mbi, (UIntPtr)mbiSize) == UIntPtr.Zero) {
                        unknown += page;
                        vqFail += page;
                        resOther += page;
                        if (sharedBit) othS1 += page; else othS0 += page;
                        continue;
                    }
                    string kind = TypeName(mbi.Type);
                    ulong alloc = mbi.AllocationBase.ToUInt64();
                    if (kind == "image") {
                        resImage += page;
                        if (!sharedBit) { privatizedImage += page; imgS0 += page; } else imgS1 += page;
                        string path;
                        if (!nameByAlloc.TryGetValue(alloc, out path)) {
                            SetLastError(0);
                            sb.Clear();
                            uint got = GetMappedFileNameW(h, (UIntPtr)va, sb, 512);
                            int err = Marshal.GetLastWin32Error();
                            if (got > 0) path = Leaf(sb.ToString());
                            else path = "unknown-image";
                            nameByAlloc[alloc] = path;
                            if (path.StartsWith("unknown")) {
                                unknown += page;
                                RememberUnnamed(unnamed, alloc, mbi, page, sharedBit, shareCount, err, err != 0 ? "getmapped_win32_" + err : "getmapped_empty");
                            }
                        } else if (path.StartsWith("unknown")) {
                            unknown += page;
                            RememberUnnamed(unnamed, alloc, mbi, page, sharedBit, shareCount, 0, path);
                        }
                        ulong cur;
                        byMod.TryGetValue(path, out cur);
                        byMod[path] = cur + page;
                        if (sharedBit) {
                            byModS1.TryGetValue(path, out cur);
                            byModS1[path] = cur + page;
                        } else {
                            byModS0.TryGetValue(path, out cur);
                            byModS0[path] = cur + page;
                        }
                    } else if (kind == "mapped") {
                        resMapped += page;
                        if (!sharedBit) { privatizedMapped += page; mapS0 += page; } else mapS1 += page;
                        string path;
                        if (!nameByAlloc.TryGetValue(alloc, out path)) {
                            SetLastError(0);
                            sb.Clear();
                            uint got = GetMappedFileNameW(h, (UIntPtr)va, sb, 512);
                            int err = Marshal.GetLastWin32Error();
                            if (got > 0) path = Leaf(sb.ToString());
                            else path = "unknown-mapped";
                            nameByAlloc[alloc] = path;
                            if (path.StartsWith("unknown")) {
                                unknown += page;
                                RememberUnnamed(unnamed, alloc, mbi, page, sharedBit, shareCount, err, err != 0 ? "getmapped_win32_" + err : "getmapped_empty");
                            }
                        } else if (path.StartsWith("unknown")) {
                            unknown += page;
                            RememberUnnamed(unnamed, alloc, mbi, page, sharedBit, shareCount, 0, path);
                        }
                        ulong cur;
                        byMapped.TryGetValue(path, out cur);
                        byMapped[path] = cur + page;
                    } else if (kind == "private") {
                        resPrivType += page;
                        if (sharedBit) privS1 += page; else privS0 += page;
                    } else {
                        unknown += page;
                        resOther += page;
                        if (sharedBit) othS1 += page; else othS0 += page;
                        RememberUnnamed(unnamed, alloc, mbi, page, sharedBit, shareCount, 0, "type_" + kind);
                    }
                }

                string t2 = UtcNow();
                ulong pmc2 = PmcWs(h);
                lines.Add("fields Shared=sharable ShareCount=process_count_max7 cow_protect=QWS_Protection5or7 privatized=IMAGE_or_MAPPED_and_Shared0 cow_private_removed_not_privatization https://learn.microsoft.com/en-us/windows/win32/api/memoryapi/nf-memoryapi-virtualqueryex");
                lines.Add(string.Format("t_before_qws={0} pmc_ws_before_qws={1}", t0, pmc0));
                lines.Add(string.Format("t_after_qws={0} pmc_ws_after_qws={1}", t1, pmc1));
                lines.Add(string.Format("t_after_classify={0} pmc_ws_after_classify={1}", t2, pmc2));
                lines.Add(string.Format(
                    "count_meta qws_retry={0} enum_ok={1} enum_err={2} enum_needed={3} enum_slots={4} enum_truncated={5} getmod_fail={6} module_keys={7} mapped_keys={8}",
                    qwsRetry, enumOk ? 1 : 0, enumOk ? "n/a" : enumErr.ToString(), nmodAvail, slots.Length, enumTruncated, getModFail, byMod.Count, byMapped.Count));
                lines.Add(string.Format(
                    "pid={0} page={1} ws_pages={2} walk_bytes={3} unexplained_remainder_before={4} unexplained_remainder_after_qws={5} unexplained_remainder_after_classify={6} sharable={7} not_sharable={8} sharecount_ge2={9} sharecount_eq1={10} sharecount_eq0={11} cow_protect={12} privatized_image={13} privatized_mapped={14} unknown={15} vq_fail={16} resident_image={17} resident_mapped={18} resident_private_type={19} resident_other={20}",
                    pid, page, count, walk, (long)pmc0 - (long)walk, (long)pmc1 - (long)walk, (long)pmc2 - (long)walk,
                    sharable, notSharable, shareGe2, shareEq1, shareEq0, cowProtect, privatizedImage, privatizedMapped, unknown, vqFail, resImage, resMapped, resPrivType, resOther));
                lines.Add(string.Format("cross type=image shared0={0} shared1={1}", imgS0, imgS1));
                lines.Add(string.Format("cross type=mapped shared0={0} shared1={1}", mapS0, mapS1));
                lines.Add(string.Format("cross type=private shared0={0} shared1={1}", privS0, privS1));
                lines.Add(string.Format("cross type=other shared0={0} shared1={1}", othS0, othS1));
                var items = new List<KeyValuePair<string, ulong>>(byMod);
                items.Sort((a, b) => b.Value.CompareTo(a.Value));
                int shown = 0;
                int moduleShownCap = 64;
                foreach (var item in items) {
                    if (shown++ >= moduleShownCap) break;
                    ulong s0 = 0, s1 = 0;
                    byModS0.TryGetValue(item.Key, out s0);
                    byModS1.TryGetValue(item.Key, out s1);
                    lines.Add(string.Format("module {0} {1} shared0={2} shared1={3}", item.Value, item.Key, s0, s1));
                }
                if (items.Count > moduleShownCap) lines.Add("module_list_truncated shown=" + moduleShownCap + " total=" + items.Count);
                var mapped = new List<KeyValuePair<string, ulong>>(byMapped);
                mapped.Sort((a, b) => b.Value.CompareTo(a.Value));
                shown = 0;
                int mappedShownCap = 15;
                foreach (var item in mapped) {
                    if (shown++ >= mappedShownCap) break;
                    lines.Add(string.Format("mapped {0} {1}", item.Value, item.Key));
                }
                if (mapped.Count > mappedShownCap) lines.Add("mapped_list_truncated shown=" + mappedShownCap + " total=" + mapped.Count);
                var unnamedList = new List<UnnamedMap>(unnamed.Values);
                unnamedList.Sort((a, b) => b.Resident.CompareTo(a.Resident));
                foreach (var u in unnamedList) {
                    lines.Add(string.Format(
                        "unnamed_mapped alloc=0x{0:x} resident={1} protect=0x{2:x} type=0x{3:x} last_error={4} why={5} sharecount_ge2={6} sharecount_eq1={7} not_sharable={8}",
                        u.Alloc, u.Resident, u.Protect, u.VqType, u.LastError, u.Why, u.ShareGe2, u.ShareEq1, u.NotSharable));
                }
            } finally {
                Marshal.FreeHGlobal(buf);
            }
        } finally {
            CloseHandle(h);
        }
        return lines;
    }

    static void RememberUnnamed(Dictionary<ulong, UnnamedMap> unnamed, ulong alloc, MEMORY_BASIC_INFORMATION mbi, ulong page, bool sharedBit, ulong shareCount, int err, string why) {
        UnnamedMap u;
        if (!unnamed.TryGetValue(alloc, out u)) {
            u = new UnnamedMap {
                Alloc = alloc,
                Protect = mbi.Protect,
                VqType = mbi.Type,
                LastError = err,
                Why = why
            };
            unnamed[alloc] = u;
        }
        u.Resident += page;
        if (!sharedBit) u.NotSharable += page;
        else if (shareCount >= 2) u.ShareGe2 += page;
        else if (shareCount == 1) u.ShareEq1 += page;
        if (u.LastError == 0 && err != 0) u.LastError = err;
        if (u.Why == null || u.Why.Length == 0) u.Why = why;
    }
}
"@

[WsResident]::Dump($TargetPid) | ForEach-Object { $_ }
