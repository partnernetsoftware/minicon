//! SDK oracle for returned status bytes and ambiguous F3 native records.
use super::mixed_output::bytes;
use super::queued_reads::{check, publish};
use std::{io, path::Path, time::Duration};
use windows_sys::Win32::{Storage::FileSystem::ReadFile, System::Console::*};

pub(super) fn run(path: &Path, overflow: bool) -> io::Result<()> {
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    check(unsafe { SetConsoleMode(output, 7) })?;
    check(unsafe { SetConsoleMode(input, 0) })?;
    if overflow {
        let _result = bytes(output, &b"\x1b[6n".repeat(65));
        std::thread::sleep(Duration::from_secs(20));
        std::process::exit(77)
    }
    let mut report = String::new();
    for mode in [0, ENABLE_VIRTUAL_TERMINAL_INPUT] {
        check(unsafe { SetConsoleMode(input, mode) })?;
        for (command, expected) in [
            ("\x1b[c", "\x1b[?1;2c"),
            ("\x1b[5n", "\x1b[0n"),
            ("\x1b[1;2H\x1b[6n", "\x1b[1;2R"),
        ] {
            for byte in command.as_bytes() {
                bytes(output, &[*byte])?;
            }
            let mut received = Vec::new();
            for _ in 0..expected.len() {
                let mut byte = [0u8; 1];
                let mut count = 0;
                check(unsafe {
                    ReadFile(
                        input,
                        byte.as_mut_ptr(),
                        1,
                        &mut count,
                        std::ptr::null_mut(),
                    )
                })?;
                if count != 1 {
                    return Err(io::Error::other("short status response"));
                }
                received.push(byte[0]);
            }
            report.push_str(&format!(
                "mode={mode};query={command:?};reply={received:?}\n"
            ));
            publish(path, &report)?;
            if received != expected.as_bytes() {
                return Err(io::Error::other("wrong native status bytes"));
            }
        }
    }
    check(unsafe { SetConsoleMode(input, 0) })?;
    bytes(output, b"\x1b]2;status-keys\x07")?;
    for controls in [16, 8, 0] {
        read_f3(input, controls, &mut report)?;
        publish(path, &report)?;
    }
    bytes(output, b"\x1b]2;status-drop\x07\x1b[6n")?;
    std::thread::sleep(Duration::from_millis(2500));
    bytes(output, b"\x1b]2;status-expired\x07")?;
    read_f3(input, 16, &mut report)?;
    publish(path, &report)?;
    bytes(output, b"\x1b]2;status-done\x07")?;
    std::process::exit(33)
}
fn read_f3(
    input: windows_sys::Win32::Foundation::HANDLE,
    controls: u32,
    report: &mut String,
) -> io::Result<()> {
    let mut record = INPUT_RECORD::default();
    let mut count = 0;
    check(unsafe { ReadConsoleInputW(input, &mut record, 1, &mut count) })?;
    if count != 1 || record.EventType != KEY_EVENT as u16 {
        return Err(io::Error::other("missing F3 record"));
    }
    let key = unsafe { record.Event.KeyEvent };
    let character = unsafe { key.uChar.UnicodeChar };
    report.push_str(&format!(
        "key={};character={character};controls={}\n",
        key.wVirtualKeyCode, key.dwControlKeyState
    ));
    if key.bKeyDown == 0
        || key.wVirtualKeyCode != 0x72
        || character != 0
        || key.dwControlKeyState != controls
    {
        return Err(io::Error::other("status report stole or changed F3"));
    }
    Ok(())
}
