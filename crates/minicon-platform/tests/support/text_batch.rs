//! Real bulk W/UTF-8 writes into an expanded native drawing buffer.
use super::mixed_output::bytes;
use std::{io, path::Path};
use windows_sys::Win32::System::Console::*;

pub(super) fn run(path: &Path, unicode: bool, mode: u32) -> io::Result<()> {
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleOutputCP(65001) } == 0
        || unsafe { SetConsoleMode(output, mode) } == 0
        || unsafe { SetConsoleScreenBufferSize(output, COORD { X: 256, Y: 64 }) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let text = "Ae\u{301}中😀BC".repeat(1024);
    let expected;
    let mut written = 0;
    if unicode {
        let units: Vec<_> = text.encode_utf16().collect();
        expected = units.len();
        if unsafe {
            WriteConsoleW(
                output,
                units.as_ptr().cast(),
                units.len() as u32,
                &mut written,
                std::ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
    } else {
        expected = text.len();
        bytes(output, text.as_bytes())?;
        written = expected as u32;
    }
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    if unsafe { GetConsoleScreenBufferInfo(output, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut cells = [CHAR_INFO::default(); 16];
    let mut area = SMALL_RECT {
        Left: 0,
        Top: 0,
        Right: 15,
        Bottom: 0,
    };
    if unsafe {
        ReadConsoleOutputW(
            output,
            cells.as_mut_ptr(),
            COORD { X: 16, Y: 1 },
            COORD { X: 0, Y: 0 },
            &mut area,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let glyphs = cells
        .iter()
        .map(|cell| format!("{:04x}", unsafe { cell.Char.UnicodeChar }))
        .collect::<Vec<_>>()
        .join(",");
    std::fs::write(
        path,
        format!(
            "build={}\nmode={mode};unicode={unicode};written={written}/{expected};size={},{};cursor={},{}\nglyphs={glyphs}\n",
            super::output_modes::windows_build()?,
            info.dwSize.X,
            info.dwSize.Y,
            info.dwCursorPosition.X,
            info.dwCursorPosition.Y
        ),
    )?;
    if unsafe { SetConsoleMode(output, 7) } == 0 {
        return Err(io::Error::last_os_error());
    }
    bytes(output, b"\x1b]2;text-batch-end:\x07")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !path.with_extension("opened").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}
