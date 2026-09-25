//! Black-box evidence for the `mux` subcommand (plan items M2, M3, M3b).
//!
//! These drive the shipped `minicon` binary's own CLI against a live instance,
//! exactly as an agent or script would, and assert on real effects: a tab list
//! that came from a running host, a tab that actually switches, keys that
//! actually reach a child process. Nothing here reaches into `src/mux.rs`'s
//! private helpers -- the unit tests in that module already cover translation,
//! and duplicating them here would prove nothing new.
//!
//! Needs a display server, like every other MiniCon GUI suite. On Linux run it
//! the way the CI gate does:
//! `xvfb-run -a -s "-screen 0 1280x900x24" cargo test --test minicon_mux`.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

static UNIQUE: AtomicU64 = AtomicU64::new(1);

/// A spawned host, killed on drop so a failed assertion cannot leak a GUI
/// process into the rest of the suite. Same shape as `minicon_control.rs`'s.
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
        format!(r"pipe:\\.\pipe\minicon-mux-{suffix}")
    } else {
        let base = agenterm_platform::ipc::native_runtime_directory();
        let _ = fs::create_dir_all(&base);
        let dir = base.join(format!("mx-{suffix}"));
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

/// Resolve `minicon` next to the test executable; see the same helper in
/// `tests/minicon_control.rs` for why `CARGO_BIN_EXE_minicon` cannot be used.
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

/// One `minicon cli` invocation: the independent observer for every `mux`
/// effect asserted below. `mux` is a translation layer over exactly this CLI,
/// so reading the result back through `cli` (and never through `mux`'s own
/// stdout) is what makes an assertion evidence rather than an echo.
fn invoke(exe: &Path, endpoint: &str, arguments: &[&str]) -> Output {
    let mut command = Command::new(exe);
    command.args(["cli", "--control", endpoint]);
    command.args(arguments);
    command.output().expect("minicon CLI must start")
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

/// One `minicon mux` invocation, driven exactly as a script would drive it.
fn mux(exe: &Path, endpoint: &str, arguments: &[&str]) -> Output {
    let mut command = Command::new(exe);
    command.args(["mux", "--control", endpoint]);
    command.args(arguments);
    command.output().expect("minicon mux must start")
}

fn mux_ok(exe: &Path, endpoint: &str, arguments: &[&str]) -> String {
    let output = mux(exe, endpoint, arguments);
    assert!(
        output.status.success(),
        "mux {arguments:?} failed: {}",
        error_text(&output)
    );
    output_text(&output)
}

/// A refused `mux` invocation: non-zero exit, nothing on stdout, and the
/// reason on stderr where a script's error handling will see it.
fn mux_refusal(exe: &Path, endpoint: &str, arguments: &[&str]) -> String {
    let output = mux(exe, endpoint, arguments);
    assert!(
        !output.status.success(),
        "mux {arguments:?} was expected to be refused but exited 0: {}",
        error_text(&output)
    );
    assert!(
        output.stdout.is_empty(),
        "a refused mux {arguments:?} must print no result: {}",
        error_text(&output)
    );
    let stderr = String::from_utf8_lossy(&output.stderr).trim().to_owned();
    assert!(
        stderr.starts_with("minicon mux:"),
        "a refusal must be attributed on stderr: {stderr:?}"
    );
    stderr
}

fn tab_ids(exe: &Path, endpoint: &str) -> Vec<String> {
    cli_json(exe, endpoint, &["list-tabs"])["tabs"]
        .as_array()
        .expect("list-tabs must carry a tabs array")
        .iter()
        .map(|tab| {
            tab["id"]
                .as_str()
                .expect("tab ID must be a string")
                .to_owned()
        })
        .collect()
}

/// The active tab as the control CLI itself reports it -- the observation
/// `mux select-window` and `mux kill-window` are checked against.
fn active_tab(exe: &Path, endpoint: &str) -> String {
    cli_json(exe, endpoint, &["list-tabs"])["tabs"]
        .as_array()
        .expect("list-tabs must carry a tabs array")
        .iter()
        .find(|tab| tab["active"] == true)
        .map(|tab| {
            tab["id"]
                .as_str()
                .expect("tab ID must be a string")
                .to_owned()
        })
        .expect("exactly one tab must be active")
}

/// Polls until the control endpoint answers, failing with a diagnosis that
/// distinguishes a dead host from a slow one. Adapted from
/// `minicon_control.rs`'s `wait_until_ready_for`.
fn wait_until_ready_for(exe: &Path, endpoint: &str, timeout: Duration, host: &mut Child) -> Value {
    let deadline = Instant::now() + timeout;
    let mut attempts = 0_u32;
    let mut last_error;
    loop {
        let output = invoke(exe, endpoint, &["list-tabs"]);
        if output.status.success() {
            if let Some(status) = host.try_wait().expect("poll minicon exit") {
                panic!("minicon exited ({status}) before its control endpoint was ready");
            }
            return serde_json::from_str(&output_text(&output))
                .expect("list-tabs output must be JSON");
        }
        attempts += 1;
        last_error = error_text(&output);
        if let Some(status) = host.try_wait().expect("poll minicon exit") {
            panic!(
                "minicon exited ({status}) before its control endpoint was ready.\n\
                 last CLI error: {last_error}"
            );
        }
        assert!(
            Instant::now() < deadline,
            "control endpoint did not become ready after {attempts} attempts over {timeout:?}; \
             on Linux run this suite under `xvfb-run`, a missing display looks exactly like \
             this.\nlast CLI error: {last_error}"
        );
        thread::sleep(Duration::from_millis(25));
    }
}

/// Spawns a host running an interactive shell and waits for its endpoint.
/// Returns the guard, the endpoint, and the root tab's `@ID`.
fn start_host(exe: &Path) -> (OwnedGui, String, String) {
    let suffix = unique_suffix();
    let endpoint = control_endpoint(&suffix);
    let screenshot = if cfg!(windows) {
        std::env::temp_dir().join(format!("minicon-mux-{suffix}.png"))
    } else {
        agenterm_platform::ipc::native_runtime_directory().join(format!("mux-shot-{suffix}.png"))
    };
    let mut host = Command::new(exe);
    host.arg("--no-activate")
        .arg("--control")
        .arg(&endpoint)
        .arg("-e");
    for arg in host_shell_args() {
        host.arg(arg);
    }
    let child = host.spawn().expect("minicon GUI must start");
    let mut gui = OwnedGui { child, screenshot };
    let listed = wait_until_ready_for(exe, &endpoint, Duration::from_secs(30), &mut gui.child);
    let root = listed["tabs"][0]["id"]
        .as_str()
        .expect("root tab ID must be a string")
        .to_owned();
    (gui, endpoint, root)
}

/// Runs one shell line in a tab through the control CLI (never through `mux`),
/// and waits for its output, so a later `mux capture-pane` has known content
/// to find.
fn run_line_in_tab(exe: &Path, endpoint: &str, target: &str, line: &str, expect: &str) {
    cli_json(
        exe,
        endpoint,
        &["send-text", "--target", target, &format!("{line}\r")],
    );
    cli_json(
        exe,
        endpoint,
        &[
            "wait-text",
            "--target",
            target,
            "--timeout-ms",
            "15000",
            expect,
        ],
    );
}

/// Shell input whose *output* differs from the text typed, so a capture that
/// only echoed the command line cannot pass for the child having run it.
fn print_marker_command(marker: &str) -> String {
    if cfg!(windows) {
        format!("for %i in ({marker}) do @echo MUX_OUT_%i")
    } else {
        format!("printf 'MUX_OUT_%s\\n' {marker}")
    }
}

/// M2: `list-windows` renders the live `list-tabs` response -- both the
/// default format and a `-F` substitution -- and M2/M3: a tmux integer index
/// and the same tab's `@ID` resolve to one and the same tab.
#[test]
fn mux_list_windows_renders_live_tabs_and_both_target_spellings_hit_one_tab() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let (_gui, endpoint, root) = start_host(exe);

    // A second tab, opened through the control CLI, so the list has more than
    // one row and the active marker has somewhere wrong to land.
    let second = cli_json(exe, &endpoint, &["new-tab"])["id"]
        .as_str()
        .expect("new-tab must report the new tab's ID")
        .to_owned();
    assert_ne!(second, root);
    assert_eq!(
        active_tab(exe, &endpoint),
        second,
        "a newly opened tab is the active one"
    );

    let lines: Vec<String> = mux_ok(exe, &endpoint, &["list-windows"])
        .lines()
        .map(str::to_owned)
        .collect();
    assert_eq!(lines.len(), 2, "one row per live tab: {lines:?}");
    assert!(
        lines[0].starts_with("0: ") && !lines[0].ends_with('*'),
        "row 0 is the inactive root: {lines:?}"
    );
    assert!(
        lines[1].starts_with("1: ") && lines[1].ends_with('*'),
        "the default format's active marker belongs on the active tab: {lines:?}"
    );

    // `-F` renders from the same live response, including the real `@ID`s.
    let formatted = mux_ok(
        exe,
        &endpoint,
        &[
            "list-windows",
            "-F",
            "#{window_index}|#{window_id}|#{window_active}",
        ],
    );
    assert_eq!(
        formatted,
        format!("0|{root}|\n1|{second}|*"),
        "-F must render the running host's own handles and active flag"
    );

    // And the marker follows the real active tab rather than a fixed row.
    cli_json(exe, &endpoint, &["select-tab", "--target", &root]);
    let formatted = mux_ok(
        exe,
        &endpoint,
        &["list-windows", "-F", "#{window_index}#{window_active}"],
    );
    assert_eq!(
        formatted, "0*\n1",
        "the active marker must move with the live active tab, got {formatted:?}"
    );

    // The compatibility claim: index `1` and the handle `@N` are the same tab.
    // Proven by what each capture sees, not by what mux says it resolved.
    let marker = format!("IDX{}", unique_suffix().replace('-', ""));
    run_line_in_tab(
        exe,
        &endpoint,
        &second,
        &print_marker_command(&marker),
        &format!("MUX_OUT_{marker}"),
    );
    let expected = format!("MUX_OUT_{marker}");
    let by_index = mux_ok(exe, &endpoint, &["capture-pane", "-t", "1", "-p"]);
    let by_handle = mux_ok(exe, &endpoint, &["capture-pane", "-t", &second, "-p"]);
    assert!(
        by_index.contains(&expected),
        "window index 1 must reach the second tab: {by_index:?}"
    );
    assert!(
        by_handle.contains(&expected),
        "handle {second} must reach the second tab: {by_handle:?}"
    );
    let by_root_index = mux_ok(exe, &endpoint, &["capture-pane", "-t", "0", "-p"]);
    assert!(
        !by_root_index.contains(&expected),
        "index 0 must be a different tab, not the one index 1 named: {by_root_index:?}"
    );
}

/// M3: `select-window`, `new-window` and `kill-window` change the real
/// workspace -- including tmux's default of killing the *active* window when
/// no `-t` is given. Every effect is read back through the control CLI.
#[test]
fn mux_window_verbs_switch_open_and_close_real_tabs() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let (_gui, endpoint, root) = start_host(exe);
    assert_eq!(tab_ids(exe, &endpoint), vec![root.clone()]);

    // new-window actually opens a tab.
    mux_ok(exe, &endpoint, &["new-window"]);
    let ids = tab_ids(exe, &endpoint);
    assert_eq!(ids.len(), 2, "new-window must open a tab: {ids:?}");
    let second = ids[1].clone();
    assert_eq!(active_tab(exe, &endpoint), second);

    // select-window switches the focused tab, by index and by handle.
    mux_ok(exe, &endpoint, &["select-window", "-t", "0"]);
    assert_eq!(
        active_tab(exe, &endpoint),
        root,
        "select-window -t 0 must focus the first tab"
    );
    mux_ok(exe, &endpoint, &["select-window", "-t", &second]);
    assert_eq!(
        active_tab(exe, &endpoint),
        second,
        "select-window -t {second} must focus that tab"
    );

    // kill-window -t closes exactly the named tab.
    mux_ok(exe, &endpoint, &["kill-window", "-t", &second]);
    let ids = tab_ids(exe, &endpoint);
    assert_eq!(
        ids,
        vec![root.clone()],
        "kill-window -t must close that one tab: {ids:?}"
    );

    // tmux's default: no `-t` kills the *active* window. The active tab is
    // deliberately not the first one, so targeting the wrong tab fails here.
    mux_ok(exe, &endpoint, &["new-window"]);
    let ids = tab_ids(exe, &endpoint);
    assert_eq!(ids.len(), 2, "{ids:?}");
    let third = ids[1].clone();
    assert_eq!(active_tab(exe, &endpoint), third);
    mux_ok(exe, &endpoint, &["kill-window"]);
    let ids = tab_ids(exe, &endpoint);
    assert_eq!(
        ids,
        vec![root.clone()],
        "kill-window with no -t must close the active tab, not another one: {ids:?}"
    );
}

/// M3b: `send-keys` reaches a real child process -- a named key and `-l`
/// literal text both -- and `capture-pane -p` returns what that child actually
/// printed.
#[test]
fn mux_send_keys_reach_the_child_and_capture_pane_returns_its_output() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let (_gui, endpoint, root) = start_host(exe);

    let marker = format!("KEY{}", unique_suffix().replace('-', ""));
    let expected = format!("MUX_OUT_{marker}");
    let command = print_marker_command(&marker);
    // `-l` types the line (and nothing runs yet) ...
    mux_ok(exe, &endpoint, &["send-keys", "-t", "0", "-l", &command]);
    // ... and the named key `Enter` is what makes the child run it. The
    // expected text is absent from the typed line, so it can only come from
    // the child having executed the command.
    assert!(
        !command.contains(&expected),
        "the marker must not be typeable: {command:?}"
    );
    mux_ok(exe, &endpoint, &["send-keys", "-t", "0", "Enter"]);
    cli_json(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &root,
            "--timeout-ms",
            "15000",
            &expected,
        ],
    );

    let captured = mux_ok(exe, &endpoint, &["capture-pane", "-t", "0", "-p"]);
    assert!(
        captured.contains(&expected),
        "capture-pane -p must return what the child printed: {captured:?}"
    );
    assert!(
        captured.contains(&command),
        "the literal text sent with -l must appear on the child's echoed line: {captured:?}"
    );
}

/// Bounded refusals as real CLI behaviour: non-zero exit, the reason on
/// stderr, and -- just as important -- no tab opened or closed on the way out.
#[test]
fn mux_bounded_refusals_exit_nonzero_and_leave_the_workspace_untouched() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let (_gui, endpoint, root) = start_host(exe);

    // Make a genuinely stale handle: open a tab, then close it.
    mux_ok(exe, &endpoint, &["new-window"]);
    let stale = tab_ids(exe, &endpoint)[1].clone();
    mux_ok(exe, &endpoint, &["kill-window", "-t", &stale]);
    let before = tab_ids(exe, &endpoint);
    assert_eq!(before, vec![root.clone()]);
    let active_before = active_tab(exe, &endpoint);

    // A non-zero pane index: tmux's split panes are the assumption MiniCon
    // does not implement.
    let error = mux_refusal(exe, &endpoint, &["capture-pane", "-t", "0.1", "-p"]);
    assert!(error.contains("split-window"), "{error}");
    assert!(error.contains("one pane"), "{error}");

    // A foreign session name: tmux's several-sessions-per-server model.
    let error = mux_refusal(exe, &endpoint, &["select-window", "-t", "other:0"]);
    assert!(error.contains("no session named"), "{error}");
    assert!(error.contains("one session"), "{error}");

    // A stale `@ID` is passed through to the server that owns the tab list, so
    // the refusal is the server's own and names the handle rather than a tmux
    // assumption. Asserted as it actually behaves.
    let error = mux_refusal(exe, &endpoint, &["select-window", "-t", &stale]);
    assert!(error.contains(&stale), "{error}");
    assert!(error.contains("does not exist"), "{error}");

    // An out-of-range tmux index is where the index-vs-handle assumption is
    // named, because renumbering is exactly what tmux does and MiniCon does
    // not.
    let error = mux_refusal(exe, &endpoint, &["select-window", "-t", "7"]);
    assert!(error.contains("out of range"), "{error}");
    assert!(error.contains("renumber"), "{error}");

    // `capture-pane -S`: refused on carried-debt C1 rather than answered from
    // the visible screen.
    let error = mux_refusal(exe, &endpoint, &["capture-pane", "-S", "-100"]);
    assert!(error.contains("C1"), "{error}");
    assert!(error.contains("scrollback"), "{error}");

    // No refusal above may have opened or closed anything, or moved focus.
    assert_eq!(
        tab_ids(exe, &endpoint),
        before,
        "a refused mux verb must have no side effect on the tab list"
    );
    assert_eq!(
        active_tab(exe, &endpoint),
        active_before,
        "a refused mux verb must not move the focus"
    );
}
