$ErrorActionPreference = "Stop"

$root = "C:\minicon-six\defender"
$binary = Join-Path $root "minicon.com"
$manifestPath = Join-Path $root "candidate-manifest.json"
$receiptPath = Join-Path $root "defender-receipt.json"
$manifest = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$evidenceScope = if ($manifest.defender_evidence_scope) { "$($manifest.defender_evidence_scope)" } else { "candidate" }
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
$scanError = ""
try {
    Start-MpScan -ScanType CustomScan -ScanPath $binary
} catch {
    $scanError = $_.Exception.Message
}
$detections = @(Get-MpThreatDetection -ErrorAction SilentlyContinue | Where-Object {
    $_.InitialDetectionTime -ge $started -or
    (@($_.Resources) | Where-Object { $_ -like "*$binary*" }).Count -gt 0
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
$after = ""
if (Test-Path -LiteralPath $binary) {
    try {
        $after = (Get-FileHash -LiteralPath $binary -Algorithm SHA256).Hash.ToLowerInvariant()
    } catch {
        if (-not $scanError) { $scanError = $_.Exception.Message }
    }
}
$verdict = if ($threats.Count -eq 0 -and $after -eq $expected -and -not $scanError) { "clean" } else { "detected" }

$receipt = [ordered]@{
    schema = 1
    kind = "minicon-defender-court"
    evidence_scope = $evidenceScope
    source_sha = "$($manifest.source_sha)"
    candidate_run = @{
        id = [long]$manifest.candidate_run.id
        attempt = [int]$manifest.candidate_run.attempt
    }
    minicon_com_sha256 = $expected
    post_scan_sha256 = $after
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
$receipt | ConvertTo-Json -Depth 5 | Set-Content -LiteralPath $receiptPath -Encoding utf8
if ($verdict -ne "clean") {
    Write-Host "FAIL Defender verdict=$verdict detection_count=$($threats.Count)"
    exit 3
}
Write-Host "PASS Defender exact Candidate SHA $after"
