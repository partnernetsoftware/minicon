param(
    [Parameter(Mandatory = $true)][string]$Bin,
    [int]$SettleSeconds = 5
)
$ErrorActionPreference = "Stop"
if (-not (Test-Path -LiteralPath $Bin)) {
    throw "missing $Bin"
}
$proc = Start-Process -FilePath $Bin -PassThru
Start-Sleep -Seconds $SettleSeconds
if ($proc.HasExited) {
    Write-Output 'TINYGUI_RECEIPT {"status":"exited"}'
    exit 1
}
$live = Get-Process -Id $proc.Id
$rssKib = [int]($live.WorkingSet64 / 1024)
$cpu = [math]::Round($live.CPU, 2)
Stop-Process -Id $proc.Id -Force
Wait-Process -Id $proc.Id -Timeout 5 -ErrorAction SilentlyContinue
Write-Output ("TINYGUI_RECEIPT {0}" -f (@{
    status = "ok"
    rss_kib = $rssKib
    cpu_s = $cpu
} | ConvertTo-Json -Compress))
