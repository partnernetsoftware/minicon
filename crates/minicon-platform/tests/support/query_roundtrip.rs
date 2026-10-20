//! Real native query producer: expected replies are literals, not parser output.
use super::mixed_output::bytes;
use super::queued_reads::{check, publish};
use std::{io, path::Path};
use windows_sys::Win32::{Storage::FileSystem::ReadFile, System::Console::*};

pub(super) fn run(path: &Path) -> io::Result<()> {
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    check(unsafe { SetConsoleMode(output, 7) })?;
    // Replies are terminal input, not edited or echoed human command lines.
    check(unsafe { SetConsoleMode(input, ENABLE_VIRTUAL_TERMINAL_INPUT) })?;
    let mut report = String::new();
    for (command, expected) in [
        ("\x1b[?2004$p", "\x1b[?2004;2$y"),
        ("\x1b[?2004h\x1b[?2004$p", "\x1b[?2004;1$y"),
        ("\x1b[?2004l\x1b[?2004$p", "\x1b[?2004;2$y"),
        ("\x1b[?7l\x1b[?7$p", "\x1b[?7;2$y"),
        ("\x1b[?7h\x1b[?7$p", "\x1b[?7;1$y"),
        ("\x1b[?25l\x1b[?25$p", "\x1b[?25;2$y"),
        ("\x1b[?25h\x1b[?25$p", "\x1b[?25;1$y"),
        ("\x1b[?2026h\x1b[?2026$p", "\x1b[?2026;0$y"),
        ("\x1b[?2026l\x1b[?65535$p", "\x1b[?65535;0$y"),
    ] {
        // Split the final query at every API byte, including the private and
        // intermediate grammar bytes; no parent's raw substring reply shortcut.
        for byte in command.as_bytes() {
            bytes(output, &[*byte])?;
        }
        let mut received = Vec::new();
        while received.last() != Some(&b'y') && received.len() < 32 {
            let mut value = [0u8; 1];
            let mut count = 0;
            check(unsafe {
                ReadFile(
                    input,
                    value.as_mut_ptr(),
                    1,
                    &mut count,
                    std::ptr::null_mut(),
                )
            })?;
            if count != 1 {
                return Err(io::Error::other("short native query input"));
            }
            received.push(value[0]);
        }
        report.push_str(&format!("query={command:?};reply={received:?}\n"));
        publish(path, &report)?;
        if received != expected.as_bytes() {
            return Err(io::Error::other("incorrect native query reply"));
        }
    }
    bytes(output, b"\x1b]2;query-roundtrip-keys\x07")?;
    let mut keys = Vec::new();
    for _ in 0..4 {
        let mut value = [0u8; 1];
        let mut count = 0;
        check(unsafe {
            ReadFile(
                input,
                value.as_mut_ptr(),
                1,
                &mut count,
                std::ptr::null_mut(),
            )
        })?;
        if count != 1 {
            return Err(io::Error::other("short native key input"));
        }
        keys.push(value[0]);
    }
    report.push_str(&format!("keys={keys:?}\n"));
    publish(path, &report)?;
    if keys != b"\x1b[Aq" {
        return Err(io::Error::other("query transport changed native keys"));
    }
    bytes(output, b"\x1b]2;query-roundtrip-done\x07")?;
    std::process::exit(31)
}
