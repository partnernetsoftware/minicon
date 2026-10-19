//! SDK receipts for literal replacement scalars and owned decoding recovery.
use super::mixed_output::{bytes, position};
use std::{io, path::Path};
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Console::{
    COORD, FillConsoleOutputCharacterW, GetStdHandle, ReadConsoleOutputCharacterW,
    STD_OUTPUT_HANDLE, SetConsoleCursorPosition, SetConsoleMode, SetConsoleOutputCP, WriteConsoleW,
};

fn wide(output: HANDLE, units: &[u16]) -> io::Result<()> {
    let mut written = 0;
    // SAFETY: initialized UTF-16 units and count, live private console.
    if unsafe {
        WriteConsoleW(
            output,
            units.as_ptr().cast(),
            units.len() as u32,
            &mut written,
            std::ptr::null(),
        )
    } == 0
        || written as usize != units.len()
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn cells(output: HANDLE, row: i16, receipt: &mut String) -> io::Result<()> {
    let mut units = [0_u16; 8];
    let mut count = 0;
    // SAFETY: initialized bounded UTF-16 output and count buffers.
    if unsafe {
        ReadConsoleOutputCharacterW(
            output,
            units.as_mut_ptr(),
            8,
            COORD { X: 0, Y: row },
            &mut count,
        )
    } == 0
        || count != 8
    {
        return Err(io::Error::last_os_error());
    }
    receipt.push_str(&format!("row{row}:"));
    for unit in units {
        receipt.push_str(&format!("{unit:04x},"));
    }
    receipt.push('\n');
    Ok(())
}

pub(super) fn run(report: &Path, owned: bool) -> io::Result<()> {
    // SAFETY: this controlled client's private console.
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(output, 15) } == 0 || unsafe { SetConsoleOutputCP(65001) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut receipt = String::new();
    wide(output, &[0x41, 0xfffd, 0x42])?;
    position(output, "wide", &mut receipt)?;
    bytes(output, b"C\xef\xbf\xbdD")?;
    position(output, "utf8", &mut receipt)?;
    let mut count = 0;
    // SAFETY: one literal fill at an in-bounds coordinate, live count output.
    if unsafe { FillConsoleOutputCharacterW(output, 0xfffd, 1, COORD { X: 7, Y: 0 }, &mut count) }
        == 0
        || count != 1
    {
        return Err(io::Error::last_os_error());
    }
    bytes(output, b"X")?;
    position(output, "fill", &mut receipt)?;
    cells(output, 0, &mut receipt)?;
    if owned {
        // Owned decoder recovery is a separate contract from the installed
        // host's handling of malformed UTF-16 and invalid code-page bytes.
        if unsafe { SetConsoleCursorPosition(output, COORD { X: 0, Y: 2 }) } == 0 {
            return Err(io::Error::last_os_error());
        }
        wide(output, &[0xd800, 0x45, 0xdc00, 0x46])?;
        position(output, "invalid-wide", &mut receipt)?;
        bytes(output, &[0xe4, b'G', 0xff, b'H'])?;
        position(output, "invalid-utf8", &mut receipt)?;
        cells(output, 2, &mut receipt)?;
    }
    std::fs::write(report, receipt)?;
    bytes(output, b"\x1b]2;unicode-end:\x07")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !report.with_extension("opened").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}
