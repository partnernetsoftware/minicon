# Resident-page split for one PID: QueryWorkingSet + QueryWorkingSetEx + VirtualQueryEx.
# PrivateMemorySize64 is commit and is not subtracted from WS.
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
    public const uint MEM_COMMIT = 0x1000;
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
    public struct WS_EX {
        public UIntPtr VirtualAddress;
        public UIntPtr VirtualAttributes;
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
    public static extern bool QueryWorkingSetEx(IntPtr process, IntPtr info, uint size);
    [DllImport("psapi.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    public static extern uint GetMappedFileNameW(IntPtr process, UIntPtr address, StringBuilder filename, uint size);

    public static string TypeName(uint type) {
        if ((type & MEM_IMAGE) != 0) return "image";
        if ((type & MEM_MAPPED) != 0) return "mapped";
        if ((type & MEM_PRIVATE) != 0) return "private";
        return "other";
    }

    public static string Leaf(string path) {
        int slash = path.LastIndexOf('\\');
        return slash >= 0 ? path.Substring(slash + 1) : path;
    }

    public static List<string> Dump(int pid, long ws, long privateCommit) {
        var lines = new List<string>();
        IntPtr h = OpenProcess(PROCESS_QUERY_INFORMATION | PROCESS_VM_READ, false, pid);
        if (h == IntPtr.Zero) throw new InvalidOperationException("OpenProcess failed");
        try {
            SYSTEM_INFO sys;
            GetSystemInfo(out sys);
            ulong page = sys.dwPageSize == 0 ? 4096UL : sys.dwPageSize;
            int guess = Math.Max(1024, (int)(ws / (long)page) + 1024);
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
                var addrs = new ulong[count];
                for (int i = 0; i < count; i++) {
                    ulong block = unchecked((ulong)Marshal.ReadInt64(buf, 8 + i * 8));
                    addrs[i] = block & ~(page - 1UL);
                }
                int exSize = Marshal.SizeOf(typeof(WS_EX));
                IntPtr ex = Marshal.AllocHGlobal(exSize * count);
                try {
                    for (int i = 0; i < count; i++) {
                        Marshal.WriteIntPtr(ex, i * exSize, (IntPtr)(long)addrs[i]);
                        Marshal.WriteIntPtr(ex, i * exSize + IntPtr.Size, IntPtr.Zero);
                    }
                    if (!QueryWorkingSetEx(h, ex, (uint)(exSize * count))) {
                        throw new InvalidOperationException("QueryWorkingSetEx failed");
                    }
                    ulong resShared = 0, resPrivate = 0, resImage = 0, resMapped = 0, resPrivType = 0, resOther = 0, resInvalid = 0;
                    var byFile = new Dictionary<string, ulong>(StringComparer.OrdinalIgnoreCase);
                    var nameByAlloc = new Dictionary<ulong, string>();
                    uint mbiSize = (uint)Marshal.SizeOf(typeof(MEMORY_BASIC_INFORMATION));
                    var sb = new StringBuilder(512);
                    for (int i = 0; i < count; i++) {
                        ulong attr = unchecked((ulong)Marshal.ReadIntPtr(ex, i * exSize + IntPtr.Size).ToInt64());
                        bool valid = (attr & 1UL) != 0;
                        if (!valid) { resInvalid += page; continue; }
                        bool shared = (attr & (1UL << 15)) != 0;
                        if (shared) resShared += page; else resPrivate += page;
                        UIntPtr va = (UIntPtr)addrs[i];
                        MEMORY_BASIC_INFORMATION mbi;
                        if (VirtualQueryEx(h, va, out mbi, (UIntPtr)mbiSize) == UIntPtr.Zero) {
                            resOther += page;
                            continue;
                        }
                        string kind = TypeName(mbi.Type);
                        if (kind == "image") resImage += page;
                        else if (kind == "mapped") resMapped += page;
                        else if (kind == "private") resPrivType += page;
                        else resOther += page;
                        if (kind == "private") continue;
                        ulong alloc = mbi.AllocationBase.ToUInt64();
                        string path;
                        if (!nameByAlloc.TryGetValue(alloc, out path)) {
                            sb.Clear();
                            uint got = GetMappedFileNameW(h, va, sb, 512);
                            path = got > 0 ? sb.ToString() : "(unnamed-" + kind + ")";
                            nameByAlloc[alloc] = path;
                        }
                        ulong cur;
                        byFile.TryGetValue(path, out cur);
                        byFile[path] = cur + page;
                    }
                    lines.Add(string.Format(
                        "pid={0} ws={1} private_commit={2} page={3} ws_pages={4} resident_shared={5} resident_private={6} resident_image={7} resident_mapped={8} resident_private_type={9} resident_other={10} resident_invalid={11}",
                        pid, ws, privateCommit, page, count, resShared, resPrivate, resImage, resMapped, resPrivType, resOther, resInvalid));
                    var items = new List<KeyValuePair<string, ulong>>(byFile);
                    items.Sort((a, b) => b.Value.CompareTo(a.Value));
                    int shown = 0;
                    foreach (var item in items) {
                        if (shown++ >= 25) break;
                        lines.Add(string.Format("resident {0} {1}", item.Value, Leaf(item.Key)));
                    }
                } finally {
                    Marshal.FreeHGlobal(ex);
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
[WsResident]::Dump($TargetPid, $p.WorkingSet64, $p.PrivateMemorySize64) | ForEach-Object { $_ }
