# Two new processes, two logical inner sizes. Same frozen PE. IME on.
# No screenshot barrier. First-frame = perf-stats present/host_direct/frames.
param(
    [Parameter(Mandatory = $true)]
    [string]$Exe,
    [Parameter(Mandatory = $true)]
    [string]$RegionsProbe,
    [Parameter(Mandatory = $true)]
    [string]$Log,
    [Parameter(Mandatory = $true)]
    [string]$Result
)

$ErrorActionPreference = "Stop"
$PSDefaultParameterValues["Out-File:Encoding"] = "utf8"
$env:AGENTERM_NO_ACTIVATE = "1"
$exitCode = 1

Add-Type @"
using System;
using System.Runtime.InteropServices;
public static class ClientGeom {
    [StructLayout(LayoutKind.Sequential)]
    public struct RECT { public int Left, Top, Right, Bottom; }
    [DllImport("user32.dll")] public static extern bool GetClientRect(IntPtr hwnd, out RECT r);
    [DllImport("user32.dll")] public static extern uint GetDpiForWindow(IntPtr hwnd);
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

function Wait-Ready([string]$Pipe, [int]$TimeoutMs) {
    $deadline = [datetime]::UtcNow.AddMilliseconds($TimeoutMs)
    while ([datetime]::UtcNow -lt $deadline) {
        $r = Invoke-Cli $Pipe @('list-tabs')
        if ($r.Code -eq 0 -and $r.Text) { return $r.Text }
        Start-Sleep -Milliseconds 50
    }
    throw "control endpoint not ready: $Pipe"
}

function Wait-FirstFrame([string]$Pipe, [int]$TimeoutMs) {
    $deadline = [datetime]::UtcNow.AddMilliseconds($TimeoutMs)
    $last = ""
    while ([datetime]::UtcNow -lt $deadline) {
        $r = Invoke-Cli $Pipe @('perf-stats')
        $last = $r.Text
        if ($r.Code -eq 0 -and $r.Text) {
            $j = $r.Text | ConvertFrom-Json
            $present = 0; $direct = 0; $frames = 0
            if ($null -ne $j.present_success) { $present = [int64]$j.present_success }
            if ($null -ne $j.host_direct_frames) { $direct = [int64]$j.host_direct_frames }
            if ($null -ne $j.frames) { $frames = [int64]$j.frames }
            if ($present -ge 1 -or $direct -ge 1 -or $frames -ge 1) {
                return @{ Json = $j; Raw = $r.Text; Present = $present; Direct = $direct; Frames = $frames }
            }
        }
        Start-Sleep -Milliseconds 40
    }
    throw "first frame not observed via perf-stats: $last"
}

function Sample-Size([string]$Label, [int]$LogicalW, [int]$LogicalH) {
    $pipe = 'pipe:\\.\pipe\minicon-size-' + [guid]::NewGuid().ToString('N')
    $gui = $null
    try {
        Write-Log ("size_begin label={0} logical={1}x{2} ime_allowed=true no_screenshot" -f $Label, $LogicalW, $LogicalH)
        $gui = Start-Process -FilePath $Exe -ArgumentList @(
            '--no-activate', '--cols', '80', '--rows', '24',
            '--control', $pipe, '-e', 'cmd.exe', '/Q', '/K'
        ) -PassThru -WindowStyle Normal
        $null = $gui.Handle
        $null = Wait-Ready $pipe 15000
        $resized = Invoke-Cli $pipe @('resize-window', '--width', [string]$LogicalW, '--height', [string]$LogicalH)
        if ($resized.Code -ne 0) { throw "resize-window: $($resized.Text)" }
        $reset = Invoke-Cli $pipe @('reset-perf-stats')
        if ($reset.Code -ne 0) { throw "reset-perf-stats: $($reset.Text)" }
        $frame = Wait-FirstFrame $pipe 10000
        $hwnd = $gui.MainWindowHandle
        $dpi = 0
        $cw = 0
        $ch = 0
        if ($hwnd -ne [IntPtr]::Zero) {
            $dpi = [ClientGeom]::GetDpiForWindow($hwnd)
            $rect = New-Object ClientGeom+RECT
            if ([ClientGeom]::GetClientRect($hwnd, [ref]$rect)) {
                $cw = $rect.Right - $rect.Left
                $ch = $rect.Bottom - $rect.Top
            }
        }
        $pixels = [uint64]$cw * [uint64]$ch
        $dib = $pixels * 4
        Write-Log ("size_geom label={0} hwnd=0x{1:x} dpi={2} client={3}x{4} pixel_area={5} dib_bytes={6} present_success={7} host_direct_frames={8} frames={9}" -f `
            $Label, [int64]$hwnd, $dpi, $cw, $ch, $pixels, $dib, $frame.Present, $frame.Direct, $frame.Frames)
        Write-Log ("size_perf label={0} {1}" -f $Label, ($frame.Raw -replace "`r|`n", " "))
        $regions = & powershell.exe -NoProfile -File $RegionsProbe -TargetPid $gui.Id
        $regions | ForEach-Object { Write-Log ("size_qws label={0} {1}" -f $Label, $_) }
        $null = Invoke-Cli $pipe @('close-window')
        if (-not $gui.WaitForExit(20000)) {
            Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
            throw "GUI WaitForExit timeout $Label"
        }
        $gui.Refresh()
        if ($null -eq $gui.ExitCode) { throw "wrapper failure: ExitCode null $Label" }
        Write-Log ("size_end label={0} pid={1} exit={2}" -f $Label, $gui.Id, $gui.ExitCode)
    } finally {
        if ($null -ne $gui -and -not $gui.HasExited) {
            Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
        }
    }
}

function Sample-Resize([string]$Label, [int]$FromW, [int]$FromH, [int]$ToW, [int]$ToH) {
    $pipe = 'pipe:\\.\pipe\minicon-size-' + [guid]::NewGuid().ToString('N')
    $gui = $null
    try {
        Write-Log ("resize_begin label={0} from={1}x{2} to={3}x{4} same_process ime_allowed=true no_screenshot" -f $Label, $FromW, $FromH, $ToW, $ToH)
        $gui = Start-Process -FilePath $Exe -ArgumentList @(
            '--no-activate', '--cols', '80', '--rows', '24',
            '--control', $pipe, '-e', 'cmd.exe', '/Q', '/K'
        ) -PassThru -WindowStyle Normal
        $null = $gui.Handle
        $null = Wait-Ready $pipe 15000
        foreach ($pair in @(@($FromW, $FromH, 'before'), @($ToW, $ToH, 'after'))) {
            $w = [int]$pair[0]
            $h = [int]$pair[1]
            $tag = [string]$pair[2]
            $resized = Invoke-Cli $pipe @('resize-window', '--width', [string]$w, '--height', [string]$h)
            if ($resized.Code -ne 0) { throw "resize-window $tag : $($resized.Text)" }
            $null = Invoke-Cli $pipe @('reset-perf-stats')
            $frame = Wait-FirstFrame $pipe 10000
            $hwnd = $gui.MainWindowHandle
            $dpi = 0
            $cw = 0
            $ch = 0
            if ($hwnd -ne [IntPtr]::Zero) {
                $dpi = [ClientGeom]::GetDpiForWindow($hwnd)
                $rect = New-Object ClientGeom+RECT
                if ([ClientGeom]::GetClientRect($hwnd, [ref]$rect)) {
                    $cw = $rect.Right - $rect.Left
                    $ch = $rect.Bottom - $rect.Top
                }
            }
            $pixels = [uint64]$cw * [uint64]$ch
            Write-Log ("resize_geom label={0} phase={1} dpi={2} client={3}x{4} pixel_area={5} dib_bytes={6} present_success={7} frames={8}" -f `
                $Label, $tag, $dpi, $cw, $ch, $pixels, ($pixels * 4), $frame.Present, $frame.Frames)
            $regions = & powershell.exe -NoProfile -File $RegionsProbe -TargetPid $gui.Id
            $regions | ForEach-Object { Write-Log ("resize_qws label={0} phase={1} {2}" -f $Label, $tag, $_) }
        }
        $null = Invoke-Cli $pipe @('close-window')
        if (-not $gui.WaitForExit(20000)) {
            Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
            throw "GUI WaitForExit timeout $Label"
        }
        $gui.Refresh()
        if ($null -eq $gui.ExitCode) { throw "wrapper failure: ExitCode null $Label" }
        Write-Log ("resize_end label={0} pid={1} exit={2}" -f $Label, $gui.Id, $gui.ExitCode)
    } finally {
        if ($null -ne $gui -and -not $gui.HasExited) {
            Stop-Process -Id $gui.Id -Force -ErrorAction SilentlyContinue
        }
    }
}

try {
    Set-Content -LiteralPath $Log -Value "start size-compare frozen-pe ime_allowed=true chinese_input_preserved no_screenshot no_32kib_court" -Encoding utf8
    Sample-Size 'small' 480 300
    Sample-Size 'large' 960 600
    Sample-Resize 'resize' 960 600 480 300
    $exitCode = 0
} catch {
    $_ | Out-String | Add-Content -LiteralPath $Log -Encoding utf8
    $exitCode = 1
} finally {
    [IO.File]::WriteAllText($Result + '.tmp', [string]$exitCode)
    Move-Item -LiteralPath ($Result + '.tmp') -Destination $Result -Force
}
exit $exitCode
