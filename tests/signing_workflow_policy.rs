const SIGNING_WORKFLOW: &str = include_str!("../.github/workflows/company-signing.yml");
const CANDIDATE_WORKFLOW: &str = include_str!("../.github/workflows/candidate.yml");

#[test]
fn signing_input_court_receives_every_identity_value_it_reads() {
    let step = SIGNING_WORKFLOW
        .split("- name: Verify unsigned identity and stage exact signing set")
        .nth(1)
        .expect("missing unsigned identity court")
        .split("- name: Upload immutable three-file signing input")
        .next()
        .expect("missing end of unsigned identity court");
    for binding in [
        "SOURCE_SHA: ${{ inputs.source_sha }}",
        "UPSTREAM_RUN_ID: ${{ inputs.minicon_com_run_id }}",
        "UPSTREAM_ATTEMPT: ${{ needs.preflight.outputs.upstream_attempt }}",
        "EXPECTED_VERSION: ${{ needs.preflight.outputs.version }}",
    ] {
        assert!(
            step.contains(binding),
            "missing workflow binding: {binding}"
        );
    }
    for read in [
        "$env:SOURCE_SHA",
        "$env:UPSTREAM_RUN_ID",
        "$env:UPSTREAM_ATTEMPT",
        "$env:EXPECTED_VERSION",
    ] {
        assert!(
            step.contains(read),
            "identity court stopped reading: {read}"
        );
    }
}

#[test]
fn qualification_receipt_is_explicitly_non_promotable() {
    assert!(SIGNING_WORKFLOW.contains("qualification_only:"));
    assert!(SIGNING_WORKFLOW.contains("release_eligible=false"));
    assert!(SIGNING_WORKFLOW.contains("release_eligible = [bool]::Parse($env:RELEASE_ELIGIBLE)"));
    assert!(CANDIDATE_WORKFLOW.contains(".release_eligible == true"));
}

#[test]
fn public_receipt_does_not_receive_protected_provider_coordinates() {
    let receipt_step = SIGNING_WORKFLOW
        .split("- name: Verify publisher, timestamp, and write before-to-after receipt")
        .nth(1)
        .expect("missing signing receipt step")
        .split("- uses: actions/upload-artifact@")
        .next()
        .expect("missing end of signing receipt step");
    for forbidden in [
        "ARTIFACT_SIGNING_ENDPOINT:",
        "ARTIFACT_SIGNING_ACCOUNT:",
        "ARTIFACT_SIGNING_PROFILE:",
        "provider_resource",
    ] {
        assert!(
            !receipt_step.contains(forbidden),
            "protected provider coordinate entered receipt step: {forbidden}"
        );
    }
}

#[test]
fn windows_signing_job_normalizes_cross_platform_shell_helpers() {
    let stage_step = SIGNING_WORKFLOW
        .split("- name: Verify unsigned identity and stage exact signing set")
        .nth(1)
        .expect("missing unsigned identity court")
        .split("- name: Upload immutable three-file signing input")
        .next()
        .expect("missing end of unsigned identity court");
    assert!(stage_step.contains("Get-ChildItem -LiteralPath signed -Filter '*.sh' -File"));
    assert!(stage_step.contains(".Replace(\"`r`n\", \"`n\")"));
    assert!(stage_step.contains("[Text.UTF8Encoding]::new($false)"));
}
