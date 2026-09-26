//! Black-box evidence for the `harness` subcommand (plan items H2, H3, H6).
//!
//! These drive the shipped `minicon` binary's own CLI, exactly as an agent or
//! script would, and assert on what an outside caller can actually observe:
//! the exit code, and the text on stderr. Nothing here reaches into
//! `src/harness.rs`'s private helpers -- that module's unit tests already
//! cover `FileTool::resolve`'s confinement and `ExecTool`'s argv handling, and
//! restating them here would prove nothing new.
//!
//! What IS only observable from outside is *ordering*: `run_harness` refuses a
//! missing backend credential before it resolves the file tool's bound, so no
//! tool is ever constructed for a task that could not have reached a model.
//! A unit test cannot see that, because from inside the module both refusals
//! are just early returns; from outside, the difference is which of two broken
//! inputs gets named.
//!
//! Needs no display server and no network: every case here is a refusal that
//! lands before any transport is opened. What these tests cannot supply is the
//! file and exec tools' behavior *during* a task, because a tool only runs
//! when a model asks it to. That is not unevidenced -- `harness_opencode`'s
//! real-socket fixtures drive a scripted server through a full tool round trip,
//! including a model-commanded file landing under the task root -- but a
//! fixture is not a live endpoint, and that last step stays BLOCKED rather
//! than claimed here.

use std::path::PathBuf;
use std::process::{Command, Output};

fn minicon_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("MINICON_TEST_BINARY") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "MINICON_TEST_BINARY is missing at {}",
            path.display()
        );
        return path;
    }
    let mut path = std::env::current_exe().expect("test executable path");
    path.pop();
    path.pop();
    path.push(format!("minicon{}", std::env::consts::EXE_SUFFIX));
    assert!(
        path.is_file(),
        "minicon is missing at {}; build it with `cargo build --bin minicon`.",
        path.display()
    );
    path
}

/// One `minicon harness ...` invocation with BOTH backend key variables
/// removed, then whichever the caller names put back. The removal is explicit
/// because a real key in the developer's own environment would otherwise let
/// the credential check pass and turn a deliberate refusal into a network
/// call.
fn harness(args: &[&str], key: Option<(&str, &str)>) -> Output {
    let mut command = Command::new(minicon_binary());
    command.arg("harness").args(args);
    command.env_remove("MINICON_DEEPSEEK_API_KEY");
    command.env_remove("MINICON_OPENCODE_API_KEY");
    if let Some((name, value)) = key {
        command.env(name, value);
    }
    command.output().expect("minicon harness runs")
}

fn stderr_of(output: &Output) -> String {
    String::from_utf8_lossy(&output.stderr).into_owned()
}

/// The ordering invariant. The root here is unresolvable AND the key is
/// absent; only the key may be named, because a task that cannot reach a
/// model must not cause a bound to be resolved first.
#[test]
fn refuses_a_missing_backend_key_before_resolving_the_root() {
    let output = harness(
        &[
            "--root",
            "/nonexistent-minicon-harness-root",
            "--task",
            "hi",
        ],
        None,
    );
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "stderr={stderr}");
    assert!(
        stderr.contains("MINICON_DEEPSEEK_API_KEY"),
        "the refusal must name the one variable the backend reads; stderr={stderr}"
    );
    assert!(
        !stderr.contains("cannot be resolved"),
        "the root must not be resolved before the credential check; stderr={stderr}"
    );
}

/// Each backend reads its own variable. A shared or guessed name would let a
/// key meant for one endpoint be sent to another.
#[test]
fn names_the_opencode_key_variable_for_the_opencode_backend() {
    let output = harness(
        &[
            "--root",
            "/nonexistent-minicon-harness-root",
            "--task",
            "hi",
            "--backend",
            "opencode-go",
        ],
        None,
    );
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "stderr={stderr}");
    assert!(
        stderr.contains("MINICON_OPENCODE_API_KEY"),
        "stderr={stderr}"
    );
    assert!(
        !stderr.contains("MINICON_DEEPSEEK_API_KEY"),
        "one backend's refusal must not name the other's variable; stderr={stderr}"
    );
}

/// An empty key is not a key. Treating it as present would send an
/// unauthenticated request and surface as a remote error instead of a local
/// one.
#[test]
fn treats_an_empty_backend_key_as_missing() {
    let output = harness(
        &["--root", ".", "--task", "hi"],
        Some(("MINICON_DEEPSEEK_API_KEY", "")),
    );
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "stderr={stderr}");
    assert!(
        stderr.contains("MINICON_DEEPSEEK_API_KEY"),
        "stderr={stderr}"
    );
}

/// Once a credential exists, an unusable `--root` is a bounded error up front
/// -- before any transport is opened, which is why this test needs no network
/// even though the key it supplies is not a real one.
#[test]
fn refuses_an_unresolvable_root_with_a_key_present() {
    let output = harness(
        &[
            "--root",
            "/nonexistent-minicon-harness-root",
            "--task",
            "hi",
        ],
        Some(("MINICON_DEEPSEEK_API_KEY", "not-a-real-key")),
    );
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "stderr={stderr}");
    assert!(
        stderr.contains("--root") && stderr.contains("cannot be resolved"),
        "stderr={stderr}"
    );
}

/// A file is not a bound. Accepting one would give the file tool a root that
/// no path can legally sit inside.
#[test]
fn refuses_a_root_that_is_not_a_directory() {
    let output = harness(
        &["--root", "Cargo.toml", "--task", "hi"],
        Some(("MINICON_DEEPSEEK_API_KEY", "not-a-real-key")),
    );
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "stderr={stderr}");
    assert!(stderr.contains("is not a directory"), "stderr={stderr}");
}

/// `--root` has no implicit current-directory default, because defaulting it
/// would widen the file tool's bound without the caller saying so.
#[test]
fn requires_root_to_be_stated_explicitly() {
    let output = harness(&["--task", "hi"], None);
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "stderr={stderr}");
    assert!(
        stderr.contains("--root") && stderr.contains("current-directory default"),
        "stderr={stderr}"
    );
}

#[test]
fn requires_a_task() {
    let output = harness(&["--root", "."], None);
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "stderr={stderr}");
    assert!(stderr.contains("--task"), "stderr={stderr}");
}

/// An unknown backend is refused by name rather than silently falling back to
/// the default, which would send the task to an endpoint the caller did not
/// choose.
#[test]
fn refuses_an_unknown_backend_by_name() {
    let output = harness(
        &[
            "--root",
            ".",
            "--task",
            "hi",
            "--backend",
            "gpt-hypothetical",
        ],
        Some(("MINICON_DEEPSEEK_API_KEY", "not-a-real-key")),
    );
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "stderr={stderr}");
    assert!(
        stderr.contains("gpt-hypothetical") && stderr.contains("deepseek"),
        "the refusal must name what was typed and what is accepted; stderr={stderr}"
    );
}

#[test]
fn refuses_an_unknown_flag() {
    let output = harness(
        &["--root", ".", "--task", "hi", "--allow-everything"],
        Some(("MINICON_DEEPSEEK_API_KEY", "not-a-real-key")),
    );
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(2), "stderr={stderr}");
    assert!(stderr.contains("--allow-everything"), "stderr={stderr}");
}

/// H4's during-task, live-backend evidence: a real bounded task against the
/// actual DeepSeek endpoint, model-commanded write landing under the task
/// root. Only runs when this environment carries a real
/// `MINICON_DEEPSEEK_API_KEY`; a fixture (see `harness_wire`'s loopback
/// tests) proves the wire mapping, but only a live call proves the key this
/// environment holds is actually accepted end to end. When the key is
/// absent the test prints why it did not run rather than silently vanishing
/// -- per AGENTS.md, unavailable evidence is BLOCKED, never silently
/// skipped.
#[test]
fn deepseek_backend_runs_a_real_bounded_task_and_writes_the_file() {
    let Ok(key) = std::env::var("MINICON_DEEPSEEK_API_KEY") else {
        eprintln!(
            "BLOCKED: deepseek_backend_runs_a_real_bounded_task_and_writes_the_file \
             skipped -- MINICON_DEEPSEEK_API_KEY is not set in this environment"
        );
        return;
    };

    let root = std::env::temp_dir().join(format!(
        "minicon-h4-live-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create live-task root");

    let output = harness(
        &[
            "--root",
            root.to_str().expect("root is utf8"),
            "--task",
            "Write the exact text OK-H4-LIVE (no quotes, no extra text) into a \
             file named result.txt using the file tool.",
        ],
        Some(("MINICON_DEEPSEEK_API_KEY", &key)),
    );
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(0), "stderr={stderr}");

    let written = std::fs::read_to_string(root.join("result.txt"))
        .expect("the model-commanded write must land under the task root");
    assert!(
        written.contains("OK-H4-LIVE"),
        "the live model's write did not carry the requested content: {written:?}"
    );

    std::fs::remove_dir_all(&root).ok();
}
