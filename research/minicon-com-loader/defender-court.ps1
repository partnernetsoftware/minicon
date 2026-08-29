$ErrorActionPreference = "Stop"

$root = "C:\minicon-six\defender"
$binary = Join-Path $root "minicon.com"
$manifestPath = Join-Path $root "candidate-manifest.json"
$receiptPath = Join-Path $root "defender-receipt.json"
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$asset = @($manifest.assets | Where-Object { $_.name -eq "minicon.com" })
if ($asset.Count -ne 1) { throw "Candidate manifest must contain exactly one minicon.com" }
$expected = "$($asset[0].sha256)".ToLowerInvariant()
$before = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
if ($before -ne $expected) { throw "pre-scan digest mismatch" }

$status = Get-MpComputerStatus
$status | Select-Object AMServiceEnabled, AntivirusEnabled, RealTimeProtectionEnabled,
    AMProductVersion, AMEngineVersion, AntivirusSignatureVersion |
    ConvertTo-Json -Compress | Write-Host
if (-not $status.AntivirusEnabled -or -not $status.RealTimeProtectionEnabled) {
    throw "Microsoft Defender is not active"
}
$started = Get-Date
Start-MpScan -ScanType CustomScan -ScanPath $binary

if (-not (Test-Path -LiteralPath $binary)) {
    throw "Defender removed or quarantined Candidate APE"
}
$after = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
if ($after -ne $expected) { throw "post-scan digest mismatch" }
$detections = @(Get-MpThreatDetection -ErrorAction SilentlyContinue | Where-Object {
    $_.InitialDetectionTime -ge $started -or
    (@($_.Resources) | Where-Object { $_ -like "*$binary*" }).Count -gt 0
})
if ($detections.Count -ne 0) {
    $detections | ConvertTo-Json -Depth 6 | Write-Error
    throw "Microsoft Defender reported $($detections.Count) detection(s)"
}

$receipt = [ordered]@{
    schema = 1
    kind = "minicon-defender-court"
    source_sha = "$($manifest.source_sha)"
    candidate_run = @{
        id = [long]$manifest.candidate_run.id
        attempt = [int]$manifest.candidate_run.attempt
    }
    minicon_com_sha256 = $after
    verdict = "clean"
    provider = "Microsoft Defender"
    product_version = "$($status.AMProductVersion)"
    engine_version = "$($status.AMEngineVersion)"
    signature_version = "$($status.AntivirusSignatureVersion)"
    signature_updated_at = "$($status.AntivirusSignatureLastUpdated.ToUniversalTime().ToString('o'))"
    scanned_at = "$((Get-Date).ToUniversalTime().ToString('o'))"
}
$receipt | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $receiptPath -Encoding utf8
Write-Host "PASS Defender exact Candidate SHA $after"
