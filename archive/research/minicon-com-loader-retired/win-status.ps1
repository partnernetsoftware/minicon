$ErrorActionPreference = "Stop"
$exe = "C:\Users\Public\minicon-ape.exe"
$out = "C:\Users\Public\mc.out"
$err = "C:\Users\Public\mc.err"
Remove-Item -LiteralPath $out, $err -ErrorAction SilentlyContinue
if (-not (Test-Path -LiteralPath $exe)) {
    "missing $exe" | Set-Content -LiteralPath $err
    exit 2
}
$p = Start-Process -FilePath $exe -ArgumentList "--status" -Wait -PassThru `
    -RedirectStandardOutput $out -RedirectStandardError $err
"exit=$($p.ExitCode)" | Set-Content -LiteralPath "C:\Users\Public\mc.rc"
exit $p.ExitCode
