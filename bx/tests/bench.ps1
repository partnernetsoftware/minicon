# bx vs node cold-start benchmark
$root = Split-Path -Parent $PSScriptRoot
$bx = Join-Path $root "target\release\bx.exe"
$hello = Join-Path $root "examples\hello.bxs"
$helloBxb = Join-Path $root "target\bx-tests\hello.bxb"
New-Item -ItemType Directory -Path (Split-Path $helloBxb) -Force | Out-Null
& $bx build $hello -o $helloBxb 2>$null | Out-Null

function Measure-Runs($label, [scriptblock]$block, $n = 15) {
    & $block | Out-Null  # warm the OS file cache
    $times = @()
    for ($i = 0; $i -lt $n; $i++) {
        $sw = [System.Diagnostics.Stopwatch]::StartNew()
        & $block | Out-Null
        $sw.Stop()
        $times += $sw.Elapsed.TotalMilliseconds
    }
    $sorted = $times | Sort-Object
    $min = $sorted[0]
    $median = $sorted[[int]($n / 2)]
    "{0,-34} min {1,7:0.0} ms   median {2,7:0.0} ms" -f $label, $min, $median
}

Write-Output "cold start, end to end (includes process spawn)"
Measure-Runs "node -e 1+1" { & node -e "1+1" }
Measure-Runs "node -e (import fs)" { & node -e "require('fs')" }
Measure-Runs "bx eval 1+1" { & $bx eval "1+1" }
Measure-Runs "bx run examples/hello.bxs" { & $bx run $hello }
Measure-Runs "bx run compiled hello.bxb" { & $bx run $helloBxb }

Write-Output ""
Write-Output "a real job: read JSON, filter, sort, emit"
$job = Join-Path $root "target\bx-tests\job.bxs"
@'
let items = parse(read(__dir__ + "/../../tests/cases/sample.json")).items
let scores = map(items, fn(it) { return [it.name, it.score * 1000] })
sort(scores, fn(a, b) { return b[1] - a[1] })
print(join(map(scores, fn(s) { return s[0] + "=" + s[1] }), " "))
'@ | Set-Content -Path $job -Encoding utf8
Measure-Runs "bx run job.bxs" { & $bx run $job }

Write-Output ""
Write-Output "pure compute: fib(24) recursive"
Measure-Runs "bx eval fib(24)" { & $bx eval "fn fib(n) { if (n < 2) { return n } return fib(n-1) + fib(n-2) } fib(24)" } 7
Measure-Runs "node -e fib(24)" { & node -e "function fib(n){return n<2?n:fib(n-1)+fib(n-2)}; fib(24)" } 7

Write-Output ""
Write-Output "large generated script: compile once, execute bytecode after"
$bigDir = Join-Path $root "target\bx-tests"
$big = Join-Path $bigDir "big.bxs"
$bigBxb = Join-Path $bigDir "big.bxb"
$lines = New-Object System.Collections.Generic.List[string]
$lines.Add("fn step(n) { return n + 1 }")
for ($i = 0; $i -lt 30000; $i++) { $lines.Add("let v$i = step($i)") }
$lines.Add('print("big done")')
[IO.File]::WriteAllLines($big, $lines)
"big.bxs has $($lines.Count) lines, $((Get-Item $big).Length) bytes"

Measure-Runs "bx build big.bxs (parse+compile)" { & $bx build $big -o $bigBxb 2>$null } 5
Measure-Runs "bx run big.bxb (decode only)" { & $bx run $bigBxb } 5
Measure-Runs "bx run big.bxs (no cache)" {
    Remove-Item (Join-Path $bigDir ".bxcache") -Recurse -Force -ErrorAction SilentlyContinue
    & $bx run $big
} 5
"big.bxb is $((Get-Item $bigBxb).Length) bytes"