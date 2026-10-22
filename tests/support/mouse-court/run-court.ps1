# Mouse court: drive the mouse fixture under one backend, inject press,
# release and wheel at known cells, capture what the child received.
param([string]$Backend)
$exe = "D:\dev\partnernetsoftware\minicon\target\debug\minicon.exe"
$fix = "$PSScriptRoot\mouse-fixture.ps1"
$ep  = "pipe:\\.\pipe\minicon-mouse-$Backend"
$outdir = "$PSScriptRoot\mouse-$Backend"
New-Item -ItemType Directory -Force $outdir | Out-Null

$args = @()
if ($Backend -ne 'agent') { $args += @('--feature', $Backend) }
$args += @('--control', $ep, '-e', 'powershell', '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', $fix)
Start-Process -FilePath $exe -ArgumentList $args
Start-Sleep -Seconds 5

& $exe cli --control $ep wait-text --target '@1' --timeout-ms 30000 MOUSE_COURT_READY | Out-Null
# Cell-addressed terminal mouse injection: press+release at (col 10, row 5),
# then a wheel tick at (col 20, row 6).
& $exe cli --control $ep send-mouse --action press --button left --column 10 --row 5 | Out-Null
& $exe cli --control $ep send-mouse --action release --button left --column 10 --row 5 | Out-Null
& $exe cli --control $ep send-wheel --column 20 --row 6 --notches -1 | Out-Null
Start-Sleep -Seconds 2
& $exe cli --control $ep send-text --target '@1' 'q' | Out-Null
Start-Sleep -Seconds 2
& $exe cli --control $ep capture-pane --target '@1' --output "$outdir\pane.txt" | Out-Null
& $exe cli --control $ep close-window | Out-Null
"mouse court $Backend -> $outdir"
