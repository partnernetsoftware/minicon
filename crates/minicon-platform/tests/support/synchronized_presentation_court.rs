//! Actual SDK producer -> opened owned PTY -> public common presentation API.
//! The consumer clock is scripted; this court makes no GUI timer/pixel claim.
use super::{ChildCommand, Fixture, Report, TerminalSize, tui_original, wait_native_read};
use minicon_console::terminal::{Callbacks, Color, Screen, TerminalPresentation, csi_query_reply};
use std::{
    path::Path,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Peer {
    phase: Option<usize>,
    replies: Vec<u8>,
}
impl Callbacks for Peer {
    fn set_window_title(&mut self, _: &mut Screen, title: &[u8]) {
        if let Some(phase) = std::str::from_utf8(title)
            .ok()
            .and_then(|s| s.strip_prefix("native-sync-"))
            .and_then(|s| s.parse().ok())
        {
            self.phase = Some(phase);
        }
    }
    fn unhandled_csi(
        &mut self,
        screen: &mut Screen,
        a: Option<u8>,
        b: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if let Some(reply) = csi_query_reply(screen, a, b, params, c) {
            self.replies.extend(reply);
        }
    }
}
struct Consumer {
    peer: TerminalPresentation<Peer>,
    original: vt100::Parser,
    now: Duration,
    replies: Vec<u8>,
    file: bool,
    consumed: usize,
    archived: usize,
}
impl Consumer {
    fn new(file: bool) -> Self {
        Self {
            peer: TerminalPresentation::with_callbacks(
                8,
                40,
                0,
                Duration::from_millis(100),
                4096,
                Peer::default(),
            ),
            original: vt100::Parser::new(8, 40, 0),
            now: Duration::from_millis(1),
            replies: Vec::new(),
            file,
            consumed: 0,
            archived: 0,
        }
    }
    fn feed(&mut self, fixture: &mut Fixture, chunk: &[u8]) {
        // Distinct arbitrary transport splitting is independent of native API writes.
        for span in chunk.chunks(if self.file { 1 } else { 37 }) {
            self.peer.process(span, self.now);
            let reply = std::mem::take(&mut self.peer.callbacks_mut().replies);
            if !reply.is_empty() {
                assert!(
                    self.peer.is_holding(),
                    "query was not serviced during a hold"
                );
                expected(self.peer.presented_screen(), true, false, (8, 40));
                expected(
                    self.peer.active_screen(),
                    false,
                    self.peer.active_screen().alternate_screen(),
                    (8, 40),
                );
                for byte in &reply {
                    fixture.master.write_all(&[*byte]).unwrap();
                }
                self.replies.extend(reply);
            }
        }
        self.consumed += chunk.len();
        fixture.bytes.extend_from_slice(chunk);
        assert!(fixture.bytes.len() < 65536);
    }
    fn stage(&mut self, fixture: &mut Fixture, report: &Report, phase: usize) {
        let deadline = Instant::now() + Duration::from_secs(12);
        while self.peer.callbacks().phase != Some(phase) {
            let chunk = fixture
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .unwrap_or_else(|e| {
                    panic!(
                        "native sync phase={phase}: {e};failure={:?};wire={:?}",
                        std::fs::read_to_string(report.0.with_extension("failure")),
                        String::from_utf8_lossy(&fixture.bytes)
                    )
                })
                .unwrap();
            self.feed(fixture, &chunk);
        }
    }
    fn archive(
        &mut self,
        fixture: &Fixture,
        report: &Report,
        phase: usize,
        base: bool,
        alt: bool,
        size: (u16, u16),
    ) {
        let data = std::fs::read(report.0.with_extension(format!("source{phase}"))).unwrap();
        tui_original(&mut self.original, &data);
        expected(self.original.screen(), base, alt, size);
        expected(self.peer.active_screen(), base, alt, size);
        let native =
            std::fs::read_to_string(report.0.with_extension(format!("native{phase}"))).unwrap();
        assert_native(&native, phase, base, size);
        let folder = report.0.parent().unwrap();
        std::fs::write(
            folder.join(format!("wire-{phase}.bin")),
            &fixture.bytes[self.archived..self.consumed],
        )
        .unwrap();
        self.archived = self.consumed;
        save(
            folder.join(format!("frame-{phase}.txt")).as_path(),
            &format!(
                "phase={phase}\nclock=scripted-monotonic\nnow_ns={}\nholding={}\n{}{}source_bytes={}\nwire_bytes_total={}\nnative={native:?}\n",
                self.now.as_nanos(),
                self.peer.is_holding(),
                screen_text("active", self.peer.active_screen()),
                screen_text("presented", self.peer.presented_screen()),
                data.len(),
                self.consumed,
            ),
        );
    }
    fn eof(&mut self, fixture: &mut Fixture) {
        let deadline = Instant::now() + Duration::from_secs(12);
        loop {
            match fixture
                .output
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            {
                Ok(Ok(chunk)) => self.feed(fixture, &chunk),
                Ok(Err(e)) => panic!("native sync output failed: {e}"),
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => break,
                Err(e) => panic!("native sync EOF did not arrive: {e}"),
            }
        }
        fixture.wait_exit(35);
    }
}

fn save(path: &Path, value: &str) {
    std::fs::write(path, value).unwrap();
}
fn screen_text(name: &str, screen: &Screen) -> String {
    format!(
        "{name}.contents={:?}\n{name}.cursor={:?}\n{name}.size={:?}\n{name}.alternate={}\n{name}.hidden={}\n{name}.wrap={}\n{name}.row_two_color={:?}\n",
        screen.contents(),
        screen.cursor_position(),
        screen.size(),
        screen.alternate_screen(),
        screen.hide_cursor(),
        screen.auto_wrap(),
        screen.cell(1, 0).unwrap().fgcolor()
    )
}
fn expected(screen: &Screen, base: bool, alternate: bool, size: (u16, u16)) {
    assert_eq!(screen.size(), size);
    assert_eq!(
        screen.contents(),
        if base {
            "MAIN-BASE"
        } else {
            "SYNC-FRAME: λ中😀\nROW-TWO"
        }
    );
    assert_eq!(screen.cursor_position(), if base { (1, 2) } else { (2, 5) });
    assert_eq!(screen.alternate_screen(), alternate);
    assert!(!screen.auto_wrap());
    assert_eq!(screen.hide_cursor(), !base);
    if !base {
        assert_eq!(screen.cell(0, 12).unwrap().contents(), "λ");
        assert_eq!(screen.cell(0, 13).unwrap().contents(), "中");
        assert_eq!(screen.cell(0, 15).unwrap().contents(), "😀");
        assert_eq!(screen.cell(1, 0).unwrap().fgcolor(), Color::Rgb(12, 34, 56));
    }
}
fn assert_native(native: &str, phase: usize, base: bool, (rows, cols): (u16, u16)) {
    let (row, col) = if base { (1, 2) } else { (2, 5) };
    assert!(
        native.starts_with(&format!("phase={phase};size={cols},{rows};")),
        "{native}"
    );
    assert!(
        native.contains(&format!(
            ";view=0,0,{},{};cursor={row},{col}\n",
            cols - 1,
            rows - 1
        )),
        "{native}"
    );
    for row in 0..rows {
        let text = match (base, row) {
            (true, 0) => "MAIN-BASE ",
            (false, 0) => "SYNC-FRAME",
            (false, 1) => "ROW-TWO   ",
            _ => "          ",
        };
        let units: Vec<_> = text.encode_utf16().collect();
        assert!(
            native
                .lines()
                .any(|line| line == format!("row{row}={units:?}")),
            "{native}"
        );
    }
}

pub(super) fn run(kinds: &[&str]) {
    for api in ["file", "wide"] {
        for kind in kinds {
            case(api, kind);
        }
    }
}
fn case(api: &str, kind: &str) {
    let report = Report::new();
    let command = ChildCommand::new(env!("CARGO_BIN_EXE_minicon-platform-console-probe"))
        .arg(format!("synchronized-output-{api}-{kind}"))
        .arg(report.0.as_os_str())
        .size(TerminalSize { rows: 8, cols: 40 })
        .rust_console();
    let mut fixture = Fixture::open(command);
    let mut consumer = Consumer::new(api == "file");
    consumer.stage(&mut fixture, &report, 0);
    expected(consumer.peer.presented_screen(), true, false, (8, 40));
    consumer.archive(&fixture, &report, 0, true, false, (8, 40));
    let _ = consumer.peer.take_damage();
    fixture.master.write_all(b"a").unwrap();
    consumer.stage(&mut fixture, &report, 1);
    assert!(consumer.peer.is_holding());
    assert_eq!(consumer.peer.deadline(), Some(Duration::from_millis(101)));
    assert!(consumer.peer.take_damage().is_empty());
    expected(consumer.peer.presented_screen(), true, false, (8, 40));
    consumer.archive(&fixture, &report, 1, false, kind == "alt", (8, 40));
    assert_eq!(consumer.replies, b"\x1b[3;6R\x1b[?2026;0$y");
    assert_eq!(
        std::fs::read_to_string(report.0.with_extension("replies")).unwrap(),
        format!(
            "api={api};units={:?}\n",
            b"\x1b[3;6R\x1b[?2026;0$y"
                .iter()
                .copied()
                .map(u16::from)
                .collect::<Vec<_>>()
        )
    );
    let admitted = wait_native_read(&fixture.child, false, true);
    assert_eq!(admitted.admitted_text, 1);
    let mut size = (8, 40);
    if kind == "timeout" {
        consumer.peer.poll(Duration::from_millis(100));
        assert!(consumer.peer.is_holding());
        expected(consumer.peer.presented_screen(), true, false, size);
        consumer.now = Duration::from_millis(101);
        consumer.peer.poll(consumer.now);
        assert!(!consumer.peer.is_holding());
        expected(consumer.peer.presented_screen(), false, false, size);
        assert_eq!(
            wait_native_read(&fixture.child, false, true).admitted_text,
            1
        );
    } else if kind == "resize" {
        size = (12, 56);
        fixture
            .master
            .resize(TerminalSize {
                rows: size.0,
                cols: size.1,
            })
            .unwrap();
        consumer.peer.edit_screen(|s| s.set_size(size.0, size.1));
        consumer.original.screen_mut().set_size(size.0, size.1);
        assert!(!consumer.peer.is_holding());
        expected(consumer.peer.presented_screen(), false, false, size);
        assert_eq!(
            wait_native_read(&fixture.child, false, true).admitted_text,
            1
        );
    }
    fixture
        .master
        .write_all(if kind == "eof" { b"q" } else { b"e" })
        .unwrap();
    if kind != "eof" {
        consumer.stage(&mut fixture, &report, 2);
        assert!(!consumer.peer.is_holding());
        expected(consumer.peer.presented_screen(), false, kind == "alt", size);
        assert!(consumer.peer.take_damage().needs_full_raster());
        consumer.archive(&fixture, &report, 2, false, kind == "alt", size);
        fixture.master.write_all(b"q").unwrap();
    }
    consumer.eof(&mut fixture);
    assert_eq!(consumer.peer.callbacks().phase, Some(3));
    if kind == "eof" {
        assert!(consumer.peer.is_holding());
        expected(consumer.peer.presented_screen(), true, false, size);
    }
    consumer.peer.finish();
    assert!(!consumer.peer.is_holding());
    expected(consumer.peer.presented_screen(), kind == "alt", false, size);
    consumer.archive(&fixture, &report, 3, kind == "alt", false, size);
    assert!(fixture.child.native_input_read_state().is_err());
    let cleanup = fixture.child.terminate_forcefully().unwrap();
    assert!(cleanup.verified_empty);
    save(
        report.0.parent().unwrap().join("case.txt").as_path(),
        &format!(
            "api={api}\nkind={kind}\nexit=35\neof=true\nverified_empty={}\nadmitted_text={}\nwire_bytes={}\nreplies={:?}\nclock=scripted-monotonic\n",
            cleanup.verified_empty, admitted.admitted_text, consumer.consumed, consumer.replies
        ),
    );
    eprintln!(
        "native-sync-case api={api};kind={kind};archive={};exit=35;eof=true;verified_empty={}",
        report.0.parent().unwrap().display(),
        cleanup.verified_empty
    );
}
