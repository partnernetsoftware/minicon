use std::path::{Path, PathBuf};
use std::process::{Child, Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use serde_json::Value;

const OUTPUT_ITERATIONS: usize = 8192;
const OUTPUT_CHUNK_BYTES: usize = 16 * 255 + 2;
const OUTPUT_BYTES: u64 = (OUTPUT_ITERATIONS * OUTPUT_CHUNK_BYTES) as u64;
const MIN_BYTES_PER_SECOND: u64 = 2 * 1024 * 1024;
const OUTPUT_DEADLINE: Duration = Duration::from_secs(30);
const SIBLING_DEADLINE: Duration = Duration::from_secs(5);

static UNIQUE: AtomicU64 = AtomicU64::new(1);

struct OwnedGui(Child);

impl Drop for OwnedGui {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
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
        format!(r"pipe:\\.\pipe\minicon-throughput-{suffix}")
    } else {
        let base = agenterm_platform::ipc::native_runtime_directory();
        let _ = std::fs::create_dir_all(&base);
        let dir = base.join(format!("tp-{suffix}"));
        let _ = std::fs::create_dir_all(&dir);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt as _;
            let _ = std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700));
        }
        let path = dir.join("c.sock");
        assert!(path.to_string_lossy().len() <= 103);
        format!("unix:{}", path.to_string_lossy())
    }
}

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
    assert!(path.is_file(), "minicon is missing at {}", path.display());
    path
}

fn invoke(exe: &Path, endpoint: &str, arguments: &[&str]) -> Output {
    Command::new(exe)
        .args(["cli", "--control", endpoint])
        .args(arguments)
        .output()
        .expect("minicon CLI must start")
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
        "CLI {arguments:?} failed: {}",
        error_text(&output)
    );
    serde_json::from_slice(&output.stdout).expect("successful CLI output must be JSON")
}

fn wait_until_ready(exe: &Path, endpoint: &str, timeout: Duration) -> Value {
    let deadline = Instant::now() + timeout;
    loop {
        let output = invoke(exe, endpoint, &["list-tabs"]);
        if output.status.success() {
            return serde_json::from_slice(&output.stdout).expect("list-tabs output must be JSON");
        }
        assert!(
            Instant::now() < deadline,
            "control endpoint did not become ready: {}",
            error_text(&output)
        );
        thread::sleep(Duration::from_millis(25));
    }
}

fn shell_marker_command(prefix: &str, suffix: &str) -> String {
    if cfg!(windows) {
        format!("[Console]::Out.WriteLine(('{prefix}'+'{suffix}'))\r")
    } else {
        format!("printf '{prefix}%s\\n' '{suffix}'\r")
    }
}

fn wait_for_shell(
    exe: &Path,
    endpoint: &str,
    tab: &str,
    prefix: &str,
    suffix: &str,
    timeout: Duration,
) {
    let marker = format!("{prefix}{suffix}");
    let command = shell_marker_command(prefix, suffix);
    cli_json(exe, endpoint, &["send-text", "--target", tab, &command]);
    let timeout_ms = timeout.as_millis().to_string();
    let waited = invoke(
        exe,
        endpoint,
        &[
            "wait-text",
            "--target",
            tab,
            "--timeout-ms",
            &timeout_ms,
            &marker,
        ],
    );
    assert!(
        waited.status.success(),
        "shell did not execute readiness marker {marker}: {}",
        error_text(&waited)
    );
}

fn tab_id(value: &Value) -> &str {
    value.as_str().expect("tab ID must be a string")
}

#[test]
#[ignore = "owned by the dedicated sustained-output qualification gate"]
fn sustained_long_output_keeps_control_and_sibling_responsive() {
    let exe = minicon_binary();
    let exe = exe.as_path();
    let endpoint = control_endpoint(&unique_suffix());
    let mut host = Command::new(exe);
    host.arg("--no-activate").arg("--control").arg(&endpoint);
    if cfg!(windows) {
        host.args(["-e", "powershell.exe", "-NoLogo", "-NoProfile", "-NoExit"]);
    } else {
        host.args(["-e", "/bin/bash", "--norc", "--noprofile"]);
    }
    let child = host.spawn().expect("minicon GUI must start");
    let mut gui = OwnedGui(child);

    let listed = wait_until_ready(exe, &endpoint, Duration::from_secs(60));
    let producer = tab_id(&listed["tabs"][0]["id"]).to_owned();
    // The control endpoint becoming ready proves only that the GUI can accept
    // commands; it does not prove that the child shell has finished startup.
    // Inject the marker through the public interface after control readiness
    // on every host, so buffered terminal input becomes the shell-readiness
    // rendezvous instead of racing output from the process launch arguments.
    wait_for_shell(
        exe,
        &endpoint,
        &producer,
        "THROUGHPUT_",
        "READY",
        Duration::from_secs(60),
    );
    let created = cli_json(exe, &endpoint, &["new-tab", "--parent", &producer]);
    let sibling = tab_id(&created["id"]).to_owned();
    // A tab record exists before its newly spawned shell necessarily begins
    // reading input. Prove that startup separately so the five-second sibling
    // criterion below measures responsiveness during output, not first-process
    // launch or antivirus scanning of a new cross-built PE.
    wait_for_shell(
        exe,
        &endpoint,
        &sibling,
        "SIBLING_",
        "READY",
        Duration::from_secs(60),
    );
    // Separate cold process/font/renderer/antivirus startup from the sustained
    // throughput interval. This bounded payload must fully drain before perf
    // counters and the 32 MiB clock are reset below.
    let warmup = if cfg!(windows) {
        "$w=[Text.Encoding]::ASCII.GetBytes((('W'*4096)+\"`r`n\")*256);\
         $o=[Console]::OpenStandardOutput();$o.Write($w,0,$w.Length);\
         [Console]::Out.WriteLine(('WARMUP_'+'DONE'))\r"
            .to_owned()
    } else {
        "yes W | head -c 1048576; printf '%s%s\\n' WARMUP_ DONE\r".to_owned()
    };
    cli_json(
        exe,
        &endpoint,
        &["send-text", "--target", &producer, &warmup],
    );
    cli_json(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &producer,
            "--timeout-ms",
            "60000",
            "WARMUP_DONE",
        ],
    );
    cli_json(exe, &endpoint, &["reset-perf-stats"]);

    let command = if cfg!(windows) {
        let prepare = format!(
            "$b=[Text.Encoding]::ASCII.GetBytes(((('0123456789ABCDEF'*255)+\"`r`n\")*{OUTPUT_ITERATIONS}));\
             [Console]::Out.WriteLine(('PAYLOAD_'+'READY'))\r"
        );
        cli_json(
            exe,
            &endpoint,
            &["send-text", "--target", &producer, &prepare],
        );
        cli_json(
            exe,
            &endpoint,
            &[
                "wait-text",
                "--target",
                &producer,
                "--timeout-ms",
                "60000",
                "PAYLOAD_READY",
            ],
        );
        "$o=[Console]::OpenStandardOutput();$o.Write($b,0,$b.Length);\
         [Console]::Out.WriteLine(('THROUGHPUT_'+'DONE_32M'))\r"
            .to_owned()
    } else {
        // Keep a clean macOS runtime target independent of Xcode/Python. The
        // fixed byte count remains identical to the Windows payload; the test
        // measures PTY draining rather than generator implementation.
        format!(
            "yes 0123456789ABCDEF | head -c {OUTPUT_BYTES}; printf '%s%s\\n' THROUGHPUT_DONE_ 32M\r"
        )
    };
    let started = Instant::now();
    cli_json(
        exe,
        &endpoint,
        &["send-text", "--target", &producer, &command],
    );

    let sibling_started = Instant::now();
    let sibling_marker = shell_marker_command("SIBLING_", "RESPONSIVE");
    cli_json(
        exe,
        &endpoint,
        &["send-text", "--target", &sibling, &sibling_marker],
    );
    cli_json(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &sibling,
            "--timeout-ms",
            "5000",
            "SIBLING_RESPONSIVE",
        ],
    );
    assert!(
        sibling_started.elapsed() <= SIBLING_DEADLINE,
        "sibling response exceeded {SIBLING_DEADLINE:?} during sustained output"
    );

    for _ in 0..8 {
        let control_started = Instant::now();
        cli_json(exe, &endpoint, &["list-tabs"]);
        cli_json(exe, &endpoint, &["perf-stats"]);
        assert!(
            control_started.elapsed() <= Duration::from_secs(2),
            "control observation stalled during sustained output"
        );
    }
    cli_json(
        exe,
        &endpoint,
        &[
            "wait-text",
            "--target",
            &producer,
            "--timeout-ms",
            "60000",
            "THROUGHPUT_DONE_32M",
        ],
    );
    let elapsed = started.elapsed();
    assert!(
        elapsed <= OUTPUT_DEADLINE,
        "{OUTPUT_BYTES} output bytes exceeded {OUTPUT_DEADLINE:?}: {elapsed:?}"
    );
    let bytes_per_second = OUTPUT_BYTES.saturating_mul(1_000_000_000)
        / elapsed.as_nanos().max(1).min(u128::from(u64::MAX)) as u64;
    assert!(
        bytes_per_second >= MIN_BYTES_PER_SECOND,
        "sustained rate {bytes_per_second} B/s is below {MIN_BYTES_PER_SECOND} B/s"
    );

    let perf = cli_json(exe, &endpoint, &["perf-stats"]);
    assert!(
        perf["pty_drained_bytes"]
            .as_u64()
            .is_some_and(|bytes| bytes >= OUTPUT_BYTES),
        "PTY receipt did not cover the fixed payload: {perf}"
    );
    // The native pipe may deliver chunks below the per-Wake budget even for a
    // large payload, in which case zero yields is the correct result rather
    // than evidence that scheduling was bypassed. The deterministic
    // `wake_budget_is_shared_without_multiplying_by_tab_count` unit test owns
    // forced backlog/yield behavior; this public journey owns real throughput,
    // complete draining, sibling responsiveness and the published counter.
    let pty_budget_yields = perf["pty_budget_yields"]
        .as_u64()
        .expect("PTY budget yield receipt must remain numeric");
    assert_eq!(perf["present_failure"], 0);
    // Windows native retained path should avoid host-copy frames; portable
    // macOS/Linux may copy. Only require the counter to be numeric/non-negative.
    assert!(
        perf["host_copy_frames"]
            .as_u64()
            .is_some_and(|frames| if cfg!(windows) { frames == 0 } else { true }),
        "host_copy_frames receipt invalid: {perf}"
    );

    eprintln!(
        "AGENTERM_CON_EVIDENCE minicon_throughput::sustained_long_output_keeps_control_and_sibling_responsive {}",
        serde_json::json!({
            "bytes": OUTPUT_BYTES,
            "elapsed_ms": elapsed.as_millis(),
            "bytes_per_second": bytes_per_second,
            "pty_drained_bytes": perf["pty_drained_bytes"],
            "pty_budget_yields": pty_budget_yields,
            "present_failure": perf["present_failure"],
            "host_copy_frames": perf["host_copy_frames"]
        })
    );

    cli_json(exe, &endpoint, &["close-window"]);
    let close_deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Some(status) = gui.0.try_wait().expect("poll closed GUI") {
            assert!(status.success(), "close-window failed with {status:?}");
            break;
        }
        assert!(
            Instant::now() < close_deadline,
            "close-window did not release the sustained-output host"
        );
        thread::sleep(Duration::from_millis(20));
    }
}
