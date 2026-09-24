# Dump a live console agent's WHOLE screen buffer, from outside it.
#
# Written by the AgenTerm lane (cc-agenterm) and carried here unchanged apart
# from this note: the blank reproduces on MiniCon's hosted Windows runners and
# not in their court, so the probe has to run where the failure is. Its
# verdict logic is theirs and is deliberately not edited here.
#
# The B1 fork: at the moment the terminal is blank, is the buffer blank too?
#   all blank            -> the content is gone or was never painted; the
#                           question becomes who repaints after a resize.
#   content outside      -> the scraped rectangle or the window is in the wrong
#     the window            place, however right the relative cursor looks.
#
# It needs no product change and no Cargo: it attaches to the agent's own
# console with AttachConsole(pid), which is why it can run against a frozen
# tree. Run it while a blank session is live:
#   powershell -ExecutionPolicy Bypass -File buffer-dump-probe.ps1 -Pid <agent>
# Called from a CI job: AttachConsole rebinds this process's standard handles to
# the agent's console, so anything written afterwards lands there and not in the
# job log -- including a redirect the job set up. Every result goes to $Out, and
# the job reads that file after the probe returns. Nothing here prints.
param(
    [Parameter(Mandatory = $true)][int]$AgentPid,
    [string]$Out = "$PSScriptRoot\buffer-dump.json",
    # A scroll-room buffer can be thousands of rows; each is one ReadConsoleOutputW.
    # Cap the walk so a probe can never be what makes a job time out.
    [int]$MaxRows = 4000
)

$ErrorActionPreference = 'Stop'

Add-Type -Namespace Dump -Name Con -MemberDefinition @'
[StructLayout(LayoutKind.Sequential)] public struct COORD { public short X, Y; }
[StructLayout(LayoutKind.Sequential)] public struct SMALL_RECT { public short Left, Top, Right, Bottom; }
[StructLayout(LayoutKind.Sequential)] public struct CONSOLE_SCREEN_BUFFER_INFO {
    public COORD dwSize; public COORD dwCursorPosition; public ushort wAttributes;
    public SMALL_RECT srWindow; public COORD dwMaximumWindowSize;
}
[StructLayout(LayoutKind.Explicit)] public struct CHAR_UNION {
    [FieldOffset(0)] public char UnicodeChar; [FieldOffset(0)] public byte AsciiChar;
}
[StructLayout(LayoutKind.Sequential)] public struct CHAR_INFO {
    public CHAR_UNION Char; public ushort Attributes;
}
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool FreeConsole();
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool AttachConsole(uint pid);
[DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern IntPtr CreateFileW(
    string name, uint access, uint share, IntPtr sa, uint disposition, uint flags, IntPtr template);
[DllImport("kernel32.dll", SetLastError=true)] public static extern bool GetConsoleScreenBufferInfo(IntPtr h, out CONSOLE_SCREEN_BUFFER_INFO info);
[DllImport("kernel32.dll", SetLastError=true, CharSet=CharSet.Unicode)] public static extern bool ReadConsoleOutputW(
    IntPtr h, [Out] CHAR_INFO[] buffer, COORD size, COORD coord, ref SMALL_RECT region);
'@

$log = [ordered]@{ agent_pid = $AgentPid }
try {
    [void][Dump.Con]::FreeConsole()
    if (-not [Dump.Con]::AttachConsole([uint32]$AgentPid)) {
        throw "AttachConsole failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
    $h = [Dump.Con]::CreateFileW('CONOUT$', ([uint32]3221225472), ([uint32]3), [IntPtr]::Zero, ([uint32]3), ([uint32]0), [IntPtr]::Zero)
    if ($h -eq [IntPtr]::new(-1)) { throw "CONOUT$ failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())" }

    $info = New-Object Dump.Con+CONSOLE_SCREEN_BUFFER_INFO
    if (-not [Dump.Con]::GetConsoleScreenBufferInfo($h, [ref]$info)) {
        throw "buffer info failed: $([Runtime.InteropServices.Marshal]::GetLastWin32Error())"
    }
    # The raw cursor, not a parser's idea of it: this is the number that decides
    # whether the window actually sits where the program writes.
    $log['buffer']   = @{ x = $info.dwSize.X; y = $info.dwSize.Y }
    $log['cursor']   = @{ x = $info.dwCursorPosition.X; y = $info.dwCursorPosition.Y }
    $log['window']   = @{ left = $info.srWindow.Left; top = $info.srWindow.Top
                          right = $info.srWindow.Right; bottom = $info.srWindow.Bottom }
    $log['cursor_inside_window'] = ($info.dwCursorPosition.Y -ge $info.srWindow.Top -and
                                    $info.dwCursorPosition.Y -le $info.srWindow.Bottom)

    # Read the whole buffer one row at a time: a single 141x9001 read is both a
    # large allocation and, on some hosts, refused outright.
    $cols = $info.dwSize.X
    $rows = [Math]::Min($info.dwSize.Y, $MaxRows)
    $log['rows_walked'] = $rows
    $log['rows_in_buffer'] = $info.dwSize.Y
    $size = New-Object Dump.Con+COORD; $size.X = [int16]$cols; $size.Y = [int16]1
    $origin = New-Object Dump.Con+COORD; $origin.X = [int16]0; $origin.Y = [int16]0
    $row = New-Object 'Dump.Con+CHAR_INFO[]' $cols

    $nonBlank = @()
    $failed = 0
    for ($y = 0; $y -lt $rows; $y++) {
        $region = New-Object Dump.Con+SMALL_RECT
        $region.Left = 0; $region.Top = [int16]$y
        $region.Right = [int16]($cols - 1); $region.Bottom = [int16]$y
        if (-not [Dump.Con]::ReadConsoleOutputW($h, $row, $size, $origin, [ref]$region)) { $failed++; continue }
        $text = -join ($row | ForEach-Object { $_.Char.UnicodeChar })
        if ($text.Trim([char]0, ' ').Length -gt 0) {
            $nonBlank += @{ row = $y
                            in_window = ($y -ge $info.srWindow.Top -and $y -le $info.srWindow.Bottom)
                            text = $text.Trim([char]0, ' ').Substring(0, [Math]::Min(80, $text.Trim([char]0,' ').Length)) }
        }
    }
    $log['rows_read_failed'] = $failed
    $log['non_blank_rows']   = $nonBlank.Count
    $log['non_blank_inside_window']  = @($nonBlank | Where-Object { $_.in_window }).Count
    $log['non_blank_outside_window'] = @($nonBlank | Where-Object { -not $_.in_window }).Count
    # The fork, stated as one field so the answer is not a matter of reading.
    $log['verdict'] = if ($nonBlank.Count -eq 0) { 'whole-buffer-blank: ask who repaints' }
                      elseif ($log['non_blank_inside_window'] -eq 0) { 'content-outside-window: the scraped rectangle is wrong' }
                      else { 'content-inside-window: the blank is not in this buffer' }
    # A sample, capped: these rows are terminal contents, so only the first few.
    $log['sample'] = @($nonBlank | Select-Object -First 5)
} catch {
    $log['threw'] = "$_"
} finally {
    # Detach before leaving: a probe must not keep the agent's console attached.
    [void][Dump.Con]::FreeConsole()
    $log | ConvertTo-Json -Depth 8 | Set-Content -Encoding UTF8 $Out
}
