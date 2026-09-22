$ErrorActionPreference = "Stop"

$root = "C:\minicon-six\defender"
$manifestPath = Join-Path $root "candidate-manifest.json"
$receiptPath = Join-Path $root "defender-receipt.json"
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$evidenceScope = if ($manifest.defender_evidence_scope) { "$($manifest.defender_evidence_scope)" } else { "candidate" }
$scanAssets = @($manifest.defender_scan_assets)
if ($scanAssets.Count -lt 1) { throw "scan manifest has no Defender assets" }
$before = [ordered]@{}
foreach ($asset in $scanAssets) {
    $path = Join-Path $root "files\$($asset.file)"
    $expected = "$($asset.sha256)".ToLowerInvariant()
    if (-not (Test-Path -LiteralPath $path)) { throw "$($asset.key): scan file missing" }
    $actual = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash.ToLowerInvariant()
    if ($actual -ne $expected) { throw "$($asset.key): pre-scan digest mismatch" }
    $before[$asset.key] = [ordered]@{ path = $path; sha256 = $expected }
}

$deadline = (Get-Date).AddSeconds(120)
do {
    $status = Get-MpComputerStatus
    if ($status.AMServiceEnabled -and $status.AntivirusEnabled -and
        $status.RealTimeProtectionEnabled -and "$($status.AMEngineVersion)" -ne "0.0.0.0") { break }
    Start-Sleep -Seconds 2
} while ((Get-Date) -lt $deadline)
$status | Select-Object AMServiceEnabled, AntivirusEnabled, RealTimeProtectionEnabled,
    AMProductVersion, AMEngineVersion, AntivirusSignatureVersion |
    ConvertTo-Json -Compress | Write-Host
if (-not $status.AMServiceEnabled -or -not $status.AntivirusEnabled -or
    -not $status.RealTimeProtectionEnabled -or "$($status.AMEngineVersion)" -eq "0.0.0.0") {
    throw "Microsoft Defender did not become active within 120 seconds"
}
$started = Get-Date
$scanError = ""
try { Start-MpScan -ScanType CustomScan -ScanPath (Join-Path $root "files") }
catch { $scanError = $_.Exception.Message }
$detections = @(Get-MpThreatDetection -ErrorAction SilentlyContinue | Where-Object {
    $_.InitialDetectionTime -ge $started -or
    (@($_.Resources) | Where-Object { $_ -like "*$root\files*" }).Count -gt 0
})
$threats = @($detections | ForEach-Object {
    $detection = $_
    $catalog = Get-MpThreatCatalog -ThreatID $detection.ThreatID -ErrorAction SilentlyContinue
    [ordered]@{
        threat_id = [long]$detection.ThreatID
        threat_name = if ($catalog) { "$($catalog.ThreatName)" } else { "unknown" }
        action_success = [bool]$detection.ActionSuccess
        resources = @($detection.Resources | ForEach-Object { "$_" })
        detected_at = "$($detection.InitialDetectionTime.ToUniversalTime().ToString('o'))"
    }
})
$assets = [ordered]@{}
foreach ($entry in $before.GetEnumerator()) {
    $after = ""
    if (Test-Path -LiteralPath $entry.Value.path) {
        try { $after = (Get-FileHash -LiteralPath $entry.Value.path -Algorithm SHA256).Hash.ToLowerInvariant() }
        catch { if (-not $scanError) { $scanError = $_.Exception.Message } }
    }
    $assets[$entry.Key] = [ordered]@{ sha256 = $entry.Value.sha256; post_scan_sha256 = $after }
}
$unchanged = @($assets.GetEnumerator() | Where-Object { $_.Value.sha256 -ne $_.Value.post_scan_sha256 }).Count -eq 0
$verdict = if ($threats.Count -eq 0 -and $unchanged -and -not $scanError) { "clean" } else { "detected" }

$receipt = [ordered]@{
    schema = 2
    kind = "minicon-defender-court"
    evidence_scope = $evidenceScope
    source_sha = "$($manifest.source_sha)"
    candidate_run = @{ id = [long]$manifest.candidate_run.id; attempt = [int]$manifest.candidate_run.attempt }
    assets = $assets
    verdict = $verdict
    provider = "Microsoft Defender"
    product_version = "$($status.AMProductVersion)"
    engine_version = "$($status.AMEngineVersion)"
    signature_version = "$($status.AntivirusSignatureVersion)"
    signature_updated_at = "$($status.AntivirusSignatureLastUpdated.ToUniversalTime().ToString('o'))"
    scanned_at = "$((Get-Date).ToUniversalTime().ToString('o'))"
    scan_error = $scanError
    threats = $threats
}
$receipt | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $receiptPath -Encoding utf8
if ($verdict -ne "clean") {
    Write-Host "FAIL Defender verdict=$verdict detection_count=$($threats.Count)"
    exit 3
}
Write-Host "PASS Defender exact asset set count=$($assets.Count)"
