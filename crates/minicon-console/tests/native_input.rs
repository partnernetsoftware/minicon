#![cfg(windows)]
//! Independent Win32 client checks, built by Cargo with its exact binary path.
use minicon_console::{ConsoleHost, ConsoleInput, DriverSession};
use serde_json::{Value, json};
use std::ffi::OsStr;
use std::os::windows::io::{AsHandle, AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::WAIT_OBJECT_0;
use windows_sys::Win32::System::Threading::{
    OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_TERMINATE, TerminateProcess,
    WaitForSingleObject,
};

static NEXT: AtomicU64 = AtomicU64::new(1);

#[test]
fn rust_console_initial_title_respects_output_admission_and_failed_sink() {
    struct Reject;
    impl std::io::Write for Reject {
        fn write(&mut self, _: &[u8]) -> std::io::Result<usize> {
            Err(std::io::ErrorKind::BrokenPipe.into())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }
    for fail in [false, true] {
        let session = DriverSession::open().unwrap();
        let foreign_session = DriverSession::open().unwrap();
        let image = PathBuf::from(std::env::var_os("SystemRoot").unwrap())
            .join("System32")
            .join("cmd.exe");
        let command = format!("\"{}\" /d /c exit /b 0", image.display());
        let child = session
            .spawn_suspended(&image, OsStr::new(&command))
            .unwrap();
        let held = child.process_handle().try_clone_to_owned().unwrap();
        let mut host = ConsoleHost::for_child(&child).unwrap();
        let mut output = Vec::new();
        child.resume().unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            assert!(Instant::now() < deadline, "initial CONNECT missing");
            match session.receive(Duration::from_millis(20)) {
                Ok(request) => {
                    let function = request.descriptor.function;
                    let call = host
                        .dispatch_with_output_ready(request, false, &mut output)
                        .unwrap();
                    assert_eq!(call.status, 0);
                    if function == 1 {
                        assert!(call.pending);
                        assert_eq!(host.pending_output_requests(), 1);
                        assert_eq!(host.connected_clients(), 0);
                        assert!(output.is_empty());
                        break;
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                Err(error) => panic!("initial CONNECT: {error}"),
            }
        }
        let foreign = foreign_session
            .spawn_suspended(&image, OsStr::new(&command))
            .unwrap();
        foreign.resume().unwrap();
        let request = foreign_session.receive(Duration::from_secs(3)).unwrap();
        assert_eq!(request.descriptor.function, 1);
        let rejected = host
            .dispatch_with_output_ready(request, false, &mut output)
            .unwrap();
        assert_eq!(rejected.status, 0xc000_0008_u32 as i32);
        assert!(!rejected.pending);
        assert_eq!(host.pending_output_requests(), 1);
        assert!(output.is_empty());
        assert!(foreign.wait(Duration::from_secs(3)).unwrap().is_some());
        drop(foreign);
        if fail {
            assert_eq!(
                host.resume_output(&mut Reject).unwrap_err().kind(),
                std::io::ErrorKind::BrokenPipe
            );
            assert_eq!(host.connected_clients(), 0);
            assert_eq!(host.open_handles(), 0);
            assert_eq!(host.control_targets(), 0);
            drop(host);
            drop(child);
            assert_eq!(
                unsafe { WaitForSingleObject(held.as_raw_handle(), 3000) },
                WAIT_OBJECT_0
            );
        } else {
            let call = host.resume_output(&mut output).unwrap().unwrap();
            assert_eq!((call.function, call.status, call.pending), (1, 0, false));
            assert_eq!(host.connected_clients(), 1);
            assert_eq!(host.pending_output_requests(), 0);
            assert!(output.starts_with(b"\x1b]0;"));
            assert!(output.ends_with(b"\x1b\\"));
            while child.wait(Duration::ZERO).unwrap().is_none() || host.connected_clients() != 0 {
                assert!(Instant::now() < deadline, "startup client did not retire");
                match session.receive(Duration::from_millis(20)) {
                    Ok(request) => {
                        assert_eq!(host.dispatch(request, &mut output).unwrap().status, 0)
                    }
                    Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
                    Err(error) => panic!("startup retirement: {error}"),
                }
            }
            assert_eq!(
                child.wait(Duration::ZERO).unwrap(),
                Some(0),
                "{}",
                String::from_utf8_lossy(&output)
            );
        }
    }
}

struct Report {
    path: PathBuf,
}
impl Report {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/conhost-research");
        std::fs::create_dir_all(&root).unwrap();
        let dir = root.join(format!(
            "native-input-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        // Exclusive creation never reuses another test's files or a stale run.
        std::fs::create_dir(&dir).unwrap();
        Self {
            path: dir.join("result.json"),
        }
    }
}
impl Drop for Report {
    fn drop(&mut self) {
        // Only explicitly owned report/coordination files; no recursive deletion.
        let _ = std::fs::remove_file(&self.path);
        let _ = std::fs::remove_file(self.path.with_extension("ready"));
        let _ = std::fs::remove_file(self.path.with_extension("go"));
        let _ = std::fs::remove_file(self.path.with_extension("peek"));
        let _ = std::fs::remove_dir(self.path.parent().unwrap());
    }
}

fn run_probe(mode: &str, input: &[ConsoleInput], text: Option<&str>, cancel: bool) -> Value {
    let report = Report::new();
    let session = DriverSession::open().unwrap();
    let image = PathBuf::from(env!("CARGO_BIN_EXE_minicon-console-probe"));
    let command = format!(
        "\"{}\" {mode} \"{}\"",
        image.display(),
        report.path.display()
    );
    let child = session
        .spawn_suspended(&image, OsStr::new(&command))
        .unwrap();
    assert_eq!(
        child.active_processes().unwrap(),
        1,
        "job ownership must precede client execution"
    );
    let mut host = ConsoleHost::new();
    let mut output = Vec::new();
    let mut supplied = false;
    let mut cancelled = false;
    let mut saw_wait = false;
    child.resume().unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        assert!(
            Instant::now() < deadline,
            "native {mode} probe did not finish"
        );
        if child.wait(Duration::ZERO).unwrap().is_some() && host.connected_clients() == 0 {
            break;
        }
        match session.receive(Duration::from_millis(20)) {
            Ok(request) => {
                let call = host.dispatch(request, &mut output).unwrap();
                assert_eq!(call.status, 0, "probe used unsupported API: {call:?}");
                if call.pending {
                    saw_wait = true;
                    if cancel {
                        assert!(!cancelled);
                        cancelled = true;
                        assert_eq!(child.wait(Duration::ZERO).unwrap(), None);
                        assert_eq!(host.cancel_pending_reads().unwrap(), 1);
                    } else if !supplied && !matches!(mode, "wait-records" | "peek-existing") {
                        supplied = true;
                        let completed = if let Some(text) = text {
                            host.enqueue_text(text, &mut output).unwrap()
                        } else {
                            host.enqueue_input(input, &mut output).unwrap()
                        };
                        assert_eq!(completed, 1);
                    } else {
                        panic!(
                            "native {mode} requested more input than its complete fixture: {call:?}"
                        );
                    }
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => panic!("native {mode} receive: {error}"),
        }
        if matches!(mode, "wait-records" | "peek-existing")
            && !supplied
            && report.path.with_extension("ready").exists()
        {
            supplied = true;
            host.enqueue_input(input, &mut output).unwrap();
        }
    }
    assert!(
        saw_wait || matches!(mode, "wait-records" | "peek-existing"),
        "probe must exercise a genuinely deferred read"
    );
    assert_eq!(
        child.wait(Duration::ZERO).unwrap(),
        Some(0),
        "probe failed before reporting native API results"
    );
    assert_eq!(host.pending_reads(), 0);
    let result: Value = serde_json::from_slice(&std::fs::read(&report.path).unwrap()).unwrap();
    eprintln!("native {mode}: {result}");
    result
}

#[test]
fn rust_console_native_readconsole_receives_utf16_line_and_count() {
    let result = run_probe("line", &[], Some("中😀\r"), false);
    assert_eq!(result["ok"], true);
    assert_eq!(result["error"], 0);
    let expected: Vec<u16> = "中😀\r\n".encode_utf16().collect();
    assert_eq!(result["count"], expected.len());
    assert_eq!(result["units"], json!(expected));
}

#[test]
fn rust_console_native_readconsole_preserves_caller_supplied_initial_characters() {
    let result = run_probe("initial-line", &[], Some("x\r"), false);
    assert_eq!(result["ok"], true);
    assert_eq!(
        result["units"],
        json!("pre-x\r\n".encode_utf16().collect::<Vec<_>>())
    );
}

#[test]
fn rust_console_native_readconsole_wakes_on_requested_control_character() {
    let input: Vec<_> = [(97, 0), (98, 0), (9, 16)]
        .into_iter()
        .map(|(character, controls)| ConsoleInput::Key {
            down: true,
            repeat: 1,
            virtual_key: 0,
            scan_code: 0,
            character,
            controls,
        })
        .collect();
    let result = run_probe("wakeup", &input, None, false);
    assert_eq!(result["ok"], true);
    assert_eq!(result["units"], json!([97, 98, 9]));
    assert_eq!(result["controls"], 16);
}

#[test]
fn rust_console_native_interrupted_read_reports_operation_aborted_not_empty_success() {
    let result = run_probe("cancel", &[], None, true);
    // Console interruption is the documented exception: BOOL succeeds while
    // LastError reports ERROR_OPERATION_ABORTED. Plain empty success leaves 0.
    assert_eq!(
        result["ok"], true,
        "preserve the Windows console interruption result"
    );
    assert_eq!(
        result["error"], 995,
        "the real Win32 client must see ERROR_OPERATION_ABORTED"
    );
    assert_eq!(result["count"], 0);
    assert_eq!(result["units"], json!([]));
}

fn record_fixture() -> Vec<ConsoleInput> {
    vec![
        ConsoleInput::Key {
            down: true,
            repeat: 2,
            virtual_key: 65,
            scan_code: 30,
            character: 0x4e2d,
            controls: 8,
        },
        ConsoleInput::Mouse {
            x: 7,
            y: 4,
            buttons: 1,
            controls: 16,
            flags: 1,
        },
        ConsoleInput::Resize {
            columns: 120,
            rows: 40,
        },
        ConsoleInput::Focus(false),
    ]
}

fn assert_records(result: &Value) {
    assert_eq!(result["ok"], true);
    assert_eq!(
        result["records"],
        json!([
            [1, 1, 2, 65, 30, 0x4e2d, 8],
            [2, 7, 4, 1, 16, 1],
            [4, 120, 40],
            [16, 0]
        ])
    );
}

#[test]
fn rust_console_native_input_records_preserve_key_mouse_resize_and_focus() {
    assert_records(&run_probe("records", &record_fixture(), None, false));
}

#[test]
fn rust_console_native_peek_returns_zero_without_consuming_future_records() {
    let result = run_probe("peek-records", &record_fixture(), None, false);
    assert_eq!(
        result["peek_count"], 0,
        "empty PeekConsoleInput must return immediately"
    );
    assert_records(&result);
}

#[test]
fn rust_console_native_peek_leaves_existing_records_for_the_real_read() {
    let result = run_probe("peek-existing", &record_fixture(), None, false);
    assert_eq!(result["peek_count"], 4);
    assert_records(&result);
}

#[test]
fn rust_console_native_input_handle_wait_wakes_when_events_arrive() {
    let result = run_probe("wait-records", &record_fixture(), None, false);
    assert_eq!(
        result["wait"], 0,
        "actual WaitForSingleObject(stdin) must wake on input"
    );
    assert_records(&result);
}

#[test]
fn rust_console_native_readfile_preserves_utf8_across_one_byte_reads() {
    let result = run_probe("raw-bytes", &[], Some("中"), false);
    assert_eq!(result["ok"], true);
    assert_eq!(result["bytes"], json!("中".as_bytes()));
}

#[test]
fn rust_console_native_readfile_preserves_surrogate_pair_across_one_byte_reads() {
    let result = run_probe("raw-emoji", &[], Some("😀"), false);
    assert_eq!(result["ok"], true);
    assert_eq!(result["bytes"], json!("😀".as_bytes()));
}

struct OwnedFixtureProcess(OwnedHandle);
impl Drop for OwnedFixtureProcess {
    fn drop(&mut self) {
        // All instances are created from our client or a positively verified
        // member of its private job. Failure cleanup never targets a bare PID.
        unsafe {
            TerminateProcess(self.0.as_raw_handle(), 0xfe);
            WaitForSingleObject(self.0.as_raw_handle(), 2000);
        }
    }
}

fn launch_probe(
    mode: &str,
    report: &Report,
    session: &DriverSession,
) -> minicon_console::ConsoleChild {
    let image = PathBuf::from(env!("CARGO_BIN_EXE_minicon-console-probe"));
    let command = format!(
        "\"{}\" {mode} \"{}\"",
        image.display(),
        report.path.display()
    );
    let child = session
        .spawn_suspended(&image, OsStr::new(&command))
        .unwrap();
    child.resume().unwrap();
    child
}

#[test]
fn rust_console_native_event_budget_is_atomic_and_prebound_input_reaches_the_client() {
    let report = Report::new();
    let session = DriverSession::open().unwrap();
    let mut host = ConsoleHost::new();
    let mut output = Vec::new();
    assert_eq!(
        host.enqueue_input(&vec![ConsoleInput::Focus(true); 4096], &mut output)
            .unwrap(),
        0
    );
    let overflow = host
        .enqueue_input(&[ConsoleInput::Focus(false)], &mut output)
        .unwrap_err();
    assert_eq!(overflow.kind(), std::io::ErrorKind::WouldBlock);
    let child = launch_probe("count-events", &report, &session);
    let deadline = Instant::now() + Duration::from_secs(5);
    while child.wait(Duration::ZERO).unwrap().is_none() || host.connected_clients() != 0 {
        assert!(Instant::now() < deadline);
        match session.receive(Duration::from_millis(20)) {
            Ok(request) => {
                assert_eq!(host.dispatch(request, &mut output).unwrap().status, 0);
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => panic!("{error}"),
        }
    }
    assert_eq!(child.wait(Duration::ZERO).unwrap(), Some(0));
    let value: Value = serde_json::from_slice(&std::fs::read(&report.path).unwrap()).unwrap();
    assert_eq!(value["ok"], true);
    assert_eq!(
        value["wait"], 0,
        "input queued before CONNECT must signal the real input handle"
    );
    assert_eq!(
        value["count"], 4096,
        "failed enqueue must not change native-visible event inventory"
    );
}

#[test]
fn rust_console_native_job_drop_terminates_a_client_blocked_in_readconsole() {
    let report = Report::new();
    let session = DriverSession::open().unwrap();
    let child = launch_probe("line", &report, &session);
    let held = OwnedFixtureProcess(child.process_handle().try_clone_to_owned().unwrap());
    let mut host = ConsoleHost::new();
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    while host.pending_reads() == 0 {
        assert!(Instant::now() < deadline);
        match session.receive(Duration::from_millis(20)) {
            Ok(request) => {
                assert_eq!(host.dispatch(request, &mut output).unwrap().status, 0);
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => panic!("{error}"),
        }
    }
    assert!(child.contains_process(held.0.as_handle()).unwrap());
    drop(child);
    // SAFETY: the held handle is a duplicate of this exact owned test client.
    assert_eq!(
        unsafe { WaitForSingleObject(held.0.as_raw_handle(), 3000) },
        WAIT_OBJECT_0,
        "dropping the job owner must reclaim the actual waiting client"
    );
    while host.connected_clients() != 0 {
        assert!(Instant::now() < deadline);
        match session.receive(Duration::from_millis(20)) {
            Ok(request) => {
                assert_eq!(host.dispatch(request, &mut output).unwrap().status, 0);
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => panic!("{error}"),
        }
    }
    assert_eq!(host.pending_reads(), 0);
}

#[test]
fn rust_console_native_job_drop_reclaims_an_actual_inherited_console_descendant() {
    let report = Report::new();
    let session = DriverSession::open().unwrap();
    let child = launch_probe("job-tree", &report, &session);
    let root = OwnedFixtureProcess(child.process_handle().try_clone_to_owned().unwrap());
    let mut host = ConsoleHost::new();
    let mut output = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !report.path.exists()
        || !report.path.with_extension("ready").exists()
        || host.connected_clients() < 2
        || child.active_processes().unwrap() < 2
    {
        assert!(
            Instant::now() < deadline,
            "both real processes must attach to the Rust host and private job"
        );
        match session.receive(Duration::from_millis(20)) {
            Ok(request) => {
                assert_eq!(host.dispatch(request, &mut output).unwrap().status, 0);
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => panic!("{error}"),
        }
    }
    let value: Value = serde_json::from_slice(&std::fs::read(&report.path).unwrap()).unwrap();
    let pid = u32::try_from(value["child_pid"].as_u64().unwrap()).unwrap();
    // SAFETY: open the PID reported by our fresh private fixture. Membership is
    // verified before the handle gains any failure-cleanup termination action.
    let handle = unsafe {
        OpenProcess(
            PROCESS_QUERY_LIMITED_INFORMATION | PROCESS_TERMINATE | 0x0010_0000,
            0,
            pid,
        )
    };
    assert!(!handle.is_null());
    // SAFETY: successful OpenProcess transferred one unique owned handle.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle) };
    assert!(
        child.contains_process(handle.as_handle()).unwrap(),
        "never adopt an unowned process from a PID report"
    );
    let descendant = OwnedFixtureProcess(handle);
    assert_eq!(child.active_processes().unwrap(), 2);
    drop(child);
    for process in [&root, &descendant] {
        // SAFETY: both exact process objects were verified as privately owned.
        assert_eq!(
            unsafe { WaitForSingleObject(process.0.as_raw_handle(), 3000) },
            WAIT_OBJECT_0,
            "owner drop must reclaim root and descendant, not only the shell PID"
        );
    }
}

#[test]
fn rust_console_native_nowait_peek_does_not_queue_behind_a_blocked_read() {
    let report = Report::new();
    let session = DriverSession::open().unwrap();
    let child = launch_probe("concurrent-peek", &report, &session);
    let mut host = ConsoleHost::new();
    let mut output = Vec::new();
    let mut read_waiting = false;
    let mut peek_called = false;
    let mut supplied = false;
    let deadline = Instant::now() + Duration::from_secs(8);
    while child.wait(Duration::ZERO).unwrap().is_none() || host.connected_clients() != 0 {
        assert!(
            Instant::now() < deadline,
            "concurrent console reads did not finish"
        );
        match session.receive(Duration::from_millis(20)) {
            Ok(request) => {
                let call = host.dispatch(request, &mut output).unwrap();
                assert_eq!(call.status, 0);
                if call.api == Some(0x0100_0005) {
                    assert!(call.pending);
                    assert!(!read_waiting);
                    read_waiting = true;
                    std::fs::write(report.path.with_extension("go"), b"read-is-pending").unwrap();
                }
                if call.api == Some(0x0100_0004) {
                    assert!(read_waiting);
                    assert!(
                        !call.pending,
                        "NOWAIT must bypass a different thread's blocked cooked read"
                    );
                    peek_called = true;
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::TimedOut => {}
            Err(error) => panic!("{error}"),
        }
        if !supplied && report.path.with_extension("peek").exists() {
            assert!(read_waiting && peek_called);
            assert_eq!(host.pending_reads(), 1);
            supplied = true;
            assert_eq!(host.enqueue_text("p\r", &mut output).unwrap(), 1);
        }
    }
    assert!(supplied && peek_called);
    assert_eq!(child.wait(Duration::ZERO).unwrap(), Some(0));
    let value: Value = serde_json::from_slice(&std::fs::read(&report.path).unwrap()).unwrap();
    assert_eq!(value["peek_ok"], true);
    assert_eq!(value["peek_count"], 0);
    assert_eq!(value["ok"], true);
    assert_eq!(value["units"], json!([112, 13, 10]));
    assert_eq!(host.pending_reads(), 0);
}
