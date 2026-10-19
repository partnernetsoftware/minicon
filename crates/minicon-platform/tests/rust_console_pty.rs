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
            "ready",
            "go",
            "opened",
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
            "73728"
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
        fixture.bytes == expected_pressure_output(9, true),
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
    while fixture.bytes.len() < 9 * 8192 {
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
    assert!(fixture.bytes == expected_pressure_output(9, true));
}

fn expected_pressure_output(chunks: usize, vary: bool) -> Vec<u8> {
    (0..chunks)
        .flat_map(|index| {
            (0..1024).flat_map(move |position| {
                let prefix = if vary {
                    b'A' + ((index * 7 + position) % 26) as u8
                } else {
                    b'a'
                };
                format!("{}中😀", char::from(prefix)).into_bytes()
            })
        })
        .collect()
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
