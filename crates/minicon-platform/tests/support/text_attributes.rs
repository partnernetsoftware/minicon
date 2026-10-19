//! Native drawing-attribute caller, independent of host implementation helpers.
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use windows_sys::Win32::Foundation::{GENERIC_READ, GENERIC_WRITE, GetLastError};
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, COORD, GetConsoleScreenBufferInfo, GetStdHandle, STD_OUTPUT_HANDLE,
    SetConsoleCursorPosition, SetConsoleMode, SetConsoleTextAttribute, WriteConsoleW,
};

pub(super) fn run(report: &Path, owned: bool) -> io::Result<()> {
    // SAFETY: this controlled child's private console and initialized SDK buffers.
    unsafe {
        let output = GetStdHandle(STD_OUTPUT_HANDLE);
        if SetConsoleMode(output, 31) == 0 {
            return Err(io::Error::last_os_error());
        }
        let read = super::console_file::open("CONOUT$", GENERIC_READ)?;
        let write = super::console_file::open("CONOUT$", GENERIC_WRITE)?;
        if SetConsoleTextAttribute(read.as_raw_handle(), 0x12) != 0 || GetLastError() != 5 {
            return Err(io::Error::other(
                "read-only attribute setter must fail with access denied",
            ));
        }
        if owned && (SetConsoleTextAttribute(output, 0x0407) != 0 || GetLastError() != 50) {
            return Err(io::Error::other(
                "unsupported grid attribute must fail explicitly",
            ));
        }
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        let mut receipt = format!("build={};", super::output_modes::windows_build()?);
        for attribute in (0..256_u16).chain([0x401e, 0x802f, 0xc03a]) {
            let index = if attribute < 256 {
                attribute
            } else {
                256 + (attribute >> 14) - 1
            };
            let position = COORD {
                X: (index % 16) as i16,
                Y: (index / 16) as i16,
            };
            if SetConsoleCursorPosition(output, position) == 0
                || SetConsoleTextAttribute(write.as_raw_handle(), attribute) == 0
                || GetConsoleScreenBufferInfo(output, &mut info) == 0
                || info.wAttributes != attribute
                || info.dwCursorPosition.X != position.X
                || info.dwCursorPosition.Y != position.Y
            {
                return Err(io::Error::other(format!(
                    "attribute or cursor mismatch at {attribute:04x}"
                )));
            }
            let text = [u16::from(b'X')];
            let mut count = 0;
            if WriteConsoleW(
                output,
                text.as_ptr().cast(),
                1,
                &mut count,
                std::ptr::null(),
            ) == 0
                || count != 1
            {
                return Err(io::Error::last_os_error());
            }
            receipt.push_str(&format!("{attribute:04x},"));
        }
        // Native queries must reflect VT changes too, rather than a native-only cache.
        let text: Vec<u16> = "\x1b[0;94;43;4;7mY".encode_utf16().collect();
        let mut count = 0;
        if WriteConsoleW(
            output,
            text.as_ptr().cast(),
            text.len() as u32,
            &mut count,
            std::ptr::null(),
        ) == 0
            || GetConsoleScreenBufferInfo(output, &mut info) == 0
        {
            return Err(io::Error::last_os_error());
        }
        receipt.push_str(&format!("vt={:04x}", info.wAttributes));
        std::fs::write(report, receipt)?;
        let marker: Vec<u16> = "\x1b]2;attributes-end:\x07".encode_utf16().collect();
        if WriteConsoleW(
            output,
            marker.as_ptr().cast(),
            marker.len() as u32,
            &mut count,
            std::ptr::null(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while !report.with_extension("opened").exists() {
            if std::time::Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        Ok(())
    }
}
