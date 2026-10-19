//! Windows input-record conversion, following pinned directio.cpp semantics.
//! The codec is shared with native titles/output; event policy stays here.
use super::{ConsoleInput, NOT_SUPPORTED};
use crate::host::encoding;
use std::{collections::VecDeque, io};
use windows_sys::Win32::Globalization::IsDBCSLeadByteEx;
use windows_sys::Win32::System::Console::{MOUSE_MOVED, NLS_IME_CONVERSION};

/// One append can update the last raw record. Preserve its identity and never
/// wrap a WORD repeat count: a separate record retains overflowing presses.
pub(super) fn coalesced(
    mut previous: ConsoleInput,
    incoming: ConsoleInput,
) -> Option<ConsoleInput> {
    match (&mut previous, incoming) {
        (
            ConsoleInput::Key {
                down: true,
                repeat,
                scan_code,
                character,
                controls,
                ..
            },
            ConsoleInput::Key {
                down: true,
                repeat: more,
                scan_code: next_scan,
                character: next_char,
                controls: next_controls,
                ..
            },
        ) if (*scan_code == next_scan || next_controls & NLS_IME_CONVERSION != 0)
            && *character == next_char
            && *controls == next_controls
            && !(0xd800..=0xdfff).contains(&next_char) =>
        {
            *repeat = repeat.checked_add(more)?;
            Some(previous)
        }
        (
            ConsoleInput::Mouse {
                x,
                y,
                flags: MOUSE_MOVED,
                ..
            },
            ConsoleInput::Mouse {
                x: next_x,
                y: next_y,
                flags: MOUSE_MOVED,
                ..
            },
        ) => {
            *x = next_x;
            *y = next_y;
            Some(previous)
        }
        (ConsoleInput::Resize { .. }, next @ ConsoleInput::Resize { .. }) => Some(next),
        _ => None,
    }
}

pub(super) fn decode(bytes: &[u8]) -> Result<Vec<ConsoleInput>, i32> {
    bytes
        .chunks_exact(20)
        .map(|record| {
            let word = |at| u16::from_le_bytes([record[at], record[at + 1]]);
            let dword = |at| u32::from_le_bytes(record[at..at + 4].try_into().unwrap());
            Ok(match word(0) {
                1 => ConsoleInput::Key {
                    down: dword(4) != 0,
                    repeat: word(8),
                    virtual_key: word(10),
                    scan_code: word(12),
                    character: word(14),
                    controls: dword(16),
                },
                2 => ConsoleInput::Mouse {
                    x: word(4) as i16,
                    y: word(6) as i16,
                    buttons: dword(8),
                    controls: dword(12),
                    flags: dword(16),
                },
                4 => ConsoleInput::Resize {
                    columns: word(4) as i16,
                    rows: word(6) as i16,
                },
                16 => ConsoleInput::Focus(dword(4) != 0),
                _ => return Err(NOT_SUPPORTED),
            })
        })
        .collect()
}

fn byte(event: ConsoleInput) -> Option<u8> {
    if let ConsoleInput::Key { character, .. } = event {
        Some(character as u8)
    } else {
        None
    }
}

fn convert(code_page: u32, bytes: &[u8], mut event: ConsoleInput, output: &mut Vec<ConsoleInput>) {
    // Native record writes skip a codec failure instead of failing the entire
    // batch. Incomplete DBCS is handled before this per-record conversion.
    for unit in encoding::decode(code_page, bytes).unwrap_or_default() {
        if let ConsoleInput::Key { character, .. } = &mut event {
            *character = unit;
        }
        output.push(event);
    }
}

/// Stage both events and the next prefix. The caller commits them together
/// only after admission, so a rejected batch cannot consume an earlier lead.
pub(super) fn ansi(
    code_page: u32,
    events: &[ConsoleInput],
    mut lead: Option<ConsoleInput>,
) -> (Vec<ConsoleInput>, Option<ConsoleInput>) {
    let mut output = Vec::with_capacity(events.len() + 1);
    let mut events = events.iter().copied().peekable();
    if events.peek().is_none() {
        return (output, lead);
    }
    if let Some(prefix) = lead.take()
        && let Some(trail) = events.peek().copied().and_then(byte)
    {
        convert(
            code_page,
            &[byte(prefix).unwrap(), trail],
            prefix,
            &mut output,
        );
        events.next();
    }
    while let Some(event) = events.next() {
        let Some(first) = byte(event) else {
            output.push(event);
            continue;
        };
        // SAFETY: numeric code-page/byte query; no pointers or shared state.
        if unsafe { IsDBCSLeadByteEx(code_page, first) } != 0 {
            match events.next() {
                None => {
                    lead = Some(event);
                    break;
                }
                Some(ConsoleInput::Key { character, .. }) => {
                    convert(code_page, &[first, character as u8], event, &mut output);
                }
                _ => {}
            }
        } else {
            convert(code_page, &[first], event, &mut output);
        }
    }
    (output, lead)
}

pub(super) struct ReadBatch {
    pub(super) bytes: Vec<u8>,
    pub(super) consumed: usize,
    pub(super) tail: VecDeque<ConsoleInput>,
}

/// Preview a bounded reply before consuming anything. A peek can change
/// reading mode, but cannot create, consume or duplicate encoded key tails.
pub(super) fn read(
    code_page: u32,
    events: &VecDeque<ConsoleInput>,
    tail: &VecDeque<ConsoleInput>,
    unicode: bool,
    peek: bool,
    capacity: usize,
) -> io::Result<ReadBatch> {
    let mut pending = if unicode {
        VecDeque::new()
    } else {
        tail.clone()
    };
    let mut output = Vec::new();
    while output.len() < capacity {
        let Some(event) = pending.pop_front() else {
            break;
        };
        output.push(event);
    }
    let mut consumed = 0;
    for mut event in events.iter().copied() {
        if output.len() >= capacity {
            break;
        }
        consumed += 1;
        if !unicode && let ConsoleInput::Key { character, .. } = event {
            let encoded = encoding::encode(code_page, &[character], 8)?.ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "input record encoding exceeds native byte budget",
                )
            })?;
            for byte in encoded {
                if let ConsoleInput::Key { character, .. } = &mut event {
                    *character = u16::from(byte);
                }
                if output.len() < capacity {
                    output.push(event);
                } else {
                    pending.push_back(event);
                }
            }
        } else {
            output.push(event);
        }
    }
    if peek {
        consumed = 0;
        pending = if unicode {
            VecDeque::new()
        } else {
            tail.clone()
        };
    }
    Ok(ReadBatch {
        bytes: output.iter().flat_map(super::record_bytes).collect(),
        consumed,
        tail: pending,
    })
}
