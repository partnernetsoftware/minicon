# Exact production PE: --no-activate must not steal foreground.
# After explicit SetForegroundWindow, English SendInput (not control PTY write).
# No MINICON_INIT_* research skip variables. No screenshot.
param(
    [Parameter(Mandatory = $true)]
    [string]$Exe,
    [Parameter(Mandatory = $true)]
    [string]$Log,
    [Parameter(Mandatory = $true)]
    [string]$Result
)

$ErrorActionPreference = "Stop"
$PSDefaultParameterValues["Out-File:Encoding"] = "utf8"
$exitCode = 1
$proc = $null

Add-Type @"
using System;
using System.Collections.Generic;
using System.Runtime.InteropServices;

public static class FgNative {
    public delegate bool EnumWindowsProc(IntPtr hWnd, IntPtr lParam);
    [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
    [DllImport("user32.dll")] public static extern IntPtr GetFocus();
    [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern IntPtr SetFocus(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr hWnd, out uint pid);
    [DllImport("user32.dll")] public static extern bool AttachThreadInput(uint idAttach, uint idAttachTo, bool fAttach);
    [DllImport("user32.dll")] public static extern bool EnumWindows(EnumWindowsProc lpEnumFunc, IntPtr lParam);
    [DllImport("user32.dll")] public static extern bool IsWindowVisible(IntPtr hWnd);
    [DllImport("user32.dll")] public static extern int GetClassNameW(IntPtr hWnd, [MarshalAs(UnmanagedType.LPWStr)] System.Text.StringBuilder lpClassName, int nMaxCount);
    [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
    [DllImport("user32.dll")] public static extern int GetKeyboardLayoutList(int nBuff, IntPtr[] lpList);
    [DllImport("user32.dll")] public static extern IntPtr GetKeyboardLayout(uint idThread);
    [DllImport("user32.dll", SetLastError = true)] public static extern uint SendInput(uint nInputs, INPUT[] pInputs, int cbSize);

    public const uint INPUT_KEYBOARD = 1;
    public const uint KEYEVENTF_KEYUP = 0x0002;

    [StructLayout(LayoutKind.Sequential)]
    public struct MOUSEINPUT {
        public int dx;
        public int dy;
        public uint mouseData;
        public uint dwFlags;
        public uint time;
        public UIntPtr dwExtraInfo;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct KEYBDINPUT {
        public ushort wVk;
        public ushort wScan;
        public uint dwFlags;
        public uint time;
        public UIntPtr dwExtraInfo;
    }
    [StructLayout(LayoutKind.Explicit)]
    public struct InputUnion {
        [FieldOffset(0)] public MOUSEINPUT mi;
        [FieldOffset(0)] public KEYBDINPUT ki;
    }
    [StructLayout(LayoutKind.Sequential)]
    public struct INPUT {
        public uint type;
        public InputUnion U;
    }

    public static IntPtr FindVisibleHwnd(uint pid) {
        IntPtr found = IntPtr.Zero;
        EnumWindows((hWnd, lParam) => {
            uint windowPid;
            GetWindowThreadProcessId(hWnd, out windowPid);
            if (windowPid != pid || !IsWindowVisible(hWnd)) return true;
            var name = new System.Text.StringBuilder(256);
            GetClassNameW(hWnd, name, name.Capacity);
            if (name.ToString() == "AgenTermNativePixelWindow") {
                found = hWnd;
                return false;
            }
            if (found == IntPtr.Zero) found = hWnd;
            return true;
        }, IntPtr.Zero);
        return found;
    }

    public static int SendVk(ushort vk) {
        var down = new INPUT();
        down.type = INPUT_KEYBOARD;
        down.U.ki.wVk = vk;
        var up = new INPUT();
        up.type = INPUT_KEYBOARD;
        up.U.ki.wVk = vk;
        up.U.ki.dwFlags = KEYEVENTF_KEYUP;
        var inputs = new INPUT[] { down, up };
        return (int)SendInput((uint)inputs.Length, inputs, Marshal.SizeOf(typeof(INPUT)));
    }
}
"@

function Write-Log([string]$line) {
    Add-Content -LiteralPath $Log -Value $line -Encoding utf8
}

function Invoke-Cli([string]$Pipe, [string[]]$Arguments) {
    $prev = $ErrorActionPreference
    $ErrorActionPreference = 'Continue'
    try {
        $out = & $Exe @('cli', '--control', $Pipe) @Arguments 2>&1 | ForEach-Object { $_.ToString() } | Out-String
        return @{ Code = $LASTEXITCODE; Text = $out.Trim() }
    } finally {
        $ErrorActionPreference = $prev
    }
}

function Get-Ws([int]$ProcessId) {
    $p = Get-Process -Id $ProcessId -ErrorAction SilentlyContinue
    if ($null -eq $p) { return 0 }
    return [int64]$p.WorkingSet64
}

try {
    Set-Content -LiteralPath $Log -Value "start no-activate-behavior exact_production_pe no_research_skip_vars no_screenshot no_pty_control_write ime_allowed=true" -Encoding utf8
    Remove-Item Env:MINICON_INIT_SKIP_FOCUS -ErrorAction SilentlyContinue
    Remove-Item Env:MINICON_INIT_ACTIVATE_AFTER_PRESENT -ErrorAction SilentlyContinue
    Remove-Item Env:MINICON_INIT_TRACE -ErrorAction SilentlyContinue
    Remove-Item Env:AGENTERM_NO_ACTIVATE -ErrorAction SilentlyContinue

    $pipe = 'pipe:\\.\pipe\minicon-noact-' + [guid]::NewGuid().ToString('N')
    $fgBefore = [FgNative]::GetForegroundWindow()
    Write-Log ("fg_before_start=0x{0:x}" -f [int64]$fgBefore)

    $proc = Start-Process -FilePath $Exe -ArgumentList @(
        '--no-activate', '--cols', '80', '--rows', '24',
        '--control', $pipe, '-e', 'cmd.exe', '/Q', '/K'
    ) -PassThru -WindowStyle Normal
    $null = $proc.Handle
    Write-Log ("pid={0}" -f $proc.Id)

    $deadline = [datetime]::UtcNow.AddMilliseconds(15000)
    $ready = $false
    while ([datetime]::UtcNow -lt $deadline) {
        $cli = Invoke-Cli $pipe @('list-tabs')
        if ($cli.Code -eq 0 -and $cli.Text) { $ready = $true; break }
        Start-Sleep -Milliseconds 40
    }
    if (-not $ready) { throw "control not ready" }

    $deadline = [datetime]::UtcNow.AddMilliseconds(10000)
    $framed = $false
    while ([datetime]::UtcNow -lt $deadline) {
        $cli = Invoke-Cli $pipe @('perf-stats')
        if ($cli.Code -eq 0 -and $cli.Text) {
            $j = $cli.Text | ConvertFrom-Json
            $present = 0; $frames = 0
            if ($null -ne $j.present_success) { $present = [int64]$j.present_success }
            if ($null -ne $j.frames) { $frames = [int64]$j.frames }
            if ($present -ge 1 -or $frames -ge 1) { $framed = $true; break }
        }
        Start-Sleep -Milliseconds 40
    }
    if (-not $framed) { throw "first frame not observed" }
    Start-Sleep -Milliseconds 250

    $hwnd = [FgNative]::FindVisibleHwnd([uint32]$proc.Id)
    $fgAfter = [FgNative]::GetForegroundWindow()
    $fgOurs = [int](($hwnd -ne [IntPtr]::Zero) -and ($fgAfter -eq $hwnd))
    $wsFrame = Get-Ws $proc.Id
    Write-Log ("after_first_frame hwnd=0x{0:x} fg=0x{1:x} fg_ours={2} ws={3}" -f [int64]$hwnd, [int64]$fgAfter, $fgOurs, $wsFrame)
    if ($hwnd -eq [IntPtr]::Zero) { throw "no visible hwnd" }
    if ($fgOurs -ne 0) {
        Write-Log "FAIL no-activate stole foreground"
        throw "no-activate stole foreground"
    }
    Write-Log "PASS no-activate did not steal foreground"

    $paneBefore = (Invoke-Cli $pipe @('capture-pane', '--max-bytes', '8000')).Text
    Write-Log ("capture_before_input_len={0}" -f $paneBefore.Length)

    [uint32]$fgPidIgnored = 0
    $fgTid = [FgNative]::GetWindowThreadProcessId($fgAfter, [ref]$fgPidIgnored)
    $ourTid = [FgNative]::GetCurrentThreadId()
    $attached = 0
    if ($fgTid -ne 0 -and $ourTid -ne 0 -and $fgTid -ne $ourTid) {
        $attached = [int][FgNative]::AttachThreadInput($fgTid, $ourTid, $true)
    }
    $fgOk = [int][FgNative]::SetForegroundWindow($hwnd)
    $null = [FgNative]::SetFocus($hwnd)
    if ($attached -ne 0) { [void][FgNative]::AttachThreadInput($fgTid, $ourTid, $false) }
    Start-Sleep -Milliseconds 80
    $fgAct = [FgNative]::GetForegroundWindow()
    $fgOursAct = [int]($fgAct -eq $hwnd)
    $wsAct = Get-Ws $proc.Id
    Write-Log ("after_explicit_activate fg_ok={0} attached={1} fg=0x{2:x} fg_ours={3} ws={4}" -f $fgOk, $attached, [int64]$fgAct, $fgOursAct, $wsAct)

    $sentA = [FgNative]::SendVk(0x41)
    $sentB = [FgNative]::SendVk(0x42)
    $sentC = [FgNative]::SendVk(0x43)
    Write-Log ("english_sendinput vk=ABC sent={0},{1},{2} pty_control_write=0" -f $sentA, $sentB, $sentC)
    Start-Sleep -Milliseconds 400
    $paneAfter = (Invoke-Cli $pipe @('capture-pane', '--max-bytes', '8000')).Text
    $wsInput = Get-Ws $proc.Id
    $hasAbc = [int]($paneAfter -match 'abc')
    Write-Log ("capture_after_english has_abc={0} ws={1}" -f $hasAbc, $wsInput)
    Write-Log ("capture_after_english_text<<<")
    Write-Log $paneAfter
    Write-Log (">>>capture_after_english_text")
    if ($hasAbc -eq 1) {
        Write-Log "PASS english SendInput visible in capture-pane"
    } else {
        Write-Log "FAIL english SendInput not visible in capture-pane"
        throw "english input not observed"
    }

    $nLayouts = [FgNative]::GetKeyboardLayoutList(0, $null)
    $layouts = New-Object IntPtr[] ([Math]::Max($nLayouts, 1))
    $nLayouts = [FgNative]::GetKeyboardLayoutList($layouts.Length, $layouts)
    $current = [FgNative]::GetKeyboardLayout(0)
    $hasZh = 0
    for ($i = 0; $i -lt $nLayouts; $i++) {
        $langid = [uint32]([int64]$layouts[$i] -band 0xffff)
        if (($langid -band 0x3ff) -eq 0x04) { $hasZh = 1 }
    }
    Write-Log ("chinese_ime_probe n_layouts={0} current_langid=0x{1:x} has_zh={2}" -f $nLayouts, ([int64]$current -band 0xffff), $hasZh)
    if ($hasZh -eq 0) {
        Write-Log "chinese_ime_BLOCKED_no_zh_keyboard_layout"
    } else {
        Write-Log "chinese_ime_BLOCKED_no_real_tsf_compose_in_utm_job"
    }

    $null = Invoke-Cli $pipe @('close-window')
    if (-not $proc.WaitForExit(20000)) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
        throw "WaitForExit timeout"
    }
    $proc.Refresh()
    if ($null -eq $proc.ExitCode) { throw "wrapper failure" }
    $exitCode = 0
} catch {
    $_ | Out-String | Add-Content -LiteralPath $Log -Encoding utf8
    $exitCode = 1
    if ($null -ne $proc -and -not $proc.HasExited) {
        Stop-Process -Id $proc.Id -Force -ErrorAction SilentlyContinue
    }
} finally {
    [IO.File]::WriteAllText($Result + '.tmp', [string]$exitCode)
    Move-Item -LiteralPath ($Result + '.tmp') -Destination $Result -Force
}
exit $exitCode
