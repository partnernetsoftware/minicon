//! Independent ReadConsoleW mode and wakeup-control precedence probe.
use super::mixed_output::{bytes, position};
use std::{io, path::Path};
use windows_sys::Win32::System::Console::*;

pub(super) fn run(kind: &str, report: &Path) -> io::Result<()> {
    let (mode, wakeup, unit, initial) = match kind {
        "processed-bs" => (7, 0, 8, false),
        "literal-bs" => (6, 0, 8, false),
        "literal-noecho-bs" => (2, 0, 8, false),
        "wakeup-bs" => (7, 1 << 8, 8, false),
        "wakeup-cr" => (7, 1 << 13, 13, false),
        "wakeup-tab" => (7, 1 << 9, 9, false),
        "wakeup-esc" => (7, 1 << 27, 27, false),
        "wakeup-literal-bs" => (6, 1 << 8, 8, false),
        "initial-wakeup-bs" => (7, 1 << 8, 8, true),
        "initial-processed-bs" => (7, 0, 8, true),
        _ => return Err(io::ErrorKind::InvalidInput.into()),
    };
    // SAFETY: private console handles owned by this SDK fixture process.
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(input, mode) } == 0
        || unsafe { SetConsoleMode(output, 15) } == 0
        || unsafe { SetConsoleOutputCP(65001) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    bytes(output, b"\x1b[2J\x1b[H>")?;
    let mut received = [0_u16; 64];
    if initial {
        received[..2].copy_from_slice(&[97, 98]);
        // The ReadConsole caller is responsible for displaying initial text.
        bytes(output, b"ab")?;
    }
    let units = if initial {
        vec![unit, 99, 13]
    } else {
        vec![97, 98, unit, 99, 13]
    };
    let records: Vec<_> = units
        .into_iter()
        .map(|unit| {
            let mut record = INPUT_RECORD {
                EventType: KEY_EVENT as u16,
                ..INPUT_RECORD::default()
            };
            record.Event.KeyEvent.bKeyDown = 1;
            record.Event.KeyEvent.wRepeatCount = 1;
            record.Event.KeyEvent.uChar.UnicodeChar = unit;
            record.Event.KeyEvent.wVirtualKeyCode = match unit {
                8 | 9 | 13 => unit,
                _ => 0,
            };
            record.Event.KeyEvent.dwControlKeyState = if unit == 99 { 0 } else { 16 };
            record
        })
        .collect();
    let mut count = 0;
    // SAFETY: initialized SDK records and a live count buffer.
    if unsafe { WriteConsoleInputW(input, records.as_ptr(), records.len() as u32, &mut count) } == 0
        || count as usize != records.len()
    {
        return Err(io::Error::last_os_error());
    }
    let mut control = CONSOLE_READCONSOLE_CONTROL {
        nLength: std::mem::size_of::<CONSOLE_READCONSOLE_CONTROL>() as u32,
        nInitialChars: if initial { 2 } else { 0 },
        dwCtrlWakeupMask: wakeup,
        dwControlKeyState: 0,
    };
    // SAFETY: initialized bounded UTF-16 output, count and SDK control buffers.
    if unsafe {
        ReadConsoleW(
            input,
            received.as_mut_ptr().cast(),
            received.len() as u32,
            &mut count,
            std::ptr::addr_of_mut!(control),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if count as usize > received.len() {
        return Err(io::Error::other("ReadConsoleW exceeded requested capacity"));
    }
    let mut receipt = format!("build={}\nread:", super::output_modes::windows_build()?);
    for unit in &received[..count as usize] {
        receipt.push_str(&format!("{unit:04x},"));
    }
    receipt.push_str(&format!("\ncontrols:{:08x}\n", control.dwControlKeyState));
    let mut queued = 0;
    // SAFETY: private input handle and initialized SDK output count.
    if unsafe { GetNumberOfConsoleInputEvents(input, &mut queued) } == 0 {
        return Err(io::Error::last_os_error());
    }
    receipt.push_str(&format!("queued:{queued}\n"));
    position(output, "cursor", &mut receipt)?;
    std::fs::write(report, receipt)?;
    bytes(output, b"\x1b]2;cooked-controls-end:\x07")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !report.with_extension("opened").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}
