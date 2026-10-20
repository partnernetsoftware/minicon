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
