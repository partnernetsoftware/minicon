use super::*;

fn terminal() -> ConTerminal {
    let mut t = ConTerminal::new(None, None);
    t.cols = 8;
    t.rows = 3;
    t.cell_w = 8;
    t.cell_h = 16;
    t.frame_width = 64;
    t.frame_height = 48;
    t.parser.screen_mut().set_size(3, 8);
    let _ = t.parser.take_damage();
    t.pty_output = Arc::new(BoundedOutputPipe::new(4096));
    t.dirty = DirtyRegion::empty();
    t
}
fn feed(t: &mut ConTerminal, bytes: &[u8]) -> DrainOutcome {
    t.pty_output.push_blocking(bytes).unwrap();
    t.drain_pty()
}
pub(super) fn pixels(t: &mut ConTerminal) -> Vec<u32> {
    let mut pixels = vec![0; (t.frame_width * t.frame_height) as usize];
    t.paint_presented_terminal(&mut Surface::new(
        &mut pixels,
        t.frame_width,
        t.frame_height,
    ));
    pixels
}

#[test]
fn gui_synchronized_pixels_snapshot_selection_and_live_input_keep_their_owners() {
    let mut t = terminal();
    feed(&mut t, b"\x1b[48;2;17;34;51m\x1b[2J\x1b[HOLD\x1b[?25l");
    t.selection = Some((
        TerminalPoint { row: 0, col: 0 },
        TerminalPoint { row: 0, col: 2 },
    ));
    let before = pixels(&mut t);
    assert_eq!(before[40 * 64 + 60], Rgb(17, 34, 51).to_xrgb());
    t.dirty = DirtyRegion::empty();
    let held = feed(&mut t,b"\x1b[?2026h\x1b[?1h\x1b[?2004h\x1b[?1002h\x1b[?1006h\x1b[48;2;68;85;102m\x1b[2J\x1b[HNEW\x1b[2;3H");
    assert!(t.parser.is_holding());
    assert!(!held.changed && !held.redraw);
    assert!(t.dirty.is_empty());
    assert_eq!(pixels(&mut t), before);
    let snapshot = t.build_snapshot();
    assert_eq!(snapshot.rows_text[0], "OLD");
    assert_eq!((snapshot.cursor.row, snapshot.cursor.col), (0, 3));
    assert!(snapshot.selection.is_some());
    assert!(t.screen_contains("OLD") && !t.screen_contains("NEW"));
    assert_eq!(
        selection_text(
            t.parser.screen(),
            TerminalPoint { row: 0, col: 0 },
            TerminalPoint { row: 0, col: 2 }
        ),
        "OLD"
    );
    assert!(t.parser.active_screen().bracketed_paste());
    let up = injected_key_event(InjectedKey::Named(NamedKey::ArrowUp), false, false, false);
    assert_eq!(
        terminal_input::key_event_to_bytes(&up, t.input_key_mode()).unwrap(),
        b"\x1bOA"
    );
    assert_eq!(
        t.mouse_mode(),
        (
            terminal_input::ApplicationMouseMode::ButtonMotion,
            terminal_input::MouseReportEncoding::Sgr
        )
    );
    t.parser.process(b"\x1b[6n\x1b[?2026$p");
    assert_eq!(
        std::mem::take(&mut t.parser.callbacks_mut().pending_replies),
        b"\x1b[2;3R\x1b[?2026;0$y"
    );
    assert_eq!(pixels(&mut t), before);
    let released = feed(&mut t, b"\x1b[?2026l");
    assert!(released.changed && released.redraw);
    assert!(t.selection.is_none());
    assert_eq!(t.build_snapshot().rows_text[0], "NEW");
    let after = pixels(&mut t);
    assert_ne!(after, before);
    assert_eq!(after[40 * 64 + 60], Rgb(68, 85, 102).to_xrgb());
}

#[test]
fn gui_synchronized_idle_deadline_recovers_after_child_exit_without_consuming_tail_early() {
    let mut t = terminal();
    feed(&mut t, b"OLD\x1b[?25l");
    feed(&mut t, b"\x1b[?2026h\rNEW");
    let deadline = t.parser.deadline().unwrap();
    t.child_gone = true;
    assert_eq!(
        t.service_presentation(deadline - Duration::from_millis(1))
            .1,
        Some(deadline)
    );
    assert_eq!(t.build_snapshot().rows_text[0], "OLD");
    let (changed, next) = t.service_presentation(deadline);
    assert!(changed && next.is_none());
    assert_eq!(t.build_snapshot().rows_text[0], "NEW");
    assert!(t.dirty.is_full());
    // A closed ring may still contain bytes, including a partial VT command.
    let mut t = terminal();
    feed(&mut t, b"OLD");
    t.pty_output
        .push_blocking(b"\x1b[?2026h\rNEW\x1b[2;3HX")
        .unwrap();
    t.pty_output.close();
    t.child_exit_pending.store(true, Ordering::Release);
    let first = t.drain_pty_with_budget(13);
    assert!(first.backlog && first.child_exited);
    assert!(t.parser.is_holding());
    assert_eq!(t.build_snapshot().rows_text[0], "OLD");
    let last = t.drain_pty();
    assert!(!last.backlog && !t.parser.is_holding());
    assert_eq!(t.build_snapshot().rows_text, vec!["NEW", "  X", ""]);
    assert_eq!(t.parser.screen().cursor_position(), (1, 3));
}

#[test]
fn gui_synchronized_explicit_resize_keeps_decoder_and_published_cursor_consistent() {
    let mut t = terminal();
    feed(&mut t, b"OLD");
    feed(&mut t, b"\x1b[?2026h\rNEW\x1b[2;");
    // Use the same direct screen-edit boundary as settled GUI geometry.
    t.parser.screen_mut().set_size(4, 10);
    assert!(!t.parser.is_holding());
    t.rows = 4;
    t.cols = 10;
    feed(&mut t, b"3HX");
    let snapshot = t.build_snapshot();
    assert_eq!((snapshot.rows, snapshot.cols), (4, 10));
    assert_eq!(snapshot.rows_text[0], "NEW");
    assert_eq!(snapshot.rows_text[1], "  X");
    assert_eq!((snapshot.cursor.row, snapshot.cursor.col), (1, 3));
}
