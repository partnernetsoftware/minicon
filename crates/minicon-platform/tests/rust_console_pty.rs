#![cfg(all(windows, feature = "rust-console"))]
//! Exercise the opened PTY facade, rather than manually servicing ConDrv.
use minicon_platform::pty::{
    ChildCommand, NativeInputOwnership, NativeTerminalKey, PtyChild, PtyMaster, TerminalSize,
};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::time::{Duration, Instant};
#[path = "../../minicon-console/tests/support/raw_batch_court.rs"]
mod raw_batch_court;

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
        Self::open_backend(command, "rust-console")
    }

    fn open_backend(command: ChildCommand, expected: &str) -> Self {
        let (master, child) = command.spawn().expect("open owned Rust PTY").into_parts();
        assert_eq!(master.backend_kind(), expected);
        Self::with_parts(master, child)
    }

    fn with_parts(mut master: PtyMaster, child: PtyChild) -> Self {
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

    fn wait_exit(&mut self, code: i32) {
        let mut waiter = self.child.try_clone_for_wait().unwrap();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let _ = send.send(waiter.wait());
        });
        let actual = receive
            .recv_timeout(Duration::from_secs(8))
            .unwrap()
            .unwrap()
            .code();
        if actual != Some(code) {
            while let Ok(Ok(chunk)) = self.output.recv_timeout(Duration::from_millis(20)) {
                self.bytes.extend(chunk);
            }
        }
        assert_eq!(
            actual,
            Some(code),
            "owned client output: {}",
            String::from_utf8_lossy(&self.bytes)
        );
    }

    fn finish(&mut self, code: i32) {
        self.wait_exit(code);
        if self.master.backend_kind() == "conpty" {
            // The system host retains its output pipe until HPCON close.
            self.child.close_pseudoconsole();
        }
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
    fn wait_ready(&self) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !self.0.with_extension("ready").exists() {
            assert!(
                Instant::now() < deadline,
                "native fixture did not publish readiness; details={:?}",
                std::fs::read_to_string(self.0.with_extension("go"))
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }
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
        for extension in [
            "ps1",
            "client-ready",
            "ready",
            "read-error",
            "read-panic",
            "go",
            "opened",
            "switched",
            "cancel-ready",
            "cancelled",
            "probe",
            "queried",
            "created",
            "signal",
            "retire-ready",
            "retired",
            "retire-tmp",
            "closed",
            "queue-tmp",
            "opened-ready",
            "detached",
            "first",
            "first.tmp",
            "ready.tmp",
            "go.tmp",
            "opened-ready.tmp",
            "orphan-ready",
            "orphan",
            "child",
            "stage0",
            "stage1",
            "stage2",
            "stage3",
            "stage4",
            "stage5",
        ] {
            let _ = std::fs::remove_file(self.0.with_extension(extension));
        }
        let _ = std::fs::remove_dir(self.0.parent().unwrap());
    }
}

fn control_receipt(mode: &str, rust: bool) -> String {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(mode)
        .arg(report.0.as_os_str());
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("control-probe-ready:");
    if mode == "control-read" {
        // A scheduling grace is not the evidence: LastError=995 with zero
        // units below proves that the SDK read was actually interrupted.
        std::thread::sleep(Duration::from_millis(60));
    }
    if !mode.starts_with("control-generate-") && mode != "control-group-parent" {
        fixture.master.write_all(&[3]).unwrap();
    }
    fixture.finish(0);
    std::fs::read_to_string(&report.0).unwrap()
}

#[test]
fn rust_console_pty_processed_ctrl_c_runs_native_handler_and_interrupts_read() {
    assert_eq!(control_receipt("control-handler", true), "|1");
    assert_eq!(control_receipt("control-read", true), "true,995,0,0|1");
}

#[test]
fn rust_console_pty_ctrl_c_handler_can_query_and_write_console_synchronously() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("control-handler-io")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.until("control-probe-ready:");
    fixture.master.write_all(&[3]).unwrap();
    fixture.finish(0);
    assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "|1");
    assert!(String::from_utf8_lossy(&fixture.bytes).contains("handler-native-write:"));
}

#[test]
fn rust_console_pty_ctrl_c_bypasses_a_full_input_event_queue() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("control-full-queue")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.until("control-probe-ready:");
    fixture.master.write_all(&[b'x'; 4096]).unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !report.0.with_extension("ready").exists() {
        assert!(
            Instant::now() < deadline,
            "native input queue did not reach capacity"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    fixture.master.write_all(&[3]).unwrap();
    fixture.finish(0);
    assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "4096,4096|1");
}

const OUTPUT_MODES: [(u32, &str); 16] = [
    (0, "|4,1|4,3|8,4|13,4|15,5|15,6|15,6|7,0"),
    (1, "|1,2|9,3|1,4|11,4|15,5|15,6|15,6|7,0"),
    (2, "|4,1|4,3|8,4|13,4|1,6|0,7|1,7|7,0"),
    (3, "|1,2|9,3|1,4|11,4|1,6|0,7|1,7|7,0"),
    (4, "|4,1|4,3|8,4|13,4|15,5|15,6|15,6|7,0"),
    (5, "|1,2|9,3|1,4|11,4|15,5|15,6|15,6|5,1"),
    (6, "|4,1|4,3|8,4|13,4|1,6|0,7|1,7|7,0"),
    (7, "|1,2|9,3|1,4|11,4|1,6|15,6|1,7|5,1"),
    (8, "|4,1|4,3|8,4|13,4|15,5|15,6|15,6|7,0"),
    (9, "|3,2|9,3|1,4|11,4|15,5|15,6|15,6|7,0"),
    (10, "|4,1|4,3|8,4|13,4|1,6|0,7|1,7|7,0"),
    (11, "|3,2|9,3|1,4|11,4|1,6|0,7|1,7|7,0"),
    (12, "|4,1|4,3|8,4|13,4|15,5|15,6|15,6|7,0"),
    (13, "|3,2|9,3|1,4|11,4|15,5|15,6|15,6|5,1"),
    (14, "|4,1|4,3|8,4|13,4|1,6|0,7|1,7|7,0"),
    (15, "|3,2|9,3|1,4|11,4|1,6|15,6|1,7|5,1"),
];

#[test]
fn rust_console_pty_no_wrap_wide_margin_makes_native_progress_without_splitting_cells() {
    let report = Report::new();
    let started = Instant::now();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("output-wide-margin")
            .arg(report.0.as_os_str())
            .size(TerminalSize { rows: 4, cols: 16 })
            .rust_console(),
    );
    fixture.finish(0);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(
        std::fs::read_to_string(&report.0).unwrap(),
        "5,3,15,1|1,15,1"
    );
    let mut parser = vt100::Parser::new(4, 16, 0);
    parser.process(&fixture.bytes);
    assert_eq!(parser.screen().cursor_position(), (1, 15));
    assert_eq!(
        parser
            .screen()
            .rows(0, 16)
            .map(|row| row.trim_end().to_owned())
            .collect::<Vec<_>>(),
        ["", "               Z", "", ""]
    );
}

fn native_buffers(rust: bool) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("buffers")
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    let mut states = Vec::new();
    for stage in 0..6 {
        fixture.until(&format!("buffers-stage-{stage}:"));
        let mut parser = vt100::Parser::new(8, 16, 0);
        parser.process(&fixture.bytes);
        states.push(parser.screen().clone());
        std::fs::write(
            report.0.with_extension(format!("stage{stage}")),
            b"observed",
        )
        .unwrap();
    }
    fixture.finish(0);
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    eprintln!("native buffers rust={rust}: {receipt}");
    for (stage, screen) in states.iter().enumerate() {
        eprintln!(
            "stage {stage}: {:?}",
            screen.rows(0, 16).collect::<Vec<_>>()
        );
    }
    for (stage, first, second, third) in [
        (0, "BASE", "", ""),
        (1, "FIRST", "", "DUP"),
        (2, "SECOND", "", ""),
        (3, "BASE", "HIDDEN", ""),
        (4, "SECOND", "MORE", ""),
        (5, "BASE", "HIDDEN", ""),
    ] {
        let rows = states[stage].rows(0, 16).collect::<Vec<_>>();
        assert_eq!(
            &rows[..3],
            &[first, second, third],
            "stage {stage}: {receipt}"
        );
    }
    assert!(receipt.contains("readonly-active:0,5"), "{receipt}");
    assert!(
        receipt.contains("first-new:size=16,8;view=0,0,15,7;pos=0,0;attr=001e;mode=7;cursor=25,1"),
        "{receipt}"
    );
    assert!(
        receipt.contains("second-new:size=16,8;view=0,0,15,7;pos=0,0;attr=001e;mode=7;cursor=25,1"),
        "{receipt}"
    );
    assert!(
        receipt.contains(
            "reopened-first:size=16,8;view=0,0,15,7;pos=5,0;attr=002d;mode=7;cursor=61,0"
        ),
        "{receipt}"
    );
    assert!(
        receipt
            .contains("active-close:size=32,12;view=0,0,15,7;pos=6,1;attr=001e;mode=7;cursor=25,1"),
        "{receipt}"
    );
}

fn check_wide_title_queries(native: &str, stage: usize, original: bool, title: &str) {
    let title: Vec<_> = title.encode_utf16().collect();
    for capacity in [0, 1, 2, 3, 4, 5, 8, 9, 10, 11, 64, 512] {
        let mut data = [0x5555; 12];
        let copied = if original {
            capacity.min(title.len())
        } else {
            capacity.saturating_sub(1).min(title.len())
        };
        for (i, unit) in title.iter().take(copied.min(12)).enumerate() {
            data[i] = if original && i + 1 == capacity {
                0
            } else {
                *unit
            };
        }
        if !original && capacity > 0 && copied < 12 {
            data[copied] = 0;
        }
        let hex = data
            .iter()
            .map(|u| format!("{u:x}"))
            .collect::<Vec<_>>()
            .join(",");
        let count = if capacity == 0 { 0 } else { title.len() };
        let expected = format!(
            "stage{stage}:original={original};wide-cap={capacity};ret={count},0;data={hex}"
        );
        assert!(
            native.lines().any(|line| line == expected),
            "missing wide query: {expected}"
        );
    }
}

fn check_ansi_title_queries(native: &str, stage: usize, original: bool, title: &[u8]) {
    for capacity in [0, 1, 2, 3, 4, 5, 8, 9, 10, 11, 64, 512] {
        let mut data = [0x55; 12];
        let count = if capacity >= title.len() {
            title.len()
        } else {
            0
        };
        if !title.is_empty() && capacity > 0 {
            if capacity < title.len() {
                data[0] = 0;
            } else {
                let copied = title.len().min(12);
                data[..copied].copy_from_slice(&title[..copied]);
                if capacity > title.len() && title.len() < 12 {
                    data[title.len()] = 0;
                }
            }
        }
        let hex = data
            .iter()
            .map(|u| format!("{u:x}"))
            .collect::<Vec<_>>()
            .join(",");
        let expected = format!(
            "stage{stage}:original={original};ansi-cap={capacity};ret={count},0;data={hex}"
        );
        assert!(
            native.lines().any(|line| line == expected),
            "missing ANSI query: {expected}"
        );
    }
}

fn title_lifetime(rust: bool) {
    #[derive(Default)]
    struct Titles(Vec<Vec<u8>>);
    impl vt100::Callbacks for Titles {
        fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
            self.0.push(title.to_vec());
        }
    }
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("title-life")
        .arg(&report.0)
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    let mut peer = vt100::Parser::new_with_callbacks(8, 16, 0, Titles::default());
    let initial = env!("CARGO_BIN_EXE_minicon-platform-console-probe");
    let expected = [
        initial,
        "child-native",
        "child-native",
        "child-vt",
        "child-vt",
        "after-child",
        "last-title",
    ];
    for (n, expected_title) in expected.iter().enumerate() {
        let marker = format!("life-title-{n}:");
        let deadline = Instant::now() + Duration::from_secs(8);
        while peer
            .screen()
            .rows(0, 16)
            .nth(7)
            .unwrap()
            .trim_end_matches(' ')
            != marker
            || !peer.at_boundary()
        {
            let chunk = fixture
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("title lifetime fence")
                .unwrap();
            peer.process(&chunk);
            fixture.bytes.extend(chunk);
        }
        eprintln!(
            "title-life rust={rust};stage={n};peer={:?};row={:?}",
            peer.callbacks()
                .0
                .last()
                .map(|t| String::from_utf8_lossy(t)),
            peer.screen().rows(0, 16).next().unwrap()
        );
        assert_eq!(
            peer.callbacks().0.last().map(Vec::as_slice),
            Some(expected_title.as_bytes())
        );
        if n == 5 {
            assert_eq!(peer.screen().rows(0, 16).next().unwrap(), "ROOTZ");
            assert_eq!(
                peer.screen().cell(0, 4).unwrap().fgcolor(),
                vt100::Color::Idx(1)
            );
        }
        std::fs::write(report.0.with_extension(format!("stage{n}")), b"observed").unwrap();
    }
    let consumed = fixture.bytes.len();
    fixture.finish(0);
    peer.process(&fixture.bytes[consumed..]);
    assert_eq!(peer.callbacks().0.last().unwrap(), b"last-title");
    if rust {
        assert_eq!(
            peer.callbacks()
                .0
                .iter()
                .filter(|title| title.as_slice() == initial.as_bytes())
                .count(),
            1
        );
    }
    eprintln!(
        "title-life rust={rust};eof={:?}",
        peer.callbacks()
            .0
            .last()
            .map(|t| String::from_utf8_lossy(t))
    );
    let receipt = std::fs::read_to_string(report.0.as_path()).unwrap();
    let hex = |s: &str| {
        s.encode_utf16()
            .map(|u| format!("{u:x}"))
            .collect::<Vec<_>>()
            .join(",")
    };
    for (n, title) in expected.iter().enumerate() {
        assert!(
            receipt
                .lines()
                .any(|line| line
                    == format!("stage{n}:current={};original={}", hex(title), hex(initial))),
            "{receipt}"
        );
    }
    eprintln!("title-life native rust={rust}:\n{receipt}");
}
#[test]
fn rust_console_pty_title_lifetime_tracks_initial_and_child_exit() {
    title_lifetime(true);
}
#[test]
#[ignore = "explicit system title lifecycle reference"]
fn rust_console_client_title_lifetime_records_system_conpty() {
    title_lifetime(false);
}

fn title_termination(rust: bool) {
    #[derive(Default)]
    struct Titles(Vec<Vec<u8>>);
    impl vt100::Callbacks for Titles {
        fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
            self.0.push(title.to_vec());
        }
    }
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("titles-termination")
        .arg(&report.0)
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    let mut peer = vt100::Parser::new_with_callbacks(8, 16, 0, Titles::default());
    let expected = [
        "bel",
        "anchor",
        "anchor",
        "anchor",
        "replacement",
        "anchor",
        "st",
        "c1",
        "c1-only",
        "c1-only",
    ];
    for (n, expected_title) in expected.iter().enumerate() {
        let marker = format!("ttl-stage-{n}:");
        let deadline = Instant::now() + Duration::from_secs(8);
        while peer
            .screen()
            .rows(0, 16)
            .nth(7)
            .unwrap()
            .trim_end_matches(' ')
            != marker
            || !peer.at_boundary()
        {
            let chunk = fixture
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("OSC termination fence")
                .unwrap();
            peer.process(&chunk);
            fixture.bytes.extend(chunk);
        }
        eprintln!(
            "title-end rust={rust};stage={n};peer={:?};row={:?}",
            peer.callbacks()
                .0
                .last()
                .map(|t| String::from_utf8_lossy(t)),
            peer.screen().rows(0, 16).next().unwrap()
        );
        assert_eq!(
            peer.callbacks().0.last().map(Vec::as_slice),
            Some(expected_title.as_bytes())
        );
        assert_eq!(peer.screen().rows(0, 16).next().unwrap(), "ROOT");
        std::fs::write(report.0.with_extension(format!("stage{n}")), b"observed").unwrap();
    }
    fixture.finish(0);
    let native = std::fs::read_to_string(&report.0).unwrap();
    for (n, title) in expected.iter().enumerate() {
        check_wide_title_queries(&native, n, false, title);
        check_wide_title_queries(
            &native,
            n,
            true,
            env!("CARGO_BIN_EXE_minicon-platform-console-probe"),
        );
    }
    eprintln!("title-end native rust={rust}:\n{native}");
}
#[test]
fn rust_console_pty_title_termination_preserves_native_and_frontend_state() {
    title_termination(true);
}
#[test]
#[ignore = "explicit system OSC cancellation reference"]
fn rust_console_client_title_termination_records_system_conpty() {
    title_termination(false);
}

fn native_titles(rust: bool) {
    #[derive(Default)]
    struct Titles(Vec<Vec<u8>>, usize);
    impl vt100::Callbacks for Titles {
        fn audible_bell(&mut self, _: &mut vt100::Screen) {
            self.1 += 1;
        }
        fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
            self.0.push(title.to_vec());
        }
    }
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("titles")
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    let mut peer = vt100::Parser::new_with_callbacks(8, 16, 0, Titles::default());
    for n in 0..15 {
        let marker = format!("ttl-stage-{n}:");
        let deadline = Instant::now() + Duration::from_secs(8);
        while peer
            .screen()
            .rows(0, 16)
            .nth(7)
            .unwrap()
            .trim_end_matches(' ')
            != marker
            || !peer.at_boundary()
        {
            let chunk = fixture
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("native title fence")
                .unwrap();
            peer.process(&chunk);
            fixture.bytes.extend(chunk);
        }
        eprintln!(
            "title rust={rust};stage={n};peer={:?};row={:?}",
            peer.callbacks()
                .0
                .last()
                .map(|t| String::from_utf8_lossy(t)),
            peer.screen().rows(0, 16).next().unwrap()
        );
        if rust {
            let expected = match n {
                1 => Some("native-中😀"),
                2 | 3 => Some("during-fragment"),
                4 => Some("vt-active"),
                5 => Some("vt-inactive"),
                6 => Some("A中"),
                7 => Some("B中"),
                8 | 9 | 11 => Some(""),
                10 => Some("A•←]2;BAD"),
                12 => Some("�Q"),
                13 => Some("mid-osc"),
                _ => None,
            };
            if let Some(expected) = expected {
                assert_eq!(
                    peer.callbacks().0.last().map(Vec::as_slice),
                    Some(expected.as_bytes())
                );
            }
            if n == 3 {
                assert_eq!(peer.screen().rows(0, 16).next().unwrap(), "ROOTZ");
                assert_eq!(
                    peer.screen().cell(0, 4).unwrap().fgcolor(),
                    vt100::Color::Idx(1)
                );
            }
            if n == 10 {
                let safe = "\x1b]0;A•←]2;BAD\x1b\\".as_bytes();
                assert!(fixture.bytes.windows(safe.len()).any(|bytes| bytes == safe));
                assert_eq!(peer.screen().rows(0, 16).next().unwrap(), "HIDDEN!");
            }
            if n == 14 {
                let expected = format!("vt;{}中😀", "x;".repeat(24));
                assert_eq!(peer.callbacks().0.last().unwrap(), expected.as_bytes());
            }
            if n == 5 {
                assert_eq!(peer.screen().rows(0, 16).next().unwrap(), "ROOTZ");
                assert_eq!(peer.callbacks().1, 1);
            }
        }
        std::fs::write(report.0.with_extension(format!("stage{n}")), b"observed").unwrap();
    }
    fixture.finish(0);
    let native = std::fs::read_to_string(&report.0).unwrap();
    assert!(native.contains("set-936=1,0\n"));
    assert!(native.contains("set-65001=1,0\n"));
    let original = env!("CARGO_BIN_EXE_minicon-platform-console-probe");
    assert!(original.is_ascii());
    for stage in 0..15 {
        check_wide_title_queries(&native, stage, true, original);
        check_ansi_title_queries(&native, stage, true, original.as_bytes());
    }
    for (stage, title) in [(4, "vt-active"), (5, "vt-inactive"), (13, "mid-osc")] {
        check_wide_title_queries(&native, stage, false, title);
        check_ansi_title_queries(&native, stage, false, title.as_bytes());
    }
    check_wide_title_queries(&native, 14, false, &format!("vt;{}中😀", "x;".repeat(24)));
    for (stage, title, ansi) in [
        (6, "A中", b"A\xd6\xd0".as_slice()),
        (7, "B中", b"B\xe4\xb8\xad".as_slice()),
        (8, "", b"".as_slice()),
        (11, "", b"".as_slice()),
        (12, "�Q", b"\xef\xbf\xbdQ".as_slice()),
    ] {
        check_wide_title_queries(&native, stage, false, title);
        check_ansi_title_queries(&native, stage, false, ansi);
    }
    if !rust {
        assert!(
            native.starts_with("build=20348\n"),
            "review title reference policy for this Windows build"
        );
    }
    for (stage, title) in [
        (1, "native-中😀"),
        (2, "during-fragment"),
        (3, "during-fragment"),
        (8, ""),
        (9, ""),
        (10, if rust { "A\x07\x1b]2;BAD" } else { "A]2;BAD" }),
    ] {
        let units: Vec<_> = title.encode_utf16().collect();
        let hex = units
            .iter()
            .map(|u| format!("{u:x}"))
            .collect::<Vec<_>>()
            .join(",");
        let expected = format!(
            "stage{stage}:original=false;wide={},0;data={hex};next=0",
            units.len()
        );
        assert!(
            native.contains(&expected),
            "missing native title: {expected}\n{native}"
        );
    }
    eprintln!("title native rust={rust}:\n{native}");
}
#[test]
fn rust_console_pty_native_wide_titles_publish_independently_of_client_grammar() {
    native_titles(true);
}
#[test]
#[ignore = "explicit native title system reference"]
fn rust_console_client_titles_match_system_conpty() {
    native_titles(false);
}

fn native_vt_layers(rust: bool) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("buffer-vt")
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    let mut peer = vt100::Parser::new(8, 16, 0);
    for n in 0..9 {
        let marker = format!("alt-stage-{n}:");
        let column = [6, 6, 3, 11, 12, 4, 3, 6, 4][n];
        let deadline = Instant::now() + Duration::from_secs(8);
        while peer
            .screen()
            .rows(0, 16)
            .nth(7)
            .unwrap()
            .trim_end_matches(' ')
            != marker
            || peer.screen().cursor_position() != (0, column)
            || !peer.at_boundary()
        {
            let chunk = fixture
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("native layer marker/cursor fence")
                .unwrap();
            peer.process(&chunk);
            fixture.bytes.extend(chunk);
        }
        let native = std::fs::read_to_string(&report.0).unwrap();
        eprintln!(
            "layers rust={rust};stage={n};alt={};rows={:?};cursor={:?}\n{native}",
            peer.screen().alternate_screen(),
            peer.screen().rows(0, 16).take(3).collect::<Vec<_>>(),
            peer.screen().cursor_position()
        );
        let expected = [
            "VTBASE",
            "NATIVE",
            "VTA",
            "VTBASECARRY",
            "VTBASECARRYZ",
            "BASE",
            "VTA",
            "NATIVE",
            "BASE",
        ][n];
        assert_eq!(
            peer.screen()
                .rows(0, 16)
                .next()
                .unwrap()
                .trim_end_matches(' '),
            expected
        );
        assert!(
            native
                .lines()
                .any(|line| line == format!("stage{n}:row0={expected:?}")),
            "{native}"
        );
        assert!(
            native.lines().any(|line| line
                .starts_with(&format!("stage{n}:size=16,8;view=0,0,15,7;pos={column},0;"))),
            "{native}"
        );
        if rust {
            assert_eq!(
                peer.screen().alternate_screen(),
                matches!(n, 0 | 2 | 3 | 4 | 6),
                "logical alternate layer must survive activation at stage {n}"
            );
        } else {
            assert!(
                native.lines().any(|line| line == "build=20348"),
                "BLOCKED: qualify native/VT layers on this system build"
            );
            assert!(
                !peer.screen().alternate_screen(),
                "this system build projects into its main peer grid"
            );
        }
        if n == 4 {
            assert_eq!(
                peer.screen().cell(0, 11).unwrap().fgcolor(),
                vt100::Color::Idx(1)
            );
        }
        std::fs::write(report.0.with_extension(format!("stage{n}")), b"observed").unwrap();
    }
    fixture.finish(0);
}

#[test]
fn rust_console_pty_native_buffers_preserve_vt_alternate_layers_and_fragments() {
    native_vt_layers(true);
}

#[test]
#[ignore = "explicit native/VT alternate system comparison"]
fn rust_console_client_native_vt_layers_record_system_projection() {
    native_vt_layers(false);
}

fn vertical_viewport_follow(rust: bool) {
    for (mode, unicode) in [(7, false), (7, true), (3, false), (3, true)] {
        let report = Report::new();
        let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg(format!(
                "viewport-follow-{mode}-{}",
                if unicode { "w" } else { "file" }
            ))
            .arg(report.0.as_os_str())
            .size(TerminalSize { rows: 4, cols: 12 });
        let mut fixture = if rust {
            Fixture::open(command.rust_console())
        } else {
            Fixture::open_backend(command, "conpty")
        };
        for stage in 0..6 {
            fixture.until(&format!("buffers-stage-{stage}:"));
            let mut peer = vt100::Parser::new(4, 12, 0);
            peer.process(&fixture.bytes);
            eprintln!(
                "viewport rust={rust};mode={mode};stage={stage};rows={:?};cursor={:?};hide={}",
                peer.screen().rows(0, 12).collect::<Vec<_>>(),
                peer.screen().cursor_position(),
                peer.screen().hide_cursor()
            );
            let native = std::fs::read_to_string(&report.0).unwrap();
            let rows = peer
                .screen()
                .rows(0, 12)
                .map(|row| row.trim_end_matches(' ').to_owned())
                .collect::<Vec<_>>();
            let expected = [
                ["012345678901", "", "", ""],
                ["L03", "L04", "L05", ""],
                ["L17", "L18", "L19", ""],
                ["R05", "L06", "L07", "L08"],
                ["R05", "BACK", "L07", "L08"],
                ["R05", "BACK", "012345678901", "L08"],
            ];
            assert_eq!(
                rows, expected[stage],
                "rust={rust};mode={mode};unicode={unicode};{native}"
            );
            assert_eq!(peer.screen().hide_cursor(), matches!(stage, 0 | 5));
            if !matches!(stage, 0 | 5) {
                assert_eq!(
                    peer.screen().cursor_position(),
                    [(0, 0), (3, 0), (3, 0), (0, 1), (2, 0), (0, 0)][stage]
                );
            }
            let top = [0, 3, 12, 0, 0, 0][stage];
            let (x, y) = [(20, 0), (0, 6), (0, 15), (1, 0), (0, 2), (20, 2)][stage];
            assert!(
                native.lines().any(|line| line
                    == format!("stage{stage}:view=0,{top},11,{};cursor={x},{y}", top + 3)),
                "{native}"
            );
            let buffer_rows = [
                ["01234567890123456789", "", ""],
                ["01234567890123456789", "L01", "L02"],
                ["L05", "L06", "L07"],
                ["R05", "L06", "L07"],
                ["R05", "BACK", "L07"],
                ["R05", "BACK", "01234567890123456789"],
            ];
            for (row, text) in buffer_rows[stage].iter().enumerate() {
                assert!(
                    native
                        .lines()
                        .any(|line| line == format!("stage{stage}:row{row}={text:?}")),
                    "{native}"
                );
            }
            assert!(
                native
                    .lines()
                    .any(|line| line == format!("stage{stage}:row15=\"\"")),
                "{native}"
            );
            std::fs::write(
                report.0.with_extension(format!("stage{stage}")),
                b"observed",
            )
            .unwrap();
        }
        fixture.finish(0);
        eprintln!(
            "viewport native rust={rust}:\n{}",
            std::fs::read_to_string(&report.0).unwrap()
        );
    }
}

#[test]
fn rust_console_pty_vertical_viewport_follows_long_output_without_horizontal_pan() {
    vertical_viewport_follow(true);
}

#[test]
#[ignore = "explicit system ConPTY viewport reference"]
fn rust_console_client_vertical_viewport_matches_system_conpty() {
    vertical_viewport_follow(false);
}

fn buffer_process_family(rust: bool, tail: bool) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(if tail {
            "buffer-family-root-tail"
        } else {
            "buffer-family-root"
        })
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    for stage in 0..36 {
        fixture.until(&format!("buffers-stage-{stage}:"));
        let mut peer = vt100::Parser::new(8, 16, 0);
        peer.process(&fixture.bytes);
        let rows = peer.screen().rows(0, 16).take(3).collect::<Vec<_>>();
        let inherited = stage % 2 == 0;
        assert_eq!(
            rows,
            if inherited {
                vec!["ALT", "CHILD", ""]
            } else {
                vec!["BASE", "", ""]
            },
            "rust={rust}; stage={stage}"
        );
        assert_eq!(
            peer.screen().cursor_position(),
            if inherited { (1, 5) } else { (0, 4) }
        );
        assert_eq!(peer.screen().hide_cursor(), inherited);
        assert_eq!(
            peer.screen().cell(0, 0).unwrap().fgcolor(),
            vt100::Color::Idx(if inherited { 13 } else { 11 })
        );
        assert_eq!(
            peer.screen().cell(0, 0).unwrap().bgcolor(),
            vt100::Color::Idx(if inherited { 2 } else { 4 })
        );
        assert!(peer.at_boundary());
        assert!(
            std::str::from_utf8(&fixture.bytes).is_ok(),
            "retired UTF-8 fragments must not leak to the peer"
        );
        let native = std::fs::read_to_string(&report.0).unwrap();
        let event = format!(
            "round={};{};",
            stage / 2,
            if inherited {
                "creator-exit=0"
            } else {
                "last-exit=0"
            }
        );
        let offset = native.rfind(&event).expect("native report for this stage");
        let native = &native[offset..];
        if inherited {
            assert!(
                native.contains(
                    "survivor:size=16,8;view=0,0,15,7;pos=5,1;attr=002d;mode=7;cursor=63,0"
                ),
                "{native}"
            );
            assert!(native.contains("active-held:row1=\"CHILD\""), "{native}");
            assert!(native.contains("base-held:row0=\"BASE\""), "{native}");
        } else {
            assert!(
                native.contains(
                    "fallback:size=16,8;view=0,0,15,7;pos=4,0;attr=001e;mode=7;cursor=25,1"
                ),
                "{native}"
            );
        }
        std::fs::write(
            report.0.with_extension(format!("stage{stage}")),
            b"observed",
        )
        .unwrap();
    }
    fixture.finish(0);
    let native = std::fs::read_to_string(&report.0).unwrap();
    eprintln!("buffer-family rust={rust};tail={tail}:\n{native}");
    assert!(native.ends_with("rounds=18\n"));
    assert_eq!(native.matches("creator-exit=0;clients=2").count(), 18);
    assert_eq!(native.matches("last-exit=0;clients=1").count(), 18);
    let mut peer = vt100::Parser::new(8, 16, 0);
    peer.process(&fixture.bytes);
    assert_eq!(peer.screen().contents(), "BASE");
    assert_eq!(peer.screen().cursor_position(), (0, 4));
    assert!(peer.at_boundary());
}

#[test]
fn rust_console_pty_native_buffers_survive_creator_exit_and_retire_after_last_child() {
    buffer_process_family(true, false);
    buffer_process_family(true, true);
}

#[test]
#[ignore = "explicit system ConPTY comparison court"]
fn rust_console_client_native_buffer_family_matches_system_conpty() {
    buffer_process_family(false, false);
}

#[test]
fn rust_console_pty_malformed_utf8_recovery_survives_native_edits_and_exit() {
    for mapped in [false, true] {
        let report = Report::new();
        let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg(if mapped {
                "utf8-recovery-mapped"
            } else {
                "utf8-recovery-raw"
            })
            .arg(report.0.as_os_str())
            .size(TerminalSize { rows: 3, cols: 8 })
            .rust_console();
        let mut fixture = Fixture::open(command);
        report.wait_ready();
        let mut peer = vt100::Parser::new(3, 8, 0);
        let deadline = Instant::now() + Duration::from_secs(8);
        while peer.screen().cell(0, 0).unwrap().contents() != "\u{fffd}"
            || peer.screen().cursor_position() != (1, 2)
        {
            let chunk = fixture
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("early replacement and native cursor must reach peer")
                .unwrap();
            peer.process(&chunk);
            fixture.bytes.extend(chunk);
        }
        assert!(peer.at_boundary());
        std::fs::write(report.0.with_extension("go"), b"observed").unwrap();
        fixture.finish(0);
        let receipt = std::fs::read_to_string(&report.0).unwrap();
        assert_eq!(
            receipt,
            format!("mapped={mapped};replacement=fffd;resumed=3042;tail=fffd")
        );
        let mut peer = vt100::Parser::new(3, 8, 0);
        peer.process(&fixture.bytes);
        assert_eq!(peer.screen().cell(0, 0).unwrap().contents(), "\u{fffd}");
        assert_eq!(peer.screen().cell(1, 2).unwrap().contents(), "あ");
        assert_eq!(peer.screen().cell(1, 4).unwrap().contents(), "\u{fffd}");
        assert_eq!(peer.screen().cursor_position(), (2, 0));
        assert!(
            fixture.bytes.ends_with(b"\xe3"),
            "only new unfinished grammar may reach the final tail"
        );
        assert!(!peer.at_boundary());
    }
}

fn unicode_text_batches(rust: bool) {
    for mode in [7, 3, 15, 11] {
        for unicode in [true, false] {
            let report = Report::new();
            let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg(format!(
                    "text-batch-{}-{mode}",
                    if unicode { "w" } else { "a" }
                ))
                .arg(report.0.as_os_str())
                .size(TerminalSize {
                    rows: 32,
                    cols: 128,
                });
            let mut fixture = if rust {
                Fixture::open(command.rust_console())
            } else {
                Fixture::open_backend(command, "conpty")
            };
            fixture.until("text-batch-end:");
            std::fs::write(report.0.with_extension("opened"), b"observed").unwrap();
            fixture.finish(0);
            let receipt = std::fs::read_to_string(&report.0).unwrap();
            eprintln!("bulk rust={rust} bytes={} {receipt}", fixture.bytes.len());
            let mut observed = vt100::Parser::new(32, 128, 0);
            observed.process(&fixture.bytes);
            eprintln!(
                "bulk first row={:?};wire prefix={:?}",
                observed.screen().rows(0, 128).next().unwrap(),
                String::from_utf8_lossy(&fixture.bytes[..fixture.bytes.len().min(220)])
            );
            let written = if unicode { 8192 } else { 13312 };
            assert!(
                receipt.contains(&format!("written={written}/{written};size=256,64;")),
                "{receipt}"
            );
            if !rust {
                assert!(
                    receipt.lines().any(|line| line == "build=20348"),
                    "BLOCKED: qualify bulk native Unicode layout on this build: {receipt}"
                );
                assert!(receipt.contains("cursor=6,36\n"), "{receipt}");
                assert!(
                    receipt.contains("glyphs=0041,0065,0301,4e2d,4e2d,fffd,fffd,0042,0043"),
                    "{receipt}"
                );
                assert!(
                    observed.screen().contents().contains('中'),
                    "missing system bulk output"
                );
                // This installed host stores combining marks/surrogate units
                // in its legacy cell layout even with VT processing enabled.
                // It is a measured compatibility gap, not the owned scalar
                // model's intended cursor or projected-row oracle.
                continue;
            }
            let cursor = if mode & 4 != 0 { "255,31" } else { "0,32" };
            assert!(receipt.contains(&format!("cursor={cursor}\n")), "{receipt}");
            let top = u16::from(mode & 4 == 0);
            assert!(
                receipt.contains(&format!("view=0,{top},127,{}\n", top + 31)),
                "{receipt}"
            );
            let mut parser = vt100::Parser::new(32, 128, 0);
            parser.process(&fixture.bytes);
            let expected = "Ae\u{301}中😀BC".repeat(16);
            for (row, text) in parser.screen().rows(0, 128).enumerate() {
                let expected_row = if top == 1 && row == 31 { "" } else { &expected };
                assert_eq!(text, expected_row, "row {row}: {receipt}");
            }
            if top == 1 {
                assert_eq!(parser.screen().cursor_position(), (31, 0));
                assert!(!parser.screen().hide_cursor());
            }
            if rust {
                assert!(
                    fixture.bytes.len() < 16384,
                    "bulk projection amplified to {} bytes: {receipt}",
                    fixture.bytes.len()
                );
            }
        }
    }
}

#[test]
fn rust_console_pty_unicode_batches_preserve_bulk_output_without_amplification() {
    unicode_text_batches(true);
}

#[test]
#[ignore = "explicit system ConPTY Unicode bulk output court"]
fn rust_console_client_unicode_batches_record_system_layout() {
    unicode_text_batches(false);
}

#[test]
fn rust_console_pty_native_buffers_preserve_identity_and_display() {
    native_buffers(true);
}

#[test]
#[ignore = "explicit system ConPTY multiple-buffer court"]
fn rust_console_client_native_buffers_match_system_conpty() {
    native_buffers(false);
}

#[test]
fn rust_console_pty_native_buffer_close_preserves_output_order_under_pressure() {
    let report = Report::new();
    let (master, child) = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("buffers-pressure")
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 4, cols: 16 })
        .rust_console()
        .spawn()
        .unwrap()
        .into_parts();
    report.wait_ready();
    let held = std::fs::read_to_string(report.0.with_extension("ready")).unwrap();
    assert!(
        held.contains("held:row0=\"ALT\""),
        "close changed active identity before output capacity: {held}"
    );
    assert!(held.contains("held-rect=ALT"), "{held}");
    assert!(held.contains("held-clients=1"), "{held}");
    let mut fixture = Fixture::with_parts(master, child);
    fixture.finish(0);
    assert_eq!(
        std::fs::read_to_string(&report.0).unwrap(),
        "closed-after-drain"
    );
    let mut parser = vt100::Parser::new(4, 16, 0);
    parser.process(&fixture.bytes);
    assert_eq!(parser.screen().rows(0, 16).next().unwrap(), "BASE");
    assert!(String::from_utf8_lossy(&fixture.bytes).contains("buffers-pressure-end:"));
}

fn geometry_parent_resize(rust: bool) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("geometry-parent")
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("geometry-parent-ready:");
    fixture
        .master
        .resize(TerminalSize { rows: 12, cols: 24 })
        .unwrap();
    fixture.until("geometry-parent-done:");
    fixture.finish(0);
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    eprintln!("geometry parent rust={rust}: {receipt}");
    let event = if rust {
        "event:24,12"
    } else {
        assert!(
            receipt.lines().any(|line| line == "build=20348"),
            "BLOCKED: qualify parent geometry events on this build: {receipt}"
        );
        "event:24,16"
    };
    assert!(receipt.contains(event), "{receipt}");
    assert!(
        receipt.contains("parent-resized:buf=24,12;view=0,0,23,11;"),
        "{receipt}"
    );
}

#[test]
fn rust_console_pty_native_geometry_parent_resize_delivers_committed_dimensions() {
    geometry_parent_resize(true);
}

#[test]
#[ignore = "explicit system ConPTY parent geometry resize court"]
fn rust_console_client_native_geometry_parent_resize_matches_system() {
    geometry_parent_resize(false);
}

fn geometry_expanded_buffer_rendering(rust: bool) {
    for mode in ["geometry-wrap-vt", "geometry-wrap-legacy"] {
        let report = Report::new();
        let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg(mode)
            .arg(report.0.as_os_str())
            .size(TerminalSize { rows: 8, cols: 16 });
        let mut fixture = if rust {
            Fixture::open(command.rust_console())
        } else {
            Fixture::open_backend(command, "conpty")
        };
        fixture.until("geometry-wrap-end:");
        std::fs::write(report.0.with_extension("opened"), b"observed").unwrap();
        fixture.finish(0);
        let receipt = std::fs::read_to_string(&report.0).unwrap();
        eprintln!("{mode} native: {receipt}");
        let mut parser = vt100::Parser::new(8, 16, 0);
        parser.process(&fixture.bytes);
        eprintln!(
            "{mode} consumer: {:?};wire={:?}",
            parser.screen().rows(0, 16).collect::<Vec<_>>(),
            String::from_utf8_lossy(&fixture.bytes)
        );
        assert!(receipt.contains("set-buffer:grow:1,0"), "{receipt}");
        assert!(
            receipt.contains("row0:\"0123456789abcdefghijklmnopqrstuv\""),
            "{receipt}"
        );
        assert!(
            receipt.contains("row1:\"wxyzABCD                        \""),
            "{receipt}"
        );
        assert_eq!(
            parser.screen().rows(0, 16).collect::<Vec<_>>(),
            ["0123456789abcdef", "wxyzABCD", "", "", "", "", "", ""]
        );
    }
}

#[test]
fn rust_console_pty_native_geometry_expanded_buffer_rendering() {
    geometry_expanded_buffer_rendering(true);
}

#[test]
#[ignore = "explicit system ConPTY expanded-buffer rendering court"]
fn rust_console_client_native_geometry_expanded_buffer_rendering() {
    geometry_expanded_buffer_rendering(false);
}

fn geometry_report(rust: bool) -> String {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("geometry")
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("geometry-end:");
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    eprintln!("geometry rust={rust}: {receipt}");
    std::fs::write(report.0.with_extension("opened"), b"observed").unwrap();
    fixture.finish(0);
    receipt
}

fn assert_native_geometry(receipt: &str, rust: bool) {
    if !rust {
        assert!(
            receipt.lines().any(|line| line == "build=20348"),
            "BLOCKED: qualify this system geometry build: {receipt}"
        );
    }
    for label in ["grow", "grow-again", "buffer16x8"] {
        assert!(
            receipt.contains(&format!("set-buffer:{label}:1,0")),
            "{receipt}"
        );
    }
    for label in ["negative", "zero", "tiny", "short-max"] {
        assert!(
            receipt.contains(&format!("set-buffer:{label}:0,87")),
            "{receipt}"
        );
    }
    for label in [
        "window20x10",
        "relative-shift",
        "offset16x8",
        "negative-window",
        "one-cell",
        "restore",
    ] {
        assert!(
            receipt.contains(&format!("set-window:{label}:1,0")),
            "{receipt}"
        );
    }
    assert!(
        receipt.contains("set-window:reversed-window:0,87"),
        "{receipt}"
    );
    for (label, buffer, view, cursor) in [
        ("initial", "16,8", "0,0,15,7", "0,0"),
        ("grow", "32,16", "0,0,15,7", "5,2"),
        ("window20x10", "20,10", "0,0,19,9", "5,2"),
        (
            "relative-shift",
            "20,10",
            if rust { "0,0,19,9" } else { "1,1,20,10" },
            "5,2",
        ),
        (
            "grow-again",
            "40,20",
            if rust { "0,0,19,9" } else { "1,1,20,10" },
            "5,2",
        ),
        (
            "offset16x8",
            "16,8",
            if rust { "0,0,15,7" } else { "2,1,16,8" },
            "5,2",
        ),
        ("one-cell", "1,1", "0,0,0,0", "0,0"),
        ("restore", "16,8", "0,0,15,7", "0,0"),
    ] {
        assert!(
            receipt.contains(&format!(
                "{label}:buf={buffer};view={view};cursor={cursor};max={buffer};largest="
            )),
            "{receipt}"
        );
    }
    assert!(
        receipt.contains("access-largest:readonly:0,0,5"),
        "{receipt}"
    );
    assert!(receipt.contains("set-buffer:readonly:0,5"), "{receipt}");
    assert!(receipt.contains("set-window:readonly:0,5"), "{receipt}");
    assert!(receipt.contains("set-buffer:writeonly:1,0"), "{receipt}");
    assert!(receipt.contains("set-window:writeonly:1,0"), "{receipt}");
    let largest = if rust { "32766,32766" } else { "1664,60" };
    if rust {
        assert!(
            receipt.contains(&format!("access-largest:writeonly:{largest},0")),
            "{receipt}"
        );
    } else {
        let value = receipt
            .lines()
            .find_map(|line| line.strip_prefix("access-largest:writeonly:"))
            .unwrap();
        let values: Vec<i32> = value.split(',').map(|n| n.parse().unwrap()).collect();
        assert!(
            values[0] > 0 && values[1] > 0 && values[2] == 0,
            "{receipt}"
        );
    }
}

#[test]
fn rust_console_pty_native_geometry_preserves_buffer_window_contracts() {
    let receipt = geometry_report(true);
    assert_native_geometry(&receipt, true);
}

#[test]
#[ignore = "explicit system ConPTY geometry court"]
fn rust_console_client_native_geometry_matches_system_conpty() {
    let receipt = geometry_report(false);
    assert_native_geometry(&receipt, false);
}

fn cursor_info_report(rust: bool) -> (String, Vec<vt100::Screen>) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("cursor-info")
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 10, cols: 48 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    let mut parser = vt100::Parser::new(10, 48, 0);
    let mut parsed = 0;
    let mut screens = Vec::new();
    for label in [
        "prime",
        "bar-hide",
        "bar-show",
        "underline",
        "same-hide",
        "new-show",
        "partial",
        "resumed",
        "final",
    ] {
        fixture.until(&format!("cursor-{label}:"));
        parser.process(&fixture.bytes[parsed..]);
        parsed = fixture.bytes.len();
        let expected_shape = match label {
            "bar-hide" | "bar-show" => vt100::CursorShape::Bar,
            "prime" | "final" => vt100::CursorShape::Block,
            _ => vt100::CursorShape::Underline,
        };
        let expected_hidden = matches!(
            label,
            "bar-hide" | "underline" | "same-hide" | "partial" | "resumed"
        );
        let expected_position = if label == "prime" {
            (0, 1)
        } else if matches!(label, "resumed" | "final") {
            (1, 4)
        } else {
            (1, 3)
        };
        let deadline = Instant::now() + Duration::from_secs(8);
        while parser.screen().cursor_shape() != expected_shape
            || parser.screen().cursor_blinking()
            || parser.screen().hide_cursor() != expected_hidden
            || label != "partial"
                && (parser.screen().cursor_position() != expected_position || !parser.at_boundary())
        {
            let chunk = fixture
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|e| {
                    panic!(
                        "cursor fence rust={rust} {label}: {e}; current={:?},{},{};{:?};wire={:?}",
                        parser.screen().cursor_shape(),
                        parser.screen().cursor_blinking(),
                        parser.screen().hide_cursor(),
                        parser.screen().cursor_position(),
                        String::from_utf8_lossy(&fixture.bytes)
                    )
                });
            let chunk = chunk.unwrap();
            parser.process(&chunk);
            fixture.bytes.extend(chunk);
            parsed = fixture.bytes.len();
        }
        let screen = parser.screen();
        eprintln!(
            "cursor rust={rust} {label}: {:?},{},{};{:?}",
            screen.cursor_shape(),
            screen.cursor_blinking(),
            screen.hide_cursor(),
            screen.cursor_position()
        );
        if label == "prime" {
            assert_eq!(screen.cell(0, 0).unwrap().contents(), "P");
        } else {
            screens.push(screen.clone());
        }
        std::fs::write(report.0.with_extension(label), b"observed").unwrap();
    }
    fixture.finish(0);
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    eprintln!("cursor native rust={rust}: {receipt}");
    (receipt, screens)
}

fn assert_cursor_info(receipt: &str, screens: &[vt100::Screen]) {
    for line in [
        "bulk:200",
        "invalid:0:0,87,true",
        "invalid:101:0,87,true",
        "invalid:4294967295:0,87,true",
        "readonly:0,5",
        "writeonly:1,0",
        "readonly-invalid:0,5",
        "input:0,6",
        "bar-hide:63,0",
        "bar-show:63,1",
        "underline:63,0",
        "same-hide:63,0",
        "new-show:100,1",
        "partial:25,0",
        "resumed:25,0",
        "final:75,1",
    ] {
        assert!(
            receipt.lines().any(|actual| actual == line),
            "missing {line}: {receipt}"
        );
    }
    for label in [
        "bar-hide",
        "bar-show",
        "underline",
        "same-hide",
        "new-show",
        "partial",
    ] {
        assert!(receipt.contains(&format!("{label}:3,1;0014")), "{receipt}");
    }
    for label in ["resumed", "final"] {
        assert!(receipt.contains(&format!("{label}:4,1;0012")), "{receipt}");
    }
    assert_eq!(screens.len(), 8);
    assert_eq!(screens[7].cell(1, 2).unwrap().contents(), "X");
    assert_eq!(screens[7].cell(1, 3).unwrap().contents(), "Z");
    assert_eq!(
        screens[7].cell(1, 3).unwrap().fgcolor(),
        vt100::Color::Idx(2)
    );
}

#[test]
fn rust_console_pty_cursor_properties_preserve_shared_appearance_and_decoder() {
    let (receipt, screens) = cursor_info_report(true);
    assert_cursor_info(&receipt, &screens);
}

#[test]
#[ignore = "explicit system ConPTY cursor property court"]
fn rust_console_client_cursor_properties_match_system_conpty() {
    let (receipt, screens) = cursor_info_report(false);
    assert_cursor_info(&receipt, &screens);
}

fn indexed_output_report(rust: bool) -> (String, vt100::Parser) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("indexed-output")
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 6, cols: 8 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("indexed-end:");
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    eprintln!("indexed output rust={rust}: {receipt}");
    std::fs::write(report.0.with_extension("opened"), b"read").unwrap();
    fixture.finish(0);
    let mut parser = vt100::Parser::new(6, 8, 0);
    parser.process(&fixture.bytes);
    (receipt, parser)
}

fn assert_indexed_output(receipt: &str, rust: bool) {
    for (label, count) in [
        ("cross", 4),
        ("styles", 5),
        ("wide", 2),
        ("oem", 3),
        ("utf8", 4),
        ("utf8-corner", 0),
        ("utf8-clipped", 4),
        ("literal", 13),
        ("end", 1),
        ("empty", 0),
        ("writeonly-text", 1),
        ("writeonly-style", 1),
    ] {
        assert!(
            receipt
                .lines()
                .any(|line| line == format!("{label}:1,0,{count}")),
            "{receipt}"
        );
    }
    for label in ["negative", "right", "below"] {
        assert!(receipt.contains(&format!("{label}:0,87,0")), "{receipt}");
    }
    for label in ["readonly-text", "readonly-style"] {
        assert!(receipt.contains(&format!("{label}:0,5,0")), "{receipt}");
    }
    let cells = receipt
        .lines()
        .find_map(|line| line.strip_prefix("cells:"))
        .unwrap();
    let expected = concat!(
        "0041/0010,0042/0011,0043/0012,0044/0013,0045/0014,0046/0015,0078/0016,0079/004f,",
        "007a/002e,0071/c031,004b/0017,004c/0060,004d/0014,004e/0015,004f/0016,0020/0017,",
        "4e2d/0110,4e2d/0211,0051/0012,0054/0013,0055/0014,0056/0015,0057/0016,0058/0017,",
        "0059/0010,005a/0011,2502/0012,2524/0013,005a/0014,4e2d/0115,4e2d/0216,0052/0017,",
        "001b/0010,005b/0011,0033/0012,0031/0013,006d/0014,000d/0015,000a/0016,0000/0017,",
        "0009/0010,0007/0011,0008/0012,0301/0013,fffd/0014,4e2d/0115,4e2d/0216,0021/0017,"
    );
    assert_eq!(cells, expected);
    assert!(
        receipt.ends_with("preserved:5,5;002e\nresumed:6,5;0024\n"),
        "{receipt}"
    );
    assert!(receipt.contains("dbcs-read:d6/0110,d0/0211,"), "{receipt}");
    let tail = if !rust && receipt.lines().any(|line| line == "build=20348") {
        "ffff"
    } else {
        "4e2d"
    };
    assert!(
        receipt.contains(&format!("dbcs-written:4e2d/0110,{tail}/0211,")),
        "{receipt}"
    );
}

#[test]
fn rust_console_pty_indexed_output_preserves_styles_and_pending_decoder() {
    let (receipt, parser) = indexed_output_report(true);
    assert_indexed_output(&receipt, true);
    assert_eq!(parser.screen().cell(0, 6).unwrap().contents(), "x");
    assert_eq!(
        parser.screen().cell(0, 6).unwrap().fgcolor(),
        vt100::Color::Idx(3)
    );
    assert!(parser.screen().cell(1, 1).unwrap().inverse());
    assert!(parser.screen().cell(1, 1).unwrap().underline());
    assert_eq!(parser.screen().cell(2, 0).unwrap().contents(), "中");
    assert!(parser.screen().cell(2, 1).unwrap().is_wide_continuation());
    assert_eq!(parser.screen().cell(5, 3).unwrap().contents(), "◌");
    assert_eq!(parser.screen().cell(5, 4).unwrap().contents(), "�");
    assert_eq!(parser.screen().cell(5, 5).unwrap().contents(), "Z");
    assert_eq!(
        parser.screen().cell(5, 5).unwrap().fgcolor(),
        vt100::Color::Idx(1)
    );
}

#[test]
#[ignore = "explicit system ConPTY indexed-output reference court"]
fn rust_console_client_indexed_output_matches_system_conpty() {
    let (receipt, _) = indexed_output_report(false);
    assert_indexed_output(&receipt, false);
}

fn scroll_output_report(rust: bool) -> (String, vt100::Parser) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("scroll-output")
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 6, cols: 8 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("scroll-end:");
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    let mut parser = vt100::Parser::new(6, 8, 0);
    parser.process(&fixture.bytes);
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    eprintln!(
        "scroll rust={rust}: {receipt}; screen={:?}",
        parser.screen().contents()
    );
    (receipt, parser)
}

fn scroll_rows(receipt: &str, name: &str) -> Vec<String> {
    let prefix = format!("{name}:");
    let line = receipt
        .lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .unwrap();
    let units: Vec<_> = line
        .trim_end_matches(',')
        .split(',')
        .map(|cell| u16::from_str_radix(cell.split_once('/').unwrap().0, 16).unwrap())
        .collect();
    assert_eq!(units.len(), 48);
    units.chunks(8).map(String::from_utf16_lossy).collect()
}

fn assert_scroll_output(receipt: &str, parser: &vt100::Parser) {
    let right = receipt
        .lines()
        .find_map(|line| line.strip_prefix("right:"))
        .unwrap();
    let cells: Vec<_> = right.trim_end_matches(',').split(',').collect();
    assert_eq!(cells[0], "002e/004f");
    assert_eq!(cells[2], "0041/001e");
    assert_eq!(cells[10], "0049/001f");
    assert_eq!(
        &scroll_rows(receipt, "right")[..2],
        ["..ABCDEF", "..IJKLMN"]
    );
    assert_eq!(&scroll_rows(receipt, "left")[..2], ["CDEFGH..", "KLMNOP.."]);
    assert_eq!(
        &scroll_rows(receipt, "clip")[..3],
        ["A_____GH", "I_ABCDOP", "Q_IJKLWX"]
    );
    assert_eq!(
        &scroll_rows(receipt, "offscreen")[1..3],
        ["I###MNOP", "Q###UVWX"]
    );
    assert_eq!(scroll_rows(receipt, "ansi")[0], "││ABCDGH");
    assert_eq!(
        &scroll_rows(receipt, "negative")[..4],
        ["....EFGH", "....MNOP", "QRABCDWX", "YZIJKL_`"]
    );
    assert!(receipt.ends_with("preserved:6,5;002e\nresumed:7,5;0024\n"));
    assert_eq!(
        parser.screen().rows(0, 8).collect::<Vec<_>>(),
        [
            "││ABCDGH",
            "IJKLMNOP",
            "QRSTUVWX",
            "YZ[\\]^_`",
            "abcdefgh",
            "ijklmnZp"
        ]
    );
    assert!(receipt.contains("read-only:0,5"), "{receipt}");
    assert!(receipt.contains("write-only:1,0"), "{receipt}");
}

#[test]
fn rust_console_pty_scroll_moves_overlap_clip_and_preserves_client_decoder() {
    let (receipt, parser) = scroll_output_report(true);
    assert_scroll_output(&receipt, &parser);
}

#[test]
#[ignore = "explicit system ConPTY native scroll differential court"]
fn rust_console_client_scroll_matches_system_conpty() {
    let (receipt, parser) = scroll_output_report(false);
    assert_scroll_output(&receipt, &parser);
}

fn rect_output_report(rust: bool) -> (String, vt100::Parser) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("rect-output")
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("rect-end:");
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    eprintln!(
        "rect rust={rust}: {receipt}; screen={:?}",
        parser.screen().contents()
    );
    (receipt, parser)
}

fn assert_rect_output(receipt: &str, parser: &vt100::Parser, rust: bool) {
    let (build, receipt) = receipt.split_once('\n').unwrap();
    assert!(receipt.starts_with(&format!(
        "clip-write:0,0,1,0\nclip-read:0,0,2,0:{}004b/000b,004c/000c,0020/0007,\n",
        "0021/0055,".repeat(12)
    )));
    assert!(receipt.contains(
        "block-read:2,2,4,3:0061/001e,0062/001f,0063/0020,0064/0021,0065/0022,0066/0023,"
    ));
    assert!(receipt.contains("ansi-read:0,4,2,4:0041/001e,00b3/002d,0042/004f,"));
    assert!(receipt.contains("ansi-wide-read:0,4,2,4:0041/001e,2502/002d,0042/004f,"));
    assert!(receipt.contains("wide-read:0,5,3,5:4e2d/0107,4e2d/0207,0051/0007,0052/0007,"));
    assert!(receipt.contains("tail-read:1,5,1,5:4e2d/0207,"));
    assert!(receipt.contains("lead-read:0,5,0,5:4e2d/0107,"));
    assert!(receipt.contains("outside-read:0,87,20,20,19,19;true"));
    if !rust && build == "build=20348" {
        assert!(receipt.contains("outside-write:1,0,20,20,21,21"));
    } else {
        assert!(receipt.contains("outside-write:1,0,20,20,19,19"));
    }
    assert!(receipt.contains("corner-write:15,7,15,7\ncorner-read:15,7,15,7:0021/0005,"));
    assert!(receipt.contains("access:0,5,0,5\n"));
    assert!(receipt.contains("font-consistent:true\n"));
    assert!(receipt.contains("font-invalid:0,0,87\n"));
    if rust {
        assert!(receipt.contains("font:0,8,16,36,400,MiniCon Virtual\n"));
    }
    assert!(receipt.ends_with("preserved:7,6;002e\nresumed:8,6;0024\n"));
    assert_eq!(
        parser.screen().contents(),
        "KL\n\n  abc\n  def\nA│B\n中QR\n       Z\n               !"
    );
    assert_eq!(
        parser.screen().cell(2, 2).unwrap().fgcolor(),
        vt100::Color::Idx(11)
    );
    assert_eq!(
        parser.screen().cell(2, 2).unwrap().bgcolor(),
        vt100::Color::Idx(4)
    );
    assert!(parser.screen().cell(5, 0).unwrap().is_wide());
    assert!(parser.screen().cell(5, 1).unwrap().is_wide_continuation());
    assert_eq!(
        parser.screen().cell(6, 7).unwrap().fgcolor(),
        vt100::Color::Idx(1)
    );
    assert_eq!(
        parser.screen().cell(6, 7).unwrap().bgcolor(),
        vt100::Color::Idx(2)
    );
}

#[test]
fn rust_console_pty_rectangles_preserve_clipping_styles_and_client_decoder() {
    let (receipt, parser) = rect_output_report(true);
    assert_rect_output(&receipt, &parser, true);
}

#[test]
#[ignore = "explicit system ConPTY rectangle differential court"]
fn rust_console_client_rectangles_match_system_conpty() {
    let (receipt, parser) = rect_output_report(false);
    assert_rect_output(&receipt, &parser, false);
}

fn replacement_scalar_report(rust: bool) -> (String, vt100::Parser) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(if rust {
            "unicode-output-owned"
        } else {
            "unicode-output"
        })
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("unicode-end:");
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    (std::fs::read_to_string(&report.0).unwrap(), parser)
}

const REPLACEMENT_RECEIPT: &str =
    "wide:3,0;0007\nutf8:6,0;0007\nfill:7,0;0007\nrow0:0041,fffd,0042,0043,fffd,0044,0058,fffd,\n";

#[test]
fn rust_console_pty_replacement_scalars_preserve_native_and_consumed_cells() {
    let (receipt, parser) = replacement_scalar_report(true);
    assert_eq!(
        receipt,
        format!(
            "{REPLACEMENT_RECEIPT}invalid-wide:4,2;0007\ninvalid-utf8:8,2;0007\nrow2:fffd,0045,fffd,0046,fffd,0047,fffd,0048,\n"
        )
    );
    assert_eq!(parser.screen().contents(), "A�BC�DX�\n\n�E�F�G�H");
    assert_eq!(parser.screen().cursor_position(), (2, 8));
}

#[test]
#[ignore = "explicit system ConPTY literal replacement-scalar differential court"]
fn rust_console_client_literal_replacement_scalars_match_system_conpty() {
    let (receipt, parser) = replacement_scalar_report(false);
    assert_eq!(receipt, REPLACEMENT_RECEIPT);
    assert_eq!(parser.screen().contents(), "A�BC�DX�");
    assert_eq!(parser.screen().cursor_position(), (0, 7));
}

fn echo_output_report(rust: bool) -> (String, vt100::Parser) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(if rust {
            "echo-output-owned"
        } else {
            "echo-output"
        })
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("echo-end:");
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    eprintln!(
        "echo rust={rust}: {receipt:?}; screen={:?}",
        parser.screen().contents()
    );
    (receipt, parser)
}

const ECHO_OUTPUT_RECEIPT: &str = "csi-read:0065,000d,000a,\ncsi-echo:0,1;0007\ncsi-finish:1,1;0004\nutf8-read:0052,000d,000a,\nutf8-echo:0,2;0004\nutf8-finish:2,2;0004\nosc-read:4e2d,d83d,de00,000d,000a,\nosc-echo:0,3;0004\nosc-finish:0,3;0004\n";

#[test]
fn rust_console_pty_cooked_echo_preserves_unfinished_client_output() {
    let (receipt, parser) = echo_output_report(true);
    assert_eq!(
        receipt,
        format!(
            "{ECHO_OUTPUT_RECEIPT}wide-read:0056,000d,000a,\nwide-echo:0,4;0004\nwide-finish:2,4;0004\n"
        )
    );
    assert_eq!(parser.screen().contents(), "e\nXR\n中中😀\nV\n😀");
    assert_eq!(parser.screen().cursor_position(), (4, 2));
    assert_eq!(
        parser.screen().cell(1, 0).unwrap().fgcolor(),
        vt100::Color::Idx(1)
    );
}

#[test]
#[ignore = "explicit system ConPTY cooked-echo differential court"]
fn rust_console_client_cooked_echo_matches_system_conpty() {
    let (receipt, _) = echo_output_report(false);
    assert_eq!(receipt, ECHO_OUTPUT_RECEIPT);
}

fn mixed_output_report(rust: bool, budget: bool) -> (String, vt100::Parser, Vec<u8>) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(if budget {
            "mixed-output-budget"
        } else {
            "mixed-output"
        })
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("mixed-end:");
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    (receipt, parser, std::mem::take(&mut fixture.bytes))
}

const MIXED_OUTPUT_RECEIPT: &str = "csi:1,0;0004\nutf8:4,1;002e\ncontrol:4,2;002e\nstrings:11,4;001f\nscroll:1,0;001f\ncell:0,0:004b\ncell:2,0:0042\ncell:0,1:0046\ncell:4,2:0058\ncell:5,3:0043\ncell:6,3:0052\ncell:7,3:0054\ncell:10,3:0051\ncell:0,7:004a\n";

fn assert_mixed_output(receipt: &str, parser: &vt100::Parser, bytes: &[u8]) {
    assert_eq!(receipt.split_once('\n').unwrap().1, MIXED_OUTPUT_RECEIPT);
    assert_eq!(
        parser
            .screen()
            .rows(0, 16)
            .map(|row| row.trim_end().to_owned())
            .collect::<Vec<_>>(),
        ["K B", "F", "    X", "     CRT😀Q", "", "", "", "J"]
    );
    assert_eq!(parser.screen().cursor_position(), (0, 1));
    assert!(
        bytes
            .windows(b"\x1b]2;native-title\x07".len())
            .any(|window| window == b"\x1b]2;native-title\x07")
    );
}

#[test]
fn rust_console_pty_mixed_output_preserves_client_fragments_and_immediate_controls() {
    let (receipt, parser, bytes) = mixed_output_report(true, false);
    assert_mixed_output(&receipt, &parser, &bytes);
}

#[test]
#[ignore = "explicit system ConPTY mixed-output differential court"]
fn rust_console_client_mixed_output_matches_system_conpty() {
    let (reference, _, _) = mixed_output_report(false, false);
    eprintln!("mixed output reference:\n{reference}");
    let (owned, parser, bytes) = mixed_output_report(true, false);
    assert_mixed_output(&owned, &parser, &bytes);
    let reference_body = reference.split_once('\n').unwrap().1;
    if reference.starts_with("build=20348\n") {
        // The measured old host stores CAN as a literal cell, shifting later
        // text by one. Preserve the exact observation rather than adopting it.
        let expected = MIXED_OUTPUT_RECEIPT
            .replace("strings:11,4", "strings:12,4")
            .replace("cell:6,3:0052", "cell:6,3:0018")
            .replace("cell:7,3:0054", "cell:7,3:0052")
            .replace("cell:10,3:0051", "cell:10,3:0020");
        assert_eq!(reference_body, expected);
    } else {
        assert_eq!(reference_body, MIXED_OUTPUT_RECEIPT);
    }
}

#[test]
fn rust_console_pty_mixed_output_budget_rejects_raw_and_wide_then_recovers() {
    let (receipt, parser, _) = mixed_output_report(true, true);
    assert_eq!(
        receipt.split_once('\n').unwrap().1,
        "raw-limit:0,1450,0\nwide-limit:0,1450\nrecovery:9,0;0007\n"
    );
    assert_eq!(parser.screen().contents(), "RECOVEROK");
}

#[test]
fn rust_console_pty_mixed_output_delivers_accepted_incomplete_tail_before_eof() {
    for (name, tail) in [
        ("csi", b"\x1b[3".as_slice()),
        ("utf8", [0xe4, 0xb8].as_slice()),
    ] {
        let report = Report::new();
        let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg(format!("mixed-output-tail-{name}"))
            .arg(report.0.as_os_str())
            .rust_console();
        let mut fixture = Fixture::open(command);
        fixture.finish(0);
        assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "accepted");
        assert!(
            fixture.bytes.ends_with(tail),
            "accepted {name} tail must precede EOF: {:?}",
            fixture.bytes
        );
    }
}

fn screen_fill_report(rust: bool) -> (String, vt100::Parser) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("screen-fill")
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 12 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("fill-end:");
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    let mut parser = vt100::Parser::new(8, 12, 0);
    if rust {
        assert!(
            !fixture.bytes.windows(2).any(|bytes| bytes == b"\x1b\x1b"),
            "literal ESC cells must not enter the VT grammar"
        );
    }
    parser.process(&fixture.bytes);
    (std::fs::read_to_string(&report.0).unwrap(), parser)
}

#[test]
fn rust_console_pty_native_fills_preserve_cells_styles_cursor_and_projection() {
    let (receipt, parser) = screen_fill_report(true);
    assert_eq!(
        receipt.split_once('\n').unwrap().1,
        include_str!("support/screen_fill.txt")
    );
    assert_eq!(
        parser
            .screen()
            .rows(0, 12)
            .map(|row| row.trim_end().to_owned())
            .collect::<Vec<_>>(),
        [
            "abcdefghijZZ",
            "Z Q中中中中x",
            "    ││ ◌◌�",
            "",
            "中中中",
            "",
            "",
            "           !"
        ]
    );
    assert_eq!(parser.screen().cursor_position(), (3, 5));
    assert_eq!(parser.screen().fgcolor(), vt100::Color::Idx(13));
    assert_eq!(parser.screen().bgcolor(), vt100::Color::Idx(2));
    assert_eq!(
        parser.screen().cell(0, 11).unwrap().fgcolor(),
        vt100::Color::Idx(15)
    );
    assert_eq!(
        parser.screen().cell(0, 11).unwrap().bgcolor(),
        vt100::Color::Idx(1)
    );
}

#[test]
fn rust_console_pty_cmd_cls_clears_screen_and_keeps_session_usable() {
    let mut fixture = Fixture::spawn("echo BEFORECLS&cls&echo AFTERCLS", |command| {
        command.size(TerminalSize { rows: 6, cols: 20 })
    });
    fixture.finish(0);
    let mut parser = vt100::Parser::new(6, 20, 0);
    parser.process(&fixture.bytes);
    assert!(!parser.screen().contents().contains("BEFORECLS"));
    assert!(parser.screen().contents().contains("AFTERCLS"));
}

#[test]
#[ignore = "explicit system ConPTY native fill differential court"]
fn rust_console_client_native_fills_match_system_conpty() {
    let (reference, _) = screen_fill_report(false);
    std::fs::write(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/conhost-research/screen-fill-reference.txt"),
        &reference,
    )
    .unwrap();
    let (owned, _) = screen_fill_report(true);
    std::fs::write(
        PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../target/conhost-research/screen-fill-owned.txt"),
        &owned,
    )
    .unwrap();
    let (_, reference_body) = reference.split_once('\n').unwrap();
    let (_, owned_body) = owned.split_once('\n').unwrap();
    assert_eq!(owned_body, include_str!("support/screen_fill.txt"));
    let normalized = if reference.starts_with("build=20348\n") {
        // This old host retains an orphaned leading half after overwriting
        // its trailing cell. Our cell edit heals the pair instead.
        reference_body.replace(
            "row:1:[005a, 4e2d, 0051, 4e2d, 4e2d, 4e2d, 4e2d, 0078]:[004f, 014f,",
            "row:1:[005a, 0020, 0051, 4e2d, 4e2d, 4e2d, 4e2d, 0078]:[004f, 004f,",
        )
    } else {
        reference_body.to_owned()
    };
    assert_eq!(owned_body, normalized);
}

fn screen_readback_report(rust: bool, complex: bool) -> String {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(match (rust, complex) {
            (true, true) => "screen-readback-complex-owned",
            (true, false) => "screen-readback-owned",
            (false, true) => "screen-readback-complex",
            (false, false) => "screen-readback",
        })
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 12 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.finish(0);
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    if rust {
        assert!(receipt.contains("\nbudget:0,50,0\n"), "{receipt}");
        receipt.replace("budget:0,50,0\n", "")
    } else {
        receipt
    }
}

fn assert_screen_readback(report: &str, complex: bool) {
    let (version, body) = report.split_once('\n').unwrap();
    let build: u32 = version.strip_prefix("build=").unwrap().parse().unwrap();
    assert!(build > 0);
    let expected = if complex {
        include_str!("support/screen_readback-complex.txt")
    } else {
        include_str!("support/screen_readback-basic.txt")
    };
    assert_eq!(body, expected);
}

#[test]
fn rust_console_pty_screen_readback_uses_owned_cells_and_access() {
    for complex in [false, true] {
        assert_screen_readback(&screen_readback_report(true, complex), complex);
    }
}

#[test]
#[ignore = "explicit system ConPTY screen readback differential court"]
fn rust_console_client_screen_readback_matches_system_conpty() {
    let reference = screen_readback_report(false, false);
    eprintln!("native screen readback reference:\n{reference}");
    assert_screen_readback(&reference, false);
    let owned = screen_readback_report(true, false);
    assert_screen_readback(&owned, false);
    assert_eq!(owned, reference);
    let complex_reference = screen_readback_report(false, true);
    eprintln!("native complex screen readback reference:\n{complex_reference}");
    if complex_reference.starts_with("build=20348\n") {
        assert!(complex_reference.contains(
            "W:0,0,12:1,0,11:[0041, 4e2d, d83d, de00, 0065, 0301, 005a, 0020, 0020, 0020, 0020]\n"
        ));
        assert!(complex_reference.contains("T:0,0,12:1,0,12:[001e, 011e, 021e, 011e, 021e, 001e, 001e, 001e, 0007, 0007, 0007, 0007]\n"));
    } else {
        assert_screen_readback(&complex_reference, true);
    }
    assert_screen_readback(&screen_readback_report(true, true), true);
}

fn text_attribute_report(rust: bool) -> (String, vt100::Parser) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(if rust {
            "text-attributes-owned"
        } else {
            "text-attributes"
        })
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 20, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("attributes-end:");
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    let mut parser = vt100::Parser::new(20, 16, 0);
    parser.process(&fixture.bytes);
    (std::fs::read_to_string(&report.0).unwrap(), parser)
}

fn assert_text_attributes(receipt: &str, parser: &vt100::Parser, system: bool) {
    let (build, receipt) = receipt.split_once(';').unwrap();
    let build: u32 = build.strip_prefix("build=").unwrap().parse().unwrap();
    assert!(build > 0);
    let mut expected = String::new();
    for (index, attribute) in (0..256_u16).chain([0x401e, 0x802f, 0xc03a]).enumerate() {
        expected.push_str(&format!("{attribute:04x},"));
        let cell = parser
            .screen()
            .cell(index as u16 / 16, index as u16 % 16)
            .unwrap();
        assert_eq!(cell.contents(), "X", "attribute {attribute:04x}");
        // Windows attributes use BGR bits; the VT palette uses RGB bits.
        let color =
            |n: u16| vt100::Color::Idx(((n & 8) | ((n & 1) << 2) | (n & 2) | ((n & 4) >> 2)) as u8);
        let normalize = |value, default, bold| match value {
            vt100::Color::Default => vt100::Color::Idx(default),
            vt100::Color::Idx(index @ 0..=7) if bold => vt100::Color::Idx(index | 8),
            value => value,
        };
        let (mut foreground, mut background) = (cell.fgcolor(), cell.bgcolor());
        let (mut expected_foreground, mut expected_background) =
            (color(attribute & 15), color((attribute >> 4) & 15));
        if system {
            foreground = normalize(foreground, 7, cell.bold());
            background = normalize(background, 0, false);
            if cell.inverse() {
                std::mem::swap(&mut foreground, &mut background);
            }
            if attribute & 0x4000 != 0 {
                std::mem::swap(&mut expected_foreground, &mut expected_background);
            }
        } else {
            assert_eq!(cell.inverse(), attribute & 0x4000 != 0);
        }
        assert_eq!(
            foreground, expected_foreground,
            "foreground {attribute:04x}"
        );
        assert_eq!(
            background, expected_background,
            "background {attribute:04x}"
        );
        let expected_underline = attribute & 0x8000 != 0 && !(system && build == 20348);
        assert_eq!(
            cell.underline(),
            expected_underline,
            "underline {attribute:04x}, system={system}, build={build}"
        );
    }
    expected.push_str(if system && build == 20348 {
        "vt=4069"
    } else {
        "vt=c069"
    });
    assert_eq!(receipt, expected);
    assert_eq!(parser.screen().cell(16, 3).unwrap().contents(), "Y");
}

#[test]
fn rust_console_pty_native_text_attributes_share_query_and_projected_state() {
    let (receipt, parser) = text_attribute_report(true);
    assert_text_attributes(&receipt, &parser, false);
}

#[test]
#[ignore = "explicit system ConPTY drawing-attribute differential court"]
fn rust_console_client_native_text_attributes_match_system_conpty() {
    let (reference, reference_screen) = text_attribute_report(false);
    assert_text_attributes(&reference, &reference_screen, true);
    let (owned, owned_screen) = text_attribute_report(true);
    assert_text_attributes(&owned, &owned_screen, false);
    eprintln!(
        "drawing attributes: reference {} / owned {}",
        reference.rsplit_once(',').unwrap().1,
        owned.rsplit_once(',').unwrap().1
    );
    if reference.starts_with("build=20348;") {
        // This measured host drops underline in its wire and VT query. Native
        // SetConsoleTextAttribute queries above still retain the requested bit.
        assert_eq!(owned.replace("vt=c069", "vt=4069"), reference);
    } else {
        assert_eq!(owned, reference);
    }
}

fn output_mode_report(mode: u32, rust: bool) -> (u32, String, Vec<String>) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(format!("output-modes-{mode}"))
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    // System-HPC shutdown deliberately discards remaining producer output.
    // Observe the complete projection while its controlled client is alive.
    fixture.until("mode-projection-end:");
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    let full_receipt = std::fs::read_to_string(&report.0).unwrap();
    let (build, receipt) = full_receipt.split_once(';').unwrap();
    let build = build
        .strip_prefix("build=")
        .unwrap()
        .parse::<u32>()
        .unwrap();
    assert!(build > 0);
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    let rows = parser
        .screen()
        .rows(0, 16)
        .map(|row| row.trim_end().to_owned())
        .collect();
    (build, receipt.to_owned(), rows)
}

fn expected_output_mode_cursor(mode: u32, cursor: &str) -> String {
    let mut expected = format!("initial=7;mode={mode}{cursor}");
    if mode & 5 == 5 {
        expected.push_str(&format!("|wrap={}|wrap={}", mode & !2, mode | 2));
    }
    expected
}

#[test]
fn rust_console_pty_output_modes_keep_controls_margins_and_projection_coherent() {
    for (mode, cursor) in OUTPUT_MODES {
        let (_, receipt, rows) = output_mode_report(mode, true);
        assert_eq!(receipt, expected_output_mode_cursor(mode, cursor));
        let processed = mode & 1 != 0;
        let vt = mode & 5 == 5;
        let wrap = mode & 2 != 0;
        let expected = [
            if vt {
                ""
            } else if processed {
                "←[2;5H!"
            } else {
                " [2;5H!"
            },
            if vt {
                " A  !"
            } else if processed {
                " A"
            } else {
                " A B"
            },
            if !processed {
                ""
            } else if mode & 8 != 0 {
                "  B"
            } else {
                "B"
            },
            if processed { " C      D" } else { " C D" },
            if processed {
                "F    E    H"
            } else {
                "     E F  G H"
            },
            if wrap {
                "              XY"
            } else {
                "              XZ"
            },
            if wrap {
                "Z              A"
            } else {
                "               B"
            },
            if wrap { "B" } else { "" },
        ];
        assert_eq!(
            rows, expected,
            "mode {mode}: owned projection must follow buffer semantics"
        );
    }
}

#[test]
#[ignore = "explicit system ConPTY output-mode differential court"]
fn rust_console_client_output_mode_cursor_and_qualified_projection_match_system_conpty() {
    for (mode, cursor) in OUTPUT_MODES {
        let (build, reference, reference_rows) = output_mode_report(mode, false);
        let (owned_build, owned, owned_rows) = output_mode_report(mode, true);
        assert_eq!(owned_build, build);
        assert_eq!(reference, expected_output_mode_cursor(mode, cursor));
        assert_eq!(owned, reference, "native SDK cursor mode {mode}");
        // 20348 forwards unprocessed C0/ESC as active VT; its legacy no-wrap
        // stream also exposes chunk-dependent overwrite at the right edge.
        // Keep those observations visible, rather than adopting them as the
        // owned buffer/projection contract. See PRD 02.34's output-mode court.
        eprintln!(
            "output-mode build={build} mode={mode}: reference={reference_rows:?}; owned={owned_rows:?}"
        );
        if build != 20348 || matches!(mode, 3 | 5 | 7 | 11 | 13 | 15) {
            assert_eq!(
                owned_rows, reference_rows,
                "qualified projected rows mode {mode}"
            );
        }
    }
}

#[test]
fn rust_console_pty_output_pressure_keeps_controls_and_native_queries_live_and_recovers_in_order() {
    for (mode, expected) in [
        ("control-output-c", "1,7,7"),
        ("control-output-break-raw", "2,0,0"),
        ("control-output-budget", "1,7,7,2"),
        ("control-output-read", "1,7,7,true,995,0"),
    ] {
        let report = Report::new();
        let (master, child) =
            ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg(mode)
                .arg(report.0.as_os_str())
                .rust_console()
                .spawn()
                .unwrap()
                .into_parts();
        report.wait_ready();
        assert_eq!(
            std::fs::read_to_string(report.0.with_extension("ready")).unwrap(),
            "65536"
        );
        let started = Instant::now();
        if mode.ends_with("-read") {
            // Give the native thread which published readiness time to enter
            // its synchronous read. The cancellation mutation must strand it.
            std::thread::sleep(Duration::from_millis(100));
        }
        if !mode.contains("break-raw") {
            master.write_all(&[3]).unwrap();
        } else {
            child.send_native_key(NativeTerminalKey::Break, 1).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(3);
        while !report.0.with_extension("go").exists() {
            assert!(
                Instant::now() < deadline,
                "native control/query stranded behind output"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(started.elapsed() < Duration::from_secs(3));
        assert_eq!(
            std::fs::read_to_string(report.0.with_extension("go")).unwrap(),
            expected
        );
        // Release exactly one 4 KiB prefix, then hold the reader until two
        // native query round trips establish a partial staging flush.
        let mut prefix = vec![0; 4096];
        assert_eq!(master.io().read(&mut prefix).unwrap(), prefix.len());
        std::fs::write(report.0.with_extension("opened"), b"drain").unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        while !report.0.with_extension("opened-ready").exists() {
            assert!(
                Instant::now() < deadline,
                "partial flush/query did not progress"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let mut fixture = Fixture::with_parts(master, child);
        fixture.bytes.extend_from_slice(&prefix);
        fixture.finish(0);
        assert_eq!(std::fs::read_to_string(&report.0).unwrap(), expected);
        let chunks = if mode.ends_with("-budget") {
            32 + 63
        } else {
            32
        };
        assert!(
            fixture.bytes == expected_pressure_output(chunks, !mode.ends_with("-budget")),
            "output recovery changed ordered chunks ({} bytes)",
            fixture.bytes.len()
        );
    }
}

#[test]
fn rust_console_pty_output_tail_survives_root_exit_before_the_first_consumer_read() {
    let report = Report::new();
    let (mut master, child) =
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("control-output-tail")
            .arg(report.0.as_os_str())
            .rust_console()
            .spawn()
            .unwrap()
            .into_parts();
    report.wait_ready();
    let mut waiter = child.try_clone_for_wait().unwrap();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(waiter.wait());
    });
    assert_eq!(
        receive
            .recv_timeout(Duration::from_secs(3))
            .unwrap()
            .unwrap()
            .code(),
        Some(0)
    );
    assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "tail");
    let writer = master.try_clone_for_startup_reader().unwrap();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        loop {
            if let Err(error) = writer.write_all(b"late-input") {
                let _ = send.send(error.kind());
                break;
            }
        }
    });
    assert_eq!(
        receive.recv_timeout(Duration::from_millis(500)).unwrap(),
        std::io::ErrorKind::BrokenPipe,
        "root exit must close input even while output awaits its first reader"
    );
    let mut fixture = Fixture::with_parts(master, child);
    fixture.finish(0);
    assert!(
        fixture.bytes == expected_pressure_output(8, true),
        "output tail changed ordered chunks ({} bytes)",
        fixture.bytes.len()
    );
}

#[test]
fn rust_console_pty_output_eof_waits_for_unattached_private_job_members() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("control-output-tail-orphan")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.wait_exit(0);
    let deadline = Instant::now() + Duration::from_secs(3);
    let expected = expected_pressure_output(8, true);
    while fixture.bytes.len() < expected.len() {
        let chunk = fixture
            .output
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .unwrap()
            .unwrap();
        fixture.bytes.extend(chunk);
    }
    assert!(
        matches!(
            fixture.output.recv_timeout(Duration::from_millis(100)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "a live unattached Job member must keep EOF pending"
    );
    assert!(!report.0.with_extension("orphan").exists());
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    assert_eq!(
        std::fs::read_to_string(report.0.with_extension("orphan")).unwrap(),
        "finished"
    );
    assert_eq!(fixture.bytes, expected);
}

fn expected_pressure_output(chunks: usize, vary: bool) -> Vec<u8> {
    let mut bytes = format!(
        "\x1b]0;{}\x1b\\",
        env!("CARGO_BIN_EXE_minicon-platform-console-probe")
    )
    .into_bytes();
    bytes.extend((0..chunks).flat_map(|index| {
        (0..1024).flat_map(move |position| {
            let prefix = if vary {
                b'A' + ((index * 7 + position) % 26) as u8
            } else {
                b'a'
            };
            format!("{}中😀", char::from(prefix)).into_bytes()
        })
    }));
    bytes
}

#[test]
fn rust_console_pty_processed_ctrl_c_survives_a_large_mixed_write_under_backpressure() {
    // Exercise controls still in the caller, in queued frames, and in the
    // worker's current frame after the native event queue is already full.
    for size in [1_000_000, 8_000, 100] {
        let report = Report::new();
        let mut fixture = Fixture::open(
            ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg("control-full-queue")
                .arg(report.0.as_os_str())
                .rust_console(),
        );
        fixture.until("control-probe-ready:");
        if size != 1_000_000 {
            fixture.master.write_all(&vec![b'x'; 4096]).unwrap();
            report.wait_ready();
        }
        let mut input = vec![b'x'; size];
        input.push(3);
        let started = Instant::now();
        let _write = fixture.master.write_all(&input);
        fixture.finish(0);
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "control was stranded behind ordinary input"
        );
        assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "4096,4096|1");
    }
}

#[test]
fn rust_console_pty_raw_ctrl_c_preserves_large_mixed_input_order_under_backpressure() {
    for separate_control in [false, true] {
        let report = Report::new();
        let mut fixture = Fixture::open(
            ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg("control-pressure-raw")
                .arg(report.0.as_os_str())
                .rust_console(),
        );
        fixture.until("control-probe-ready:");
        // A mode query round trip after readiness establishes raw ownership before
        // the writer can request any control priority.
        let deadline = Instant::now() + Duration::from_secs(8);
        while fixture.child.native_input_ownership().unwrap() != NativeInputOwnership::RawNative {
            assert!(Instant::now() < deadline);
            std::thread::sleep(Duration::from_millis(2));
        }
        let writer = fixture.master.try_clone_for_startup_reader().unwrap();
        let (send, receive) = mpsc::channel();
        std::thread::spawn(move || {
            let mut bytes = vec![b'x'; 80_000];
            if !separate_control {
                bytes.push(3);
            }
            let _ = send.send(writer.write_all(&bytes));
        });
        report.wait_ready();
        assert!(
            matches!(
                receive.recv_timeout(Duration::from_millis(100)),
                Err(mpsc::RecvTimeoutError::Timeout)
            ),
            "write must actually encounter backpressure"
        );
        std::fs::write(report.0.with_extension("go"), b"read").unwrap();
        if separate_control {
            fixture.master.write_all(&[3]).unwrap();
        }
        receive
            .recv_timeout(Duration::from_secs(15))
            .unwrap()
            .unwrap();
        fixture.finish(0);
        assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "80001,0,0");
    }
}

#[test]
fn rust_console_pty_processed_mixed_write_delivers_one_control_and_preserves_all_normal_input() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("control-pressure-processed")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.until("control-probe-ready:");
    let mut bytes = vec![b'x'; 80_000];
    bytes.push(3);
    fixture.master.write_all(&bytes).unwrap();
    fixture.finish(0);
    assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "80000,1,1");
}

#[test]
fn rust_console_pty_single_ctrl_c_overtakes_another_threads_blocked_input_write() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("control-full-queue")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.until("control-probe-ready:");
    let writer = fixture.master.try_clone_for_startup_reader().unwrap();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(writer.write_all(&vec![b'x'; 1_000_000]));
    });
    report.wait_ready();
    let started = Instant::now();
    fixture.master.write_all(&[3]).unwrap();
    let error = receive
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    fixture.finish(0);
    assert!(started.elapsed() < Duration::from_secs(3));
    assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "4096,4096|1");
}

#[test]
fn rust_console_pty_break_cancels_inflight_write_and_flushes_transport_backlog() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("control-break-raw-queue-hold")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.until("control-probe-ready:");
    let writer = fixture.master.try_clone_for_startup_reader().unwrap();
    let (send, receive) = mpsc::channel();
    std::thread::spawn(move || {
        let _ = send.send(writer.write_all(&vec![b'x'; 1_000_000]));
    });
    report.wait_ready();
    fixture
        .child
        .send_native_key(NativeTerminalKey::Break, 1)
        .unwrap();
    let error = receive
        .recv_timeout(Duration::from_secs(3))
        .unwrap()
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "4096,0,258|2");
}

#[test]
fn rust_console_pty_break_flushes_full_raw_and_cooked_queues_and_resets_native_ready() {
    for mode in ["control-break-queue", "control-break-raw-queue"] {
        let report = Report::new();
        let mut fixture = Fixture::open(
            ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg(mode)
                .arg(report.0.as_os_str())
                .rust_console(),
        );
        fixture.until("control-probe-ready:");
        assert!(
            fixture
                .child
                .send_native_key(NativeTerminalKey::Break, 0)
                .is_err()
        );
        fixture.master.write_all(&[b'x'; 4096]).unwrap();
        report.wait_ready();
        fixture
            .child
            .send_native_key(NativeTerminalKey::Break, 1)
            .unwrap();
        fixture.finish(0);
        assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "4096,0,258|2");
    }
}

#[test]
fn rust_console_pty_break_interrupts_cooked_and_raw_native_reads() {
    for mode in ["control-break-read", "control-break-raw-read"] {
        let report = Report::new();
        let mut fixture = Fixture::open(
            ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg(mode)
                .arg(report.0.as_os_str())
                .rust_console(),
        );
        fixture.until("control-probe-ready:");
        if mode == "control-break-read" {
            fixture.master.write_all(b"partial").unwrap();
            fixture.until("partial");
        } else {
            std::thread::sleep(Duration::from_millis(60));
        }
        fixture
            .child
            .send_native_key(NativeTerminalKey::Break, 1)
            .unwrap();
        fixture.finish(0);
        assert_eq!(
            std::fs::read_to_string(&report.0).unwrap(),
            "true,995,0,0|2"
        );
    }
}

#[test]
fn rust_console_pty_break_resets_ready_with_live_file_conversion_tails() {
    for (mode, text, expected) in [
        ("control-break-tail", "中", "228,0,0,258|2"),
        ("control-break-wide-tail", "AB\r", "65,0,0,258|2"),
    ] {
        let report = Report::new();
        let mut fixture = Fixture::open(
            ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg(mode)
                .arg(report.0.as_os_str())
                .rust_console(),
        );
        fixture.until("break-tail-send:");
        fixture.master.write_all(text.as_bytes()).unwrap();
        report.wait_ready();
        fixture
            .child
            .send_native_key(NativeTerminalKey::Break, 1)
            .unwrap();
        fixture.finish(0);
        assert_eq!(
            std::fs::read_to_string(&report.0).unwrap(),
            expected,
            "{mode}"
        );
    }
}

#[test]
#[ignore = "explicit system ConPTY unsupported native Break transport court"]
fn rust_console_client_unqualified_native_break_transport_rejects_without_delivery() {
    let report = Report::new();
    let mut fixture = Fixture::open_backend(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("control-handler")
            .arg(report.0.as_os_str()),
        "conpty",
    );
    fixture.until("control-probe-ready:");
    assert!(matches!(
        fixture.child.send_native_key(NativeTerminalKey::Break, 1),
        Err(minicon_platform::pty::PtyError::Unsupported { .. })
    ));
    fixture.master.write_all(&[3]).unwrap();
    fixture.finish(0);
    assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "|1");
}

#[test]
#[ignore = "explicit system ConPTY Win32-input control sequence court"]
fn rust_console_client_system_conpty_accepts_win32_break_sequence() {
    let report = Report::new();
    let mut fixture = Fixture::open_backend(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("control-handler")
            .arg(report.0.as_os_str()),
        "conpty",
    );
    fixture.until("control-probe-ready:");
    fixture.master.write_all(b"\x1b[3;70;0;1;264;1_").unwrap();
    fixture.finish(0);
    assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "|2");
}

fn break_tail_observation(mode: &str, rust: bool) -> String {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(mode)
        .arg(report.0.as_os_str());
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("break-tail-send:");
    fixture
        .master
        .write_all(if mode.contains("wide-tail") {
            b"AB\r"
        } else {
            "中".as_bytes()
        })
        .unwrap();
    report.wait_ready();
    if rust {
        fixture
            .child
            .send_native_key(NativeTerminalKey::Break, 1)
            .unwrap();
    } else {
        fixture.master.write_all(b"\x1b[3;70;0;1;264;1_").unwrap();
    }
    let deadline = Instant::now() + Duration::from_secs(8);
    while !report.0.with_extension("go").exists() {
        assert!(
            Instant::now() < deadline,
            "Break did not reach observation barrier"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    fixture.master.write_all(b"Z\r").unwrap();
    std::fs::write(report.0.with_extension("opened"), b"input-sent").unwrap();
    fixture.finish(0);
    std::fs::read_to_string(&report.0).unwrap()
}

#[test]
#[ignore = "explicit system ConPTY Break tail/readback differential court"]
fn rust_console_client_break_tail_readback_matches_system_conpty() {
    let mut reference = Vec::new();
    let mut owned = Vec::new();
    for mode in [
        "control-break-tail-observe",
        "control-break-wide-tail-observe",
    ] {
        reference.push(break_tail_observation(mode, false));
        owned.push(break_tail_observation(mode, true));
        eprintln!(
            "{mode}: system={:?}; owned={:?}",
            reference.last(),
            owned.last()
        );
    }
    assert_eq!(owned[0], "228,0,0,258;read:90,13|2");
    assert_eq!(
        reference[0].split_once(',').unwrap().1,
        owned[0].split_once(',').unwrap().1
    );
    assert_eq!(owned[1], "65,0,0,258;read:3,66,13,10|2");
    assert_eq!(owned[1], reference[1]);
}

#[test]
fn rust_console_pty_unprocessed_ctrl_c_is_input_without_handler_delivery() {
    assert_eq!(control_receipt("control-raw", true), "true,0,1,3|0");
}

#[test]
fn rust_console_pty_native_generate_ctrl_event_supports_c_and_break_with_raw_input() {
    assert_eq!(control_receipt("control-generate-c", true), "true,0|1");
    assert_eq!(control_receipt("control-generate-break", true), "true,0|2");
}

#[test]
fn rust_console_pty_generated_break_targets_only_the_attached_child_process_group() {
    assert_eq!(
        control_receipt("control-group-parent", true),
        "true,0|child:|2|parent:0"
    );
}

#[test]
fn rust_console_pty_native_generate_ctrl_event_rejects_invalid_event_and_group() {
    assert_eq!(
        control_receipt("control-generate-invalid", true),
        "false,87|false,87|0"
    );
}

#[test]
#[ignore = "explicit system ConPTY native control generation differential court"]
fn rust_console_client_generate_ctrl_event_matches_system_conpty() {
    for (mode, expected) in [
        ("control-generate-c", "true,0|1"),
        ("control-generate-break", "true,0|2"),
        ("control-generate-invalid", "false,87|false,87|0"),
        ("control-group-parent", "true,0|child:|2|parent:0"),
    ] {
        let reference = control_receipt(mode, false);
        eprintln!("system ConPTY {mode}: {reference}");
        assert_eq!(reference, expected);
        assert_eq!(control_receipt(mode, true), reference);
    }
}

#[test]
#[ignore = "explicit system ConPTY control-handler differential court"]
fn rust_console_client_ctrl_c_matches_system_conpty() {
    for (mode, expected) in [
        ("control-handler", "|1"),
        ("control-handler-io", "|1"),
        ("control-read", "true,995,0,0|1"),
        ("control-raw", "true,0,1,3|0"),
    ] {
        let reference = control_receipt(mode, false);
        eprintln!("system ConPTY {mode}: {reference}");
        assert_eq!(reference, expected);
        assert_eq!(control_receipt(mode, true), reference);
    }
}

#[test]
fn rust_console_pty_ctrl_c_preserves_the_native_default_termination_status() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("control-default")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.until("control-probe-ready:");
    fixture.master.write_all(&[3]).unwrap();
    fixture.finish(0xc000013au32 as i32);
    assert!(!report.0.exists());
}

#[test]
fn rust_console_pty_ctrl_c_targets_only_its_owned_console() {
    let reports = [Report::new(), Report::new()];
    let mut fixtures: Vec<_> = reports
        .iter()
        .map(|report| {
            Fixture::open(
                ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                    .arg("control-handler")
                    .arg(report.0.as_os_str())
                    .rust_console(),
            )
        })
        .collect();
    for fixture in &mut fixtures {
        fixture.until("control-probe-ready:");
    }
    fixtures[0].master.write_all(&[3]).unwrap();
    fixtures[0].finish(0);
    assert_eq!(std::fs::read_to_string(&reports[0].0).unwrap(), "|1");
    assert!(
        !reports[1].0.exists(),
        "other private console received the control"
    );
    fixtures[1].master.write_all(&[3]).unwrap();
    fixtures[1].finish(0);
    assert_eq!(std::fs::read_to_string(&reports[1].0).unwrap(), "|1");
}

#[test]
fn rust_console_pty_inherited_input_output_survive_creator_exit_and_freeconsole() {
    for mode in ["inherit-parent-exit", "inherit-parent-free"] {
        let report = Report::new();
        let mut fixture = Fixture::open(
            ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg(mode)
                .arg(report.0.as_os_str())
                .rust_console(),
        );
        fixture.until("inherit-parent-ready:");
        fixture.wait_exit(0);
        if mode == "inherit-parent-free" {
            assert!(report.0.with_extension("detached").exists());
        }
        std::fs::write(report.0.with_extension("go"), b"creator-exited").unwrap();
        let deadline = Instant::now() + Duration::from_secs(8);
        while !report.0.with_extension("opened-ready").exists() {
            assert!(
                Instant::now() < deadline,
                "inherited child did not report native opens"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            std::fs::read_to_string(report.0.with_extension("opened")).unwrap(),
            "true,0,true,0",
            "{mode}"
        );
        fixture.until("inherit-child-ready:");
        fixture.master.write_all("中😀\r".as_bytes()).unwrap();
        fixture.finish(0);
        assert!(String::from_utf8_lossy(&fixture.bytes).contains("inherit-value:中😀"));
        let result = std::fs::read_to_string(&report.0).unwrap();
        assert_eq!(result, "true,0,5,19,中😀\r\n", "{mode}");
    }
}

#[test]
fn rust_console_pty_inherited_readonly_output_keeps_its_native_access_limit() {
    assert_eq!(inherited_readonly_report(true), "true,0,true,0|false,5,0");
}

#[test]
fn rust_console_pty_inherited_input_preserves_a_partial_utf8_byte_tail_after_parent_exit() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("inherit-parent-tail")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.until("inherit-tail-send:");
    fixture.master.write_all("中".as_bytes()).unwrap();
    fixture.wait_exit(0);
    assert_eq!(
        std::fs::read_to_string(report.0.with_extension("first")).unwrap(),
        "228"
    );
    std::fs::write(report.0.with_extension("go"), b"creator-exited").unwrap();
    fixture.finish(0);
    assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "184,173");
}

#[test]
fn rust_console_pty_final_file_close_discards_its_partial_byte_tail_and_input_signal() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("inherit-close-tail")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.until("inherit-tail-send:");
    fixture.master.write_all("中".as_bytes()).unwrap();
    fixture.finish(0);
    assert_eq!(std::fs::read_to_string(&report.0).unwrap(), "228,258,0");
}

fn inherited_readonly_report(rust: bool) -> String {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("inherit-parent-readonly")
        .arg(report.0.as_os_str());
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("inherit-parent-ready:");
    fixture.wait_exit(0);
    std::fs::write(report.0.with_extension("go"), b"creator-exited").unwrap();
    let deadline = Instant::now() + Duration::from_secs(8);
    while !report.0.exists() {
        assert!(
            Instant::now() < deadline,
            "inherited native result not published"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    fixture.finish(0);
    format!(
        "{}|{}",
        std::fs::read_to_string(report.0.with_extension("opened")).unwrap(),
        std::fs::read_to_string(&report.0).unwrap()
    )
}

#[test]
#[ignore = "explicit system ConPTY inheritance differential court"]
fn rust_console_client_inherited_restricted_handle_matches_system_conpty() {
    let reference = inherited_readonly_report(false);
    eprintln!("system ConPTY inherited readonly: {reference}");
    assert_eq!(reference, "true,0,true,0|false,5,0");
    let owned = inherited_readonly_report(true);
    assert_eq!(owned, reference);
}

#[test]
#[ignore = "explicit external Node inherited-stdio court; missing client is BLOCKED"]
fn rust_console_client_node_spawn_inherits_console_stdio() {
    let script = "const c=require('child_process').spawn(process.execPath,['-e',\"if(!process.stdout.isTTY)throw Error('TTY required');process.stdout.write('node-child-inherited\\\\n');process.exitCode=37;\"],{stdio:'inherit'});c.once('exit',code=>{process.stdout.write('node-parent-exit:'+code+'\\n');process.exitCode=code;});";
    let mut fixture = Fixture::open(
        ChildCommand::new(external_client("MINICON_TEST_NODE"))
            .arg("-e")
            .arg(script)
            .env("TERM", "xterm-256color")
            .rust_console(),
    );
    fixture.finish(37);
    let text = String::from_utf8_lossy(&fixture.bytes);
    assert!(text.contains("node-child-inherited"), "{text}");
    assert!(text.contains("node-parent-exit:37"), "{text}");
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

fn powershell_geometry(name: &str) {
    let script = "$ErrorActionPreference='Stop';[Console]::SetBufferSize(32,16);if([Console]::BufferWidth -ne 32 -or [Console]::BufferHeight -ne 16 -or [Console]::WindowWidth -ne 16 -or [Console]::WindowHeight -ne 8){exit 19};$raw=$Host.UI.RawUI;$cells=$raw.NewBufferCellArray([string[]]@('中Q'),[ConsoleColor]::Yellow,[ConsoleColor]::DarkBlue);$pos=New-Object System.Management.Automation.Host.Coordinates -ArgumentList 20,0;$raw.SetBufferContents($pos,$cells);$rect=New-Object System.Management.Automation.Host.Rectangle -ArgumentList 20,0,22,0;$read=$raw.GetBufferContents($rect);if($read[0,0].Character -cne '中' -or $read[0,2].Character -cne 'Q'){exit 20};[Console]::SetWindowSize(20,10);if([Console]::WindowWidth -ne 20 -or [Console]::WindowHeight -ne 10 -or [Console]::BufferWidth -ne 20 -or [Console]::BufferHeight -ne 10){exit 21};if([Console]::LargestWindowWidth -le 0 -or [Console]::LargestWindowHeight -le 0){exit 22};[Console]::SetWindowSize(16,8);[Console]::SetCursorPosition(0,0);[Console]::Write('geometry-app-ok:');exit 0";
    let mut fixture = Fixture::open(
        ChildCommand::new(external_client(name))
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(script)
            .size(TerminalSize { rows: 8, cols: 16 })
            .rust_console(),
    );
    fixture.until("geometry-app-ok:");
    fixture.finish(0);
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    assert_eq!(
        parser.screen().rows(0, 16).next().unwrap(),
        "geometry-app-ok:"
    );
}

#[test]
#[ignore = "explicit external Windows PowerShell geometry court; missing client is BLOCKED"]
fn rust_console_client_windows_powershell_native_geometry() {
    powershell_geometry("MINICON_TEST_WINDOWS_POWERSHELL");
}

#[test]
#[ignore = "explicit external PowerShell 7 geometry court; missing client is BLOCKED"]
fn rust_console_client_powershell7_native_geometry() {
    powershell_geometry("MINICON_TEST_PWSH");
}

fn powershell_unicode_batches(name: &str) {
    let script = "$ErrorActionPreference='Stop';[Console]::OutputEncoding=[Text.UTF8Encoding]::new();[Console]::SetBufferSize(256,64);$text=('Ae'+[char]0x301+'中😀BC')*2048;[Console]::Write($text);if([Console]::BufferWidth -ne 256 -or [Console]::BufferHeight -ne 64 -or [Console]::CursorLeft -ne 255 -or [Console]::CursorTop -ne 63 -or [Console]::WindowTop -ne 32){exit 19};[Console]::Write(([char]27)+']2;bulk-app-end:'+([char]7));exit 0";
    let mut fixture = Fixture::open(
        ChildCommand::new(external_client(name))
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(script)
            .size(TerminalSize {
                rows: 32,
                cols: 128,
            })
            .rust_console(),
    );
    fixture.until("bulk-app-end:");
    fixture.finish(0);
    let mut parser = vt100::Parser::new(32, 128, 0);
    parser.process(&fixture.bytes);
    let expected = "Ae\u{301}中😀BC".repeat(16);
    for (row, text) in parser.screen().rows(0, 128).enumerate() {
        assert_eq!(text, expected, "{name}, row {row}");
    }
    assert!(
        fixture.bytes.len() < 32768,
        "{name}: bulk output amplified to {} bytes",
        fixture.bytes.len()
    );
}

#[test]
#[ignore = "explicit Windows PowerShell bulk Unicode client; missing client is BLOCKED"]
fn rust_console_client_windows_powershell_unicode_batches() {
    powershell_unicode_batches("MINICON_TEST_WINDOWS_POWERSHELL");
}

#[test]
#[ignore = "explicit PowerShell 7 bulk Unicode client; missing client is BLOCKED"]
fn rust_console_client_powershell7_unicode_batches() {
    powershell_unicode_batches("MINICON_TEST_PWSH");
}

fn powershell_buffers(name: &str) {
    let report = Report::new();
    let ack = report
        .0
        .with_extension("opened")
        .to_string_lossy()
        .replace('\'', "''");
    let script=r#"$ErrorActionPreference='Stop';Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;
public static class BufferApi {
 [DllImport("kernel32.dll",SetLastError=true)] public static extern IntPtr GetStdHandle(int n);
 [DllImport("kernel32.dll",SetLastError=true)] public static extern IntPtr CreateConsoleScreenBuffer(uint access,uint share,IntPtr security,uint flags,IntPtr data);
 [DllImport("kernel32.dll",SetLastError=true)] public static extern bool SetConsoleActiveScreenBuffer(IntPtr h);
 [DllImport("kernel32.dll",SetLastError=true)] public static extern bool CloseHandle(IntPtr h);
 [DllImport("kernel32.dll",CharSet=CharSet.Unicode,SetLastError=true)] static extern bool WriteConsoleW(IntPtr h,string text,uint count,out uint written,IntPtr reserved);
 public static void Put(IntPtr h,string text) {uint written;if(!WriteConsoleW(h,text,(uint)text.Length,out written,IntPtr.Zero)||written!=text.Length)throw new Exception("native buffer write failed");}
}
'@;
[Console]::SetBufferSize(32,12);[Console]::Write('BASE');
$base=[BufferApi]::GetStdHandle(-11);
$alt=[BufferApi]::CreateConsoleScreenBuffer(3221225472,3,[IntPtr]::Zero,1,[IntPtr]::Zero);
if($alt -eq [IntPtr](-1)){throw 'create failed'};
[BufferApi]::Put($alt,'ALT中');if(![BufferApi]::SetConsoleActiveScreenBuffer($alt)){throw 'activate failed'};
if([Console]::BufferWidth -ne 32){exit 19};[Console]::CursorSize=43;
[Console]::SetCursorPosition(0,1);[Console]::Write('BASE-HIDDEN');
$area=New-Object System.Management.Automation.Host.Rectangle -ArgumentList 0,1,10,1;
$read=$Host.UI.RawUI.GetBufferContents($area);if($read[0,0].Character -cne 'B' -or $read[0,10].Character -cne 'N'){exit 20};
[BufferApi]::Put($alt,([char]27)+']2;buffers-app-alt:'+([char]7));
$deadline=[DateTime]::UtcNow.AddSeconds(8);while(!(Test-Path -LiteralPath '__ACK__')){if([DateTime]::UtcNow -gt $deadline){throw 'observation timeout'};Start-Sleep -Milliseconds 2};
if(![BufferApi]::SetConsoleActiveScreenBuffer($base)){throw 'restore failed'};if(![BufferApi]::CloseHandle($alt)){throw 'close failed'};
if([Console]::CursorSize -ne 43 -or [Console]::BufferWidth -ne 32){exit 21};
[Console]::SetCursorPosition(0,3);[Console]::Write('buffers-app-ok:');exit 0"#.replace("__ACK__",&ack);
    let mut fixture = Fixture::open(
        ChildCommand::new(external_client(name))
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(script)
            .size(TerminalSize { rows: 8, cols: 16 })
            .rust_console(),
    );
    fixture.until("buffers-app-alt:");
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    assert_eq!(parser.screen().rows(0, 16).next().unwrap(), "ALT中");
    assert_eq!(parser.screen().rows(0, 16).nth(1).unwrap(), "");
    std::fs::write(report.0.with_extension("opened"), b"observed").unwrap();
    fixture.until("buffers-app-ok:");
    fixture.finish(0);
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    assert_eq!(parser.screen().rows(0, 16).next().unwrap(), "BASE");
    assert_eq!(parser.screen().rows(0, 16).nth(1).unwrap(), "BASE-HIDDEN");
    assert_eq!(
        parser.screen().rows(0, 16).nth(3).unwrap(),
        "buffers-app-ok:"
    );
}

#[test]
#[ignore = "explicit external Windows PowerShell multiple-buffer court; missing client is BLOCKED"]
fn rust_console_client_windows_powershell_native_buffers() {
    powershell_buffers("MINICON_TEST_WINDOWS_POWERSHELL");
}

#[test]
#[ignore = "explicit external PowerShell 7 multiple-buffer court; missing client is BLOCKED"]
fn rust_console_client_powershell7_native_buffers() {
    powershell_buffers("MINICON_TEST_PWSH");
}

fn powershell_cursor_properties(name: &str) {
    let script = "$ErrorActionPreference='Stop';[Console]::CursorSize=73;[Console]::CursorVisible=$false;if([Console]::CursorSize -ne 73 -or [Console]::CursorVisible){exit 19};$Host.UI.RawUI.CursorSize=35;if($Host.UI.RawUI.CursorSize -ne 35){exit 20};[Console]::Write(([char]27)+'[6 q');[Console]::CursorVisible=$true;$Host.UI.RawUI.CursorSize=35;if([Console]::CursorSize -ne 35 -or ![Console]::CursorVisible){exit 21};[Console]::Write('cursor-properties-ok:');exit 0";
    let mut fixture = Fixture::open(
        ChildCommand::new(external_client(name))
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(script)
            .rust_console(),
    );
    fixture.until("cursor-properties-ok:");
    fixture.finish(0);
    let mut parser = vt100::Parser::new(30, 120, 0);
    parser.process(&fixture.bytes);
    assert_eq!(parser.screen().cursor_shape(), vt100::CursorShape::Bar);
    assert!(!parser.screen().cursor_blinking());
    assert!(!parser.screen().hide_cursor());
}

#[test]
#[ignore = "explicit external Windows PowerShell cursor property court; missing client is BLOCKED"]
fn rust_console_client_windows_powershell_cursor_properties() {
    powershell_cursor_properties("MINICON_TEST_WINDOWS_POWERSHELL");
}

#[test]
#[ignore = "explicit external PowerShell 7 cursor property court; missing client is BLOCKED"]
fn rust_console_client_powershell7_cursor_properties() {
    powershell_cursor_properties("MINICON_TEST_PWSH");
}

fn external_client(name: &str) -> PathBuf {
    let image = PathBuf::from(std::env::var_os(name).unwrap_or_else(|| {
        panic!("BLOCKED: {name} must name an explicitly selected native client")
    }));
    assert!(
        image.is_absolute() && image.is_file(),
        "BLOCKED: invalid {name}"
    );
    image
}

fn powershell_console_roundtrip(variable: &str) {
    let script = "[Console]::InputEncoding=[Text.UTF8Encoding]::new();[Console]::OutputEncoding=[Text.UTF8Encoding]::new();[Console]::Write('ps-ready:');$value=[Console]::ReadLine();[Console]::WriteLine('ps-value='+$value);exit 27";
    let mut fixture = Fixture::open(
        ChildCommand::new(external_client(variable))
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-Command")
            .arg(script)
            .env("TERM", "xterm-256color")
            .rust_console(),
    );
    fixture.until("ps-ready:");
    fixture.master.write_all("中😀\r".as_bytes()).unwrap();
    fixture.finish(27);
    let text = String::from_utf8_lossy(&fixture.bytes);
    assert!(text.contains("ps-value=中😀"), "{text}");
    assert!(!text.contains("Exception"), "{text}");
}

#[test]
#[ignore = "explicit external Windows PowerShell RawUI rectangle court; missing client is BLOCKED"]
fn rust_console_client_windows_powershell_rawui_rectangles() {
    let script = "$ErrorActionPreference='Stop';$raw=$Host.UI.RawUI;$cells=$raw.NewBufferCellArray([string[]]@('A中','CDX'),[ConsoleColor]::Yellow,[ConsoleColor]::DarkBlue);$pos=New-Object System.Management.Automation.Host.Coordinates -ArgumentList 2,2;$raw.SetBufferContents($pos,$cells);$area=New-Object System.Management.Automation.Host.Rectangle -ArgumentList 2,2,4,3;$read=$raw.GetBufferContents($area);if($read[0,0].Character -cne 'A' -or $read[1,1].Character -cne 'D' -or $read[0,1].Character -cne '中' -or $read[1,2].Character -cne 'X' -or $read[0,0].ForegroundColor -ne [ConsoleColor]::Yellow -or $read[0,0].BackgroundColor -ne [ConsoleColor]::DarkBlue){exit 19};[Console]::SetCursorPosition(0,5);[Console]::Write('rawui-rect-ok:');exit 0";
    let mut fixture = Fixture::open(
        ChildCommand::new(external_client("MINICON_TEST_WINDOWS_POWERSHELL"))
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(script)
            .size(TerminalSize { rows: 8, cols: 16 })
            .rust_console(),
    );
    fixture.until("rawui-rect-ok:");
    fixture.finish(0);
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    assert_eq!(parser.screen().cell(2, 2).unwrap().contents(), "A");
    assert_eq!(parser.screen().cell(3, 3).unwrap().contents(), "D");
    assert_eq!(parser.screen().cell(2, 3).unwrap().contents(), "中");
    assert!(parser.screen().cell(2, 4).unwrap().is_wide_continuation());
    assert_eq!(
        parser.screen().cell(2, 2).unwrap().fgcolor(),
        vt100::Color::Idx(11)
    );
    assert_eq!(
        parser.screen().cell(2, 2).unwrap().bgcolor(),
        vt100::Color::Idx(4)
    );
}

#[test]
#[ignore = "explicit external Windows PowerShell managed buffer-move court; missing client is BLOCKED"]
fn rust_console_client_windows_powershell_managed_buffer_move() {
    let script = "$ErrorActionPreference='Stop';[Console]::OutputEncoding=[Text.Encoding]::GetEncoding(936);$raw=$Host.UI.RawUI;$cells=$raw.NewBufferCellArray([string[]]@('A中','CDX'),[ConsoleColor]::Yellow,[ConsoleColor]::DarkBlue);$pos=New-Object System.Management.Automation.Host.Coordinates -ArgumentList 2,2;$raw.SetBufferContents($pos,$cells);[Console]::MoveBufferArea(2,2,3,2,2,3,[char]'.',[ConsoleColor]::White,[ConsoleColor]::DarkRed);$area=New-Object System.Management.Automation.Host.Rectangle -ArgumentList 2,2,4,4;$read=$raw.GetBufferContents($area);if($read[0,0].Character -cne '.' -or $read[1,0].Character -cne 'A' -or $read[1,1].Character -cne '中' -or $read[2,1].Character -cne 'D' -or $read[2,2].Character -cne 'X' -or $read[0,0].BackgroundColor -ne [ConsoleColor]::DarkRed -or $read[1,0].ForegroundColor -ne [ConsoleColor]::Yellow){exit 19};[Console]::SetCursorPosition(0,6);[Console]::Write('managed-move-ok:');exit 0";
    let mut fixture = Fixture::open(
        ChildCommand::new(external_client("MINICON_TEST_WINDOWS_POWERSHELL"))
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(script)
            .size(TerminalSize { rows: 8, cols: 16 })
            .rust_console(),
    );
    fixture.until("managed-move-ok:");
    fixture.finish(0);
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    assert_eq!(parser.screen().cell(2, 2).unwrap().contents(), ".");
    assert_eq!(parser.screen().cell(3, 2).unwrap().contents(), "A");
    assert_eq!(parser.screen().cell(3, 3).unwrap().contents(), "中");
    assert_eq!(parser.screen().cell(4, 4).unwrap().contents(), "X");
}

#[test]
#[ignore = "explicit external Windows PowerShell RawUI scroll court; missing client is BLOCKED"]
fn rust_console_client_windows_powershell_rawui_scroll() {
    let script = "$ErrorActionPreference='Stop';$raw=$Host.UI.RawUI;$cells=$raw.NewBufferCellArray([string[]]@('A中','CDX'),[ConsoleColor]::Yellow,[ConsoleColor]::DarkBlue);$pos=New-Object System.Management.Automation.Host.Coordinates -ArgumentList 2,2;$raw.SetBufferContents($pos,$cells);$src=New-Object System.Management.Automation.Host.Rectangle -ArgumentList 2,2,4,3;$dst=New-Object System.Management.Automation.Host.Coordinates -ArgumentList 2,3;$clip=New-Object System.Management.Automation.Host.Rectangle -ArgumentList 0,0,15,7;$fill=New-Object System.Management.Automation.Host.BufferCell -ArgumentList ([char]'.'),([ConsoleColor]::White),([ConsoleColor]::DarkRed),([System.Management.Automation.Host.BufferCellType]::Complete);$raw.ScrollBufferContents($src,$dst,$clip,$fill);$area=New-Object System.Management.Automation.Host.Rectangle -ArgumentList 2,2,4,4;$read=$raw.GetBufferContents($area);if($read[0,0].Character -cne '.' -or $read[1,0].Character -cne 'A' -or $read[1,1].Character -cne '中' -or $read[2,1].Character -cne 'D' -or $read[2,2].Character -cne 'X' -or $read[0,0].BackgroundColor -ne [ConsoleColor]::DarkRed -or $read[1,0].ForegroundColor -ne [ConsoleColor]::Yellow){exit 19};[Console]::SetCursorPosition(0,6);[Console]::Write('rawui-scroll-ok:');exit 0";
    let mut fixture = Fixture::open(
        ChildCommand::new(external_client("MINICON_TEST_WINDOWS_POWERSHELL"))
            .arg("-NoLogo")
            .arg("-NoProfile")
            .arg("-NonInteractive")
            .arg("-Command")
            .arg(script)
            .size(TerminalSize { rows: 8, cols: 16 })
            .rust_console(),
    );
    fixture.until("rawui-scroll-ok:");
    fixture.finish(0);
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    assert_eq!(parser.screen().cell(2, 2).unwrap().contents(), ".");
    assert_eq!(
        parser.screen().cell(2, 2).unwrap().bgcolor(),
        vt100::Color::Idx(1)
    );
    assert_eq!(parser.screen().cell(3, 2).unwrap().contents(), "A");
    assert_eq!(parser.screen().cell(3, 3).unwrap().contents(), "中");
    assert!(parser.screen().cell(3, 4).unwrap().is_wide_continuation());
    assert_eq!(parser.screen().cell(4, 3).unwrap().contents(), "D");
}

#[test]
#[ignore = "explicit external Windows PowerShell court; missing client is BLOCKED"]
fn rust_console_client_windows_powershell_console_roundtrip() {
    powershell_console_roundtrip("MINICON_TEST_WINDOWS_POWERSHELL");
}

#[test]
#[ignore = "explicit external PowerShell 7 court; missing client is BLOCKED"]
fn rust_console_client_powershell7_console_roundtrip() {
    powershell_console_roundtrip("MINICON_TEST_PWSH");
}

#[test]
#[ignore = "explicit external Node court; missing client is BLOCKED"]
fn rust_console_client_node_raw_tty_unicode_roundtrip() {
    let script = "process.stdout.write('node-tty:'+process.stdout.isTTY+','+process.stdin.isTTY+'\\n');process.stdin.setRawMode(true);process.stdout.write('node-ready:');let s='';process.stdin.on('data',d=>{s+=d;if(s.includes('\\r')){process.stdout.write('\\nnode-value:'+s.split('\\r')[0]+'\\n');process.stdin.pause();process.exitCode=29;}});";
    let mut fixture = Fixture::open(
        ChildCommand::new(external_client("MINICON_TEST_NODE"))
            .arg("-e")
            .arg(script)
            .env("TERM", "xterm-256color")
            .rust_console(),
    );
    fixture.until("node-ready:");
    fixture.master.write_all("中😀\r".as_bytes()).unwrap();
    fixture.finish(29);
    let text = String::from_utf8_lossy(&fixture.bytes);
    assert!(text.contains("node-tty:true,true"), "{text}");
    assert!(text.contains("node-value:中😀"), "{text}");
}

#[test]
#[ignore = "explicit external Node readline court; missing client is BLOCKED"]
fn rust_console_client_node_readline_edits_a_line_in_the_tty() {
    let script = "if(!process.stdin.isTTY||!process.stdout.isTTY)throw Error('TTY required');const rl=require('readline').createInterface({input:process.stdin,output:process.stdout,terminal:true});rl.question('node-edit-ready:',value=>{process.stdout.write('\\nnode-edited:'+value+'\\n');rl.close();process.exitCode=31;});";
    let mut fixture = Fixture::open(
        ChildCommand::new(external_client("MINICON_TEST_NODE"))
            .arg("-e")
            .arg(script)
            .env("TERM", "xterm-256color")
            .rust_console(),
    );
    fixture.until("node-edit-ready:");
    fixture.master.write_all(b"ac\x1b[Db\r").unwrap();
    fixture.finish(31);
    let text = String::from_utf8_lossy(&fixture.bytes);
    assert!(text.contains("node-edited:abc"), "{text}");
}

fn native_object_report(rust: bool) -> String {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("objects")
        .arg(report.0.as_os_str());
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.finish(0);
    std::fs::read_to_string(&report.0).unwrap()
}

#[test]
fn rust_console_pty_native_objects_preserve_access_share_duplicates_and_close_budget() {
    let report = native_object_report(true);
    eprintln!("owned object report: {report}");
    let values: Vec<u32> = report.split(',').map(|s| s.parse().unwrap()).collect();
    assert_eq!(&values[..14], &[1, 0, 0, 5, 0, 5, 1, 0, 0, 5, 0, 5, 1, 0]);
    assert_eq!(&values[14..], &[0, 5, 1, 0, 1, 0, 1, 0, 1, 0]);
}

#[test]
#[ignore = "explicit system ConPTY differential court; unavailable reference is BLOCKED"]
fn rust_console_client_native_object_access_matches_the_system_conpty() {
    let owned = native_object_report(true);
    let reference = native_object_report(false);
    eprintln!("owned objects: {owned}; system ConPTY objects: {reference}");
    assert_eq!(owned, reference);
}

#[test]
fn rust_console_pty_native_writeconsoleinput_roundtrips_actual_sdk_records() {
    let report = Report::new();
    let mut fixture = Fixture::open(
        ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg("write-records")
            .arg(report.0.as_os_str())
            .rust_console(),
    );
    fixture.finish(0);
    assert_eq!(
        std::fs::read_to_string(&report.0).unwrap(),
        "2,2,2,20013,3,16,0"
    );
}

fn input_flush_report(rust: bool) {
    for kind in ["records", "pending", "wide", "bytes"] {
        for suffix in ["", "-raw"] {
            let mode = format!("input-flush-{kind}{suffix}");
            let report = Report::new();
            let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg(&mode)
                .arg(report.0.as_os_str());
            let mut fixture = if rust {
                Fixture::open(command.rust_console())
            } else {
                Fixture::open_backend(command, "conpty")
            };
            fixture.finish(0);
            let observed = std::fs::read_to_string(&report.0).unwrap();
            println!("input-flush rust={rust};mode={mode}\n{observed}");
            let (build, observed) = observed.split_once('\n').unwrap();
            assert!(
                build
                    .strip_prefix("build=")
                    .unwrap()
                    .parse::<u32>()
                    .unwrap()
                    > 0
            );
            let expected = match kind {
                "records" => {
                    "before=3,0\nread-only=0,5,3\noutput=0,6,3\ninvalid=0,6,3\nflushed=0,258\nempty=0,258\nnew=1,1,0\nread=[90]\n"
                }
                "pending" => "pending=0,258\nread=[65, 66, 67, 13, 10]\n",
                "wide" => "first=65,0\nflushed=0,258\ntail=[66, 13, 10]\nafter=0,258\n",
                "bytes" => "first=228,0\nflushed=0,258\ntail=[90]\nafter=0,258\n",
                _ => unreachable!(),
            };
            if kind == "bytes" && !rust {
                // Installed 20348's UTF-8 first byte is unreliable (also
                // observed in the existing Break court). Keep its raw receipt;
                // only flush/post-flush behavior is a reference oracle here.
                let (first, observed) = observed.split_once('\n').unwrap();
                let (first, count) = first
                    .strip_prefix("first=")
                    .unwrap()
                    .split_once(',')
                    .unwrap();
                first.parse::<u8>().unwrap();
                assert_eq!(count, "0");
                assert_eq!(observed, expected.split_once('\n').unwrap().1);
            } else {
                assert_eq!(observed, expected, "{mode}");
            }
        }
    }
}

#[test]
fn rust_console_pty_native_input_flush_preserves_reads_and_caches() {
    input_flush_report(true);
}

#[test]
#[ignore = "explicit system ConPTY native input flush differential court"]
fn rust_console_client_native_input_flush_matches_system_conpty() {
    input_flush_report(false);
}

#[test]
fn rust_console_pty_native_input_flush_bypasses_output_pressure() {
    for suffix in ["", "-raw"] {
        let report = Report::new();
        let (master, child) =
            ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg(format!("input-flush-pressure{suffix}"))
                .arg(report.0.as_os_str())
                .rust_console()
                .spawn()
                .unwrap()
                .into_parts();
        assert_eq!(master.backend_kind(), "rust-console");
        // No reader exists yet: progress must not depend on terminal drain.
        report.wait_ready();
        let observed = std::fs::read_to_string(report.0.with_extension("ready")).unwrap();
        let (build, observed) = observed.split_once('\n').unwrap();
        assert!(
            build
                .strip_prefix("build=")
                .unwrap()
                .parse::<u32>()
                .unwrap()
                > 0
        );
        assert_eq!(observed, "flushed=0,258\n");
        let mut fixture = Fixture::with_parts(master, child);
        std::fs::write(report.0.with_extension("go"), b"drain").unwrap();
        fixture.finish(0);
        assert!(fixture.bytes.ends_with(&vec![b'P'; 65536]));
    }
}

fn ansi_input_report(kind: &str, rust: bool) -> String {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(format!("ansi-input-{kind}"))
        .arg(report.0.as_os_str());
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.finish(0);
    let observed = std::fs::read_to_string(&report.0).unwrap();
    println!("ansi-input rust={rust};kind={kind}\n{observed}");
    let (build, observed) = observed.split_once('\n').unwrap();
    assert!(
        build
            .strip_prefix("build=")
            .unwrap()
            .parse::<u32>()
            .unwrap()
            > 0
    );
    observed.to_owned()
}

fn ansi_input_cases(rust: bool) {
    let common = [
        "single", "oem", "pair", "split", "rights", "codepage", "invalid", "utf8", "nonkey",
        "pending",
    ];
    // Non-key trails produce variable key data on installed 20348. Their
    // failed diagnostic receipts are retained; only the owned modern path is
    // qualified for that case. The reference court covers the other 9 cases.
    for kind in common.into_iter().filter(|kind| rust || *kind != "nonkey") {
        let observed = ansi_input_report(kind, rust);
        let simple = "K:1,3,65,30,233,16";
        let pair = "K:1,3,65,30,20013,16";
        let lead = "lead=0,0,258\nzero=0,0,258\n";
        let expected = match kind {
            "single" => ansi_record_snapshot(
                "written=4\n",
                "K:1,3,65,30,233,16|F:1|M:-2,3,5,8,1|K:0,3,66,31,65,17",
                4,
            ),
            "oem" => ansi_record_snapshot("written=1\n", simple, 1),
            "pair" => ansi_record_snapshot("written=1\n", pair, 1),
            "split" => ansi_record_snapshot(&format!("{lead}written=1\n"), pair, 1),
            "rights" => ansi_record_snapshot(
                &format!("{lead}ro=0,5,0,0\nout=0,6,0,0\nbad=0,6,4294967295,0\nwritten=1\n"),
                pair,
                1,
            ),
            "codepage" => ansi_record_snapshot(
                &format!("{lead}written=2\n"),
                if rust {
                    "K:1,3,65,30,8217,16|K:1,3,65,30,8224,16"
                } else {
                    "K:1,3,65,30,8217,16|K:1,3,66,31,8224,17"
                },
                2,
            ),
            "invalid" => ansi_record_snapshot("written=1\n", "K:1,3,65,30,12539,16", 1),
            "utf8" => ansi_record_snapshot(
                "written=3\n",
                "K:1,3,65,30,65533,16|K:1,3,66,31,65533,17|K:1,3,67,32,65533,18",
                3,
            ),
            "nonkey" => {
                ansi_record_snapshot("batch=1\n", "K:1,3,67,32,90,18", 1)
                    + &ansi_record_snapshot("split=2\n", "F:1|K:1,3,67,32,90,18", 2)
            }
            "pending" => format!("{lead}written=1\ntext=1,20013\n"),
            _ => unreachable!(),
        };
        // Explicit old-build differences: modern pinned source preserves lead
        // identity on CP change and skips non-key trails without union casts.
        assert_eq!(observed, expected, "ANSI records: rust={rust}, kind={kind}");
    }
}

#[test]
fn rust_console_pty_ansi_input_records_preserve_codepages_prefixes_and_native_metadata() {
    ansi_input_cases(true);
}

#[test]
#[ignore = "explicit system ConPTY ANSI input-record differential court"]
fn rust_console_client_ansi_input_record_writes_compare_system_conpty() {
    ansi_input_cases(false);
}

#[test]
fn rust_console_pty_ansi_input_record_capacity_is_checked_after_conversion_without_losing_prefix() {
    let observed = ansi_input_report("capacity", true);
    assert_eq!(
        observed,
        ansi_record_snapshot(
            "lead=0,0,258\nzero=0,0,258\nfull=0,87,0,4095\nfit=1\npair-fit=1\nwritten=1\n",
            "K:1,3,65,30,20013,16",
            1
        )
    );
}

fn ansi_record_snapshot(prefix: &str, records: &str, count: usize) -> String {
    format!("{prefix}queued={count},0\npeek={records}\nread={records}\nempty=0,258\n")
}

fn ansi_record_read_report(kind: &str, rust: bool) -> String {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(format!("ansi-read-{kind}"))
        .arg(report.0.as_os_str());
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| fixture.finish(0)));
    if let Err(panic) = result {
        eprintln!(
            "native read error={:?}; panic={:?}",
            std::fs::read_to_string(report.0.with_extension("read-error")),
            std::fs::read_to_string(report.0.with_extension("read-panic"))
        );
        std::panic::resume_unwind(panic);
    }
    let observed = std::fs::read_to_string(&report.0).unwrap();
    println!("ansi-read rust={rust};kind={kind}\n{observed}");
    let (build, body) = observed.split_once('\n').unwrap();
    let build = build
        .strip_prefix("build=")
        .unwrap()
        .parse::<u32>()
        .unwrap();
    if !rust {
        assert_eq!(build, 20348, "reference snapshots qualify only this build");
    } else {
        assert!(build > 0);
    }
    body.to_owned()
}

fn ansi_record_read_cases(rust: bool) {
    for kind in [
        "single",
        "split",
        "utf8",
        "tail",
        "flush",
        "codepage",
        "switch",
        "text",
        "zero",
        "close",
        "rights",
        "surrogate",
        "wait",
        "behind",
    ] {
        // Installed reference disconnects at the second UTF-8 narrow Peek.
        // Failed diagnostics are retained; that reference case is unqualified.
        if !rust && kind == "utf8" {
            println!("BLOCKED reference utf8: retained SDK p2 pipe-disconnect diagnostics");
            continue;
        }
        let observed = ansi_record_read_report(kind, rust);
        assert_eq!(
            observed,
            ansi_record_read_expected(kind, rust),
            "read case: {kind}, rust={rust}"
        );
    }
}

fn ansi_record_read_expected(kind: &str, rust: bool) -> String {
    let text = if rust {
        include_str!("support/ansi_read_owned.txt")
    } else {
        include_str!("support/ansi_read_reference_20348.txt")
    };
    let text = text.replace("\r\n", "\n");
    let prefix = format!("[{kind}]\n");
    let body = text
        .split_once(&prefix)
        .unwrap()
        .1
        .split("\n[")
        .next()
        .unwrap();
    format!("{}\n", body.trim_end())
}

#[test]
fn rust_console_pty_ansi_record_reads_preserve_peek_capacity_and_tail_lifetimes() {
    ansi_record_read_cases(true);
}

#[test]
#[ignore = "explicit system ConPTY ANSI record-read differential court"]
fn rust_console_client_ansi_record_read_receipts_compare_system_conpty() {
    ansi_record_read_cases(false);
}

#[test]
fn rust_console_pty_record_queries_stay_live_under_output_pressure() {
    let report = Report::new();
    let (master, child) = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg("ansi-read-pressure")
        .arg(report.0.as_os_str())
        .rust_console()
        .spawn()
        .unwrap()
        .into_parts();
    assert_eq!(master.backend_kind(), "rust-console");
    report.wait_ready();
    let ready = std::fs::read_to_string(report.0.with_extension("ready")).unwrap();
    println!("ansi-read pressure\n{}", ready);
    let (build, body) = ready.split_once('\n').unwrap();
    assert!(
        build
            .strip_prefix("build=")
            .unwrap()
            .parse::<u32>()
            .unwrap()
            > 0
    );
    assert_eq!(body, ansi_record_read_expected("pressure", true));
    let mut fixture = Fixture::with_parts(master, child);
    std::fs::write(report.0.with_extension("go"), b"drain").unwrap();
    fixture.finish(0);
    let mut expected = format!(
        "\x1b]0;{}\x1b\\",
        env!("CARGO_BIN_EXE_minicon-platform-console-probe")
    )
    .into_bytes();
    expected.extend(vec![b'P'; 65536]);
    assert_eq!(fixture.bytes, expected);
}

fn cooked_edit_report(kind: &str, rust: bool) -> String {
    let report = Report::new();
    let cols = if matches!(kind, "cmd" | "powershell") {
        80
    } else if kind == "echo-narrow" {
        1
    } else {
        16
    };
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(format!("cooked-edit-{kind}"))
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("cooked-edit-end:");
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    let (build, receipt) = receipt.split_once('\n').unwrap();
    let build: u32 = build.strip_prefix("build=").unwrap().parse().unwrap();
    if rust {
        assert!(build > 0);
    } else {
        assert_eq!(build, 20348);
    }
    let mut parser = vt100::Parser::new(8, cols, 0);
    parser.process(&fixture.bytes);
    let echo = kind.starts_with("echo-") || kind == "cmd";
    let rows: Vec<_> = parser
        .screen()
        .rows(0, cols)
        .map(|row| row.trim_end_matches(' ').to_owned())
        .collect();
    let image = if echo {
        rows.join("\n").trim_end_matches('\n').to_owned()
    } else {
        parser.screen().contents()
    };
    let mut body = format!(
        "{receipt}screen:{:?}\nfrontend-cursor:{:?}\n",
        image,
        parser.screen().cursor_position()
    );
    if echo {
        body.push_str(&format!("rows:{:?}\n", rows));
        println!(
            "cooked-echo-logical rust={rust};kind={kind};text={:?}",
            parser.screen().contents()
        );
        println!(
            "cooked-echo-wire rust={rust};kind={kind};bytes={}",
            fixture.bytes.len()
        );
        if rust && kind == "echo-scroll-home" {
            let marker = b"cooked-echo-begin:";
            let start = fixture
                .bytes
                .windows(marker.len())
                .position(|w| w == marker)
                .unwrap()
                + marker.len();
            let tail = &fixture.bytes[start..];
            let start = start
                + if tail.starts_with(b"\x07") {
                    1
                } else {
                    assert!(tail.starts_with(b"\x1b\\"));
                    2
                };
            let end_marker = b"cooked-edit-end:";
            let end = fixture
                .bytes
                .windows(end_marker.len())
                .position(|w| w == end_marker)
                .unwrap();
            let end = fixture.bytes[..end]
                .windows(2)
                .rposition(|w| w == b"\x1b]")
                .unwrap();
            assert!(
                end - start <= 512,
                "bulk echo exceeded wire budget: {}",
                end - start
            );
        }
    }
    if kind == "echo-pending-csi" {
        assert_eq!(
            parser.screen().cell(1, 0).unwrap().fgcolor(),
            vt100::Color::Idx(1)
        );
    }
    println!("cooked-edit rust={rust};kind={kind};build={build}\n{body}");
    body
}

fn cooked_edit_cases(rust: bool) {
    for kind in [
        "insert",
        "implicit-insert",
        "inherit-overwrite",
        "overwrite",
        "toggle",
        "delete",
        "end",
        "backspace",
        "escape",
        "escape-masked",
        "initial",
        "enter-middle",
        "unicode-motion",
        "unicode-overwrite",
        "pair-overwrite",
        "invalid",
        "invalid-retained",
        "f1",
        "edges",
        "ctrl-home",
        "ctrl-end",
        "repeat",
        "masked-middle",
        "literal-bs",
        "escape-alt",
    ] {
        let actual = cooked_edit_report(kind, rust);
        assert_eq!(
            actual,
            cooked_edit_expected(kind, rust),
            "kind={kind};rust={rust}"
        );
    }
}

fn cooked_echo_cases(rust: bool) {
    for kind in [
        "insert",
        "implicit-insert",
        "inherit-overwrite",
        "overwrite",
        "toggle",
        "delete",
        "end",
        "backspace",
        "escape",
        "escape-masked",
        "initial",
        "enter-middle",
        "unicode-motion",
        "unicode-overwrite",
        "pair-overwrite",
        "invalid",
        "invalid-retained",
        "f1",
        "edges",
        "ctrl-home",
        "ctrl-end",
        "repeat",
        "masked-middle",
        "literal-bs",
        "escape-alt",
        "cross-row",
        "tab-erase",
        "control-erase",
        "wide-wrap",
        "scroll-home",
        "initial-tab",
        "initial-wide",
        "pending-csi",
        "legacy-wrap",
        "no-wrap",
        "tail-high",
        "narrow",
    ] {
        let mode = format!("echo-{kind}");
        assert_eq!(
            cooked_edit_report(&mode, rust),
            cooked_echo_expected(&mode, rust),
            "kind={kind};rust={rust}"
        );
    }
    let cmd = cooked_edit_report("cmd", rust);
    assert_eq!(cmd, cooked_echo_expected("cmd", rust));
}

fn cooked_echo_expected(kind: &str, rust: bool) -> String {
    let text = if rust {
        include_str!("support/cooked_echo_owned.txt")
    } else {
        include_str!("support/cooked_echo_reference_20348.txt")
    }
    .replace("\r\n", "\n");
    let prefix = format!("[{kind}]\n");
    let body = text
        .split_once(&prefix)
        .unwrap()
        .1
        .split("\n[")
        .next()
        .unwrap();
    format!("{}\n", body.trim_end())
}

#[test]
fn rust_console_pty_cooked_echo_edits_use_shared_layout() {
    cooked_echo_cases(true);
}

fn cooked_buffer_cases(rust: bool) {
    for kind in ["switch", "close"] {
        let report = Report::new();
        let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg(format!("cooked-buffer-{kind}"))
            .arg(report.0.as_os_str())
            .size(TerminalSize { rows: 8, cols: 16 });
        let mut fixture = if rust {
            Fixture::open(command.rust_console())
        } else {
            Fixture::open_backend(command, "conpty")
        };
        fixture.until("cooked-buffer-revealed:");
        let value = std::fs::read_to_string(&report.0).unwrap();
        let expected = if kind == "close" {
            r#"{"units":[97, 88, 98, 13, 10],"created_after_read":15,"base":"B>","base_cursor":[0,2],"read_buffer":"","read_cursor":[-1,-1]}"#
        } else {
            r#"{"units":[97, 88, 98, 13, 10],"created_after_read":0,"base":"B>","base_cursor":[0,2],"read_buffer":"A>aXb","read_cursor":[1,0]}"#
        };
        assert_eq!(value, expected);
        let mut peer = vt100::Parser::new(8, 16, 0);
        peer.process(&fixture.bytes);
        let first = peer.screen().rows(0, 16).next().unwrap();
        assert_eq!(
            first.trim_end_matches(' '),
            if kind == "close" { "B>" } else { "A>aXb" }
        );
        std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
        fixture.until("cooked-buffer-end:");
        fixture.finish(0);
        let mut peer = vt100::Parser::new(8, 16, 0);
        peer.process(&fixture.bytes);
        assert_eq!(peer.screen().contents(), "B>");
        eprintln!("cooked-buffer rust={rust};kind={kind}: {value}");
    }
}

#[test]
fn rust_console_pty_cooked_reads_retain_their_original_drawing_buffer() {
    cooked_buffer_cases(true);
}

#[test]
#[ignore = "explicit system ConPTY cooked-read buffer/lifetime court"]
fn rust_console_client_cooked_read_buffers_compare_system_conpty() {
    cooked_buffer_cases(false);
}

fn cancelled_buffer_cases(rust: bool) {
    for kind in ["hidden", "active"] {
        let report = Report::new();
        let mode = format!("cooked-cancel-{kind}");
        let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg(mode)
            .arg(report.0.as_os_str())
            .size(TerminalSize { rows: 8, cols: 16 });
        let mut fixture = if rust {
            Fixture::open(command.rust_console())
        } else {
            Fixture::open_backend(command, "conpty")
        };
        fixture.until("cooked-cancel-ready:");
        if rust {
            fixture
                .child
                .send_native_key(NativeTerminalKey::Break, 1)
                .unwrap();
        } else {
            // The reference facade has no qualified native Break transport.
            // Processed Ctrl+C is its existing actual keyboard interruption path.
            fixture.master.write_all(&[3]).unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        while !report.0.with_extension("cancelled").exists() {
            assert!(
                Instant::now() < deadline,
                "native cancellation {rust}/{kind} stalled"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert_eq!(
            std::fs::read_to_string(report.0.with_extension("cancelled")).unwrap(),
            if rust { "true,995,0,2" } else { "true,995,0,1" }
        );
        std::fs::write(report.0.with_extension("probe"), b"query-and-open").unwrap();
        fixture.until("cooked-cancel-end:");
        let value = std::fs::read_to_string(&report.0).unwrap();
        assert_eq!(
            value,
            format!(
                r#"{{"ok":true,"error":995,"count":0,"event":{},"created":15,"base":"B>","cursor":[0,2]}}"#,
                if rust { 2 } else { 1 }
            )
        );
        let mut peer = vt100::Parser::new(8, 16, 0);
        peer.process(&fixture.bytes);
        assert_eq!(peer.screen().contents(), "B>");
        assert_eq!(peer.screen().cursor_position(), (0, 2));
        std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
        fixture.finish(0);
        eprintln!("cooked-cancel rust={rust};kind={kind}: {value}");
    }
}

#[test]
fn rust_console_pty_break_reclaims_cancelled_read_buffers_and_publishes_fallback() {
    cancelled_buffer_cases(true);
}

#[test]
#[ignore = "explicit system ConPTY cancelled-read buffer lifetime court"]
fn rust_console_client_cancelled_read_buffers_compare_system_conpty() {
    cancelled_buffer_cases(false);
}

fn retiring_buffer_cases(rust: bool) {
    for kind in ["input-close", "disconnect"] {
        let report = Report::new();
        let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg(format!("cooked-retire-{kind}"))
            .arg(report.0.as_os_str())
            .size(TerminalSize { rows: 8, cols: 16 });
        let mut fixture = if rust {
            Fixture::open(command.rust_console())
        } else {
            Fixture::open_backend(command, "conpty")
        };
        fixture.until("cooked-retire-ready:");
        let deadline = Instant::now() + Duration::from_secs(10);
        while !report.0.exists() {
            assert!(
                Instant::now() < deadline,
                "retirement {rust}/{kind} stalled"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let value = std::fs::read_to_string(&report.0).unwrap();
        assert_eq!(
            value,
            format!(
                r#"{{"ok":{},"error":{},"count":{},"detached":{},"secondary":[],"units":{}}}"#,
                kind != "disconnect",
                if kind == "disconnect" { 5 } else { 0 },
                if kind == "disconnect" { 0 } else { 5 },
                kind == "disconnect",
                if kind == "disconnect" {
                    "[]"
                } else {
                    "[97, 88, 98, 13, 10]"
                }
            )
        );
        std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
        fixture.finish(0);
        let mut peer = vt100::Parser::new(8, 16, 0);
        peer.process(&fixture.bytes);
        assert_eq!(peer.screen().contents(), "B>");
        assert_eq!(peer.screen().cursor_position(), (0, 2));
        eprintln!("cooked-retire rust={rust};kind={kind}: {value}");
    }
}

#[test]
fn rust_console_pty_read_retirement_reveals_the_surviving_buffer() {
    retiring_buffer_cases(true);
}

#[test]
#[ignore = "explicit system ConPTY read-retirement lifetime court"]
fn rust_console_client_read_retirement_compares_system_conpty() {
    retiring_buffer_cases(false);
}

fn queued_read_pressure_cases(rust: bool) {
    let kinds: &[&str] = if rust {
        &[
            "interrupt-file",
            "interrupt-console",
            "interrupt-records",
            "signal-file",
            "disconnect-file",
            "disconnect-console",
            "disconnect-records",
        ]
    } else {
        &[
            "signal-file",
            "disconnect-file",
            "disconnect-console",
            "disconnect-records",
        ]
    };
    for &kind in kinds {
        let report = Report::new();
        let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg(format!("queued-read-{kind}"))
            .arg(report.0.as_os_str());
        let (master, child) = if rust {
            command.rust_console().spawn().unwrap().into_parts()
        } else {
            command.spawn().unwrap().into_parts()
        };
        assert_eq!(
            master.backend_kind(),
            if rust { "rust-console" } else { "conpty" }
        );
        report.wait_ready();
        if kind.starts_with("interrupt") || kind.starts_with("signal") {
            if rust && kind != "signal-file" {
                child.send_native_key(NativeTerminalKey::Break, 1).unwrap();
            } else {
                master.write_all(&[3]).unwrap();
            }
        }
        let record = matches!(kind, "interrupt-records" | "signal-file");
        let mut parts = Some((master, child));
        let draining = if record {
            let deadline = Instant::now() + Duration::from_secs(10);
            while !report.0.with_extension("signal").exists() {
                assert!(
                    Instant::now() < deadline,
                    "record read did not survive keyboard control"
                );
                std::thread::sleep(Duration::from_millis(2));
            }
            assert_eq!(
                std::fs::read_to_string(report.0.with_extension("signal")).unwrap(),
                "read-pending"
            );
            assert!(!report.0.exists());
            let (master, child) = parts.take().unwrap();
            let fixture = Fixture::with_parts(master, child);
            std::fs::write(report.0.with_extension("records"), b"provide-data").unwrap();
            Some(fixture)
        } else {
            None
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !report.0.exists() {
            assert!(
                Instant::now() < deadline,
                "queued pressure {rust}/{kind} stalled"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let value = std::fs::read_to_string(&report.0).unwrap();
        let (body, pending) = value.rsplit_once(",\"writer_pending\":").unwrap();
        eprintln!("queued-read rust={rust};kind={kind}: {value}");
        assert!(matches!(pending, "true}" | "false}"));
        if rust && kind.starts_with("interrupt") && !record {
            assert_eq!(pending, "true}", "writer must stay blocked until drain");
        }
        // Under stock ConPTY output pressure, FreeConsole closes these reads
        // with ALERTED rather than the accepted-waiter ThreadDying status.
        // Keep that observed provider difference explicit in the oracle.
        assert_eq!(
            body,
            format!(
                r#"{{"ok":{},"error":{},"count":{},"unit":{},"event":{},"mode":{},"secondary":[]"#,
                kind.starts_with("interrupt") || kind.starts_with("signal") || !rust,
                if kind == "interrupt-console" || (!rust && kind == "disconnect-console") {
                    995
                } else if kind.starts_with("interrupt") || kind.starts_with("signal") || !rust {
                    0
                } else {
                    5
                },
                usize::from(record),
                if record { 90 } else { 0 },
                if kind == "signal-file" {
                    1
                } else if kind.starts_with("interrupt") {
                    2
                } else {
                    0
                },
                if kind == "interrupt-console" {
                    7
                } else if matches!(kind, "interrupt-file" | "signal-file") {
                    1
                } else {
                    0
                }
            )
        );
        std::fs::write(report.0.with_extension("opened"), b"drain").unwrap();
        let mut fixture = draining.unwrap_or_else(|| {
            let (master, child) = parts.take().unwrap();
            Fixture::with_parts(master, child)
        });
        fixture.finish(0);
        assert!(std::str::from_utf8(&fixture.bytes).is_ok());
        if rust && (kind.starts_with("interrupt") || kind.starts_with("signal")) {
            assert!(
                fixture.bytes.ends_with("a中😀".repeat(32768).as_bytes()),
                "interruption must preserve every queued writer byte"
            );
        }
    }
}

fn pending_mode_cases(rust: bool) {
    for api in ["file", "console"] {
        for case in ["cooked-text", "raw-text", "cooked-signal", "raw-signal"] {
            let kind = format!("{api}-{case}");
            let report = Report::new();
            let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg(format!("mode-wait-{kind}"))
                .arg(report.0.as_os_str());
            let mut fixture = if rust {
                Fixture::open(command.rust_console())
            } else {
                Fixture::open_backend(command, "conpty")
            };
            report.wait_ready();
            let signalled = case.ends_with("signal");
            if signalled {
                fixture.master.write_all(&[3]).unwrap();
            }
            let deadline = Instant::now() + Duration::from_secs(10);
            while !report.0.exists() {
                assert!(Instant::now() < deadline, "mode-wait {rust}/{kind} stalled");
                std::thread::sleep(Duration::from_millis(2));
            }
            let value = std::fs::read_to_string(&report.0).unwrap();
            let cooked = case.starts_with("cooked");
            let interrupted = cooked && signalled;
            eprintln!("mode-wait rust={rust};kind={kind}: {value}");
            assert_eq!(
                value,
                format!(
                    r#"{{"ok":true,"error":{},"units":{:?},"event":{},"mode":{}}}"#,
                    if interrupted && api == "console" {
                        995
                    } else {
                        0
                    },
                    if interrupted {
                        vec![]
                    } else if cooked {
                        vec![90, 13, 10]
                    } else if case == "raw-text" {
                        vec![90, 13]
                    } else {
                        vec![90]
                    },
                    u32::from(signalled),
                    if cooked { 1 } else { 3 }
                )
            );
            std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
            fixture.finish(0);
        }
    }
}

fn raw_batch_cases(rust: bool) {
    for kind in raw_batch_court::kinds(!rust) {
        let report = Report::new();
        let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg(format!("raw-batch-{kind}"))
            .arg(report.0.as_os_str());
        let mut fixture = if rust {
            Fixture::open(command.rust_console())
        } else {
            Fixture::open_backend(command, "conpty")
        };
        let deadline = Instant::now() + Duration::from_secs(10);
        while !report.0.exists() {
            assert!(
                Instant::now() < deadline,
                "raw batch {rust}/{kind} stalled; stage={:?}",
                std::fs::read_to_string(report.0.with_extension("ready"))
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        let value = std::fs::read_to_string(&report.0).unwrap();
        eprintln!("raw-batch rust={rust};kind={kind}: {value}");
        assert_eq!(value, raw_batch_court::expected(&kind));
        std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
        fixture.finish(0);
    }
}

#[test]
fn rust_console_pty_raw_reads_batch_to_capacity_without_stealing_records() {
    raw_batch_cases(true);
}

#[test]
#[ignore = "explicit native raw-read batch comparison with installed system ConPTY"]
fn rust_console_client_raw_read_batches_compare_system_conpty() {
    raw_batch_cases(false);
}

#[test]
fn rust_console_pty_pending_text_reads_keep_their_admitted_mode() {
    pending_mode_cases(true);
}

#[test]
#[ignore = "explicit system ConPTY pending-read mode-change comparison"]
fn rust_console_client_pending_text_modes_compare_system_conpty() {
    pending_mode_cases(false);
}

#[test]
fn rust_console_pty_queued_reads_finish_under_unread_output_pressure() {
    queued_read_pressure_cases(true);
}

#[test]
#[ignore = "explicit stock ConPTY read-disconnect/output-pressure court"]
fn rust_console_client_queued_read_disconnect_compares_system_conpty() {
    queued_read_pressure_cases(false);
}

#[test]
#[ignore = "explicit system ConPTY echoed cooked-editor differential court"]
fn rust_console_client_cooked_echo_edits_compare_system_conpty() {
    cooked_echo_cases(false);
}

#[test]
fn rust_console_pty_cooked_editor_keeps_utf16_and_native_editing_state() {
    cooked_edit_cases(true);
}

#[test]
#[ignore = "explicit external Windows PowerShell cooked-read court; missing client is BLOCKED"]
fn rust_console_pty_cooked_editor_feeds_powershell_console_readline() {
    assert_eq!(
        cooked_edit_report("powershell", true),
        cooked_edit_expected("powershell", true)
    );
}

fn cooked_edit_expected(kind: &str, rust: bool) -> String {
    let text = if rust {
        include_str!("support/cooked_edit_owned.txt")
    } else {
        include_str!("support/cooked_edit_reference_20348.txt")
    }
    .replace("\r\n", "\n");
    let prefix = format!("[{kind}]\n");
    let body = text
        .split_once(&prefix)
        .unwrap()
        .1
        .split("\n[")
        .next()
        .unwrap();
    format!("{}\n", body.trim_end())
}

#[test]
#[ignore = "explicit system ConPTY cooked-editor differential court"]
fn rust_console_client_cooked_editor_compares_system_conpty() {
    cooked_edit_cases(false);
    assert_eq!(
        cooked_edit_report("powershell", false),
        cooked_edit_expected("powershell", false)
    );
}

fn cooked_controls_report(kind: &str, rust: bool) -> (String, vt100::Parser) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(format!("cooked-controls-{kind}"))
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 16 });
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.until("cooked-controls-end:");
    std::fs::write(report.0.with_extension("opened"), b"release").unwrap();
    fixture.finish(0);
    let receipt = std::fs::read_to_string(&report.0).unwrap();
    let mut parser = vt100::Parser::new(8, 16, 0);
    parser.process(&fixture.bytes);
    println!(
        "cooked-controls rust={rust};kind={kind}\n{receipt}screen={:?};cursor={:?}",
        parser.screen().contents(),
        parser.screen().cursor_position()
    );
    let (build, receipt) = receipt.split_once('\n').unwrap();
    let build = build
        .strip_prefix("build=")
        .unwrap()
        .parse::<u32>()
        .unwrap();
    if rust {
        assert!(build > 0);
    } else {
        assert_eq!(build, 20348, "reference qualifies only the measured build");
    }
    (receipt.to_owned(), parser)
}

fn cooked_controls_cases(rust: bool) {
    for kind in [
        "processed-bs",
        "literal-bs",
        "literal-noecho-bs",
        "wakeup-bs",
        "wakeup-cr",
        "wakeup-tab",
        "wakeup-esc",
        "wakeup-literal-bs",
        "initial-wakeup-bs",
        "initial-processed-bs",
    ] {
        let (units, controls, queued, cursor, screen) = match kind {
            "processed-bs" | "initial-processed-bs" => {
                ("0061,0063,000d,000a,", 0, 0, (1, 0), ">ac")
            }
            "literal-bs" => (
                "0061,0062,0008,0063,000d,",
                0,
                0,
                (0, 0),
                if rust { ">ab^Hc" } else { ">ac" },
            ),
            "literal-noecho-bs" => ("0061,0062,0008,0063,000d,", 0, 0, (0, 1), ">"),
            "wakeup-bs" | "wakeup-literal-bs" | "initial-wakeup-bs" => {
                ("0061,0062,0008,", 16, 2, (0, 3), ">ab")
            }
            "wakeup-cr" => ("0061,0062,000d,", 16, 2, (0, 3), ">ab"),
            "wakeup-tab" => ("0061,0062,0009,", 16, 2, (0, 3), ">ab"),
            "wakeup-esc" => ("0061,0062,001b,", 16, 2, (0, 3), ">ab"),
            _ => unreachable!(),
        };
        let (receipt, parser) = cooked_controls_report(kind, rust);
        assert_eq!(
            receipt,
            format!(
                "read:{units}\ncontrols:{controls:08x}\nqueued:{queued}\ncursor:{},{};0007\n",
                cursor.1, cursor.0
            ),
            "kind={kind};rust={rust}"
        );
        assert_eq!(
            parser.screen().contents(),
            screen,
            "kind={kind};rust={rust}"
        );
        assert_eq!(
            parser.screen().cursor_position(),
            cursor,
            "kind={kind};rust={rust}"
        );
    }
}

#[test]
fn rust_console_pty_cooked_controls_preserve_mode_and_wakeup_precedence() {
    cooked_controls_cases(true);
}

#[test]
#[ignore = "explicit system ConPTY cooked-control differential court"]
fn rust_console_client_cooked_controls_compare_system_conpty() {
    cooked_controls_cases(false);
}

fn input_merge_report(kind: &str, rust: bool) -> String {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(format!("input-merge-{kind}"))
        .arg(report.0.as_os_str());
    let mut fixture = if rust {
        Fixture::open(command.rust_console())
    } else {
        Fixture::open_backend(command, "conpty")
    };
    fixture.finish(0);
    let observed = std::fs::read_to_string(&report.0).unwrap();
    println!("input-merge rust={rust};kind={kind}\n{observed}");
    let (build, body) = observed.split_once('\n').unwrap();
    let build = build
        .strip_prefix("build=")
        .unwrap()
        .parse::<u32>()
        .unwrap();
    if rust {
        assert!(build > 0);
    } else {
        assert_eq!(build, 20348, "reference snapshots qualify only this build");
    }
    body.to_owned()
}

fn input_merge_cases(rust: bool) {
    for kind in [
        "keys",
        "batch",
        "keyup",
        "identity",
        "ime",
        "surrogate",
        "mouse",
        "resize",
        "focus",
        "barrier",
        "ansi",
        "dbcs",
        "max",
        "overflow",
    ] {
        let observed = input_merge_report(kind, rust);
        assert_eq!(
            observed,
            input_merge_expected(kind, rust),
            "merge case {kind}, rust={rust}"
        );
    }
}

#[test]
fn rust_console_pty_input_coalescing_preserves_native_single_write_boundaries() {
    input_merge_cases(true);
}

#[test]
#[ignore = "explicit system ConPTY input-coalescing differential court"]
fn rust_console_client_input_coalescing_compares_system_conpty() {
    input_merge_cases(false);
}

#[test]
fn rust_console_pty_input_coalescing_admits_full_queue_updates_and_bounds_overflow() {
    for kind in ["capacity", "resize-capacity"] {
        let observed = input_merge_report(kind, true);
        assert_eq!(
            observed,
            input_merge_expected(kind, true),
            "merge capacity {kind}"
        );
    }
}

#[test]
fn rust_console_pty_transport_and_native_keys_share_input_coalescing() {
    for kind in ["transport", "native-key"] {
        let report = Report::new();
        let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
            .arg(format!("input-merge-{kind}"))
            .arg(report.0.as_os_str());
        let mut fixture = Fixture::open(command.rust_console());
        fixture.until("input-merge-ready:");
        if kind == "transport" {
            fixture.master.write_all(b"A").unwrap();
        } else {
            fixture
                .child
                .send_native_key(NativeTerminalKey::Up, 1)
                .unwrap();
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        while !report.0.with_extension("first").exists() {
            assert!(Instant::now() < deadline, "first input never arrived");
            std::thread::sleep(Duration::from_millis(2));
        }
        if kind == "transport" {
            fixture.master.write_all(b"A").unwrap();
        } else {
            fixture
                .child
                .send_native_key(NativeTerminalKey::Up, 1)
                .unwrap();
        }
        fixture.finish(0);
        let observed = std::fs::read_to_string(&report.0).unwrap();
        println!("input-merge rust=true;kind={kind}\n{observed}");
        let (build, body) = observed.split_once('\n').unwrap();
        assert!(
            build
                .strip_prefix("build=")
                .unwrap()
                .parse::<u32>()
                .unwrap()
                > 0
        );
        assert_eq!(
            body,
            input_merge_expected(kind, true),
            "merge transport {kind}"
        );
    }
}

fn input_merge_expected(kind: &str, rust: bool) -> String {
    let text = if rust {
        include_str!("support/input_merge_owned.txt")
    } else {
        include_str!("support/input_merge_reference_20348.txt")
    };
    let text = text.replace("\r\n", "\n");
    let prefix = format!("[{kind}]\n");
    let body = text
        .split_once(&prefix)
        .unwrap()
        .1
        .split("\n[")
        .next()
        .unwrap();
    format!("{}\n", body.trim_end())
}

#[test]
fn rust_console_pty_vt_mode_preserves_native_keys_and_text_sequences() {
    for (mode, expected) in [("vt-key", "1,1,38,72,256"), ("vt-text-key", "27,91,65")] {
        let report = Report::new();
        let mut fixture = Fixture::open(
            ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
                .arg(mode)
                .arg(report.0.as_os_str())
                .rust_console(),
        );
        fixture.until("native-pty-probe-ready:");
        assert_eq!(
            fixture.child.native_input_ownership().unwrap(),
            NativeInputOwnership::RawVt
        );
        fixture.master.write_all(b"\x1b[").unwrap();
        for _ in 0..2 {
            fixture
                .master
                .resize(TerminalSize { rows: 24, cols: 80 })
                .unwrap();
        }
        fixture.master.write_all(b"A").unwrap();
        fixture.finish(0);
        assert_eq!(
            std::fs::read_to_string(&report.0).unwrap(),
            expected,
            "{mode}"
        );
    }
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
