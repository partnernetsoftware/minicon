const POWERSHELL: &str = include_str!("../scripts/inspect-authenticode.ps1");
const PORTABLE: &str = include_str!("../scripts/inspect-authenticode.sh");
const TRUST_BUNDLE: &str = include_str!("../scripts/fetch-microsoft-trust-bundle.sh");
const README: &str = include_str!("../README.md");
const POLICY: &str = include_str!("../CODE_SIGNING_POLICY.md");

#[test]
fn windows_inspector_covers_trust_timestamp_and_product_identity() {
    for contract in [
        "pns-authenticode-inspector/v3",
        "Get-AuthenticodeSignature",
        "PARTNERNET SOFTWARE PTY LTD",
        "TimeStamperCertificate",
        "ExpectedProductName",
        "ExpectedProductVersion",
        "product_name",
        "product_version",
        "file_description",
        "original_filename",
        "schema_version = 2",
        "file_name",
        "sha256",
        "size_bytes",
        "timestamp_certificate",
        "exit 6",
        "exit 69",
    ] {
        assert!(
            POWERSHELL.contains(contract),
            "missing inspector contract: {contract}"
        );
    }
    assert!(!POWERSHELL.contains("path = $resolved"));
}

#[test]
fn portable_inspector_does_not_claim_windows_authority() {
    let words = README.split_whitespace().collect::<Vec<_>>().join(" ");
    assert!(PORTABLE.contains("pns-authenticode-inspector/v3"));
    assert!(PORTABLE.contains("osslsigncode verify"));
    assert!(PORTABLE.contains("--ca-file"));
    assert!(PORTABLE.contains("-TSA-CAfile"));
    assert!(PORTABLE.contains("osslsigncode extract-signature"));
    assert!(PORTABLE.contains("no extractable embedded Authenticode signature"));
    assert!(PORTABLE.contains("embedded signature exists, but portable verification failed"));
    assert!(PORTABLE.contains("Windows Get-AuthenticodeSignature is authoritative"));
    assert!(
        TRUST_BUNDLE.contains("5367f20c7ade0e2bca790915056d086b720c33c1fa2a2661acf787e3292e1270")
    );
    assert!(
        TRUST_BUNDLE.contains("36e731cfa9bfd69dafb643809f6dec500902f7197daeaad86ea0159a2268a2b8")
    );
    assert!(TRUST_BUNDLE.contains("mv -- \"$staged\" \"$output\""));
    assert!(words.contains("no public release has been signed yet"));
}

#[test]
fn public_policy_exposes_readiness_and_irreversible_revocation_boundaries() {
    for contract in [
        "check-product-signing-readiness.sh",
        "--qualification",
        "Deleting a certificate profile does not revoke signatures",
        "Certificate revocation is a separate, irreversible owner action",
        "company-dev-hub/tree/main/skills/sign-windows-artifacts",
    ] {
        assert!(
            POLICY.contains(contract),
            "missing signing operations contract: {contract}"
        );
    }
}
