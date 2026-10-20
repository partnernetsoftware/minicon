//! Public common-library consumer; literal frame/reply oracles, no native host.
use minicon_console::terminal::{Callbacks, Color, Screen, TerminalPresentation, csi_query_reply};
use std::time::Duration;

#[derive(Default)]
struct Events {
    replies: Vec<u8>,
    titles: Vec<String>,
    phases: Vec<(bool, bool)>,
    resets: usize,
}
impl Callbacks for Events {
    fn synchronized_update(&mut self, screen: &mut Screen, enabled: bool) {
        self.phases.push((enabled, screen.bracketed_paste()));
    }
    fn reset(&mut self, _: &mut Screen) {
        self.resets += 1;
    }
    fn set_window_title(&mut self, _: &mut Screen, title: &[u8]) {
        self.titles.push(String::from_utf8(title.to_vec()).unwrap());
    }
    fn unhandled_csi(
        &mut self,
        screen: &mut Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        if let Some(reply) = csi_query_reply(screen, i1, i2, params, c) {
            self.replies.extend(reply);
        }
    }
}
fn terminal(timeout_ms: u64, bytes: usize) -> TerminalPresentation<Events> {
    let mut terminal = TerminalPresentation::with_callbacks(
        3,
        16,
        4,
        Duration::from_millis(timeout_ms),
        bytes,
        Events::default(),
    );
    terminal.process(b"OLD", Duration::ZERO);
    let _ = terminal.take_damage();
    terminal
}
fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}
const BEGIN: &[u8] = b"\x1b[?2026h";
const END: &[u8] = b"\x1b[?2026l";

#[test]
fn synchronized_presentation_split_frames_keep_queries_and_titles_live() {
    let body = "\x1b[?2026h\x1b[2J\x1b[H新界\x1b[38;2;3;5;7mZ\x1b[6n\x1b[?2026$p\x1b]2;held\x07"
        .as_bytes();
    for split in 0..=body.len() {
        for end_split in 0..=END.len() {
            let mut t = terminal(100, 4096);
            t.process(&body[..split], ms(1));
            t.process(&body[split..], ms(2));
            assert!(t.is_holding());
            assert_eq!(t.presented_screen().contents(), "OLD");
            assert_eq!(t.presented_screen().cursor_position(), (0, 3));
            assert_eq!(t.active_screen().contents(), "新界Z");
            assert_eq!(t.active_screen().cursor_position(), (0, 5));
            assert_eq!(t.callbacks().replies, b"\x1b[1;6R\x1b[?2026;0$y");
            assert_eq!(t.callbacks().titles, ["held"]);
            assert!(t.take_damage().is_empty());
            t.process(&END[..end_split], ms(3));
            if end_split != END.len() {
                assert_eq!(t.presented_screen().contents(), "OLD");
            }
            t.process(&END[end_split..], ms(4));
            assert!(!t.is_holding());
            assert_eq!(t.presented_screen().contents(), "新界Z");
            assert_eq!(t.presented_screen().cursor_position(), (0, 5));
            assert_eq!(
                t.presented_screen().cell(0, 4).unwrap().fgcolor(),
                Color::Rgb(3, 5, 7)
            );
            assert!(t.take_damage().needs_full_raster());
            assert!(t.take_damage().is_empty());
        }
    }
    // Internal parser spans are not externally presented frames. A begin in
    // a later span must retain the last frame exposed before this call.
    let mut t = terminal(100, 16_384);
    let mut burst = vec![b'x'; 5000];
    burst.extend_from_slice(BEGIN);
    burst.extend_from_slice(b"PARTIAL");
    t.process(&burst, ms(1));
    assert!(t.is_holding());
    assert_eq!(t.presented_screen().contents(), "OLD");
    t.process(END, ms(2));
    assert_eq!(
        t.presented_screen().contents(),
        t.active_screen().contents()
    );
}

#[test]
fn synchronized_presentation_timeout_cannot_be_renewed_or_rearmed_without_end() {
    let mut t = terminal(100, 4096);
    t.process(BEGIN, ms(10));
    t.process(b"\rNEW", ms(20));
    t.process(BEGIN, ms(90));
    assert_eq!(t.deadline(), Some(ms(110)));
    t.poll(ms(109));
    assert_eq!(t.presented_screen().contents(), "OLD");
    t.poll(ms(110));
    assert_eq!(t.presented_screen().contents(), "NEW");
    assert!(!t.is_holding());
    t.process(BEGIN, ms(111));
    t.process(b"!", ms(112));
    assert_eq!(t.presented_screen().contents(), "NEW!");
    assert!(!t.is_holding());
    t.process(END, ms(113));
    t.process(BEGIN, ms(120));
    assert_eq!(t.deadline(), Some(ms(220)));
    t.poll(ms(1));
    t.process(BEGIN, ms(2));
    assert_eq!(t.deadline(), Some(ms(220)));
    t.poll(ms(219));
    assert!(t.is_holding());
    t.poll(ms(220));
    assert!(!t.is_holding());
    t.process(END, ms(1));
    t.process(BEGIN, ms(2));
    assert_eq!(t.deadline(), Some(ms(320)));
}

#[test]
fn synchronized_presentation_byte_quota_recovers_and_preserves_decoder() {
    let mut t = terminal(1000, 12);
    t.process(BEGIN, ms(1)); // Eight bytes count against this phase.
    t.process(b"\rN", ms(2));
    assert!(t.is_holding());
    assert_eq!(t.presented_screen().contents(), "OLD");
    t.process(&[0xe7, 0x95], ms(3)); // At quota, in the middle of U+754C.
    assert!(!t.is_holding());
    assert_eq!(t.presented_screen().contents(), "NLD");
    t.process(&[0x8c], ms(4));
    assert_eq!(t.presented_screen().contents(), "N界");
    t.process(BEGIN, ms(5));
    assert!(!t.is_holding());
    t.process(END, ms(6));
    t.process(BEGIN, ms(7));
    assert!(t.is_holding());
    t.process(&vec![b'x'; 10_000], ms(8));
    assert!(!t.is_holding());
    assert_eq!(
        t.presented_screen().contents(),
        t.active_screen().contents()
    );
}

#[test]
fn synchronized_presentation_private_parameters_keep_order_and_invalid_markers_do_not_hold() {
    let mut t = terminal(100, 4096);
    t.process(b"\x1b[?2004;2026h\rNEW", ms(1));
    assert_eq!(t.callbacks().phases, [(true, true)]);
    assert!(!t.presented_screen().bracketed_paste());
    assert!(t.active_screen().bracketed_paste());
    t.process(b"\x1b[?2004;2026l", ms(2));
    assert_eq!(t.callbacks().phases, [(true, true), (false, false)]);
    assert_eq!(t.presented_screen().contents(), "NEW");
    for invalid in [
        b"\x1b[?2026:1h".as_slice(),
        b"\x1b[?2026$h",
        b"\x1b[2026h",
        b"\x1b[?2026$ h",
    ] {
        t.process(invalid, ms(3));
        assert!(!t.is_holding(), "{invalid:?}");
    }
    assert_eq!(t.callbacks().phases.len(), 2);
    // ESC cancels an unfinished OSC; the following CSI is a real command.
    t.process(b"\x1b]2;payload \x1b[?2026h\x07", ms(4));
    assert!(t.is_holding());
    assert!(t.callbacks().titles.is_empty());
    assert_eq!(t.callbacks().phases.len(), 3);
    t.process(END, ms(5));
    assert!(!t.is_holding());
}

#[test]
fn synchronized_presentation_resize_reset_and_eof_release_without_losing_input_grammar() {
    let mut t = terminal(100, 4096);
    t.process(b"\x1b[?2026h\rNEW\x1b[2;", ms(1));
    t.edit_screen(|screen| screen.set_size(4, 20));
    assert!(!t.is_holding());
    assert_eq!(t.presented_screen().size(), (4, 20));
    assert_eq!(t.presented_screen().contents(), "NEW");
    t.process(b"3HX", ms(2));
    assert_eq!(t.presented_screen().cell(1, 2).unwrap().contents(), "X");
    t.process(END, ms(3));
    t.process(BEGIN, ms(4));
    t.process(b"\x1bc", ms(5));
    assert!(!t.is_holding());
    assert_eq!(t.callbacks().resets, 1);
    assert_eq!(t.presented_screen().contents(), "");
    t.process(b"\x1b[?2026hDONE\x1b[2;", ms(6));
    assert_eq!(t.presented_screen().contents(), "");
    t.finish();
    assert!(!t.is_holding());
    assert_eq!(t.presented_screen().contents(), "DONE");
    assert!(t.take_damage().needs_full_raster());
}

#[test]
fn synchronized_presentation_zero_limits_disable_holding() {
    for (timeout, bytes) in [(0, 4096), (100, 0)] {
        let mut t = terminal(timeout, bytes);
        t.process(b"\x1b[?2026h\rNEW", ms(1));
        assert!(!t.is_holding());
        assert_eq!(t.presented_screen().contents(), "NEW");
    }
    let mut plain = TerminalPresentation::new(2, 4, 0, ms(10), 128);
    plain.process(b"abc", ms(1));
    assert_eq!(plain.presented_screen().contents(), "abc");
}

#[test]
fn synchronized_presentation_ordinary_frames_borrow_state_and_keep_incremental_damage() {
    let mut t = terminal(100, 4096);
    assert!(std::ptr::eq(t.active_screen(), t.presented_screen()));
    t.process(b"!", ms(1));
    assert!(std::ptr::eq(t.active_screen(), t.presented_screen()));
    assert_eq!(t.presented_screen().contents(), "OLD!");
    let damage = t.take_damage();
    assert!(!damage.needs_full_raster());
    assert_eq!((damage.rows().first(), damage.rows().end()), (0, 1));
    assert_eq!(damage.cursor_before(), Some((0, 3)));
    assert_eq!(damage.cursor_after(), Some((0, 4)));
    t.process(b"\x1b[1D?", ms(2));
    assert!(std::ptr::eq(t.active_screen(), t.presented_screen()));
    assert_eq!(t.presented_screen().contents(), "OLD?");
    assert!(!t.take_damage().needs_full_raster());
    // Plain OSC payload and invalid/subparameter markers do not create holds.
    t.process(b"\x1b]2;?2026h\x07\x1b[?2026:1h", ms(3));
    assert!(std::ptr::eq(t.active_screen(), t.presented_screen()));
    assert_eq!(t.callbacks().titles, ["?2026h"]);
    t.process(BEGIN, ms(4));
    assert!(!std::ptr::eq(t.active_screen(), t.presented_screen()));
    t.process(b"\rNEW!", ms(5));
    assert_eq!(t.presented_screen().contents(), "OLD?");
    t.process(END, ms(6));
    assert!(std::ptr::eq(t.active_screen(), t.presented_screen()));
    assert_eq!(t.presented_screen().contents(), "NEW!");
    assert!(t.take_damage().needs_full_raster());
}

#[test]
fn synchronized_presentation_preflight_rearms_after_split_end_or_reset_without_replaying_effects() {
    for reset in [false, true] {
        let mut t = terminal(100, 4096);
        t.process(BEGIN, ms(1));
        t.process(b"\rLIVE", ms(2));
        t.poll(ms(101));
        assert!(std::ptr::eq(t.active_screen(), t.presented_screen()));
        t.process(BEGIN, ms(102));
        assert!(!t.is_holding());
        assert!(std::ptr::eq(t.active_screen(), t.presented_screen()));
        let prefix = if reset {
            b"\x1bc\x1b[?20".as_slice()
        } else {
            b"\x1b[?2026l\x1b[?20"
        };
        // Keep reset/end and new begin in one batch, including partial grammar.
        t.process(prefix, ms(103));
        let prior = t.presented_screen().contents();
        t.process(b"26h\x1b[H\x1b[2JNEW\x1b]2;one\x07", ms(104));
        assert!(t.is_holding());
        assert_eq!(t.presented_screen().contents(), prior);
        assert_eq!(t.active_screen().contents(), "NEW");
        assert_eq!(t.callbacks().titles, ["one"]);
        assert_eq!(t.callbacks().resets, usize::from(reset));
        t.process(END, ms(105));
        assert_eq!(t.presented_screen().contents(), "NEW");
        assert!(std::ptr::eq(t.active_screen(), t.presented_screen()));
        t.process(BEGIN, ms(106));
        t.process(b"\rLIVE", ms(107));
        t.poll(ms(206));
        assert_eq!(t.presented_screen().contents(), "LIVE");
        // Here the previous expired phase is reset and a complete new begin
        // arrives in the same call: the preflight must model the transition.
        let batch = if reset {
            b"\x1bc\x1b[?2026hX".as_slice()
        } else {
            b"\x1b[?2026l\x1b[?2026h\x1b[H\x1b[2JX"
        };
        t.process(batch, ms(207));
        assert!(t.is_holding());
        assert_eq!(t.presented_screen().contents(), "LIVE");
        assert_eq!(t.active_screen().contents(), "X");
        assert_eq!(t.callbacks().resets, usize::from(reset) * 2);
    }
    // An OSC cancelled by ESC followed by CSI must be inspected as a real begin.
    let mut t = terminal(100, 4096);
    t.process(b"\x1b]2;unfinished", ms(1));
    t.process(b"\x1b[?2026h\rNEW", ms(2));
    assert!(t.is_holding());
    assert_eq!(t.presented_screen().contents(), "OLD");
    assert!(t.callbacks().titles.is_empty());
}
