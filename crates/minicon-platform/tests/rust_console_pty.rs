#![cfg(all(windows, feature = "rust-console"))]
//! Exercise the opened PTY facade, rather than manually servicing ConDrv.
use minicon_platform::pty::{
    ChildCommand, NativeInputOwnership, NativeTerminalKey, PtyChild, PtyMaster, TerminalSize,
};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};

struct Fixture {
    master: PtyMaster,
    child: PtyChild,
    output: Receiver<std::io::Result<Vec<u8>>>,
    bytes: Vec<u8>,
}

impl Fixture {
    fn spawn(script: &str, configure: impl FnOnce(ChildCommand) -> ChildCommand) -> Self {
        let image = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("cmd.exe");
        Self::open(configure(
            ChildCommand::new(image)
                .arg("/d")
                .arg("/c")
                .arg(script)
                .rust_console(),
        ))
    }

    fn open(command: ChildCommand) -> Self {
        let (mut master, child) = command.spawn().expect("open owned Rust PTY").into_parts();
        assert_eq!(master.backend_kind(), "rust-console");
        let reader = master.try_clone_for_startup_reader().unwrap();
        let (send, output) = mpsc::channel();
        std::thread::spawn(move || {
            let mut buffer = [0; 4096];
            loop {
                match reader.io().read(&mut buffer) {
                    Ok(0) => break,
                    Ok(count) => {
                        if send.send(Ok(buffer[..count].to_vec())).is_err() {
                            break;
                        }
                    }
                    Err(error) => {
                        let _ = send.send(Err(error));
                        break;
                    }
                }
            }
        });
        Self {
            master,
            child,
            output,
            bytes: Vec::new(),
        }
    }

    fn until(&mut self, text: &str) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !String::from_utf8_lossy(&self.bytes).contains(text) {
            let chunk = self
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|error| {
                    panic!(
                        "missing {text:?}: {error}; output={:?}",
                        String::from_utf8_lossy(&self.bytes)
                    )
                });
            self.bytes
                .extend(chunk.expect("owned PTY reader must not fail"));
        }
    }

    fn finish(&mut self, code: i32) {
        let mut waiter = self.child.try_clone_for_wait().unwrap();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(waiter.wait());
        });
        assert_eq!(
            receive
                .recv_timeout(Duration::from_secs(8))
                .unwrap()
                .unwrap()
                .code(),
            Some(code)
        );
        loop {
            match self.output.recv_timeout(Duration::from_secs(2)) {
                Ok(chunk) => self
                    .bytes
                    .extend(chunk.expect("owned PTY reader must not fail")),
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    panic!("owned PTY reader did not reach EOF")
                }
            }
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = self.child.terminate_forcefully();
        self.child.close_pseudoconsole();
    }
}

struct Report(PathBuf);
impl Report {
    fn new() -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/conhost-research");
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join(format!(
            "platform-pty-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&path).unwrap();
        Self(path.join("result.txt"))
    }
}
impl Drop for Report {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.0);
        let _ = std::fs::remove_dir(self.0.parent().unwrap());
    }
}

#[test]
fn rust_console_pty_facade_reports_real_initial_and_resized_console_dimensions() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("resize")
            .arg(report.0.as_os_str())
            .size(TerminalSize {
                rows: 37,
                cols: 109,
            })
            .rust_console(),
    );
    fixture.until("native-pty-probe-ready:");
    fixture
        .master
        .resize(TerminalSize {
            rows: 41,
            cols: 133,
        })
        .unwrap();
    fixture.finish(0);
    assert_eq!(
        std::fs::read_to_string(&report.0).unwrap(),
        "109,37,133,41,133,41"
    );
}

#[test]
fn rust_console_pty_facade_delivers_native_key_identity_without_console_attach() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("key")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.until("native-pty-probe-ready:");
    assert_eq!(
        fixture.child.native_input_ownership().unwrap(),
        NativeInputOwnership::RawNative
    );
    fixture
        .child
        .send_native_key(NativeTerminalKey::Up, 3)
        .unwrap();
    fixture.finish(0);
    assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "1,3,38,72,256");
}

#[test]
fn rust_console_pty_facade_job_cleanup_stops_a_nonreading_client() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("sleep")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.until("native-pty-probe-ready:");
    let receipt = fixture.child.terminate_forcefully().unwrap();
    assert_eq!(receipt.members_observed, 1);
    assert!(receipt.verified_empty);
    fixture.finish(1);
}

#[test]
fn rust_console_pty_facade_preserves_environment_cwd_and_exit_status() {
    let directory = std::env::temp_dir();
    let mut fixture = Fixture::spawn("set MINICON_RUST_PTY_VALUE&cd&exit /b 23", |command| {
        command
            .env("MINICON_RUST_PTY_VALUE", "native-owned-value")
            .current_dir(&directory)
    });
    fixture.finish(23);
    let text = String::from_utf8_lossy(&fixture.bytes);
    assert!(
        text.contains("MINICON_RUST_PTY_VALUE=native-owned-value"),
        "{text}"
    );
    assert!(
        text.to_lowercase().contains(
            &directory
                .to_string_lossy()
                .trim_end_matches(['\\', '/'])
                .to_lowercase()
        ),
        "{text}"
    );
}

#[test]
fn rust_console_pty_facade_keeps_utf8_and_crlf_across_stream_fragments() {
    let mut fixture = Fixture::spawn(
        "set /p MINICON_RUST_PTY_INPUT=first:&set MINICON_RUST_PTY_INPUT&set /p MINICON_RUST_PTY_SECOND=second:&set MINICON_RUST_PTY_SECOND",
        |command| command,
    );
    fixture.until("first:");
    assert_eq!(
        fixture.child.native_input_ownership().unwrap(),
        NativeInputOwnership::Cooked
    );
    let bytes = "中😀".as_bytes();
    for byte in bytes {
        fixture.master.write_all(&[*byte]).unwrap();
        // Two acknowledged worker laps force a pipe poll between fragments;
        // rapid writes alone could coalesce into one native read.
        for _ in 0..2 {
            fixture
                .master
                .resize(TerminalSize { rows: 24, cols: 80 })
                .unwrap();
        }
    }
    fixture.master.write_all(b"\r").unwrap();
    fixture.until("second:");
    fixture.master.write_all(b"\nkept\r").unwrap();
    fixture.finish(0);
    let text = String::from_utf8_lossy(&fixture.bytes);
    assert!(text.contains("MINICON_RUST_PTY_INPUT=中😀"), "{text}");
    assert!(text.contains("MINICON_RUST_PTY_SECOND=kept"), "{text}");
}

#[test]
fn rust_console_pty_facade_terminates_a_blocked_reader_with_verified_empty_job() {
    let mut fixture = Fixture::spawn("set /p MINICON_RUST_PTY_WAIT=blocked:", |command| command);
    fixture.until("blocked:");
    let started = Instant::now();
    let receipt = fixture.child.terminate_forcefully().unwrap();
    assert_eq!(receipt.members_observed, 1);
    assert!(receipt.verified_empty);
    assert!(started.elapsed() < Duration::from_secs(3));
    fixture.finish(1);
}

#[test]
fn rust_console_pty_facade_rejects_invalid_screen_size_before_launch() {
    assert!(
        ChildCommand::new("cmd.exe")
            .rust_console()
            .size(TerminalSize {
                rows: 32767,
                cols: 32767
            })
            .spawn()
            .is_err()
    );
}
