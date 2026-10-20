//! Bounded replies for queries dispatched by the shared VT parser.
use super::{MouseProtocolEncoding, MouseProtocolMode, Screen};

/// Answer a validated CSI callback without owning a transport or reply queue.
/// Each recognized query produces at most 16 bytes. Callers send the result
/// back to the child; parsing its output does not itself write to the PTY.
///
/// Supports primary DA, status/cursor reports and single-mode private DECRQM.
/// Only modes with implemented, observable semantics are reported as set/reset.
/// Unknown modes (including synchronized output 2026) report status zero.
/// Malformed, multi-parameter and unrelated commands produce no reply.
#[must_use]
pub fn csi_query_reply(
    screen: &Screen,
    intermediate1: Option<u8>,
    intermediate2: Option<u8>,
    params: &[&[u16]],
    final_byte: char,
) -> Option<Vec<u8>> {
    let parameter = match params {
        [] => 0,
        [[value]] => *value,
        _ => return None,
    };
    match (intermediate1, intermediate2, final_byte, parameter) {
        (None, None, 'c', 0) => Some(b"\x1b[?1;2c".to_vec()),
        (None, None, 'n', 5) => Some(b"\x1b[0n".to_vec()),
        (None, None, 'n', 6) => {
            let (row, column) = screen.cursor_position();
            Some(format!("\x1b[{};{}R", u32::from(row) + 1, u32::from(column) + 1).into_bytes())
        }
        (Some(b'?'), Some(b'$'), 'p', mode) => {
            Some(format!("\x1b[?{mode};{}$y", private_mode_status(screen, mode)).into_bytes())
        }
        _ => None,
    }
}

fn private_mode_status(screen: &Screen, mode: u16) -> u8 {
    let set = match mode {
        1 => screen.application_cursor(),
        6 => screen.origin_mode(),
        7 => screen.auto_wrap(),
        9 => screen.mouse_protocol_mode() == MouseProtocolMode::Press,
        25 => !screen.hide_cursor(),
        47 | 1049 => screen.alternate_screen(),
        1000 => screen.mouse_protocol_mode() == MouseProtocolMode::PressRelease,
        1002 => screen.mouse_protocol_mode() == MouseProtocolMode::ButtonMotion,
        1003 => screen.mouse_protocol_mode() == MouseProtocolMode::AnyMotion,
        1005 => screen.mouse_protocol_encoding() == MouseProtocolEncoding::Utf8,
        1006 => screen.mouse_protocol_encoding() == MouseProtocolEncoding::Sgr,
        2004 => screen.bracketed_paste(),
        _ => return 0,
    };
    if set { 1 } else { 2 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_query_replies_follow_live_modes_and_reject_nonqueries() {
        let mut parser = vt100::Parser::new(4, 20, 0);
        for mode in [1, 6, 7, 9, 25, 47, 1049, 1000, 1002, 1003, 1005, 1006, 2004] {
            for (command, status) in [('h', 1), ('l', 2)] {
                parser.process(format!("\x1b[?{mode}{command}").as_bytes());
                let before = parser.screen().contents_formatted();
                assert_eq!(
                    csi_query_reply(parser.screen(), Some(b'?'), Some(b'$'), &[&[mode]], 'p'),
                    Some(format!("\x1b[?{mode};{status}$y").into_bytes())
                );
                assert_eq!(parser.screen().contents_formatted(), before);
            }
        }
        for mode in [0, 2026, 2027, 65535] {
            assert_eq!(
                csi_query_reply(parser.screen(), Some(b'?'), Some(b'$'), &[&[mode]], 'p'),
                Some(format!("\x1b[?{mode};0$y").into_bytes())
            );
        }
        for (a, b, p, c) in [
            (Some(b'?'), None, vec![&[2004][..]], 'p'),
            (Some(b'?'), Some(b'$'), vec![&[2004, 1][..]], 'p'),
            (Some(b'?'), Some(b'$'), vec![&[2004][..], &[7][..]], 'p'),
            (None, None, vec![&[1][..]], 'c'),
            (None, None, vec![&[5][..], &[6][..]], 'n'),
            (Some(b'>'), None, vec![&[0][..]], 'c'),
            (Some(b'?'), Some(b'$'), vec![&[2004][..]], 'y'),
        ] {
            assert_eq!(csi_query_reply(parser.screen(), a, b, &p, c), None);
        }
        parser.process(b"\x1b[3;8H");
        assert_eq!(
            csi_query_reply(parser.screen(), None, None, &[&[6]], 'n'),
            Some(b"\x1b[3;8R".to_vec())
        );
        assert_eq!(
            csi_query_reply(parser.screen(), None, None, &[&[5]], 'n'),
            Some(b"\x1b[0n".to_vec())
        );
        assert_eq!(
            csi_query_reply(parser.screen(), None, None, &[], 'c'),
            Some(b"\x1b[?1;2c".to_vec())
        );
    }
}
