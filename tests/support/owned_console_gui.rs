//! Explicit product selection and actual native-window owned-PTY journey.
use super::*;

#[test]
fn gui_owned_console_offline_selection_reports_or_rejects() {
    let exe = minicon_binary();
    let output = Command::new(&exe)
        .args(["--status", "--feature=rust-console"])
        .output()
        .unwrap();
    if cfg!(all(windows, feature = "experimental-rust-console")) {
        assert!(output.status.success(), "{}", error_text(&output));
        assert!(output_text(&output).contains("pty backend    rust-console"));
        let output = Command::new(&exe)
            .args(["--status", "--feature=rust-console,conpty"])
            .env_remove("AGENTERM_FORCE_CONSOLE_AGENT")
            .env_remove("FORCE_CONSOLE_AGENT")
            .output()
            .unwrap();
        assert!(output.status.success(), "{}", error_text(&output));
        assert!(!output_text(&output).contains("pty backend    rust-console"));
    } else {
        assert_eq!(output.status.code(), Some(2));
        assert!(
            error_text(&output)
                .contains("requires Windows and a build with experimental-rust-console")
        );
        let output = Command::new(&exe)
            .args(["--feature=rust-console", "-e", "unused-program"])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(2));
    }
}

#[cfg(all(windows, feature = "experimental-rust-console"))]
#[test]
fn gui_owned_console_native_window_queries_frames_seed_and_cleanup() {
    use minicon_platform::process_reference::{ProcessReference, ProcessWait};
    fn checkpoint(path: &Path, phase: usize) {
        let deadline = Instant::now() + Duration::from_secs(8);
        while !path.with_extension(format!("native{phase}")).is_file() {
            assert!(
                Instant::now() < deadline,
                "native GUI checkpoint {phase}: {:?}",
                fs::read_to_string(path.with_extension("failure"))
            );
            thread::sleep(Duration::from_millis(1));
        }
    }
    fn image(path: &Path) -> (Vec<u8>, png::OutputInfo) {
        let decoder = png::Decoder::new(fs::File::open(path).unwrap());
        let mut reader = decoder.read_info().unwrap();
        let mut pixels = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut pixels).unwrap();
        assert!(matches!(
            info.color_type,
            png::ColorType::Rgba | png::ColorType::Rgb
        ));
        (pixels, info)
    }
    fn row_pixels(path: &Path, geometry: &Value) -> Vec<u8> {
        let (pixels, info) = image(path);
        let channels = info.color_type.samples();
        let x = geometry["terminal_viewport"]["x"].as_u64().unwrap() as usize;
        let y = geometry["terminal_viewport"]["y"].as_u64().unwrap() as usize;
        let width = (geometry["grid"]["cols"].as_u64().unwrap()
            * geometry["grid"]["cell_w"].as_u64().unwrap()) as usize;
        let height = geometry["grid"]["cell_h"].as_u64().unwrap() as usize;
        assert!(x + width <= info.width as usize && y + height <= info.height as usize);
        (y..y + height)
            .flat_map(|row| {
                pixels[(row * info.width as usize + x) * channels
                    ..(row * info.width as usize + x + width) * channels]
                    .iter()
                    .copied()
            })
            .collect()
    }
    fn title_pixels(path: &Path, geometry: &Value) -> Vec<u8> {
        let (pixels, info) = image(path);
        let channels = info.color_type.samples();
        let band = &geometry["composer_band"];
        let x = band["x"].as_u64().unwrap() as usize;
        let y = band["y"].as_u64().unwrap() as usize;
        let width = band["width"].as_u64().unwrap() as usize / 3;
        let height = geometry["grid"]["cell_h"].as_u64().unwrap() as usize;
        assert!(x + width <= info.width as usize && y + height <= info.height as usize);
        (y..y + height)
            .chain(info.height as usize - 2 * height..info.height as usize)
            .flat_map(|row| {
                pixels[(row * info.width as usize + x) * channels
                    ..(row * info.width as usize + x + width) * channels]
                    .iter()
                    .copied()
            })
            .collect()
    }
    fn terminal_pixels(path: &Path, geometry: &Value) -> Vec<u8> {
        let (pixels, info) = image(path);
        let channels = info.color_type.samples();
        let x = geometry["terminal_viewport"]["x"].as_u64().unwrap() as usize;
        let y = geometry["terminal_viewport"]["y"].as_u64().unwrap() as usize;
        let width = (geometry["grid"]["cols"].as_u64().unwrap()
            * geometry["grid"]["cell_w"].as_u64().unwrap()) as usize;
        let height = (geometry["grid"]["rows"].as_u64().unwrap()
            * geometry["grid"]["cell_h"].as_u64().unwrap()) as usize;
        assert!(x + width <= info.width as usize && y + height <= info.height as usize);
        (y..y + height)
            .flat_map(|row| {
                pixels[(row * info.width as usize + x) * channels
                    ..(row * info.width as usize + x + width) * channels]
                    .iter()
                    .copied()
            })
            .collect()
    }
    fn native_client_agreement(
        gui: &mut OwnedGui,
        folder: &Path,
        expected: &Path,
        geometry: &Value,
    ) {
        use minicon_platform::{
            process_window,
            screenshot::{NativeCaptureArea, ScreenshotWindowHandle, capture_native_window_png},
        };
        let expected_terminal = terminal_pixels(expected, geometry);
        let expected_titles = title_pixels(expected, geometry);
        let path = folder.join("native-client.png");
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut attempts = 0;
        loop {
            assert!(
                gui.child.try_wait().unwrap().is_none(),
                "owned host exited before client readback"
            );
            // Exact fixture PID only; no process inventory or foreground capture.
            let facts = process_window::facts(gui.child.id());
            assert!(
                facts.present && facts.window_id != 0 && facts.title.contains("native-sync-3"),
                "owned native window identity/title unavailable: {facts:?}"
            );
            // SAFETY: this fixture owns the live host and keeps its window open
            // through this synchronous read. Destruction is requested below,
            // after all captures and stable-identity checks have completed.
            let handle =
                unsafe { ScreenshotWindowHandle::from_raw(facts.window_id as isize) }.unwrap();
            capture_native_window_png(
                handle,
                &path,
                NativeCaptureArea::Client {
                    left: 0,
                    top: 0,
                    width: geometry["frame"]["width"].as_u64().unwrap() as i32,
                    height: geometry["frame"]["height"].as_u64().unwrap() as i32,
                },
            )
            .unwrap();
            attempts += 1;
            let after = process_window::facts(gui.child.id());
            assert_eq!(
                facts.window_id, after.window_id,
                "owned HWND changed during capture"
            );
            if terminal_pixels(&path, geometry) == expected_terminal
                && title_pixels(&path, geometry) == expected_titles
            {
                let evidence = serde_json::json!({"host_pid":gui.child.id(),"window_id":facts.window_id,"window_title":facts.title,
                    "source":"GetDC(owned HWND)/BitBlt/GetDIBits","attempts":attempts,"terminal_equal":true,"titles_equal":true});
                fs::write(
                    folder.join("native-client.json"),
                    serde_json::to_vec_pretty(&evidence).unwrap(),
                )
                .unwrap();
                return;
            }
            assert!(
                Instant::now() < deadline,
                "OS client readback did not match owned frame; attempts={attempts};archive={}",
                folder.display()
            );
            thread::sleep(Duration::from_millis(20));
        }
    }
    let exe = minicon_binary();
    let probe = exe
        .parent()
        .unwrap()
        .join("minicon-platform-console-probe.exe");
    assert!(probe.is_file(), "SDK probe missing");
    for api in ["file", "wide"] {
        for kind in ["end", "eof"] {
            let suffix = unique_suffix();
            let endpoint = control_endpoint(&suffix);
            let folder = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                .join("target/conhost-research")
                .join(format!("owned-window-{suffix}-{api}-{kind}"));
            fs::create_dir_all(&folder).unwrap();
            let report = folder.join("result.txt");
            let trace = folder.join("host.log");
            let child = Command::new(&exe)
                .args([
                    "--no-activate",
                    "--feature=rust-console",
                    "--control",
                    &endpoint,
                    "-e",
                ])
                .arg(&probe)
                .arg(format!("synchronized-output-{api}-window-{kind}"))
                .arg(&report)
                .stdout(std::process::Stdio::null())
                .stderr(fs::File::create(&trace).unwrap())
                .spawn()
                .unwrap();
            let mut gui = OwnedGui {
                child,
                screenshot: folder.join("unused.png"),
            };
            let listed = wait_until_ready_for(
                &exe,
                &endpoint,
                Duration::from_secs(15),
                Some(&mut gui.child),
            );
            let tab = tab_id(&listed["tabs"][0]["id"]).to_owned();
            checkpoint(&report, 0);
            cli_json(&exe, &endpoint, &["wait-text", "MAIN-BASE"]);
            // Keep PNG work bounded without changing the presentation policy.
            // The SDK uses actual viewport dimensions and needs three rows
            // and ten columns for its fixed coordinates/readback.
            cli_json(
                &exe,
                &endpoint,
                &["resize-window", "--width", "640", "--height", "400"],
            );
            let resize_deadline = Instant::now() + Duration::from_secs(8);
            let ui = loop {
                let ui = cli_json(&exe, &endpoint, &["ui-snapshot"]);
                let geometry = &ui["geometry"];
                let scale = geometry["scale_permille"].as_u64().unwrap_or(0);
                let width = geometry["frame"]["width"].as_u64().unwrap_or(0);
                let height = geometry["frame"]["height"].as_u64().unwrap_or(0);
                let cols = geometry["grid"]["cols"].as_u64().unwrap_or(0);
                let rows = geometry["grid"]["rows"].as_u64().unwrap_or(0);
                let cell_w = geometry["grid"]["cell_w"].as_u64().unwrap_or(0);
                let cell_h = geometry["grid"]["cell_h"].as_u64().unwrap_or(0);
                if scale > 0
                    && (width * 1000 + scale / 2) / scale == 640
                    && (height * 1000 + scale / 2) / scale == 400
                    && cols >= 10
                    && rows >= 3
                    && cell_w > 0
                    && cell_h > 0
                    && cols * cell_w <= geometry["terminal_viewport"]["width"].as_u64().unwrap_or(0)
                    && rows * cell_h
                        <= geometry["terminal_viewport"]["height"]
                            .as_u64()
                            .unwrap_or(0)
                {
                    break ui;
                }
                assert!(
                    Instant::now() < resize_deadline,
                    "owned fixture resize did not settle at 640x400 logical pixels: {ui}"
                );
                thread::sleep(Duration::from_millis(10));
            };
            let geometry = &ui["geometry"];
            assert_eq!(geometry["pty_backend"], "rust-console");
            assert!(geometry["grid"]["rows"].as_u64().unwrap() >= 3);
            assert!(geometry["grid"]["cols"].as_u64().unwrap() >= 10);
            fs::write(
                folder.join("geometry.json"),
                serde_json::to_vec_pretty(&ui).unwrap(),
            )
            .unwrap();
            let pid: u32 = fs::read_to_string(report.with_extension("pid"))
                .unwrap()
                .parse()
                .unwrap();
            let client = ProcessReference::open(pid).unwrap();
            let before = folder.join("before.png");
            let held = folder.join("held.png");
            let after = folder.join("after.png");
            cli_json(
                &exe,
                &endpoint,
                &["screenshot-pane", "--output", before.to_str().unwrap()],
            );
            let before_row = row_pixels(&before, geometry);
            let expected: Vec<u16> = b"\x1b[3;6R\x1b[?2026;0$y"
                .iter()
                .copied()
                .map(u16::from)
                .collect();
            let expected_replies = format!("api={api};units={expected:?}\n");
            let replies = report.with_extension("replies");
            // PRD 24.01's bounded hold is an observation window, not a wait
            // for the SDK's slower per-row native readback to finish.
            let observation_window = Duration::from_millis(250);
            let started = Instant::now();
            cli_json(&exe, &endpoint, &["send-text", "a"]);
            while !replies.is_file() {
                assert!(
                    started.elapsed() < observation_window,
                    "BLOCKED: SDK query reply missed the held-frame observation window;api={api};kind={kind};elapsed={:?};archive={}",
                    started.elapsed(),
                    folder.display()
                );
                thread::sleep(Duration::from_millis(1));
            }
            // These native reads prove the GUI parsed the new active frame.
            // The producer publishes this marker before its row-by-row SDK
            // snapshot, which is checked after the time-sensitive captures.
            assert_eq!(fs::read_to_string(&replies).unwrap(), expected_replies);
            let reply_elapsed = started.elapsed();
            // Both are public, read-only observations. Start their CLI
            // processes together; each records its own completion, while the
            // GUI retains ownership of request ordering and frame state.
            let ((text, text_elapsed), png_elapsed) = thread::scope(|scope| {
                let text = scope.spawn(|| {
                    let text = cli_text(&exe, &endpoint, &["capture-pane"]);
                    (text, started.elapsed())
                });
                let png = scope.spawn(|| {
                    cli_json(
                        &exe,
                        &endpoint,
                        &["screenshot-pane", "--output", held.to_str().unwrap()],
                    );
                    started.elapsed()
                });
                (text.join().unwrap(), png.join().unwrap())
            });
            assert!(
                text_elapsed < observation_window,
                "BLOCKED: text capture missed the held-frame observation window;api={api};kind={kind};reply={reply_elapsed:?};capture={text_elapsed:?};archive={}",
                folder.display()
            );
            assert!(text.starts_with("MAIN-BASE"), "{text}");
            eprintln!(
                "held-observation api={api};kind={kind};reply={reply_elapsed:?};capture={text_elapsed:?};png={png_elapsed:?};bound={observation_window:?}"
            );
            assert!(
                png_elapsed < observation_window,
                "BLOCKED: PNG capture missed the held-frame observation window;api={api};kind={kind};elapsed={png_elapsed:?};archive={}",
                folder.display()
            );
            assert_eq!(
                row_pixels(&held, geometry),
                before_row,
                "held terminal row exposed active state"
            );
            fs::write(
                folder.join("held.json"),
                serde_json::to_vec_pretty(&text).unwrap(),
            )
            .unwrap();
            fs::write(
                folder.join("hold-observation.json"),
                serde_json::to_vec_pretty(&serde_json::json!({
                    "query_reply_ms": reply_elapsed.as_secs_f64() * 1000.0,
                    "capture_ms": text_elapsed.as_secs_f64() * 1000.0,
                    "held_png_ms": png_elapsed.as_secs_f64() * 1000.0,
                    "bound_ms": observation_window.as_millis(),
                }))
                .unwrap(),
            )
            .unwrap();
            checkpoint(&report, 1);
            assert_eq!(
                fs::read_to_string(report.with_extension("replies")).unwrap(),
                expected_replies
            );
            if kind == "end" {
                cli_json(&exe, &endpoint, &["send-text", "e"]);
                checkpoint(&report, 2);
            }
            cli_json(&exe, &endpoint, &["send-text", "q"]);
            let exit = cli_json(
                &exe,
                &endpoint,
                &["wait-tab-exit", "--target", &tab, "--timeout-ms", "8000"],
            );
            fs::write(
                folder.join("exit.json"),
                serde_json::to_vec_pretty(&exit).unwrap(),
            )
            .unwrap();
            let tabs = cli_json(&exe, &endpoint, &["list-tabs"]);
            assert_eq!(tabs["tabs"][0]["child_exit_code"], 35);
            cli_json(&exe, &endpoint, &["wait-text", "SYNC-FRAME: λ中😀"]);
            cli_json(
                &exe,
                &endpoint,
                &["screenshot-pane", "--output", after.to_str().unwrap()],
            );
            assert_ne!(row_pixels(&after, geometry), before_row);
            let titles = cli_json(&exe, &endpoint, &["list-tabs"]);
            assert_eq!(titles["tabs"][0]["title"], "native-sync-3");
            let ui_before = cli_json(&exe, &endpoint, &["ui-snapshot"]);
            assert_eq!(ui_before["composer_focused"], false);
            native_client_agreement(&mut gui, &folder, &after, geometry);
            // A click in the exited terminal keeps terminal focus and asks
            // for the composer/status band to repaint. Current-title pixels
            // must already match that independent repaint, without OCR or
            // comparing unrelated cursor/IME/clipboard regions.
            let scale = geometry["scale_permille"].as_u64().unwrap();
            let column = ((geometry["terminal_viewport"]["x"].as_u64().unwrap() + 2) * 1000
                / scale)
                .to_string();
            let row = ((geometry["terminal_viewport"]["y"].as_u64().unwrap() + 2) * 1000 / scale)
                .to_string();
            cli_json(
                &exe,
                &endpoint,
                &[
                    "send-ui-mouse",
                    "--action",
                    "click",
                    "--button",
                    "left",
                    "--column",
                    &column,
                    "--row",
                    &row,
                ],
            );
            assert_eq!(
                cli_json(&exe, &endpoint, &["ui-snapshot"])["composer_focused"],
                false
            );
            let repainted = folder.join("title-repaint.png");
            cli_json(
                &exe,
                &endpoint,
                &["screenshot-pane", "--output", repainted.to_str().unwrap()],
            );
            assert!(
                title_pixels(&after, geometry) == title_pixels(&repainted, geometry),
                "title pixels changed only after an unrelated band repaint"
            );
            assert_eq!(
                client.wait_for_exit(Some(Duration::from_secs(8))).unwrap(),
                ProcessWait::Exited
            );
            // Reopen from an empty workspace: selection must survive seed
            // creation without carrying any old PTY/process handle.
            cli_json(&exe, &endpoint, &["close-tab", "--target", &tab]);
            for phase in [0, 1, 2, 3] {
                for ext in ["source", "native"] {
                    let p = report.with_extension(format!("{ext}{phase}"));
                    if p.exists() {
                        fs::rename(&p, folder.join(format!("first-{ext}{phase}"))).unwrap();
                    }
                }
            }
            fs::rename(report.with_extension("pid"), folder.join("first.pid")).unwrap();
            cli_json(&exe, &endpoint, &["new-tab"]);
            checkpoint(&report, 0);
            let seeded = cli_json(&exe, &endpoint, &["ui-snapshot"]);
            assert_eq!(seeded["geometry"]["pty_backend"], "rust-console");
            let pid: u32 = fs::read_to_string(report.with_extension("pid"))
                .unwrap()
                .parse()
                .unwrap();
            let waiting = ProcessReference::open(pid).unwrap();
            let seeded_tabs = cli_json(&exe, &endpoint, &["list-tabs"]);
            let seeded_tab = tab_id(&seeded_tabs["tabs"][0]["id"]);
            cli_json(&exe, &endpoint, &["close-tab", "--target", seeded_tab]);
            assert_eq!(
                waiting.wait_for_exit(Some(Duration::from_secs(8))).unwrap(),
                ProcessWait::Exited,
                "closing a waiting tab leaked its SDK process"
            );
            cli_json(&exe, &endpoint, &["close-window"]);
            let deadline = Instant::now() + Duration::from_secs(8);
            loop {
                if let Some(status) = gui.child.try_wait().unwrap() {
                    assert!(
                        status.success(),
                        "{status};trace={:?}",
                        fs::read_to_string(&trace)
                    );
                    break;
                }
                assert!(Instant::now() < deadline, "GUI close stalled");
                thread::sleep(Duration::from_millis(10));
            }
            eprintln!(
                "owned-window api={api};kind={kind};archive={};client_exit=35;seed_waiter_exited=true;host_exit=0",
                folder.display()
            );
        }
    }
}
