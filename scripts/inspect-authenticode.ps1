[CmdletBinding()]
param(
    [Parameter(Mandatory = $true, Position = 0)]
    [string]$Path,

    [string]$ExpectedOrganization = 'PARTNERNET SOFTWARE PTY LTD'
)

$ErrorActionPreference = 'Stop'
if ($null -eq (Get-Command Get-AuthenticodeSignature -ErrorAction SilentlyContinue)) {
    [Console]::Error.WriteLine(
        'Get-AuthenticodeSignature is unavailable; run this court on Windows.'
    )
    exit 69
}

$resolved = (Resolve-Path -LiteralPath $Path).Path
$signature = Get-AuthenticodeSignature -LiteralPath $resolved
$signer = $signature.SignerCertificate
$timestamp = $signature.TimeStamperCertificate
$result = [ordered]@{
    schema_version = 1
    path = $resolved
    status = [string]$signature.Status
    status_message = $signature.StatusMessage
    signer = if ($null -eq $signer) { $null } else { [ordered]@{
        subject = $signer.Subject
        issuer = $signer.Issuer
        thumbprint = $signer.Thumbprint
        not_before = $signer.NotBefore.ToUniversalTime().ToString('o')
        not_after = $signer.NotAfter.ToUniversalTime().ToString('o')
    } }
    timestamp = if ($null -eq $timestamp) { $null } else { [ordered]@{
        subject = $timestamp.Subject
        issuer = $timestamp.Issuer
        thumbprint = $timestamp.Thumbprint
    } }
}
$result | ConvertTo-Json -Depth 5

if ($signature.Status -eq [System.Management.Automation.SignatureStatus]::NotSigned) {
    exit 2
}
if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or $null -eq $signer) {
    exit 3
}
$publisherPattern = '(?:^|,\s*)O=' + [regex]::Escape($ExpectedOrganization) + '(?:,|$)'
if ($signer.Subject -notmatch $publisherPattern) {
    exit 4
}
if ($null -eq $timestamp) {
    exit 5
}
exit 0
