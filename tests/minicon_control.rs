use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

static UNIQUE: AtomicU64 = AtomicU64::new(1);

struct OwnedGui {
    child: Child,
    screenshot: PathBuf,
}

impl Drop for OwnedGui {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
        let _ = fs::remove_file(&self.screenshot);
    }
}

#[cfg(windows)]
struct ClipboardRestore(Option<String>);

#[cfg(windows)]
impl Drop for ClipboardRestore {
    fn drop(&mut self) {
        let _ = agenterm_platform::clipboard::set_text(self.0.as_deref().unwrap_or_default());
    }
}

fn unique_suffix() -> String {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock must follow Unix epoch")
        .as_nanos();
    format!(
        "{}-{}-{}",
        std::process::id() % 100_000,
        nanos % 1_000_000_000,
        UNIQUE.fetch_add(1, Ordering::Relaxed)
    )
}

fn control_endpoint(suffix: &str) -> String {
    if cfg!(windows) {
        format!(r"pipe:\\.\pipe\minicon-test-{suffix}")
    } else {
        let base = agenterm_platform::ipc::native_runtime_directory();
        let _ = fs::create_dir_all(&base);
        let dir = base.join(format!("ct-{suffix}"));
        let _ = fs::create_dir_all(&dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = fs::set_permissions(&dir, fs::Permissions::from_mode(0o700));
        }
        let path = dir.join("c.sock");
        assert!(
            path.to_string_lossy().len() <= 103,
            "Unix control socket path too long: {}",
            path.display()
        );
        format!("unix:{}", path.to_string_lossy())
    }
}

fn host_shell_args() -> Vec<&'static str> {
    if cfg!(windows) {
        vec!["cmd.exe", "/Q", "/K"]
    } else {
        vec!["/bin/bash", "--norc", "--noprofile"]
    }
}

fn shell_flood_command(prefix: &str) -> String {
    if cfg!(windows) {
        format!("for /L %i in (1,1,1000000) do @echo {prefix}%i\r")
    } else {
        format!("for i in $(seq 1 200000); do echo {prefix}$i; done\r")
    }
}

fn shell_load_done_command() -> &'static str {
    if cfg!(windows) {
        "for /L %i in (1,1,2000) do @echo LOAD_%i & echo LOAD_DONE\r"
    } else {
        "for i in $(seq 1 2000); do echo LOAD_$i; done; echo LOAD_DONE\r"
    }
}

fn shell_vt_noise_command() -> &'static str {
    if cfg!(windows) {
        "for /F \"delims=\" %e in ('echo prompt $E^| cmd') do @set \"ESC=%e\"\r"
    } else {
        // Unix path injects CSI noise in one shot below.
        "true\r"
    }
}

fn shell_vt_noise_body() -> &'static str {
    if cfg!(windows) {
        "for /L %i in (1,1,1200) do @echo %ESC%[999999999999999999;999999999999999999;999999999999999999mVT_NOISE_%i%ESC%[0m\r"
    } else {
        "for i in $(seq 1 1200); do printf '\\033[999mVT_NOISE_%s\\033[0m\\n' \"$i\"; done\r"
    }
}

fn invoke(exe: &Path, endpoint: &str, arguments: &[&str]) -> Output {
    let mut command = Command::new(exe);
    command.args(["cli", "--control", endpoint]);
    command.args(arguments);
    command.output().expect("minicon CLI must start")
}

fn output_text(output: &Output) -> String {
    String::from_utf8_lossy(&output.stdout).trim().to_owned()
}

fn error_text(output: &Output) -> String {
    format!(
        "stdout={} stderr={}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    )
}

fn cli_json(exe: &Path, endpoint: &str, arguments: &[&str]) -> Value {
    let output = invoke(exe, endpoint, arguments);
    assert!(
        output.status.success(),
        "CLI failed: {}",
        error_text(&output)
    );
    serde_json::from_str(&output_text(&output)).expect("successful CLI output must be JSON")
}

fn cli_text(exe: &Path, endpoint: &str, arguments: &[&str]) -> String {
    let output = invoke(exe, endpoint, arguments);
    assert!(
        output.status.success(),
        "CLI failed: {}",
        error_text(&output)
    );
    output_text(&output)
}

/// A host whose `-e` program cannot be spawned exits non-zero and names the
/// failure, rather than lingering as a windowless process.
///
/// This test used to assert something stronger — that such a host dies *before*
/// its control endpoint answers at all. That ordering was a side effect of
/// binding the endpoint inside the window's `opened` callback, and it stopped
/// holding when the endpoint became process-owned: the listener is now up
/// before the first shell is spawned, so a doomed host can answer once on its
/// way out. The ordering was never the guarantee worth having, and bending the
/// product to preserve it would have undone the change on purpose. What matters
/// — and what is asserted here — is that the failure is fatal, is attributed to
/// the spawn, and leaves nothing running.
///
/// `wait_until_ready_for` separately re-checks liveness after its first
/// successful answer, so no other test can mistake a corpse for a healthy start.
#[test]
fn a_host_whose_program_cannot_be_spawned_dies_and_says_why() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let suffix = unique_suffix();
    let endpoint = control_endpoint(&suffix);
    let child = Command::new(exe)
        .arg("--no-activate")
        .arg("--control")
        .arg(&endpoint)
        .arg("-e")
        .arg("minicon-no-such-program-for-this-test")
        .spawn()
        .expect("spawn minicon with a bad program");
    let mut gui = OwnedGui {
        child,
        screenshot: std::env::temp_dir().join(format!("minicon-{suffix}.png")),
    };

    let deadline = Instant::now() + Duration::from_secs(15);
    let status = loop {
        if let Some(status) = gui.child.try_wait().expect("poll minicon exit") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "a host with an unspawnable program must not keep running"
        );
        thread::sleep(Duration::from_millis(25));
    };
    assert!(
        !status.success(),
        "an unspawnable program must be a non-zero exit, got {status}"
    );
    // And the endpoint must be gone with it: a listener outliving its process
    // would leave a stale socket that the next run cannot bind.
    let after = invoke(exe, &endpoint, &["list-tabs"]);
    assert!(
        !after.status.success(),
        "the control endpoint answered after the host exited"
    );
}

/// Polls until the control endpoint answers `list-tabs`, or fails with a
/// diagnosis. Passing the host process makes the two failure modes distinct:
/// a host that has *exited* is reported immediately with its status (a crash
/// or a bad `-e`, not a slow start), while a live-but-silent host reports how
/// many attempts were made and the last CLI error. Without this, "control
/// endpoint did not become ready" reads the same whether the product died or
/// the machine was simply busy.
fn wait_until_ready_for(
    exe: &Path,
    endpoint: &str,
    timeout: Duration,
    host: Option<&mut Child>,
) -> Value {
    let deadline = Instant::now() + timeout;
    let mut attempts = 0_u32;
    let mut last_error;
    let mut host = host;
    loop {
        let output = invoke(exe, endpoint, &["list-tabs"]);
        if output.status.success() {
            // The endpoint is bound before the first shell is spawned, so a
            // host that is about to die of a spawn failure can answer once on
            // its way out. "Ready" has to mean the process is still there after
            // it answered, or this helper would report a corpse as a healthy
            // start and every caller downstream would fail somewhere less
            // obvious.
            if let Some(host) = host.as_deref_mut()
                && let Some(status) = host.try_wait().expect("poll minicon exit")
            {
                panic!(
                    "minicon exited ({status}) before its control endpoint was ready; \
                     the host died rather than being slow.\nit answered once on the way out"
                );
            }
            return serde_json::from_str(&output_text(&output))
                .expect("list-tabs output must be JSON");
        }
        attempts += 1;
        last_error = error_text(&output);
        if let Some(host) = host.as_deref_mut()
            && let Some(status) = host.try_wait().expect("poll minicon exit")
        {
            panic!(
                "minicon exited ({status}) before its control endpoint was ready; \
                 the host died rather than being slow.\nlast CLI error: {last_error}"
            );
        }
        assert!(
            Instant::now() < deadline,
            "control endpoint did not become ready after {attempts} attempts over {timeout:?}; \
             the host is still running, so this is a slow start, not a crash.\nlast CLI error: {last_error}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn tab_id(value: &Value) -> &str {
    value.as_str().expect("tab ID must be a string")
}

/// Resolve `minicon` next to the test executable.
///
/// `CARGO_BIN_EXE_minicon` only exists for bins declared in the *same*
/// package, and `minicon` moved to its own workspace package -- the
/// compile error that produces is what took linux-x86_64 red at the all-target
/// Clippy gate. An integration test runs from `target/<profile>/deps/`, so the
/// binary sits one directory up. Same resolution as `tests/minicon_blackbox.rs`.
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
        "minicon is missing at {}; build it with \
         `cargo build --bin minicon`. It left this package \
         in the lightweight-package split, so a bare `cargo test` no \
         longer builds it as a side effect.",
        path.display()
    );
    path
}

#[test]
fn gui_control_surface_isolated_multitab_black_box() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let suffix = unique_suffix();
    let endpoint = control_endpoint(&suffix);
    let screenshot = if cfg!(windows) {
        std::env::temp_dir().join(format!("minicon-{suffix}.png"))
    } else {
        agenterm_platform::ipc::native_runtime_directory().join(format!("shot-{suffix}.png"))
    };
    let mut host = Command::new(exe);
    host.arg("--no-activate")
        .arg("--control")
        .arg(&endpoint)
        .arg("-e");
    for arg in host_shell_args() {
        host.arg(arg);
    }
    // Launch with an interactive shell; inject ROOT_READY after control is up.
    let child = host.spawn().expect("minicon GUI must start");
    let mut gui = OwnedGui { child, screenshot };

    let listed = wait_until_ready_for(
        exe,
        &endpoint,
        Duration::from_secs(15),
        Some(&mut gui.child),
    );
    let root = tab_id(&listed["tabs"][0]["id"]).to_owned();
    assert_eq!(listed["tabs"][0]["active"], true);
    cli_json(exe, &endpoint, &["reset-perf-stats"]);

    cli_json(
        exe,
        &endpoint,
        &["send-text", "--target", &root, "echo ROOT_READY\r"],
    );
    cli_json(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &root,
            "--timeout-ms",
            "10000",
            "ROOT_READY",
        ],
    );
    cli_json(
        exe,
        &endpoint,
        &["send-text", "--target", &root, "echo ROOT_ONLY\r"],
    );
    cli_json(
        exe,
        &endpoint,
        &["wait-text", "--target", &root, "ROOT_ONLY"],
    );

    let enabled_ime = cli_json(exe, &endpoint, &["send-ui-ime", "enabled"]);
    assert_eq!(enabled_ime["route"], "terminal");
    assert_eq!(enabled_ime["action"], "enabled");
    cli_json(
        exe,
        &endpoint,
        &["send-ui-ime", "preedit", "nihao", "--cursor", "5"],
    );
    let terminal_preedit = cli_json(exe, &endpoint, &["ui-snapshot"]);
    // The published `ui-snapshot` shape is a public contract (PRD 02.26 lists
    // it), so pin the top-level keys: a rename that orphans the documentation
    // fails here instead of shipping a field nobody can find.
    {
        let object = terminal_preedit.as_object().expect("ui-snapshot object");
        for key in [
            "active",
            "workspace_empty",
            "settings_open",
            "host_notice",
            "control_pointer_owner",
            "terminal_clipboard_paste",
            "ui_language",
            "ui_theme",
            "composer_focused",
            "composer_text",
            "composer_preedit",
            "composer_submit_error",
            "composer_input",
            "terminal_ime_preedit",
            "ime_status",
            "pending_control_waits",
            "pending_control_screenshots",
            "a11y_pending_actions",
            "a11y_pending_bytes",
            "a11y_dropped_actions",
        ] {
            assert!(
                object.contains_key(key),
                "ui-snapshot lost the documented key {key:?}"
            );
        }
    }
    assert_eq!(terminal_preedit["terminal_ime_preedit"], "nihao");
    assert_eq!(terminal_preedit["composer_preedit"], "");
    let ime_status = &terminal_preedit["ime_status"];
    assert!(ime_status["known"].is_boolean());
    assert!(ime_status["name"].is_string());
    assert!(ime_status["available"].is_boolean());
    assert!(ime_status["open"].is_boolean());
    assert!(ime_status["native_mode"].is_boolean());
    assert!(ime_status["full_shape"].is_boolean());
    assert!(
        ime_status["label"]
            .as_str()
            .is_some_and(|label| label.starts_with("IME:"))
    );
    let terminal_commit = cli_json(
        exe,
        &endpoint,
        &["send-ui-ime", "commit", "echo 你好_IME_OK\r"],
    );
    assert_eq!(terminal_commit["route"], "terminal");
    assert_eq!(terminal_commit["action"], "commit");
    cli_json(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &root,
            "--timeout-ms",
            "10000",
            "你好_IME_OK",
        ],
    );
    assert_eq!(
        cli_json(exe, &endpoint, &["ui-snapshot"])["terminal_ime_preedit"],
        ""
    );
    cli_json(exe, &endpoint, &["send-ui-ime", "disabled"]);

    cli_json(exe, &endpoint, &["send-ui-keys", "Ctrl+Shift+I"]);
    let composer_preedit = cli_json(exe, &endpoint, &["send-ui-ime", "preedit", "zhongwen"]);
    assert_eq!(composer_preedit["route"], "composer");
    let focused_preedit = cli_json(exe, &endpoint, &["ui-snapshot"]);
    assert_eq!(focused_preedit["composer_preedit"], "zhongwen");
    assert_eq!(focused_preedit["terminal_ime_preedit"], "");
    cli_json(exe, &endpoint, &["send-ui-ime", "commit", "中文"]);
    let focused_commit = cli_json(exe, &endpoint, &["ui-snapshot"]);
    assert_eq!(focused_commit["composer_text"], "中文");
    assert_eq!(focused_commit["composer_preedit"], "");
    cli_json(
        exe,
        &endpoint,
        &["send-ui-keys", "Ctrl+A", "Backspace", "Escape"],
    );

    #[cfg(windows)]
    {
        let _restore = ClipboardRestore(agenterm_platform::clipboard::get_text(1024 * 1024).ok());
        agenterm_platform::clipboard::set_text("echo ASYNC_CLIPBOARD_OK\r")
            .expect("test clipboard text must be published");
        cli_json(exe, &endpoint, &["send-ui-keys", "Ctrl+Shift+V"]);
        cli_json(
            exe,
            &endpoint,
            &[
                "wait-text",
                "--target",
                &root,
                "--timeout-ms",
                "10000",
                "ASYNC_CLIPBOARD_OK",
            ],
        );
        let snapshot = cli_json(exe, &endpoint, &["ui-snapshot"]);
        assert_eq!(snapshot["terminal_clipboard_paste"]["state"], "idle");
        assert_eq!(
            snapshot["terminal_clipboard_paste"]["target"],
            serde_json::Value::Null
        );
        assert_eq!(
            snapshot["terminal_clipboard_paste"]["error"],
            serde_json::Value::Null
        );
    }

    let created = cli_json(exe, &endpoint, &["new-tab", "--parent", &root]);
    let child_id = tab_id(&created["id"]).to_owned();
    assert_eq!(created["parent"], root);
    cli_json(
        exe,
        &endpoint,
        &["send-text", "--target", &child_id, "echo KEY_EVENT"],
    );
    cli_json(
        exe,
        &endpoint,
        &["send-keys", "--target", &child_id, "Enter"],
    );
    cli_json(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &child_id,
            "--timeout-ms",
            "10000",
            "KEY_EVENT",
        ],
    );
    cli_json(
        exe,
        &endpoint,
        &[
            "send-text",
            "--target",
            &child_id,
            shell_load_done_command(),
        ],
    );
    cli_json(exe, &endpoint, &["select-tab", "--target", &root]);
    cli_json(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &child_id,
            "--timeout-ms",
            "15000",
            "LOAD_DONE",
        ],
    );
    cli_json(
        exe,
        &endpoint,
        &["send-text", "--target", &child_id, shell_vt_noise_command()],
    );
    cli_json(
        exe,
        &endpoint,
        &["send-text", "--target", &child_id, shell_vt_noise_body()],
    );
    thread::scope(|scope| {
        let mut requests = Vec::new();
        let endpoint = endpoint.as_str();
        let child_target = child_id.as_str();
        for index in 0..24 {
            requests.push(scope.spawn(move || match index % 3 {
                0 => invoke(exe, endpoint, &["list-tabs"]),
                1 => invoke(exe, endpoint, &["perf-stats"]),
                _ => invoke(
                    exe,
                    endpoint,
                    &[
                        "capture-pane",
                        "--target",
                        child_target,
                        "--max-bytes",
                        "1048576",
                    ],
                ),
            }));
        }
        for request in requests {
            let output = request.join().expect("control request thread must join");
            assert!(
                output.status.success(),
                "concurrent control request failed: {}",
                error_text(&output)
            );
        }
    });
    cli_json(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &child_id,
            "--timeout-ms",
            "15000",
            "VT_NOISE_1200",
        ],
    );
    let perf = cli_json(exe, &endpoint, &["perf-stats"]);
    assert!(perf["frames"].as_u64().is_some_and(|frames| frames > 0));
    assert!(
        perf["pty_drained_bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes > 0)
    );
    assert!(
        perf["control_requests"]
            .as_u64()
            .is_some_and(|requests| requests >= 24)
    );
    assert!(perf["control_budget_yields"].as_u64().is_some());

    let root_text = cli_text(exe, &endpoint, &["capture-pane", "--target", &root]);
    let child_text = cli_text(exe, &endpoint, &["capture-pane", "--target", &child_id]);
    assert!(root_text.contains("ROOT_ONLY"));
    assert!(!root_text.contains("LOAD_DONE"));
    assert!(child_text.contains("VT_NOISE_1200"));

    let mouse_move_receipt = cli_json(
        exe,
        &endpoint,
        &[
            "send-mouse",
            "--target",
            &child_id,
            "--action",
            "move",
            "--button",
            "none",
            "--column",
            "1",
            "--row",
            "1",
        ],
    );
    assert_eq!(mouse_move_receipt["delivered"], true);
    assert_eq!(mouse_move_receipt["route"], "noop");
    assert_eq!(mouse_move_receipt["changed"], false);
    let mouse_click_receipt = cli_json(
        exe,
        &endpoint,
        &[
            "send-mouse",
            "--target",
            &child_id,
            "--action",
            "click",
            "--button",
            "left",
            "--column",
            "1",
            "--row",
            "1",
        ],
    );
    assert_eq!(mouse_click_receipt["delivered"], true);
    assert_eq!(mouse_click_receipt["route"], "selection");
    assert_eq!(mouse_click_receipt["changed"], true);
    let wheel_receipt = cli_json(
        exe,
        &endpoint,
        &[
            "send-wheel",
            "--target",
            &child_id,
            "--column",
            "1",
            "--row",
            "1",
            "--notches",
            "1",
        ],
    );
    assert_eq!(wheel_receipt["route"], "scrollback");
    assert_eq!(wheel_receipt["delivered_notches"], 1);
    assert_eq!(wheel_receipt["changed"], true);

    let screenshot_text = gui.screenshot.to_string_lossy().into_owned();
    let screenshot_receipt = cli_json(
        exe,
        &endpoint,
        &[
            "screenshot-pane",
            "--target",
            &child_id,
            "--output",
            &screenshot_text,
        ],
    );
    assert!(
        screenshot_receipt["encode_ns"]
            .as_u64()
            .is_some_and(|elapsed| elapsed > 0),
        "screenshot receipt must expose positive encoding time: {screenshot_receipt}"
    );
    let png = fs::read(&gui.screenshot).expect("screenshot must exist after successful reply");
    assert!(png.starts_with(b"\x89PNG\r\n\x1a\n"));

    let timed_out = invoke(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &child_id,
            "--timeout-ms",
            "25",
            "IMPOSSIBLE_TEST_MARKER",
        ],
    );
    assert!(!timed_out.status.success(), "missing text must time out");
    let invalid = invoke(exe, &endpoint, &["capture-pane", "--target", "@999999"]);
    assert!(!invalid.status.success(), "unknown tab must fail");
    let still_running = invoke(
        exe,
        &endpoint,
        &["wait-tab-exit", "--target", &root, "--timeout-ms", "25"],
    );
    assert!(
        !still_running.status.success(),
        "wait-tab-exit must time out for a live terminal"
    );

    cli_json(
        exe,
        &endpoint,
        &["send-text", "--target", &child_id, "echo CHILD_FINAL\r"],
    );
    cli_json(
        exe,
        &endpoint,
        &["wait-text", "--target", &child_id, "CHILD_FINAL"],
    );
    cli_json(
        exe,
        &endpoint,
        &["send-text", "--target", &child_id, "exit 7\r"],
    );
    let exited = cli_json(
        exe,
        &endpoint,
        &[
            "wait-tab-exit",
            "--target",
            &child_id,
            "--timeout-ms",
            "10000",
        ],
    );
    assert_eq!(exited["id"], child_id);
    assert_eq!(exited["child_alive"], false);
    assert_eq!(exited["child_exit_code"], 7);
    let child_final = cli_text(exe, &endpoint, &["capture-pane", "--target", &child_id]);
    assert!(child_final.contains("CHILD_FINAL"));
    let selected_exited = cli_json(exe, &endpoint, &["select-tab", "--target", &child_id]);
    assert_eq!(selected_exited["active"], child_id);
    for arguments in [
        vec!["send-text", "--target", &child_id, "late text"],
        vec!["send-paste", "--target", &child_id, "late paste"],
        vec!["send-keys", "--target", &child_id, "A"],
        vec!["send-ui-keys", "A"],
    ] {
        let rejected = invoke(exe, &endpoint, &arguments);
        assert!(
            !rejected.status.success(),
            "input to an exited tab must fail: {}",
            error_text(&rejected)
        );
        assert!(
            error_text(&rejected).contains("terminal process has exited"),
            "exited-tab failure must be explicit: {}",
            error_text(&rejected)
        );
    }

    cli_json(exe, &endpoint, &["send-ui-ime", "preedit", "retry-ime"]);
    let rejected_ime = invoke(exe, &endpoint, &["send-ui-ime", "commit", "unavailable"]);
    assert!(!rejected_ime.status.success());
    assert!(
        error_text(&rejected_ime).contains("terminal process has exited"),
        "exited terminal IME commit must fail explicitly: {}",
        error_text(&rejected_ime)
    );
    assert_eq!(
        cli_json(exe, &endpoint, &["ui-snapshot"])["terminal_ime_preedit"],
        "retry-ime"
    );
    cli_json(exe, &endpoint, &["send-ui-ime", "disabled"]);

    cli_json(exe, &endpoint, &["send-ui-keys", "Ctrl+Shift+I"]);
    cli_json(
        exe,
        &endpoint,
        &["send-ui-keys", "R", "E", "T", "R", "Y", "Enter"],
    );
    let soft_newline = cli_json(exe, &endpoint, &["ui-snapshot"]);
    assert_eq!(soft_newline["composer_focused"], true);
    assert_eq!(soft_newline["composer_text"], "RETRY\n");
    assert_eq!(
        soft_newline["composer_submit_error"],
        serde_json::Value::Null
    );

    cli_json(exe, &endpoint, &["send-ui-keys", "Ctrl+O"]);
    let failed_composer = cli_json(exe, &endpoint, &["ui-snapshot"]);
    assert_eq!(failed_composer["composer_focused"], true);
    assert_eq!(failed_composer["composer_text"], "RETRY\n");
    assert!(
        failed_composer["composer_submit_error"]
            .as_str()
            .is_some_and(|error| error.contains("terminal process has exited")),
        "failed composer submit must be observable without losing text: {failed_composer}"
    );
    cli_json(exe, &endpoint, &["send-ui-keys", "Escape"]);

    let exited_mouse = cli_json(
        exe,
        &endpoint,
        &[
            "send-mouse",
            "--target",
            &child_id,
            "--action",
            "click",
            "--button",
            "left",
            "--column",
            "0",
            "--row",
            "0",
        ],
    );
    assert_eq!(exited_mouse["delivered"], true);
    assert_eq!(exited_mouse["route"], "selection");
    assert_eq!(exited_mouse["changed"], true);

    let exited_wheel = cli_json(
        exe,
        &endpoint,
        &[
            "send-wheel",
            "--target",
            &child_id,
            "--column",
            "0",
            "--row",
            "0",
            "--notches",
            "1",
        ],
    );
    assert_eq!(exited_wheel["route"], "scrollback");
    assert_eq!(exited_wheel["delivered_notches"], 1);
    assert_eq!(exited_wheel["changed"], true);

    cli_json(
        exe,
        &endpoint,
        &[
            "send-mouse",
            "--target",
            &child_id,
            "--action",
            "press",
            "--button",
            "left",
            "--column",
            "0",
            "--row",
            "0",
        ],
    );
    let cancelled_pointer = cli_json(exe, &endpoint, &["cancel-pointer"]);
    assert_eq!(cancelled_pointer["cancelled_owner"], child_id);
    let cancelled_snapshot = cli_json(exe, &endpoint, &["ui-snapshot"]);
    assert_eq!(
        cancelled_snapshot["control_pointer_owner"],
        serde_json::Value::Null
    );
    let idempotent_cancel = cli_json(exe, &endpoint, &["cancel-pointer"]);
    assert_eq!(
        idempotent_cancel["cancelled_owner"],
        serde_json::Value::Null
    );
    let release_after_cancel = invoke(
        exe,
        &endpoint,
        &[
            "send-mouse",
            "--target",
            &child_id,
            "--action",
            "release",
            "--button",
            "left",
            "--column",
            "0",
            "--row",
            "0",
        ],
    );
    assert!(!release_after_cancel.status.success());
    assert!(error_text(&release_after_cancel).contains("no matching control pointer press"));

    let held_pointer = cli_json(
        exe,
        &endpoint,
        &[
            "send-mouse",
            "--target",
            &child_id,
            "--action",
            "press",
            "--button",
            "left",
            "--column",
            "0",
            "--row",
            "0",
        ],
    );
    assert_eq!(held_pointer["route"], "selection");
    let held_snapshot = cli_json(exe, &endpoint, &["ui-snapshot"]);
    assert_eq!(held_snapshot["control_pointer_owner"], child_id);
    cli_json(exe, &endpoint, &["select-tab", "--target", &root]);
    let switched_snapshot = cli_json(exe, &endpoint, &["ui-snapshot"]);
    assert_eq!(
        switched_snapshot["control_pointer_owner"],
        serde_json::Value::Null
    );
    let stale_release = invoke(
        exe,
        &endpoint,
        &[
            "send-mouse",
            "--target",
            &child_id,
            "--action",
            "release",
            "--button",
            "left",
            "--column",
            "0",
            "--row",
            "0",
        ],
    );
    assert!(!stale_release.status.success());
    assert!(
        error_text(&stale_release).contains("no matching control pointer press"),
        "tab activation must cancel the old control gesture: {}",
        error_text(&stale_release)
    );

    cli_json(
        exe,
        &endpoint,
        &[
            "send-mouse",
            "--target",
            &child_id,
            "--action",
            "press",
            "--button",
            "left",
            "--column",
            "0",
            "--row",
            "0",
        ],
    );
    let replacement = cli_json(exe, &endpoint, &["new-tab"]);
    let replacement_id = replacement["id"].as_str().unwrap().to_owned();
    let stale_background_release = invoke(
        exe,
        &endpoint,
        &[
            "send-mouse",
            "--target",
            &child_id,
            "--action",
            "release",
            "--button",
            "left",
            "--column",
            "0",
            "--row",
            "0",
        ],
    );
    assert!(!stale_background_release.status.success());
    assert!(
        error_text(&stale_background_release).contains("no matching control pointer press"),
        "new-tab must cancel a background control gesture: {}",
        error_text(&stale_background_release)
    );
    cli_json(exe, &endpoint, &["close-tab", "--target", &replacement_id]);

    cli_json(
        exe,
        &endpoint,
        &[
            "send-text",
            "--target",
            &root,
            "echo ROOT_AFTER_CHILD_EXIT\r",
        ],
    );
    cli_json(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &root,
            "--timeout-ms",
            "10000",
            "ROOT_AFTER_CHILD_EXIT",
        ],
    );
    let listed_after_exit = cli_json(exe, &endpoint, &["list-tabs"]);
    let root_state = listed_after_exit["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["id"] == root))
        .expect("root tab remains listed");
    let child_state = listed_after_exit["tabs"]
        .as_array()
        .and_then(|tabs| tabs.iter().find(|tab| tab["id"] == child_id))
        .expect("exited child tab remains listed");
    assert_eq!(root_state["child_alive"], true);
    assert_eq!(child_state["child_alive"], false);
    assert_eq!(child_state["child_exit_code"], 7);

    let mut flood_ids = Vec::new();
    for index in 0..4 {
        let flood = cli_json(exe, &endpoint, &["new-tab", "--parent", &root]);
        let flood_id = tab_id(&flood["id"]).to_owned();
        let ready = format!("QUEUE_CLOSE_START_{index}");
        let ready_command = format!("echo {ready}\r");
        cli_json(
            exe,
            &endpoint,
            &["send-text", "--target", &flood_id, &ready_command],
        );
        cli_json(
            exe,
            &endpoint,
            &[
                "wait-text",
                "--target",
                &flood_id,
                "--timeout-ms",
                "10000",
                &ready,
            ],
        );
        let fill = format!("QUEUE_FILL_{index}_");
        let fill_command = shell_flood_command(&fill);
        cli_json(
            exe,
            &endpoint,
            &["send-text", "--target", &flood_id, &fill_command],
        );
        cli_json(
            exe,
            &endpoint,
            &[
                "wait-text",
                "--target",
                &flood_id,
                "--timeout-ms",
                "10000",
                &fill,
            ],
        );
        flood_ids.push(flood_id);
    }
    let active_before_shots = cli_json(exe, &endpoint, &["ui-snapshot"])["active"].clone();
    let mut screenshot_jobs = Vec::new();
    for (index, flood_id) in flood_ids.iter().enumerate() {
        let path =
            std::env::temp_dir().join(format!("minicon-{suffix}-concurrent-shot-{index}.png"));
        let child = Command::new(exe)
            .args([
                "cli",
                "--control",
                &endpoint,
                "screenshot-pane",
                "--target",
                flood_id,
                "--output",
                path.to_str().expect("screenshot path is Unicode"),
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("concurrent screenshot CLI must start");
        screenshot_jobs.push((child, path));
    }
    let screenshot_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let mut running = false;
        for (child, _) in &mut screenshot_jobs {
            running |= child
                .try_wait()
                .expect("poll concurrent screenshot CLI")
                .is_none();
        }
        if !running {
            break;
        }
        assert!(
            Instant::now() < screenshot_deadline,
            "concurrent screenshot requests did not finish within their bounded deadline"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let mut screenshot_successes = 0;
    let mut screenshot_busy = 0;
    for (child, path) in screenshot_jobs {
        let output = child
            .wait_with_output()
            .expect("concurrent screenshot CLI must be reapable");
        if output.status.success() {
            screenshot_successes += 1;
            let bytes = fs::read(&path).expect("successful screenshot publishes its PNG");
            assert!(bytes.starts_with(&[0x89, b'P', b'N', b'G']));
        } else {
            let error = String::from_utf8_lossy(&output.stderr);
            assert!(
                error.contains("a screenshot is already pending"),
                "concurrent screenshot failed without typed busy result: {error}"
            );
            screenshot_busy += 1;
        }
        let _ = fs::remove_file(path);
    }
    // The public control server may finish one screenshot before it accepts
    // the next queued CLI connection, especially on fast/native renderers.
    // Multiple successes are therefore sequential, not evidence of multiple
    // in-flight screenshots. Every request must still end as either a valid
    // PNG or the typed busy result, and the pending count below must drain.
    assert!(screenshot_successes >= 1);
    assert_eq!(screenshot_successes + screenshot_busy, flood_ids.len());
    let after_concurrent_shots = cli_json(exe, &endpoint, &["ui-snapshot"]);
    assert_eq!(after_concurrent_shots["pending_control_screenshots"], 0);
    assert_eq!(after_concurrent_shots["active"], active_before_shots);

    let raced_path = std::env::temp_dir().join(format!("minicon-{suffix}-raced-shot.png"));
    let raced_path_text = raced_path
        .to_str()
        .expect("raced screenshot path is Unicode")
        .to_owned();
    cli_json(exe, &endpoint, &["reset-perf-stats"]);
    let mut raced_shot = Command::new(exe)
        .args([
            "cli",
            "--control",
            &endpoint,
            "screenshot-pane",
            "--target",
            &flood_ids[0],
            "--output",
            &raced_path_text,
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("raced screenshot CLI must start");
    // Starting a second public CLI process is not the screenshot response
    // boundary. On a cold or contended native runner that process can remain
    // unscheduled while another CLI connection selects the tab, causing this
    // test to charge process-launch latency to the 10-second GUI response
    // criterion. Observe the request in the public snapshot first; only then
    // race active-tab selection against owned screenshot work and start the
    // response deadline.
    let registration_deadline = Instant::now() + Duration::from_secs(30);
    let raced_while_pending = loop {
        if raced_shot
            .try_wait()
            .expect("poll screenshot registration")
            .is_some()
        {
            // A fast renderer can complete between public snapshots. That is
            // already stronger than the bounded response criterion, but it
            // leaves no pending work for this attempt to race.
            break false;
        }
        let state = cli_json(exe, &endpoint, &["ui-snapshot"]);
        if state["pending_control_screenshots"].as_u64() == Some(1) {
            break true;
        }
        assert!(
            Instant::now() < registration_deadline,
            "screenshot CLI did not register its request within the process-launch deadline"
        );
        thread::sleep(Duration::from_millis(20));
    };
    cli_json(exe, &endpoint, &["select-tab", "--target", &root]);
    let raced_deadline = Instant::now() + Duration::from_secs(10);
    if raced_while_pending {
        while raced_shot
            .try_wait()
            .expect("poll raced screenshot CLI")
            .is_none()
        {
            assert!(
                Instant::now() < raced_deadline,
                "registered screenshot racing active-tab selection did not complete"
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
    let raced_output = raced_shot
        .wait_with_output()
        .expect("raced screenshot CLI must be reapable");
    assert!(
        raced_output.status.success(),
        "raced screenshot failed: {}",
        error_text(&raced_output)
    );
    let raced_bytes = fs::read(&raced_path).expect("raced screenshot publishes its PNG");
    assert!(raced_bytes.starts_with(&[0x89, b'P', b'N', b'G']));
    let _ = fs::remove_file(raced_path);
    assert_eq!(cli_json(exe, &endpoint, &["ui-snapshot"])["active"], root);
    assert_eq!(
        cli_json(exe, &endpoint, &["perf-stats"])["discarded_capture_frames"],
        1
    );
    for (index, flood_id) in flood_ids.iter().enumerate() {
        let mut close_flood = Command::new(exe)
            .args([
                "cli",
                "--control",
                &endpoint,
                "close-tab",
                "--target",
                flood_id,
            ])
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .expect("flooded tab close CLI must start");
        let close_deadline = Instant::now() + Duration::from_secs(3);
        while close_flood
            .try_wait()
            .expect("poll flooded tab close")
            .is_none()
        {
            assert!(
                Instant::now() < close_deadline,
                "closing flooded tab {index} exceeded the bounded shutdown deadline"
            );
            thread::sleep(Duration::from_millis(20));
        }
        let close_output = close_flood
            .wait_with_output()
            .expect("flooded tab close CLI must be reapable");
        assert!(
            close_output.status.success(),
            "flooded tab {index} close failed: {}",
            error_text(&close_output)
        );
        let listed = cli_json(exe, &endpoint, &["list-tabs"]);
        assert!(
            listed["tabs"]
                .as_array()
                .is_some_and(|tabs| tabs.iter().all(|tab| tab["id"] != *flood_id)),
            "closed flooded tab remained in tree: {listed}"
        );
        let root_marker = format!("ROOT_AFTER_FLOOD_CLOSE_{index}");
        let root_command = format!("echo {root_marker}\r");
        cli_json(
            exe,
            &endpoint,
            &["send-text", "--target", &root, &root_command],
        );
        cli_json(
            exe,
            &endpoint,
            &[
                "wait-text",
                "--target",
                &root,
                "--timeout-ms",
                "10000",
                &root_marker,
            ],
        );
    }

    cli_json(exe, &endpoint, &["close-tab", "--target", &root]);
    let after_close = cli_json(exe, &endpoint, &["list-tabs"]);
    assert_eq!(after_close["tabs"].as_array().map(Vec::len), Some(1));
    assert_eq!(after_close["tabs"][0]["id"], child_id);
    assert!(after_close["tabs"][0]["parent"].is_null());
    let mut pending_wait = Command::new(exe)
        .args([
            "cli",
            "--control",
            &endpoint,
            "wait-text",
            "--target",
            &child_id,
            "--timeout-ms",
            "10000",
            "NEVER_MATCH_CLOSED_TAB",
        ])
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .expect("pending wait CLI must start");
    let pending_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let state = cli_json(exe, &endpoint, &["ui-snapshot"]);
        if state["pending_control_waits"].as_u64() == Some(1) {
            assert_eq!(state["pending_control_screenshots"], 0);
            break;
        }
        assert!(
            Instant::now() < pending_deadline,
            "wait-text was not registered before tab close: {state}"
        );
        thread::sleep(Duration::from_millis(20));
    }
    cli_json(exe, &endpoint, &["close-tab", "--target", &child_id]);
    let cancel_deadline = Instant::now() + Duration::from_secs(3);
    while pending_wait
        .try_wait()
        .expect("poll cancelled wait CLI")
        .is_none()
    {
        assert!(
            Instant::now() < cancel_deadline,
            "closing a tab did not cancel its pending wait"
        );
        thread::sleep(Duration::from_millis(20));
    }
    let cancelled = pending_wait
        .wait_with_output()
        .expect("cancelled wait CLI must be reapable");
    assert!(!cancelled.status.success());
    let cancel_error = String::from_utf8_lossy(&cancelled.stderr);
    assert!(
        cancel_error.contains(&format!("terminal {child_id} closed")),
        "pending wait did not receive a typed close error: {cancel_error}"
    );
    let empty = cli_json(exe, &endpoint, &["ui-snapshot"]);
    assert_eq!(empty["workspace_empty"], true);
    assert!(
        gui.child
            .try_wait()
            .expect("poll greeting window")
            .is_none(),
        "closing the final tab must retain the greeting window"
    );
    cli_json(exe, &endpoint, &["close-window"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    let status = loop {
        if let Some(status) = gui.child.try_wait().expect("poll explicitly closed GUI") {
            break status;
        }
        assert!(
            Instant::now() < deadline,
            "explicit close-window did not exit the greeting GUI"
        );
        thread::sleep(Duration::from_millis(20));
    };
    assert!(status.success(), "explicit close failed with {status:?}");
}

/// Regression ceiling for one idle tab: MiniCon **host** RSS / working set.
/// Child shells are outside this budget. This is a fail-closed climb tripwire
/// established before the Unix whole-font leak repair. Product intent is
/// 10 MiB idle; see `prd/PRD_02_27_con_delivery.md`. Do not treat this
/// constant as the product size.
const IDLE_ONE_TAB_HOST_RSS_BYTES: u64 = 384 * 1024 * 1024;
/// Extra host RSS allowed for a second live tab. Not the child's own RSS.
const EXTRA_TAB_HOST_RSS_DELTA_BYTES: u64 = 16 * 1024 * 1024;
/// Host RSS may not climb by this much across four extra-tab open/close
/// cycles. Allocators and GPU mappings need not return every page; a
/// 100 MiB climb is a leak, a 20 MiB wobble on macOS is not.
const TAB_CYCLE_HOST_RSS_GROWTH_BYTES: u64 = 32 * 1024 * 1024;

#[cfg(windows)]
#[repr(C)]
struct ProcessMemoryCounters {
    cb: u32,
    page_fault_count: u32,
    peak_working_set_size: usize,
    working_set_size: usize,
    quota_peak_paged_pool_usage: usize,
    quota_paged_pool_usage: usize,
    quota_peak_non_paged_pool_usage: usize,
    quota_non_paged_pool_usage: usize,
    pagefile_usage: usize,
    peak_pagefile_usage: usize,
}

#[cfg(windows)]
#[link(name = "kernel32")]
unsafe extern "system" {
    fn K32GetProcessMemoryInfo(
        process: *mut std::ffi::c_void,
        counters: *mut ProcessMemoryCounters,
        size: u32,
    ) -> i32;
}

fn host_rss_bytes_stable(child: &Child) -> u64 {
    let mut samples = [host_rss_bytes(child), 0, 0];
    for sample in samples.iter_mut().skip(1) {
        thread::sleep(Duration::from_millis(40));
        *sample = host_rss_bytes(child);
    }
    samples.sort_unstable();
    samples[1]
}

// On Windows the `not(windows)` block below is compiled out, which leaves the
// `return` looking redundant; on every other host it is what ends the Windows
// block early. The lint cannot see across the cfg split.
#[cfg_attr(
    windows,
    expect(clippy::needless_return, reason = "cfg-split early return")
)]
fn host_rss_bytes(child: &Child) -> u64 {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        let mut counters = ProcessMemoryCounters {
            cb: std::mem::size_of::<ProcessMemoryCounters>() as u32,
            page_fault_count: 0,
            peak_working_set_size: 0,
            working_set_size: 0,
            quota_peak_paged_pool_usage: 0,
            quota_paged_pool_usage: 0,
            quota_peak_non_paged_pool_usage: 0,
            quota_non_paged_pool_usage: 0,
            pagefile_usage: 0,
            peak_pagefile_usage: 0,
        };
        let ok =
            unsafe { K32GetProcessMemoryInfo(child.as_raw_handle(), &mut counters, counters.cb) };
        assert!(
            ok != 0,
            "K32GetProcessMemoryInfo must report the MiniCon host"
        );
        return counters.working_set_size as u64;
    }
    #[cfg(not(windows))]
    {
        let output = Command::new("ps")
            .args(["-o", "rss=", "-p", &child.id().to_string()])
            .output()
            .expect("ps must report MiniCon host RSS");
        assert!(
            output.status.success(),
            "ps failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let kb: u64 = String::from_utf8_lossy(&output.stdout)
            .trim()
            .parse()
            .unwrap_or_else(|_| {
                panic!(
                    "ps RSS was not an integer: {:?}",
                    String::from_utf8_lossy(&output.stdout)
                )
            });
        kb.saturating_mul(1024)
    }
}

fn format_mib(bytes: u64) -> String {
    format!("{:.2} MiB", bytes as f64 / (1024.0 * 1024.0))
}

/// MiniCon stays small by keeping the **host** process bounded. Child shells
/// spend whatever they spend. This court launches the real GUI, reads host
/// RSS/working-set, and fails closed when the named debug ceiling is crossed.
#[test]
fn host_process_rss_stays_within_named_budget() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let suffix = unique_suffix();
    let endpoint = control_endpoint(&suffix);
    let screenshot = if cfg!(windows) {
        std::env::temp_dir().join(format!("minicon-rss-{suffix}.png"))
    } else {
        agenterm_platform::ipc::native_runtime_directory().join(format!("rss-{suffix}.png"))
    };
    let mut host = Command::new(exe);
    host.arg("--no-activate")
        .arg("--cols")
        .arg("80")
        .arg("--rows")
        .arg("24")
        .arg("--control")
        .arg(&endpoint)
        .arg("-e");
    for arg in host_shell_args() {
        host.arg(arg);
    }
    let child = host.spawn().expect("minicon GUI must start");
    let mut gui = OwnedGui { child, screenshot };

    let listed = wait_until_ready_for(
        exe,
        &endpoint,
        Duration::from_secs(15),
        Some(&mut gui.child),
    );
    let root = tab_id(&listed["tabs"][0]["id"]).to_owned();
    assert_eq!(listed["tabs"].as_array().map(Vec::len), Some(1));

    let idle = host_rss_bytes_stable(&gui.child);
    eprintln!(
        "minicon host RSS idle one-tab: {} ({idle} bytes)",
        format_mib(idle)
    );
    assert!(idle > 0, "host RSS must be observable: {idle}");
    assert!(
        idle <= IDLE_ONE_TAB_HOST_RSS_BYTES,
        "idle one-tab MiniCon host RSS {} exceeds named debug ceiling {}",
        format_mib(idle),
        format_mib(IDLE_ONE_TAB_HOST_RSS_BYTES)
    );

    // Optional native heap evidence belongs to this exact idle GUI, after the
    // RSS sample and before PTY load. Tools are diagnostics, not product APIs.
    #[cfg(target_os = "macos")]
    if let Some(directory) = std::env::var_os("MINICON_RSS_DIAGNOSTICS_DIR") {
        let directory = PathBuf::from(directory);
        fs::create_dir_all(&directory).expect("create RSS diagnostics directory");
        for (tool, flag, file) in [
            ("/usr/bin/vmmap", "-w", "idle-vmmap.txt"),
            ("/usr/bin/heap", "-s", "idle-heap.txt"),
        ] {
            let output = Command::new(tool)
                .args([flag, &gui.child.id().to_string()])
                .output()
                .expect("start native memory diagnostic");
            assert!(output.status.success(), "{tool}: {}", error_text(&output));
            fs::write(directory.join(file), output.stdout).expect("save memory diagnostic");
        }
    }

    cli_json(
        exe,
        &endpoint,
        &["send-text", "--target", &root, shell_load_done_command()],
    );
    cli_json(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &root,
            "--timeout-ms",
            "10000",
            "LOAD_DONE",
        ],
    );
    let after_load = host_rss_bytes_stable(&gui.child);
    eprintln!(
        "minicon host RSS after 2000-line load: {} ({after_load} bytes)",
        format_mib(after_load)
    );
    assert!(
        after_load <= IDLE_ONE_TAB_HOST_RSS_BYTES,
        "2000-line PTY load grew MiniCon host RSS to {} above the idle ceiling {}",
        format_mib(after_load),
        format_mib(IDLE_ONE_TAB_HOST_RSS_BYTES)
    );

    let mut after_close = after_load;
    let mut first_extra_delta = 0;
    for cycle in 1..=4 {
        let created = cli_json(exe, &endpoint, &["new-tab"]);
        let extra = tab_id(&created["id"]).to_owned();
        let two_tabs = host_rss_bytes_stable(&gui.child);
        let delta = two_tabs.saturating_sub(after_close);
        if cycle == 1 {
            first_extra_delta = delta;
        }
        eprintln!(
            "minicon host RSS two-tab cycle {cycle}: {} (delta {})",
            format_mib(two_tabs),
            format_mib(delta)
        );
        assert!(
            delta <= EXTRA_TAB_HOST_RSS_DELTA_BYTES,
            "second tab added {} to MiniCon host RSS; named extra-tab delta is {}",
            format_mib(delta),
            format_mib(EXTRA_TAB_HOST_RSS_DELTA_BYTES)
        );
        cli_json(exe, &endpoint, &["close-tab", "--target", &extra]);
        after_close = host_rss_bytes_stable(&gui.child);
    }
    let cycle_growth = after_close.saturating_sub(after_load);
    eprintln!(
        "minicon host RSS after four extra-tab cycles: {} (growth {})",
        format_mib(after_close),
        format_mib(cycle_growth)
    );
    assert!(
        cycle_growth <= TAB_CYCLE_HOST_RSS_GROWTH_BYTES,
        "four extra-tab cycles grew MiniCon host RSS by {}; named leak bound is {}",
        format_mib(cycle_growth),
        format_mib(TAB_CYCLE_HOST_RSS_GROWTH_BYTES)
    );
    eprintln!(
        "MINICON_HOST_RSS_RECEIPT {}",
        serde_json::json!({
            "os": std::env::consts::OS,
            "arch": std::env::consts::ARCH,
            "idle_bytes": idle,
            "after_load_bytes": after_load,
            "extra_tab_delta_bytes": first_extra_delta,
            "cycle_growth_bytes": cycle_growth,
            "idle_ceiling_bytes": IDLE_ONE_TAB_HOST_RSS_BYTES,
            "extra_tab_ceiling_bytes": EXTRA_TAB_HOST_RSS_DELTA_BYTES,
            "cycle_growth_ceiling_bytes": TAB_CYCLE_HOST_RSS_GROWTH_BYTES,
        })
    );

    cli_json(exe, &endpoint, &["close-window"]);
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if gui.child.try_wait().expect("poll closed RSS GUI").is_some() {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "close-window did not exit the RSS court GUI"
        );
        thread::sleep(Duration::from_millis(20));
    }
}

// A raw PTY peer distinguishes pasted draft bytes from the final Enter.
// It does not echo input, so only bytes actually received can satisfy the check.
#[cfg(unix)]
#[test]
fn composer_send_delivers_paste_then_submit_to_raw_application() {
    let binary = minicon_binary();
    let endpoint = control_endpoint(&unique_suffix());
    let script = r"stty raw -echo; printf '\033[?2004hCOMPOSER_READY\r\n'; for n in 18 25; do dd bs=1 count=$n 2>/dev/null | od -An -tx1 | tr -d ' \n'; printf '\r\n'; done; sleep 10";
    let child = Command::new(&binary)
        .args([
            "--no-activate",
            "--control",
            &endpoint,
            "-e",
            "/bin/sh",
            "-c",
            script,
        ])
        .spawn()
        .expect("raw PTY GUI");
    let mut gui = OwnedGui {
        child,
        screenshot: std::env::temp_dir().join(unique_suffix()),
    };
    wait_until_ready_for(
        &binary,
        &endpoint,
        Duration::from_secs(15),
        Some(&mut gui.child),
    );
    cli_json(
        &binary,
        &endpoint,
        &["wait-text", "--timeout-ms", "10000", "COMPOSER_READY"],
    );
    cli_json(&binary, &endpoint, &["send-ui-keys", "Ctrl+Shift+I"]);
    for draft in ["hello", "first\nsecond"] {
        cli_json(&binary, &endpoint, &["send-ui-ime", "commit", draft]);
        cli_json(&binary, &endpoint, &["send-ui-keys", "Ctrl+O"]);
        let expected = format!("\x1b[200~{}\x1b[201~\r", draft.replace('\n', "\r"));
        let hex: String = expected.bytes().map(|b| format!("{b:02x}")).collect();
        cli_json(
            &binary,
            &endpoint,
            &["wait-text", "--timeout-ms", "10000", &hex],
        );
        assert_eq!(
            cli_json(&binary, &endpoint, &["ui-snapshot"])["composer_text"],
            ""
        );
    }
}
/// A shell that will not start must not end the host. The initial tab runs a
/// copy of a real shell that is deleted once it is running (Windows marks a
/// running image delete-pending), so the *second* spawn of the same program
/// cannot find it — the exact per-tab failure that used to return `Err`, which
/// the host turns into an exit for every tab. After the failure the host must
/// still answer control, report the reason as `host_notice` in `ui-snapshot`,
/// and keep the first tab alive.
#[test]
fn a_new_tab_that_cannot_start_is_a_notice_not_an_exit() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let suffix = unique_suffix();
    let endpoint = control_endpoint(&suffix);
    let screenshot = std::env::temp_dir().join(format!("minicon-{suffix}.png"));

    // A disposable copy of a real shell, deleted after it starts so the next
    // spawn fails while the running one is unaffected.
    let dir = std::env::temp_dir().join(format!("minicon-oneshot-{suffix}"));
    fs::create_dir_all(&dir).expect("scratch dir");
    let stub = dir.join(format!("oneshot{}", std::env::consts::EXE_SUFFIX));
    // The first tab must run a program that stays alive, survives its own file
    // being deleted, and still leaves the *second* spawn of the same path to
    // fail. That "delete the running image, it keeps running" assumption only
    // holds for a self-contained binary. On Windows a copy of a real shell
    // works (a running image is delete-pending). On macOS neither a copied
    // system binary (an arm64e platform binary the kernel refuses) nor a shell
    // script (macOS /bin/sh re-opens the script path as it runs, so deleting
    // it kills the shell) survives, so compile a tiny standalone binary: once
    // running it holds its own inode and outlives its file, while the second
    // spawn of the deleted path still fails.
    let shell_args: Vec<&str> = if cfg!(windows) {
        let real_path = resolve_on_path("cmd.exe").expect("a real shell on PATH");
        fs::copy(&real_path, &stub).expect("copy the shell to a disposable name");
        vec!["/Q", "/K"]
    } else {
        let src = dir.join("oneshot.rs");
        fs::write(
            &src,
            "fn main() { std::thread::sleep(std::time::Duration::from_secs(86_400)); }\n",
        )
        .expect("write the stub source");
        let rustc = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
        let built = Command::new(&rustc)
            .arg("-O")
            .arg(&src)
            .arg("-o")
            .arg(&stub)
            .status()
            .expect("run rustc to build the stub");
        assert!(built.success(), "rustc must build the stub binary");
        Vec::new()
    };

    let mut host = Command::new(exe);
    host.arg("--no-activate")
        .arg("--control")
        .arg(&endpoint)
        .arg("-e");
    host.arg(&stub);
    for arg in &shell_args {
        host.arg(arg);
    }
    let child = host.spawn().expect("minicon GUI must start");
    let mut gui = OwnedGui { child, screenshot };

    let listed = wait_until_ready_for(
        exe,
        &endpoint,
        Duration::from_secs(15),
        Some(&mut gui.child),
    );
    let first = tab_id(&listed["tabs"][0]["id"]).to_owned();

    // The stub must actually be running before this test means anything: it
    // asserts a *failed* second tab leaves the first alive, which proves
    // nothing if the first was never alive. Poll until its child is up.
    let alive_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let now = cli_json(exe, &endpoint, &["list-tabs"]);
        if now["tabs"][0]["child_alive"] == true {
            break;
        }
        assert!(
            Instant::now() < alive_deadline,
            "the first tab's program must be alive before the failed-second-tab test; snapshot: {now}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }

    // Make the same program fail to start for the *second* tab without
    // disturbing the first. On Windows delete it: a running image is
    // delete-pending, so the first tab keeps running while the next spawn of
    // the deleted path fails. On Unix drop its execute bit instead: deleting a
    // running image *terminates* it on macOS (the kernel kills a process whose
    // executable file is unlinked, unlike Linux which keeps it on its inode),
    // so a delete would end the first tab and defeat the test; a running
    // process is unaffected by a permission change, while a fresh execve of the
    // now-unexecutable path fails with EACCES.
    #[cfg(windows)]
    {
        let _ = fs::remove_file(&stub);
        if stub.exists() {
            let moved = dir.with_file_name(format!("minicon-oneshot-gone-{suffix}"));
            let _ = fs::rename(&dir, &moved);
        }
    }
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&stub, fs::Permissions::from_mode(0o000))
            .expect("drop the stub's execute bit so the second spawn fails");
    }
    // The user gesture (Ctrl+Shift+T) must fail as a *notice*, not by ending
    // the host: the control `new-tab` reply is a separate path that reports to
    // its caller. Capture the window before and after so the notice is proven
    // painted, not merely present in `ui-snapshot`.
    let before_shot = std::env::temp_dir().join(format!("minicon-notice-before-{suffix}.png"));
    cli_json(
        exe,
        &endpoint,
        &[
            "screenshot-pane",
            "--target",
            &first,
            "--output",
            &before_shot.to_string_lossy(),
        ],
    );
    cli_json(exe, &endpoint, &["send-ui-keys", "Ctrl+Shift+T"]);
    let after_shot = std::env::temp_dir().join(format!("minicon-notice-after-{suffix}.png"));
    cli_json(
        exe,
        &endpoint,
        &[
            "screenshot-pane",
            "--target",
            &first,
            "--output",
            &after_shot.to_string_lossy(),
        ],
    );

    let snapshot = cli_json(exe, &endpoint, &["ui-snapshot"]);
    let notice = &snapshot["host_notice"];
    assert!(
        !notice.is_null(),
        "a new tab that cannot start must surface a host_notice; snapshot: {snapshot}"
    );
    assert!(
        notice
            .as_str()
            .is_some_and(|n| n.contains("could not open a terminal")),
        "the notice must name the failed open; got {notice}"
    );

    // The original tab is still alive, so the failure stayed contained.
    let listed = cli_json(exe, &endpoint, &["list-tabs"]);
    let tabs = listed["tabs"].as_array().expect("tabs array");
    assert!(
        tabs.iter()
            .any(|t| tab_id(&t["id"]) == first && t["child_alive"] == true),
        "the first tab must survive a failed second tab; snapshot: {listed}"
    );

    // The status strip is host UI; the notice must actually reach it. Compare
    // the strip band (near the composer top, right of the tab column) between
    // the two shots. The window keeps its size, so any difference is content.
    fn decode(path: &Path) -> (usize, usize, usize, Vec<u8>) {
        let decoder = png::Decoder::new(fs::File::open(path).expect("open screenshot"));
        let mut reader = decoder.read_info().expect("read screenshot info");
        let mut bytes = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut bytes).expect("decode screenshot");
        bytes.truncate(info.buffer_size());
        let channels = match info.color_type {
            png::ColorType::Rgb => 3,
            png::ColorType::Rgba => 4,
            other => panic!("unexpected screenshot color type {other:?}"),
        };
        (info.width as usize, info.height as usize, channels, bytes)
    }
    let (bw, bh, before_channels, before) = decode(&before_shot);
    let (aw, ah, after_channels, after) = decode(&after_shot);
    assert_eq!((aw, ah), (bw, bh), "the window changed size while noticing");
    assert_eq!(before_channels, after_channels, "color type changed");
    let band_changed = (0..bh)
        // The strip sits just above the composer input row, which the earlier
        // `ui-snapshot` geometry puts near the bottom band of the window.
        .filter(|y| *y * 10 >= bh * 8 && *y * 10 <= bh * 9)
        .flat_map(|y| (bw / 3..bw * 5 / 6).map(move |x| (x, y)))
        .step_by(3)
        .filter(|(x, y)| {
            let at = (y * bw + x) * before_channels;
            before[at..at + 3] != after[at..at + 3]
        })
        .count();
    assert!(
        band_changed > 0,
        "the status strip did not repaint when the notice appeared"
    );
    let _ = fs::remove_file(&before_shot);
    let _ = fs::remove_file(&after_shot);

    let _ = &mut gui;
}

/// `--headless` starts a working process with no window at all.
///
/// Not the same claim as detaching one later: here `opened` never runs, so the
/// session, the PTY and the endpoint all have to come up without a surface to
/// measure. The grid comes from `--cols`/`--rows` instead.
#[test]
fn a_headless_start_runs_a_session_and_can_grow_a_window_later() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let suffix = unique_suffix();
    let endpoint = control_endpoint(&suffix);
    let screenshot = if cfg!(windows) {
        std::env::temp_dir().join(format!("minicon-headless-{suffix}.png"))
    } else {
        agenterm_platform::ipc::native_runtime_directory().join(format!("headless-{suffix}.png"))
    };
    let mut host = Command::new(exe);
    host.arg("--headless")
        .arg("--no-activate")
        .arg("--cols")
        .arg("80")
        .arg("--rows")
        .arg("24")
        .arg("--control")
        .arg(&endpoint)
        .arg("-e");
    for arg in host_shell_args() {
        host.arg(arg);
    }
    let child = host.spawn().expect("minicon must start headless");
    let mut gui = OwnedGui { child, screenshot };
    let listed = wait_until_ready_for(
        exe,
        &endpoint,
        Duration::from_secs(15),
        Some(&mut gui.child),
    );
    assert_eq!(
        listed["tabs"].as_array().map(Vec::len),
        Some(1),
        "a headless start must still open its session"
    );
    let tab = tab_id(&listed["tabs"][0]["id"]).to_owned();

    let marker = format!("headless-{suffix}");
    let sent = invoke(exe, &endpoint, &["send-text", "--target", &tab, &marker]);
    assert!(sent.status.success(), "{}", error_text(&sent));
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let captured = invoke(exe, &endpoint, &["capture-pane", "--target", &tab]);
        if captured.status.success() && output_text(&captured).contains(&marker) {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "a headless session never echoed what was written to it"
        );
        thread::sleep(Duration::from_millis(100));
    }

    let attached = invoke(exe, &endpoint, &["attach-gui"]);
    if !attached.status.success() {
        let text = error_text(&attached);
        assert!(
            text.contains("unsupported") || text.contains("detachable"),
            "a refusal must name the missing capability, got: {text}"
        );
        let _ = &mut gui;
        return;
    }
    let snapshot = wait_for_geometry(exe, &endpoint);
    assert!(
        snapshot["geometry"]["frame"]["width"].as_u64().unwrap_or(0) > 0,
        "attaching to a headless process did not produce a window: {snapshot}"
    );
    let captured = invoke(exe, &endpoint, &["capture-pane", "--target", &tab]);
    assert!(
        output_text(&captured).contains(&marker),
        "the headless session's scrollback did not survive gaining a window"
    );

    let _ = &mut gui;
}

/// Detaching releases the window; the process, its sessions and this endpoint
/// do not notice.
///
/// This is the boundary amendment made observable. Before it, the control
/// endpoint was bound inside the window's `opened` callback, so "no window"
/// and "no endpoint" were the same state and neither could be tested apart
/// from the other. The assertions below are exactly the pair that used to be
/// impossible: the endpoint answers with no window, and the session's
/// scrollback survives the window that was displaying it.
#[test]
fn detaching_the_gui_keeps_the_process_and_its_sessions() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let suffix = unique_suffix();
    let endpoint = control_endpoint(&suffix);
    let screenshot = if cfg!(windows) {
        std::env::temp_dir().join(format!("minicon-detach-{suffix}.png"))
    } else {
        agenterm_platform::ipc::native_runtime_directory().join(format!("detach-{suffix}.png"))
    };
    let mut host = Command::new(exe);
    host.arg("--no-activate")
        .arg("--cols")
        .arg("80")
        .arg("--rows")
        .arg("24")
        .arg("--control")
        .arg(&endpoint)
        .arg("-e");
    for arg in host_shell_args() {
        host.arg(arg);
    }
    let child = host.spawn().expect("minicon GUI must start");
    let mut gui = OwnedGui { child, screenshot };
    let listed = wait_until_ready_for(
        exe,
        &endpoint,
        Duration::from_secs(15),
        Some(&mut gui.child),
    );
    let tab = tab_id(&listed["tabs"][0]["id"]).to_owned();

    // Something in the scrollback that must outlive the window.
    let marker = format!("detach-marker-{suffix}");
    let sent = invoke(exe, &endpoint, &["send-text", "--target", &tab, &marker]);
    assert!(sent.status.success(), "{}", error_text(&sent));

    let detached = invoke(exe, &endpoint, &["detach-gui"]);
    if !detached.status.success() {
        // A platform without a detachable window must say so in a typed error
        // rather than report success, and the host must still be healthy.
        let text = error_text(&detached);
        assert!(
            text.contains("unsupported") || text.contains("detachable"),
            "a refusal must name the missing capability, got: {text}"
        );
        let alive = invoke(exe, &endpoint, &["list-tabs"]);
        assert!(
            alive.status.success(),
            "refusing a detach must not kill the host"
        );
        let _ = &mut gui;
        return;
    }

    // The endpoint answers with no window at all — the pair that could not
    // previously be separated.
    let listed = cli_json(exe, &endpoint, &["list-tabs"]);
    assert_eq!(listed["tabs"].as_array().map(Vec::len), Some(1));
    assert!(
        gui.child.try_wait().expect("poll minicon exit").is_none(),
        "detaching must not end the process"
    );
    let captured = invoke(exe, &endpoint, &["capture-pane", "--target", &tab]);
    assert!(captured.status.success(), "{}", error_text(&captured));
    assert!(
        output_text(&captured).contains(&marker),
        "the session's scrollback did not survive its window"
    );

    // Detach is idempotent, and attaching brings a window back.
    let again = invoke(exe, &endpoint, &["detach-gui"]);
    assert!(again.status.success(), "{}", error_text(&again));
    let attached = invoke(exe, &endpoint, &["attach-gui"]);
    assert!(attached.status.success(), "{}", error_text(&attached));
    let snapshot = wait_for_geometry(exe, &endpoint);
    assert!(
        snapshot["geometry"]["frame"]["width"].as_u64().unwrap_or(0) > 0,
        "attaching did not produce a window with real geometry: {snapshot}"
    );
    let captured = invoke(exe, &endpoint, &["capture-pane", "--target", &tab]);
    assert!(
        output_text(&captured).contains(&marker),
        "the scrollback did not survive the reattach"
    );

    let _ = &mut gui;
}

/// Polls `ui-snapshot` until a window reports non-zero geometry, so the test
/// does not race the window the attach just asked for.
fn wait_for_geometry(exe: &Path, endpoint: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let snapshot = cli_json(exe, endpoint, &["ui-snapshot"]);
        if snapshot["geometry"]["frame"]["width"].as_u64().unwrap_or(0) > 0 {
            return snapshot;
        }
        assert!(
            Instant::now() < deadline,
            "no window appeared after attach-gui"
        );
        thread::sleep(Duration::from_millis(100));
    }
}

/// Collapsing the sidebar must actually hand the column to the terminal, and
/// expanding must hand it back exactly — not approximately. A rail that shrinks
/// the chrome without widening the grid is the failure this pins: the user sees
/// a narrower sidebar and the same amount of terminal, which is the change
/// looking like it worked while doing nothing.
#[test]
fn collapsing_the_sidebar_widens_the_terminal_and_expanding_restores_it_exactly() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let suffix = unique_suffix();
    let endpoint = control_endpoint(&suffix);
    let screenshot = if cfg!(windows) {
        std::env::temp_dir().join(format!("minicon-rail-{suffix}.png"))
    } else {
        agenterm_platform::ipc::native_runtime_directory().join(format!("rail-{suffix}.png"))
    };
    let mut host = Command::new(exe);
    host.arg("--no-activate")
        .arg("--cols")
        .arg("80")
        .arg("--rows")
        .arg("24")
        .arg("--control")
        .arg(&endpoint)
        .arg("-e");
    for arg in host_shell_args() {
        host.arg(arg);
    }
    let child = host.spawn().expect("minicon GUI must start");
    let mut gui = OwnedGui { child, screenshot };
    let _ = wait_until_ready_for(
        exe,
        &endpoint,
        Duration::from_secs(15),
        Some(&mut gui.child),
    );

    let viewport = |exe: &Path, endpoint: &str| -> (u64, u64, u64) {
        let snapshot = cli_json(exe, endpoint, &["ui-snapshot"]);
        let geometry = &snapshot["geometry"];
        (
            geometry["terminal_viewport"]["x"].as_u64().expect("x"),
            geometry["terminal_viewport"]["width"]
                .as_u64()
                .expect("width"),
            geometry["grid"]["cols"].as_u64().expect("cols"),
        )
    };

    let expanded = viewport(exe, &endpoint);
    let toggled = invoke(exe, &endpoint, &["send-ui-keys", "ctrl+shift+b"]);
    assert!(toggled.status.success(), "{}", error_text(&toggled));
    let collapsed = wait_for_change(exe, &endpoint, expanded, &viewport);
    assert!(
        collapsed.0 < expanded.0,
        "the rail did not move the terminal left: {collapsed:?} vs {expanded:?}"
    );
    assert!(
        collapsed.1 > expanded.1 && collapsed.2 > expanded.2,
        "the terminal did not gain the column the sidebar gave up: {collapsed:?} vs {expanded:?}"
    );

    let toggled = invoke(exe, &endpoint, &["send-ui-keys", "ctrl+shift+b"]);
    assert!(toggled.status.success(), "{}", error_text(&toggled));
    let restored = wait_for_change(exe, &endpoint, collapsed, &viewport);
    assert_eq!(
        restored, expanded,
        "expanding did not restore the exact geometry it started from"
    );

    let _ = &mut gui;
}

/// The control endpoint belongs to the process, not to the window.
///
/// It used to be bound inside the window's `opened` callback, so a client that
/// connected during startup was refused outright — and, more importantly for
/// what comes next, the endpoint could not outlive a window. This pins the
/// observable half of that change: a client racing startup finds a listener.
///
/// It also pins the failure direction. A bind failure must now be a startup
/// failure, reported before anything is put on screen, rather than a window
/// that appears and then dies.
#[test]
fn the_control_endpoint_answers_a_client_that_races_the_window() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let suffix = unique_suffix();
    let endpoint = control_endpoint(&suffix);
    let mut host = Command::new(exe);
    host.arg("--no-activate")
        .arg("--control")
        .arg(&endpoint)
        .arg("-e");
    for arg in host_shell_args() {
        host.arg(arg);
    }
    let child = host.spawn().expect("minicon GUI must start");
    let mut gui = OwnedGui {
        child,
        screenshot: std::env::temp_dir().join(format!("minicon-unused-{suffix}.png")),
    };

    // Poll hard from the first instant. What is being asserted is not that the
    // endpoint eventually works — every other test covers that — but that the
    // first answer arrives without the process having had to finish opening a
    // window first. A refused connection here is the regression.
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let mut refusals = 0_u32;
    loop {
        let output = invoke(exe, &endpoint, &["list-tabs"]);
        if output.status.success() {
            break;
        }
        let text = error_text(&output);
        // "not found" is the endpoint not existing yet; anything else is a
        // real protocol failure and should not be swallowed by a retry loop.
        assert!(
            text.contains("connect") || text.contains("No such file") || text.contains("cannot"),
            "unexpected control failure while racing startup: {text}"
        );
        refusals += 1;
        assert!(
            std::time::Instant::now() < deadline,
            "the endpoint never answered; {refusals} refusals, last: {text}"
        );
        std::thread::sleep(Duration::from_millis(20));
    }

    let _ = &mut gui;
}

/// A second process cannot bind an endpoint that is already taken, and it must
/// say so and exit rather than opening a window it will immediately lose.
#[test]
fn a_taken_endpoint_fails_at_startup_instead_of_opening_a_window() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let suffix = unique_suffix();
    let endpoint = control_endpoint(&suffix);
    let mut host = Command::new(exe);
    host.arg("--no-activate")
        .arg("--control")
        .arg(&endpoint)
        .arg("-e");
    for arg in host_shell_args() {
        host.arg(arg);
    }
    let child = host.spawn().expect("minicon GUI must start");
    let mut gui = OwnedGui {
        child,
        screenshot: std::env::temp_dir().join(format!("minicon-unused2-{suffix}.png")),
    };
    let _ = wait_until_ready_for(
        exe,
        &endpoint,
        Duration::from_secs(15),
        Some(&mut gui.child),
    );

    let mut second = Command::new(exe);
    second
        .arg("--no-activate")
        .arg("--control")
        .arg(&endpoint)
        .arg("-e");
    for arg in host_shell_args() {
        second.arg(arg);
    }
    let output = second
        .output()
        .expect("the second host must run to completion");
    assert!(
        !output.status.success(),
        "a duplicate endpoint must not be accepted"
    );
    let text = format!(
        "{}{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(
        text.contains("control endpoint unavailable"),
        "the failure must name the endpoint as the cause, got: {text}"
    );

    let _ = &mut gui;
}

/// Polls `ui-snapshot` until the terminal **grid** differs from `previous`, so
/// the test does not race the repaint that follows a key.
///
/// Waiting on the viewport rectangle is not enough: the host rect moves in the
/// frame the sidebar changes, while the cell grid is re-derived on the queued
/// resize a moment later. Keying on the grid means the assertion runs only once
/// the terminal has actually been told its new size. Panics rather than
/// returning the old value, because a silent timeout would make a broken toggle
/// look like a passing test.
fn wait_for_change(
    exe: &Path,
    endpoint: &str,
    previous: (u64, u64, u64),
    read: &dyn Fn(&Path, &str) -> (u64, u64, u64),
) -> (u64, u64, u64) {
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    loop {
        let current = read(exe, endpoint);
        if current.2 != previous.2 {
            return current;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "geometry never changed from {previous:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Finds a bare program on `PATH` (Windows also tries `PATHEXT`), so the test
/// can point `-e` at a real image it is able to copy and then remove.
fn resolve_on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let extensions: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".COM;.EXE;.BAT;.CMD".to_owned())
            .split(';')
            .filter(|e| !e.is_empty())
            .map(str::to_owned)
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for extension in &extensions {
            // A program that already names its extension is looked up as-is.
            let candidate = if Path::new(program).extension().is_some() {
                dir.join(program)
            } else {
                dir.join(format!("{program}{extension}"))
            };
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

/// `install-cli` is what makes `minicon` reachable from a shell after a GUI
/// install: a macOS `.app` bundle keeps its executable at
/// `Contents/MacOS/minicon`, deliberately off `PATH`, yet MiniCon is also a
/// control CLI. Cover the whole contract here — link, execute *through* the
/// link, unlink — plus the two refusals. The refusals are the part a careless
/// implementation gets wrong: this command must never be able to replace or
/// delete a real file it does not own (a package manager's binary, a
/// hand-placed copy), only a symlink it created.
#[cfg(unix)]
#[test]
fn install_cli_links_unlinks_and_refuses_to_clobber_real_files() {
    let exe = minicon_binary();
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let unique = UNIQUE.fetch_add(1, Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("minicon-install-cli-{stamp}-{unique}"));
    fs::create_dir_all(&dir).expect("scratch prefix");
    let link = dir.join("minicon");

    let install = Command::new(&exe)
        .args(["install-cli", "--prefix"])
        .arg(&dir)
        .output()
        .expect("install-cli");
    assert!(
        install.status.success(),
        "install-cli failed: {}",
        String::from_utf8_lossy(&install.stderr)
    );
    assert!(
        fs::symlink_metadata(&link)
            .expect("link exists")
            .file_type()
            .is_symlink(),
        "install-cli did not create a symlink"
    );

    // The link must actually run the binary, not merely exist.
    let version = Command::new(&link)
        .arg("--version")
        .output()
        .expect("run through the link");
    assert!(version.status.success());
    assert!(
        String::from_utf8_lossy(&version.stdout).contains("minicon"),
        "running through the link did not report a version"
    );

    let uninstall = Command::new(&exe)
        .args(["uninstall-cli", "--prefix"])
        .arg(&dir)
        .output()
        .expect("uninstall-cli");
    assert!(uninstall.status.success());
    assert!(
        fs::symlink_metadata(&link).is_err(),
        "the link survived uninstall-cli"
    );

    // A real file occupying the name is neither replaced nor deleted.
    fs::write(&link, b"not ours").expect("place a real file");
    let clobber = Command::new(&exe)
        .args(["install-cli", "--prefix"])
        .arg(&dir)
        .output()
        .expect("install-cli over a real file");
    assert!(
        !clobber.status.success(),
        "install-cli replaced a real file"
    );
    let remove = Command::new(&exe)
        .args(["uninstall-cli", "--prefix"])
        .arg(&dir)
        .output()
        .expect("uninstall-cli over a real file");
    assert!(
        !remove.status.success(),
        "uninstall-cli deleted a real file"
    );
    assert_eq!(
        fs::read(&link).expect("real file intact"),
        b"not ours",
        "the real file was modified"
    );

    let _ = fs::remove_dir_all(&dir);
}
