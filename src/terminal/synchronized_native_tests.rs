//! Native SDK -> opened owned PTY -> GUI drain/query/snapshot/raster path.
//! Uses the production terminal raster offscreen; no native window/compositor.
use super::*;
use std::path::Path;

fn stage(t: &mut ConTerminal, report: &Path, phase: usize) {
    let expected = format!("native-sync-{phase}");
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        t.drain_pty();
        if t.parser.callbacks_mut().title.as_deref() == Some(&expected) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "native GUI phase {phase};failure={:?}",
            std::fs::read_to_string(report.with_extension("failure"))
        );
        std::thread::sleep(Duration::from_millis(1));
    }
}
fn capture(t: &mut ConTerminal, folder: &Path, name: &str) -> Vec<u32> {
    let pixels = super::synchronized_tests::pixels(t);
    agent_interface::write_png_atomic(
        &folder.join(format!("{name}.png")),
        &pixels,
        t.frame_width,
        t.frame_height,
    )
    .unwrap();
    agent_interface::write_snapshot_atomic(
        &folder.join(format!("{name}.json")),
        &t.build_snapshot(),
    )
    .unwrap();
    pixels
}

#[test]
fn gui_synchronized_native_sdk_queries_pixels_and_eof_share_the_real_session_path() {
    use std::sync::atomic::AtomicU64;
    static NEXT: AtomicU64 = AtomicU64::new(1);
    let executable = std::env::current_exe().unwrap();
    let probe = executable
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("minicon-platform-console-probe.exe");
    assert!(
        probe.is_file(),
        "workspace SDK probe missing: {}",
        probe.display()
    );
    for api in ["file", "wide"] {
        for kind in ["end", "eof"] {
            let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target/conhost-research")
                .join(format!(
                    "gui-native-{}-{}",
                    std::process::id(),
                    NEXT.fetch_add(1, Ordering::Relaxed)
                ));
            std::fs::create_dir_all(&folder).unwrap();
            let report = folder.join("result.txt");
            let (mut master, child) = ChildCommand::new(&probe)
                .arg(format!("synchronized-output-{api}-{kind}"))
                .arg(report.as_os_str())
                .size(TerminalSize { rows: 8, cols: 40 })
                .rust_console()
                .spawn()
                .unwrap()
                .into_parts();
            assert_eq!(master.backend_kind(), "rust-console");
            let reader = master.try_clone_for_startup_reader().unwrap();
            let mut t = ConTerminal::new(None, None);
            // Production spawn replaces the constructor's closed, idle pipe.
            // This fixture attaches its own PTY and must do the same.
            t.pty_output = Arc::new(BoundedOutputPipe::new(PTY_QUEUE_BYTES));
            t.cols = 40;
            t.rows = 8;
            t.cell_w = 8;
            t.cell_h = 16;
            t.frame_width = 320;
            t.frame_height = 128;
            t.parser.screen_mut().set_size(8, 40);
            let _ = t.parser.take_damage();
            t.master = Some(master);
            t.child = Some(child);
            let output = Arc::clone(&t.pty_output);
            let pump = std::thread::spawn(move || {
                let mut bytes = [0; 4096];
                loop {
                    match reader.io().read(&mut bytes) {
                        Ok(0) => break,
                        Ok(count) => {
                            if output.push_blocking(&bytes[..count]).is_err() {
                                break;
                            }
                        }
                        Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(e) => panic!("native GUI read: {e}"),
                    }
                }
                output.close();
            });
            stage(&mut t, &report, 0);
            assert_eq!(t.build_snapshot().rows_text[0], "MAIN-BASE");
            // Warm the real glyph path before starting the 250 ms frame policy.
            let before = capture(&mut t, &folder, "before");
            t.write_pty(b"a").unwrap();
            stage(&mut t, &report, 1);
            assert!(
                t.parser.is_holding(),
                "native frame exceeded GUI policy before checkpoint"
            );
            assert_eq!(t.build_snapshot().rows_text[0], "MAIN-BASE");
            assert_eq!(
                t.parser.active_screen().contents(),
                "SYNC-FRAME: λ中😀\nROW-TWO"
            );
            let held = capture(&mut t, &folder, "held");
            assert_eq!(held, before);
            let expected_units: Vec<_> = b"\x1b[3;6R\x1b[?2026;0$y"
                .iter()
                .copied()
                .map(u16::from)
                .collect();
            assert_eq!(
                std::fs::read_to_string(report.with_extension("replies")).unwrap(),
                format!("api={api};units={expected_units:?}\n")
            );
            // These successful native reads require drain_pty to have sent the
            // actual callbacks' replies through this session's real master.
            if kind == "end" {
                t.write_pty(b"e").unwrap();
                stage(&mut t, &report, 2);
                assert!(!t.parser.is_holding());
            }
            t.write_pty(b"q").unwrap();
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                t.drain_pty();
                let state = t.pty_output.drain(0, |_| {});
                if state.closed && !state.backlog {
                    break;
                }
                assert!(Instant::now() < deadline, "native GUI EOF stalled");
                std::thread::sleep(Duration::from_millis(1));
            }
            pump.join().unwrap();
            let mut waiter = t.child.as_ref().unwrap().try_clone_for_wait().unwrap();
            let status = waiter.wait().unwrap();
            assert_eq!(status.code(), Some(35));
            t.child_exit_code_encoded
                .store(encode_child_exit_code(status.code()), Ordering::Release);
            t.child_exit_pending.store(true, Ordering::Release);
            t.drain_pty();
            assert!(!t.parser.is_holding());
            let snapshot = t.build_snapshot();
            assert_eq!(snapshot.rows_text[0], "SYNC-FRAME: λ中😀");
            assert_eq!(snapshot.rows_text[1], "ROW-TWO");
            assert_eq!((snapshot.cursor.row, snapshot.cursor.col), (2, 5));
            assert!(!snapshot.child_alive);
            assert_eq!(snapshot.child_exit_code, Some(35));
            let after = capture(&mut t, &folder, "after");
            assert_ne!(after, before);
            let cleanup = t.child.as_ref().unwrap().terminate_forcefully().unwrap();
            assert!(cleanup.verified_empty);
            eprintln!(
                "gui-native api={api};kind={kind};archive={};exit=35;eof=true;empty=true",
                folder.display()
            );
        }
    }
}
