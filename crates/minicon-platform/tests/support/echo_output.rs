//! Independent SDK client mixing cooked echo with unfinished client output.
use super::mixed_output::{bytes, position};
use std::{io, path::Path};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Console::{
    GetStdHandle, INPUT_RECORD, KEY_EVENT, ReadConsoleW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    SetConsoleMode, SetConsoleOutputCP, WriteConsoleInputW, WriteConsoleW,
};

fn line(input: HANDLE, units: &[u16], phase: &str, receipt: &mut String) -> io::Result<()> {
    let records: Vec<_> = units
        .iter()
        .map(|&unit| {
            let mut record = INPUT_RECORD {
                EventType: KEY_EVENT as u16,
                ..INPUT_RECORD::default()
            };
            record.Event.KeyEvent.bKeyDown = 1;
            record.Event.KeyEvent.wRepeatCount = 1;
            record.Event.KeyEvent.uChar.UnicodeChar = unit;
            record.Event.KeyEvent.wVirtualKeyCode = match unit {
                8 => 8,
                13 => 13,
                _ => 0,
            };
            record
        })
        .collect();
    let mut written = 0;
    // SAFETY: initialized SDK key records and a live private input handle.
    if unsafe { WriteConsoleInputW(input, records.as_ptr(), records.len() as u32, &mut written) }
        == 0
        || written as usize != records.len()
    {
        return Err(io::Error::last_os_error());
    }
    let mut received = [0_u16; 32];
    let mut count = 0;
    // SAFETY: initialized bounded UTF-16 output and count buffers.
    if unsafe {
        ReadConsoleW(
            input,
            received.as_mut_ptr().cast(),
            received.len() as u32,
            &mut count,
            std::ptr::null(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    receipt.push_str(&format!("{phase}-read:"));
    for unit in &received[..count as usize] {
        receipt.push_str(&format!("{unit:04x},"));
    }
    receipt.push('\n');
    Ok(())
}

pub(super) fn run(report: &Path, owned: bool) -> io::Result<()> {
    // SAFETY: this controlled client's private console handles.
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(input, 7) } == 0
        || unsafe { SetConsoleMode(output, 15) } == 0
        || unsafe { SetConsoleOutputCP(65001) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut receipt = String::new();
    bytes(output, b"\x1b[3")?;
    line(input, &[b'e' as u16, 13], "csi", &mut receipt)?;
    position(output, "csi-echo", &mut receipt)?;
    bytes(output, b"1mX")?;
    position(output, "csi-finish", &mut receipt)?;

    bytes(output, &[0xe4])?;
    line(
        input,
        &[b'Q' as u16, 8, b'R' as u16, 13],
        "utf8",
        &mut receipt,
    )?;
    position(output, "utf8-echo", &mut receipt)?;
    bytes(output, &[0xb8, 0xad])?;
    position(output, "utf8-finish", &mut receipt)?;

    bytes(output, b"\x1b]2;echo-")?;
    line(
        input,
        &[0x4e2d, 0xd83d, 8, 0xd83d, 0xde00, 13],
        "osc",
        &mut receipt,
    )?;
    position(output, "osc-echo", &mut receipt)?;
    bytes(output, b"title\x07")?;
    position(output, "osc-finish", &mut receipt)?;
    if owned {
        // Owned streaming UTF-16 contract: an output high surrogate must not
        // consume the next echoed input unit. This is separate from the
        // installed system host's per-WriteConsoleW decoding behavior.
        wide_unit(output, 0xd83d)?;
        line(input, &[b'V' as u16, 13], "wide", &mut receipt)?;
        position(output, "wide-echo", &mut receipt)?;
        wide_unit(output, 0xde00)?;
        position(output, "wide-finish", &mut receipt)?;
    }
    std::fs::write(report, receipt)?;
    bytes(output, b"\x1b]2;echo-end:\x07")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !report.with_extension("opened").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}

fn wide_unit(output: HANDLE, unit: u16) -> io::Result<()> {
    let mut count = 0;
    // SAFETY: one initialized UTF-16 input unit and live count buffer.
    if unsafe {
        WriteConsoleW(
            output,
            (&unit as *const u16).cast(),
            1,
            &mut count,
            std::ptr::null(),
        )
    } == 0
        || count != 1
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}
