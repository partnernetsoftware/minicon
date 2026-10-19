//! Malformed UTF-8, native cursor interleaving and final-tail SDK producer.
use super::mixed_output::bytes;
use std::{io, path::Path};
use windows_sys::Win32::System::Console::*;

fn cell(output: windows_sys::Win32::Foundation::HANDLE, x: i16, y: i16) -> io::Result<u16> {
    let mut units = [0; 2];
    let mut count = 0;
    // SAFETY: two live UTF-16 output units and count, private console handle.
    // Include the continuation cell so readback does not clip a wide glyph.
    if unsafe {
        ReadConsoleOutputCharacterW(
            output,
            units.as_mut_ptr(),
            2,
            COORD { X: x, Y: y },
            &mut count,
        )
    } == 0
        || count == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(units[0])
}

pub(super) fn run(path: &Path, mapped: bool) -> io::Result<()> {
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleOutputCP(65001) } == 0 || unsafe { SetConsoleMode(output, 7) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if mapped && unsafe { SetConsoleScreenBufferSize(output, COORD { X: 16, Y: 3 }) } == 0 {
        return Err(io::Error::last_os_error());
    }
    bytes(output, b"\xe4\xb8")?;
    bytes(output, b"\xe3")?;
    if cell(output, 0, 0)? != 0xfffd {
        return Err(io::Error::other("missing immediate native replacement"));
    }
    if unsafe { SetConsoleCursorPosition(output, COORD { X: 2, Y: 1 }) } == 0 {
        return Err(io::Error::last_os_error());
    }
    std::fs::write(path.with_extension("ready"), b"native-replacement")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !path.with_extension("go").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    bytes(output, b"\x81\x82")?;
    if cell(output, 2, 1)? != 0x3042 {
        return Err(io::Error::other("native edit corrupted unfinished scalar"));
    }
    bytes(output, b"\xe4\xb8\xe3")?;
    if cell(output, 4, 1)? != 0xfffd {
        return Err(io::Error::other("missing final native replacement"));
    }
    if unsafe { SetConsoleCursorPosition(output, COORD { X: 0, Y: 2 }) } == 0 {
        return Err(io::Error::last_os_error());
    }
    std::fs::write(
        path,
        format!("mapped={mapped};replacement=fffd;resumed=3042;tail=fffd"),
    )?;
    Ok(())
}
