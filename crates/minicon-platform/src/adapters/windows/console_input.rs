//! Shared Windows VT-key decoder used by both console transports.

#[derive(Clone, Copy)]
pub(super) struct Key {
    pub(super) virtual_key: u16,
    pub(super) unicode: u16,
    pub(super) trailing_surrogate: Option<u16>,
    pub(super) control: bool,
    pub(super) shift: bool,
    pub(super) alt: bool,
}
impl Key {
    pub(super) fn controls(self) -> u32 {
        use windows_sys::Win32::System::Console::{
            LEFT_ALT_PRESSED, LEFT_CTRL_PRESSED, SHIFT_PRESSED,
        };
        (if self.control { LEFT_CTRL_PRESSED } else { 0 })
            | (if self.shift { SHIFT_PRESSED } else { 0 })
            | (if self.alt { LEFT_ALT_PRESSED } else { 0 })
    }
}

/// One key press from the front of a terminal byte stream.
///
/// `None` means the bytes so far are a prefix of something longer and the
/// caller should wait for more.
pub(super) fn decode_key(bytes: &[u8]) -> Option<(Key, usize)> {
    const VK_BACK: u16 = 0x08;
    const VK_TAB: u16 = 0x09;
    const VK_RETURN: u16 = 0x0D;
    const VK_ESCAPE: u16 = 0x1B;
    const VK_END: u16 = 0x23;
    const VK_HOME: u16 = 0x24;
    const VK_F1: u16 = 0x70;

    let plain = |virtual_key: u16, unicode: u16, used: usize| {
        Some((
            Key {
                virtual_key,
                unicode,
                trailing_surrogate: None,
                control: false,
                shift: false,
                alt: false,
            },
            used,
        ))
    };

    match bytes.first()? {
        0x1B => {
            if bytes.len() < 2 {
                // Could be a lone Escape or the start of a sequence. Waiting
                // one more read is better than guessing wrong on every arrow.
                return None;
            }
            match bytes[1] {
                b'[' => decode_csi(bytes),
                b'O' if bytes.len() >= 3 => match bytes[2] {
                    b'P'..=b'S' => plain(VK_F1 + u16::from(bytes[2] - b'P'), 0, 3),
                    b'H' => plain(VK_HOME, 0, 3),
                    b'F' => plain(VK_END, 0, 3),
                    _ => plain(VK_ESCAPE, 0x1B, 1),
                },
                b'O' => None,
                _ => plain(VK_ESCAPE, 0x1B, 1),
            }
        }
        b'\r' | b'\n' => plain(VK_RETURN, u16::from(b'\r'), 1),
        0x08 | 0x7F => plain(VK_BACK, u16::from(b'\x08'), 1),
        b'\t' => plain(VK_TAB, u16::from(b'\t'), 1),
        byte @ 0x01..=0x1A => Some((
            Key {
                // Ctrl+A is 0x01, and the console expects the letter's own
                // virtual key with the control modifier set.
                virtual_key: u16::from(b'A') + u16::from(*byte) - 1,
                unicode: u16::from(*byte),
                trailing_surrogate: None,
                control: true,
                shift: false,
                alt: false,
            },
            1,
        )),
        _ => {
            // Anything else is text. Decode one whole UTF-8 scalar so a
            // multi-byte character is never split into replacement bytes.
            let text = std::str::from_utf8(bytes).map_or_else(
                |error| {
                    let valid = error.valid_up_to();
                    (valid > 0).then(|| std::str::from_utf8(&bytes[..valid]).unwrap_or_default())
                },
                Some,
            )?;
            let character = text.chars().next()?;
            let mut units = [0_u16; 2];
            let encoded = character.encode_utf16(&mut units);
            Some((
                Key {
                    virtual_key: 0,
                    unicode: encoded[0],
                    trailing_surrogate: encoded.get(1).copied(),
                    control: false,
                    shift: false,
                    alt: false,
                },
                character.len_utf8(),
            ))
        }
    }
}

fn decode_csi(bytes: &[u8]) -> Option<(Key, usize)> {
    const VK_PRIOR: u16 = 0x21;
    const VK_NEXT: u16 = 0x22;
    const VK_END: u16 = 0x23;
    const VK_HOME: u16 = 0x24;
    const VK_LEFT: u16 = 0x25;
    const VK_UP: u16 = 0x26;
    const VK_RIGHT: u16 = 0x27;
    const VK_DOWN: u16 = 0x28;
    const VK_INSERT: u16 = 0x2D;
    const VK_DELETE: u16 = 0x2E;
    const VK_F1: u16 = 0x70;

    // Find the final byte of the sequence, which is what identifies it.
    let end = bytes[2..]
        .iter()
        .position(|byte| byte.is_ascii_alphabetic() || *byte == b'~')?;
    let final_byte = bytes[2 + end];
    let parameters = &bytes[2..2 + end];
    let used = 3 + end;
    let modifier = std::str::from_utf8(parameters)
        .ok()
        .and_then(|p| p.strip_prefix("1;"))
        .and_then(|p| p.parse::<u8>().ok())
        .filter(|m| (1..=8).contains(m));
    let key = |virtual_key: u16| {
        Some((
            Key {
                virtual_key,
                unicode: 0,
                trailing_surrogate: None,
                control: modifier.is_some_and(|m| (m - 1) & 4 != 0),
                shift: modifier.is_some_and(|m| (m - 1) & 1 != 0),
                alt: modifier.is_some_and(|m| (m - 1) & 2 != 0),
            },
            used,
        ))
    };
    match final_byte {
        b'P'..=b'S' if parameters.is_empty() || parameters == b"1" || modifier.is_some() => {
            key(VK_F1 + u16::from(final_byte - b'P'))
        }
        b'A' => key(VK_UP),
        b'B' => key(VK_DOWN),
        b'C' => key(VK_RIGHT),
        b'D' => key(VK_LEFT),
        b'H' => key(VK_HOME),
        b'F' => key(VK_END),
        b'~' => {
            let number: u16 = std::str::from_utf8(parameters)
                .ok()
                .and_then(|text| text.split(';').next()?.parse().ok())?;
            match number {
                1 | 7 => key(VK_HOME),
                2 => key(VK_INSERT),
                3 => key(VK_DELETE),
                4 | 8 => key(VK_END),
                5 => key(VK_PRIOR),
                6 => key(VK_NEXT),
                11..=15 => key(VK_F1 + number - 11),
                17..=21 => key(VK_F1 + number - 12),
                23 | 24 => key(VK_F1 + number - 13),
                _ => Some((
                    Key {
                        virtual_key: 0,
                        unicode: 0,
                        trailing_surrogate: None,
                        control: false,
                        shift: false,
                        alt: false,
                    },
                    used,
                )),
            }
        }
        // An unrecognized sequence is consumed rather than delivered as text:
        // a stray "[200~" typed into a shell is worse than a dropped key.
        _ => Some((
            Key {
                virtual_key: 0,
                unicode: 0,
                trailing_surrogate: None,
                control: false,
                shift: false,
                alt: false,
            },
            used,
        )),
    }
}

#[cfg(test)]
mod response_key_tests {
    use super::*;
    #[test]
    fn console_agent_modified_function_keys_preserve_identity_and_native_modifiers() {
        for (final_byte, vk) in [(b'P', 0x70), (b'Q', 0x71), (b'R', 0x72), (b'S', 0x73)] {
            for modifier in 1..=8 {
                let wire = format!("\x1b[1;{modifier}{}", char::from(final_byte));
                for end in 1..wire.len() {
                    assert!(decode_key(&wire.as_bytes()[..end]).is_none());
                }
                let (key, used) = decode_key(wire.as_bytes()).unwrap();
                assert_eq!((key.virtual_key, key.unicode, used), (vk, 0, wire.len()));
                let bits = modifier - 1;
                assert_eq!(
                    key.controls(),
                    if bits & 1 != 0 { 16 } else { 0 }
                        | if bits & 2 != 0 { 2 } else { 0 }
                        | if bits & 4 != 0 { 8 } else { 0 }
                );
            }
        }
    }
}

/// One mouse report decoded from the SGR (DECSET 1006) form a terminal emits.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct MouseReport {
    /// The SGR button code, including the modifier, motion and wheel bits.
    pub(super) code: u32,
    /// Zero-based cell coordinates; the wire form is one-based.
    pub(super) column: i16,
    pub(super) row: i16,
    /// `M` is a press, `m` a release.
    pub(super) pressed: bool,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum SgrMouse {
    Report(MouseReport, usize),
    /// A prefix of a report: wait for more bytes rather than delivering the
    /// escape literally.
    Incomplete,
    /// Not a mouse report; let the key decoder have it.
    Other,
}

/// Decodes `ESC [ < code ; column ; row (M|m)`.
///
/// Both console transports translate the terminal's SGR bytes into the
/// `MOUSE_EVENT_RECORD` a console delivers natively — the same thing a program
/// receives when it runs in a plain conhost window. One owner, two consumers:
/// the console agent and the rust-console provider (the latter silently
/// dropped every mouse report until this decoder was shared, 2026-10-22).
pub(super) fn decode_sgr_mouse(bytes: &[u8]) -> SgrMouse {
    const PREFIX: [u8; 3] = [0x1b, b'[', b'<'];
    for (offset, want) in PREFIX.iter().enumerate() {
        match bytes.get(offset) {
            None => return SgrMouse::Incomplete,
            Some(byte) if byte == want => {}
            Some(_) => return SgrMouse::Other,
        }
    }
    let mut index = PREFIX.len();
    let mut fields = [0_u32; 3];
    for (slot, field) in fields.iter_mut().enumerate() {
        let start = index;
        while let Some(byte) = bytes.get(index) {
            if !byte.is_ascii_digit() {
                break;
            }
            *field = field
                .saturating_mul(10)
                .saturating_add(u32::from(byte - b'0'));
            index += 1;
        }
        if index == start {
            // No digits yet. With bytes still to come this is a report that has
            // not finished arriving, and calling it `Other` here delivered the
            // prefix to the child as a literal ESC -- the very thing this
            // decoder exists to prevent. Only a non-digit that is actually
            // present makes it something else.
            return match bytes.get(index) {
                None => SgrMouse::Incomplete,
                Some(_) => SgrMouse::Other,
            };
        }
        match bytes.get(index) {
            None => return SgrMouse::Incomplete,
            Some(b';') if slot < 2 => index += 1,
            Some(b'M' | b'm') if slot == 2 => {}
            Some(_) => return SgrMouse::Other,
        }
    }
    let pressed = bytes[index] == b'M';
    let cell = |value: u32| i16::try_from(value.saturating_sub(1)).unwrap_or(i16::MAX);
    SgrMouse::Report(
        MouseReport {
            code: fields[0],
            column: cell(fields[1]),
            row: cell(fields[2]),
            pressed,
        },
        index + 1,
    )
}

/// The record fields for one decoded mouse report, as plain integers so each
/// transport builds its own native record (`INPUT_RECORD` for the agent,
/// `ConsoleInput::Mouse` for the rust-console provider) from one mapping.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) struct NativeMouse {
    pub(super) column: i16,
    pub(super) row: i16,
    pub(super) buttons: u32,
    pub(super) controls: u32,
    pub(super) flags: u32,
}

/// Maps one decoded SGR report onto console mouse-record semantics: button
/// state, the signed wheel delta in the high word, motion and modifiers.
pub(super) fn native_mouse(report: MouseReport) -> NativeMouse {
    use windows_sys::Win32::System::Console::{
        FROM_LEFT_1ST_BUTTON_PRESSED, FROM_LEFT_2ND_BUTTON_PRESSED, LEFT_ALT_PRESSED,
        LEFT_CTRL_PRESSED, MOUSE_MOVED, MOUSE_WHEELED, RIGHTMOST_BUTTON_PRESSED, SHIFT_PRESSED,
    };
    const SHIFT_BIT: u32 = 4;
    const ALT_BIT: u32 = 8;
    const CTRL_BIT: u32 = 16;
    const MOTION_BIT: u32 = 32;
    const WHEEL_BIT: u32 = 64;

    let mut buttons = 0_u32;
    let mut flags = 0_u32;
    if report.code & WHEEL_BIT != 0 {
        flags |= MOUSE_WHEELED;
        // The wheel delta rides in the high word, signed: SGR 64 scrolls up.
        let delta: i16 = if report.code & 0x03 == 0 { 120 } else { -120 };
        buttons |= (delta as u16 as u32) << 16;
    } else {
        if report.pressed {
            buttons |= match report.code & 0x03 {
                0 => FROM_LEFT_1ST_BUTTON_PRESSED,
                1 => FROM_LEFT_2ND_BUTTON_PRESSED,
                2 => RIGHTMOST_BUTTON_PRESSED,
                _ => 0,
            };
        }
        if report.code & MOTION_BIT != 0 {
            flags |= MOUSE_MOVED;
        }
    }
    let mut controls = 0_u32;
    if report.code & SHIFT_BIT != 0 {
        controls |= SHIFT_PRESSED;
    }
    if report.code & ALT_BIT != 0 {
        controls |= LEFT_ALT_PRESSED;
    }
    if report.code & CTRL_BIT != 0 {
        controls |= LEFT_CTRL_PRESSED;
    }
    NativeMouse {
        column: report.column,
        row: report.row,
        buttons,
        controls,
        flags,
    }
}
