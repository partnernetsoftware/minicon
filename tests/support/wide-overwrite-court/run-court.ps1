# Same-court comparison: run the misalignment fixture under each backend,
# wait for the sentinel, capture parser state and pixels, kill the window.
param([string]$Backend)  # "agent" | "conpty" | "rust-console"
$exe = "D:\dev\partnernetsoftware\minicon\target\debug\minicon.exe"
$fix = "$PSScriptRoot\misalign-fixture.ps1"
$ep  = "pipe:\\.\pipe\minicon-misalign-$Backend"
$outdir = "$PSScriptRoot\court-$Backend"
New-Item -ItemType Directory -Force $outdir | Out-Null

$args = @()
if ($Backend -ne 'agent') { $args += @('--feature', $Backend) }
$args += @('--control', $ep, '-e', 'powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $fix)
Start-Process -FilePath $exe -ArgumentList $args
Start-Sleep -Seconds 5

& $exe cli --control $ep wait-text --target '@1' --timeout-ms 60000 FIXTURE_DONE | Out-Null
Start-Sleep -Seconds 2   # let any trailing agent poll settle
& $exe cli --control $ep capture-pane --target '@1' --output "$outdir\pane.txt" | Out-Null
& $exe cli --control $ep screenshot-pane --target '@1' --output "$outdir\shot.png" | Out-Null
& $exe cli --control $ep ui-snapshot | Set-Content "$outdir\ui.json"
& $exe cli --control $ep close-window | Out-Null
"court $Backend done -> $outdir"
