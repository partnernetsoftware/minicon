# Watch a live console agent's window rectangle change, from outside it.
#
# Written by the AgenTerm lane (cc-agenterm) and carried here unchanged apart
# from this note: the blank reproduces on MiniCon's hosted Windows runners and
# not in their court, so the observation has to run where the failure is.
#
# The dump proved the window is 1x1 at (41,6) while resize() never returned Err.
# No rectangle console_agent.rs can write has Left != 0 -- `minimal` is
# {0,0,0,0} and both `window` and `corrected` are {0, top, target.X-1, ...}.
# So the 1x1 at column 41 was written by something else, after resize returned.
# This samples the rectangle over a zoom journey and records every transition
# with a timestamp, so "when did it become 1x1, and what did it look like just
# before" stops being a matter of inference.
#
# Needs no product change and no Cargo (AttachConsole), so a frozen tree is fine:
#   powershell -ExecutionPolicy Bypass -File window-watch.ps1 -AgentPid <pid> -Seconds 60
# Prints nothing: AttachConsole rebinds stdout to the agent's console. Read $Out
# after it returns.
param(
    [Parameter(Mandatory = $true)][int]$AgentPid,
    [int]$Seconds = 60,
    [int]$IntervalMs = 50,
    [string]$Out = "$PSScriptRoot\window-watch.json"
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace Watch -Name Con -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)] public struct COORD { public short X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct SMALL_RECT { public short Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct CONSOLE_SCREEN_BUFFER_INFO {
    public COORD dwSize; public COORD dwCursorPosition; public ushort wAttributes;
    public SMALL_RECT srWindow; public COORD dwMaximumWindowSize;
}
[StructLayout(LayoutKind.Sequential, CharSet=CharSet.Unicode)] public struct CONSOLE_FONT_INFOEX {
    public uint cbSize; public uint nFont; public COORD dwFontSize;
    public uint FontFamily; public uint FontWeight;
    [MarshalAs(UnmanagedType.ByValTStr, SizeConst=32)] public string FaceName;
}
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool FreeConsole();
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool AttachConsole(uint pid);
[DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern IntPtr CreateFileW(
    string name, uint access, uint share, IntPtr sa, uint disposition, uint flags, IntPtr template);
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CONSOLE_SCREEN_BUFFER_INFO info);
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool GetCurrentConsoleFontEx(IntPtr h, bool max, ref CONSOLE_FONT_INFOEX f);
[DllImport("kernel32.dll", SetLastError=true)] public static extern COORD GetLargestConsoleWindowSize(IntPtr h);
'@

$log = [ordered]@{ agent_pid = $AgentPid; interval_ms = $IntervalMs }
try {
    [void][Watch.Con]::FreeConsole()
    if (-not [Watch.Con]::AttachConsole([uint32]$AgentPid)) {
        throw "AttachConsole failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
    $h = [Watch.Con]::CreateFileW('CONOUT$', ([uint32]3221225472), ([uint32]3), [IntPtr]::Zero, ([uint32]3), ([uint32]0), [IntPtr]::Zero)
    if ($h -eq [IntPtr]::new(-1)) { throw "CONOUT$ failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }

    $transitions = @()
    $last = ''
    $samples = 0
    $deadline = (Get-Date).AddSeconds($Seconds)
    while ((Get-Date) -lt $deadline) {
        $info = New-Object Watch.Con+CONSOLE_SCREEN_BUFFER_INFO
        if ([Watch.Con]::GetConsoleScreenBufferInfo($h, [ref]$info)) {
            $samples++
            $font = New-Object Watch.Con+CONSOLE_FONT_INFOEX
            $font.cbSize = [uint32][Runtime.InteropServices.Marshal]::SizeOf($font)
            [void][Watch.Con]::GetCurrentConsoleFontEx($h, $false, [ref]$font)
            $largest = [Watch.Con]::GetLargestConsoleWindowSize($h)
            # One line per distinct state: a 50ms poll over a whole journey is
            # thousands of samples, and only the changes carry information.
            $key = "$($info.srWindow.Left),$($info.srWindow.Top),$($info.srWindow.Right),$($info.srWindow.Bottom)|$($info.dwSize.X)x$($info.dwSize.Y)|$($font.dwFontSize.X)x$($font.dwFontSize.Y)"
            if ($key -ne $last) {
                $last = $key
                $transitions += [ordered]@{
                    at        = (Get-Date).ToString('HH:mm:ss.fff')
                    sample    = $samples
                    window    = @{ left = $info.srWindow.Left; top = $info.srWindow.Top
                                   right = $info.srWindow.Right; bottom = $info.srWindow.Bottom }
                    cols      = $info.srWindow.Right - $info.srWindow.Left + 1
                    rows      = $info.srWindow.Bottom - $info.srWindow.Top + 1
                    buffer    = @{ x = $info.dwSize.X; y = $info.dwSize.Y }
                    cursor    = @{ x = $info.dwCursorPosition.X; y = $info.dwCursorPosition.Y }
                    font      = @{ w = $font.dwFontSize.X; h = $font.dwFontSize.Y }
                    largest   = @{ x = $largest.X; y = $largest.Y }
                    # Anything console_agent.rs writes has left = 0. A transition
                    # with left != 0 was written by something else.
                    left_is_zero = ($info.srWindow.Left -eq 0)
                    degenerate   = ($info.srWindow.Right -eq $info.srWindow.Left -or
                                    $info.srWindow.Bottom -eq $info.srWindow.Top)
                }
            }
        }
        Start-Sleep -Milliseconds $IntervalMs
    }
    $log['samples'] = $samples
    $log['transitions'] = $transitions
    $log['first_degenerate'] = @($transitions | Where-Object { $_.degenerate } | Select-Object -First 1)
    $log['any_left_nonzero'] = @($transitions | Where-Object { -not $_.left_is_zero }).Count
} catch {
    $log['threw'] = "$_"
} finally {
    [void][Watch.Con]::FreeConsole()
    $log | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 $Out
}
