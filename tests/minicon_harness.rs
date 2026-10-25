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
//! Most cases here are refusals that land before any transport is opened, so
//! they need no display server and no network. The during-task behavior of
//! the file and exec tools -- which only run when a model asks -- is instead
//! evidenced by the live, env-gated tests below (`..._runs_a_real_bounded_task_
//! ...`), each `BLOCKED` rather than silently skipped when its backend's API
//! key is not set in this environment.

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

/// A real CLI invocation, native loopback HTTP, both real tools and persisted
/// resume identity. The fixture is a model endpoint, never a tool executor.
#[test]
fn gateway_worker_runs_its_own_tools_and_refuses_changed_resume_binding() {
    gateway_worker_journey(None);
}

#[test]
fn gateway_worker_uses_custom_profile_alias_for_tools_and_resume() {
    gateway_worker_journey(Some("work"));
}

fn gateway_worker_journey(alias: Option<&'static str>) {
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            Arc, Mutex,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };
    let entropy = minicon_platform::entropy::secure_random_array::<16>().unwrap();
    let nonce: String = entropy.iter().map(|byte| format!("{byte:02x}")).collect();
    let root = std::env::temp_dir().join(format!("minicon-gateway-{}-{nonce}", std::process::id()));
    // An existing directory is a hard fixture failure, never shared state.
    std::fs::create_dir(&root).unwrap();
    // A literal echo cannot distinguish the task root from CMD's silent
    // Windows-directory fallback. Only a relative read can return this value;
    // the command and model prompt never carry the challenge contents.
    let exec_nonce = format!(
        "EXEC-CWD-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    std::fs::write(root.join("exec-challenge.txt"), exec_nonce.as_bytes()).unwrap();
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    listener.set_nonblocking(true).unwrap();
    let base = format!("http://{}/v1", listener.local_addr().unwrap());
    let changed = Arc::new(AtomicBool::new(false));
    let done = Arc::new(AtomicBool::new(false));
    let observed = Arc::new(Mutex::new((0usize, Vec::<String>::new(), false)));
    let changed_worker = changed.clone();
    let done_worker = done.clone();
    let observed_worker = observed.clone();
    let peer = std::thread::spawn(move || {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !done_worker.load(Ordering::Acquire) && Instant::now() < deadline {
            let (mut socket, _) = match listener.accept() {
                Ok(pair) => pair,
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                }
                Err(error) => panic!("model fixture accept: {error}"),
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut bytes = Vec::new();
            while !bytes.ends_with(b"\r\n\r\n") {
                assert!(bytes.len() < 16384);
                let mut byte = [0];
                socket.read_exact(&mut byte).unwrap();
                bytes.push(byte[0]);
            }
            let head = String::from_utf8(bytes).unwrap();
            let headers: Vec<_> = head
                .lines()
                .skip(1)
                .filter_map(|line| line.split_once(':'))
                .collect();
            let header = |name: &str| {
                headers
                    .iter()
                    .find(|(key, _)| key.eq_ignore_ascii_case(name))
                    .map(|(_, value)| value.trim())
                    .unwrap_or("")
            };
            assert_eq!(header("authorization"), "Bearer synthetic-local-key");
            let session = header("x-minicon-session").to_owned();
            assert_eq!(session.len(), 32);
            let size: usize = header("content-length").parse().unwrap_or(0);
            assert!(size < 1024 * 1024);
            let mut body = vec![0; size];
            socket.read_exact(&mut body).unwrap();
            let mut observed = observed_worker.lock().unwrap();
            observed.1.push(session);
            let reply = if head.starts_with("GET /v1/minicon/binding ") {
                let mut binding = serde_json::json!({"profile":if alias.is_some() { "synthetic-custom-client" } else { "default" }, "provider":"deepseek",
                    "account":if changed_worker.load(Ordering::Acquire) {"account-b"} else {"account-a"},
                    "model":"fixture-model", "generation":"fixture-generation"});
                if let Some(alias) = alias {
                    binding["alias"] = alias.into();
                }
                binding
            } else {
                assert!(head.starts_with("POST /v1/chat/completions "), "{head}");
                let request: serde_json::Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(request["model"], alias.unwrap_or("minicon-selected"));
                observed.0 += 1;
                if observed.0 == 1 {
                    #[cfg(windows)]
                    let argv = serde_json::json!(["cmd.exe", "/d", "/c",
                        "if defined MINICON_LOCAL_API_KEY (exit /b 9) else (type exec-challenge.txt)"]);
                    #[cfg(unix)]
                    let argv = serde_json::json!(["sh", "-c",
                        "test -z \"$MINICON_LOCAL_API_KEY\" && cat exec-challenge.txt"]);
                    serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,
                        "tool_calls":[
                            {"id":"file-1","type":"function","function":{"name":"file",
                                "arguments":serde_json::json!({"op":"write","path":"result.txt","contents":"FILE-OK"}).to_string()}},
                            {"id":"exec-1","type":"function","function":{"name":"exec",
                                "arguments":serde_json::json!({"argv":argv}).to_string()}}
                        ]},"finish_reason":"tool_calls"}]})
                } else {
                    if observed.0 == 2 {
                        let messages = request["messages"].as_array().unwrap();
                        let results: Vec<_> = messages.iter().filter(|message| message["role"] == "tool").collect();
                        assert_eq!(results.len(), 2);
                        assert!(results.iter().any(|result| result["content"].as_str().unwrap().contains(&exec_nonce)), "exec must read the controlled relative file from the task root");
                        assert!(!results.iter().any(|result| result["content"].as_str().unwrap().contains("synthetic-local-key")));
                        observed.2 = true;
                    }
                    serde_json::json!({"choices":[{"message":{"role":"assistant",
                        "content":"TASK COMPLETE"},"finish_reason":"stop"}]})
                }
            }.to_string();
            write!(socket, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{reply}", reply.len()).unwrap();
        }
    });
    #[cfg(windows)]
    let program = "cmd.exe";
    #[cfg(unix)]
    let program = "sh";
    let invoke = |resume: bool| {
        harness(
            &[
                "--root",
                root.to_str().unwrap(),
                "--task",
                "local tools",
                "--gateway",
                &base,
                "--allow-cmd",
                program,
                if resume { "--continue" } else { "--session" },
                "bound-session",
            ],
            Some(("MINICON_LOCAL_API_KEY", "synthetic-local-key")),
        )
    };
    let first = invoke(false);
    let first_binding = std::fs::read(root.join(".minicon-harness-sessions/bound-session.json"))
        .ok()
        .and_then(|bytes| serde_json::from_slice::<serde_json::Value>(&bytes).ok())
        .map(|saved| saved["gateway_binding"].clone());
    if alias.is_none() && first.status.success() {
        // The default profile must also resume sessions saved before the
        // additive alias field existed, rather than treating them as corrupt.
        let path = root.join(".minicon-harness-sessions/bound-session.json");
        let mut saved: serde_json::Value =
            serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        saved["gateway_binding"]
            .as_object_mut()
            .unwrap()
            .remove("alias");
        std::fs::write(path, serde_json::to_vec(&saved).unwrap()).unwrap();
    }
    let second = invoke(true);
    changed.store(true, Ordering::Release);
    let third = invoke(true);
    done.store(true, Ordering::Release);
    peer.join().unwrap();
    assert!(first.status.success(), "{}", stderr_of(&first));
    assert!(
        second.status.success(),
        "{}; synthetic first binding: {:?}; expected alias: {}",
        stderr_of(&second),
        first_binding,
        alias.unwrap_or("minicon-selected")
    );
    assert_eq!(third.status.code(), Some(2));
    assert!(
        stderr_of(&third).contains("binding changed"),
        "{}",
        stderr_of(&third)
    );
    assert_eq!(
        std::fs::read_to_string(root.join("result.txt")).unwrap(),
        "FILE-OK"
    );
    let observed = observed.lock().unwrap();
    assert_eq!(
        observed.0, 3,
        "changed binding must not make another model request"
    );
    assert!(observed.2, "real exec result must reach the model");
    assert!(observed.1.iter().all(|session| session == &observed.1[0]));
    std::fs::remove_dir_all(root).unwrap();
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

/// H3's during-task, live-backend evidence: a real bounded task that must
/// call the exec tool, not just the file tool, to answer correctly. The
/// model is never shown the input file's byte count, so it cannot fabricate
/// the answer from the task text alone -- it has to actually run `wc -c`
/// through `ExecTool::run` and read the real stdout back.
#[test]
fn deepseek_backend_runs_a_real_bounded_task_through_the_exec_tool() {
    let Ok(key) = std::env::var("MINICON_DEEPSEEK_API_KEY") else {
        eprintln!(
            "BLOCKED: deepseek_backend_runs_a_real_bounded_task_through_the_exec_tool \
             skipped -- MINICON_DEEPSEEK_API_KEY is not set in this environment"
        );
        return;
    };

    let root = std::env::temp_dir().join(format!(
        "minicon-h3-live-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create live-task root");
    // A byte count with nothing round about it: the model cannot guess this
    // from the task text, so writing the right number back proves `wc -c`
    // actually ran.
    let input_bytes = "h3-exec-live-evidence-payload-oddball\n";
    std::fs::write(root.join("input.txt"), input_bytes).expect("write input.txt");
    let expected_count = input_bytes.len().to_string();

    let output = harness(
        &[
            "--root",
            root.to_str().expect("root is utf8"),
            "--task",
            "Run `wc -c input.txt` with the exec tool. Its stdout starts with a \
             number followed by whitespace and the filename -- that number is a \
             byte count. Write ONLY that number, with no other text, into a file \
             named count.txt using the file tool.",
            "--allow-cmd",
            "wc",
        ],
        Some(("MINICON_DEEPSEEK_API_KEY", &key)),
    );
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(0), "stderr={stderr}");

    let written = std::fs::read_to_string(root.join("count.txt"))
        .expect("the model-commanded write must land under the task root");
    assert!(
        written.trim() == expected_count,
        "the live model's exec-tool-derived byte count is wrong: got {written:?}, \
         wanted {expected_count:?}"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// `{LOOP}`'s during-task, live-backend evidence: a real multi-step task that
/// must actually jump (`decide-continue` sends it back to `categorize`) at
/// least once before it ends on the literal stop phrase, rather than the
/// model completing linearly on its very first answer. `run_loop`'s jump
/// message carries the ORIGINAL task forward every iteration (not just the
/// prior draft -- the backend closure itself is stateless across calls), so
/// the model reads the same counter instructions each time; what actually
/// carries state between iterations is the `file` tool's real file under
/// `--root`, the same bound every iteration shares. A counter starting at 0
/// and required to reach 2 before the model may say the stop phrase forces at
/// least one real jump, proving `{LOOP}`'s state machine end to end -- the
/// unit fixtures in `src/harness.rs` prove the bounded rules in isolation
/// with a fake closure; this is the one live-model proof the context-
/// engineering plan's `{LOOP}` leaf still needed. `BLOCKED`, not silently
/// skipped, when no live key is set.
#[test]
fn deepseek_backend_loop_jumps_at_least_once_before_the_stop_phrase() {
    let Ok(key) = std::env::var("MINICON_DEEPSEEK_API_KEY") else {
        eprintln!(
            "BLOCKED: deepseek_backend_loop_jumps_at_least_once_before_the_stop_phrase \
             skipped -- MINICON_DEEPSEEK_API_KEY is not set in this environment"
        );
        return;
    };

    let root = std::env::temp_dir().join(format!(
        "minicon-loop-live-{}-{}",
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
            "Use the file tool to read counter.txt under the root; if it does not exist, \
             treat its value as 0. Increment that number by 1 and write the new number back \
             to counter.txt with the file tool (digits only, no other text). Then: if the new \
             number is less than 2, end your reply with exactly the text CONTINUE-NOW and \
             nothing after it. If the new number is 2 or more, end your reply with exactly the \
             text TASK COMPLETE and nothing after it. Do this every time you are asked, even if \
             asked again.",
        ],
        Some(("MINICON_DEEPSEEK_API_KEY", &key)),
    );
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(0), "stderr={stderr}");

    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(
        stdout.contains("TASK COMPLETE"),
        "the loop must end on the real stop phrase, not the iteration ceiling: {stdout}"
    );

    let counter = std::fs::read_to_string(root.join("counter.txt"))
        .expect("the model-commanded counter write must land under the task root");
    assert!(
        counter.trim() == "2",
        "the counter must have reached exactly 2 -- one jump, then stop: got {counter:?}"
    );

    std::fs::remove_dir_all(&root).ok();
}

/// H5's during-task, live-backend evidence: a real bounded task against the
/// real hosted opencode-go service (`https://opencode.ai/zen/go`), model
/// commanded write landing under the task root. Confirmed live (2026-09-26,
/// direct `curl`) that this service: is HTTPS-only, requires a bearer key,
/// requires the `x-opencode-session` header this backend now sends
/// unconditionally, and has no model literally named `"opencode"` -- so this
/// test also exercises `MINICON_OPENCODE_MODEL`, naming a real catalog model.
/// When the key is absent the test prints why it did not run rather than
/// silently vanishing -- per AGENTS.md, unavailable evidence is BLOCKED,
/// never silently skipped.
#[test]
fn opencode_backend_runs_a_real_bounded_task_and_writes_the_file() {
    let Ok(key) = std::env::var("MINICON_OPENCODE_API_KEY") else {
        eprintln!(
            "BLOCKED: opencode_backend_runs_a_real_bounded_task_and_writes_the_file \
             skipped -- MINICON_OPENCODE_API_KEY is not set in this environment"
        );
        return;
    };

    let root = std::env::temp_dir().join(format!(
        "minicon-h5-live-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("clock")
            .as_nanos()
    ));
    std::fs::create_dir_all(&root).expect("create live-task root");

    let mut command = Command::new(minicon_binary());
    command
        .arg("harness")
        .args([
            "--root",
            root.to_str().expect("root is utf8"),
            "--backend",
            "opencode-go",
            "--task",
            "Write the exact text OK-H5-LIVE (no quotes, no extra text) into a \
             file named result.txt using the file tool.",
        ])
        .env_remove("MINICON_DEEPSEEK_API_KEY")
        .env("MINICON_OPENCODE_API_KEY", &key)
        .env("MINICON_OPENCODE_BASE_URL", "https://opencode.ai/zen/go")
        .env("MINICON_OPENCODE_MODEL", "deepseek-flash");
    let output = command.output().expect("minicon harness runs");
    let stderr = stderr_of(&output);
    assert_eq!(output.status.code(), Some(0), "stderr={stderr}");

    let written = std::fs::read_to_string(root.join("result.txt"))
        .expect("the model-commanded write must land under the task root");
    assert!(
        written.contains("OK-H5-LIVE"),
        "the live model's write did not carry the requested content: {written:?}"
    );

    std::fs::remove_dir_all(&root).ok();
}
