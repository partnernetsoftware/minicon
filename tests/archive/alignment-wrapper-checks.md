# Archived alignment wrapper checks

Retired from the active Rust alignment suite on 2026-10-26.
Decision and replacement owner: [PRD 33](../../prd/PRD_02_33_test_orchestration.md#owner-selected-development-loop-2026-10-26).
Original source is retained below as history, not executable evidence.

```rust
/// `scripts/build.sh` is the documented build entry, and the README names its
/// modes (`release`, `dev`, `check`, `test`). Pin that contract: the wrapper
/// exists and is executable, and an unknown mode is refused with the usage
/// rather than silently defaulting to a full release build. The interpreter it
/// needs is the reason this test runs the script at all — a host without
/// `python3` (a Windows Git shell has only `python`) must not be a broken
/// documented path.
#[test]
fn build_script_accepts_the_documented_modes() {
    let script = repo_root().join("scripts/build.sh");
    assert!(script.is_file(), "scripts/build.sh must exist");
    let Ok(bash) = which_bash() else {
        eprintln!("skipping: no bash on PATH");
        return;
    };

    // An unknown mode is a usage error (exit 2), not a build.
    let output = Command::new(&bash)
        .arg(&script)
        .arg("definitely-not-a-mode")
        .current_dir(repo_root())
        .output()
        .expect("run scripts/build.sh");
    assert_eq!(
        output.status.code(),
        Some(2),
        "an unknown mode must be a usage error: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("usage: scripts/build.sh"),
        "the refusal must print the usage"
    );
}

/// A `bash` on PATH, or `None` so a platform without one skips rather than
/// fails a gate that does not apply to it.
fn which_bash() -> Result<PathBuf, ()> {
    let path = std::env::var_os("PATH").ok_or(())?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(if cfg!(windows) { "bash.exe" } else { "bash" });
        if candidate.is_file() {
            // Windows ships a WSL launcher named bash.exe in System32. It is
            // not a shell when WSL is absent, and must not shadow Git Bash.
            if cfg!(windows)
                && !Command::new(&candidate)
                    .arg("--version")
                    .output()
                    .is_ok_and(|output| {
                        output.status.success() && output.stdout.starts_with(b"GNU bash")
                    })
            {
                continue;
            }
            return Ok(candidate);
        }
    }
    Err(())
}

/// Every `.md` under `dir`, recursively. Missing directories yield nothing, so
/// the gate does not fail on a tree that is not checked out.

#[test]
fn the_documented_test_gate_still_denies_the_dangerous_lints() {
    let root = repo_root();
    let script = fs::read_to_string(root.join("scripts/build.sh")).expect("read build.sh");
    let agents = fs::read_to_string(root.join("AGENTS.md")).expect("read AGENTS.md");

    let lints_start = script
        .find("test_lints='build.rustflags=")
        .expect("shared test lint configuration exists");
    let lints = script[lints_start..]
        .lines()
        .next()
        .expect("test lint configuration line");
    for mode in ["  test-suite) cargo test", "  test) cargo test"] {
        let arm_start = script.find(mode).expect("test mode runs cargo test");
        let arm = &script[arm_start..];
        let arm = &arm[..arm.find(";;").expect("test arm ends")];
        assert!(
            arm.contains("--config \"$test_lints\""),
            "{mode} must use the shared dangerous-test-lint configuration"
        );
    }

    for lint in ["dead_code", "unused_variables", "unused_must_use"] {
        assert!(
            lints.contains(lint),
            "the test modes no longer deny {lint}, so a test can silently run nothing"
        );
        assert!(
            agents.contains(lint),
            "AGENTS.md no longer explains why {lint} is denied"
        );
    }
    // The rule names the entry point it is about, and the README keeps
    // documenting that same entry.
    assert!(
        agents.contains("./scripts/build.sh test"),
        "AGENTS.md must name the test entry it describes"
    );
    assert!(
        script.contains("usage: scripts/build.sh [release|dev|check|test|test-suite]"),
        "the script's own usage line lists the modes"
    );
}


```
