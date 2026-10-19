//! Independent native output-mode and cursor receipts; no host-side helpers.
use std::io;
use std::path::Path;
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, COORD, GetConsoleMode, GetConsoleScreenBufferInfo, GetStdHandle,
    STD_OUTPUT_HANDLE, SetConsoleCursorPosition, SetConsoleMode, SetConsoleOutputCP, WriteConsoleW,
};
use windows_sys::Win32::System::SystemInformation::OSVERSIONINFOEXW;

#[link(name = "ntdll")]
unsafe extern "system" {
    fn RtlGetVersion(info: *mut OSVERSIONINFOEXW) -> i32;
}

pub(super) fn wide_margin(report: &Path) -> io::Result<()> {
    // SAFETY: only this child's private console, bounded initialized buffers.
    unsafe {
        let output = GetStdHandle(STD_OUTPUT_HANDLE);
        if SetConsoleMode(output, 5) == 0
            || SetConsoleCursorPosition(output, COORD { X: 15, Y: 1 }) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let text = [0x4e2d_u16, 0xd83d, 0xde00];
        let mut count = 0;
        if WriteConsoleW(
            output,
            text.as_ptr().cast(),
            text.len() as u32,
            &mut count,
            std::ptr::null(),
        ) == 0
            || count != 3
        {
            return Err(io::Error::last_os_error());
        }
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        let mut mode = 0;
        if GetConsoleScreenBufferInfo(output, &mut info) == 0
            || GetConsoleMode(output, &mut mode) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut receipt = format!(
            "{mode},{count},{},{}",
            info.dwCursorPosition.X, info.dwCursorPosition.Y
        );
        let final_text = [u16::from(b'Z')];
        if WriteConsoleW(
            output,
            final_text.as_ptr().cast(),
            1,
            &mut count,
            std::ptr::null(),
        ) == 0
            || count != 1
            || GetConsoleScreenBufferInfo(output, &mut info) == 0
        {
            return Err(io::Error::last_os_error());
        }
        receipt.push_str(&format!(
            "|{count},{},{}",
            info.dwCursorPosition.X, info.dwCursorPosition.Y
        ));
        std::fs::write(report, receipt)
    }
}

pub(super) fn run(mode: u32, report: &Path) -> io::Result<()> {
    // SAFETY: the controlled child's actual console, SDK-sized structures and
    // initialized UTF-16 write buffers. No borrowed handle becomes a File.
    unsafe {
        let output = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut initial = 0;
        if GetConsoleMode(output, &mut initial) == 0 {
            return Err(io::Error::last_os_error());
        }
        if SetConsoleOutputCP(437) == 0 || SetConsoleMode(output, mode) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut actual = 0;
        if GetConsoleMode(output, &mut actual) == 0 {
            return Err(io::Error::last_os_error());
        }
        let mut version = OSVERSIONINFOEXW {
            dwOSVersionInfoSize: std::mem::size_of::<OSVERSIONINFOEXW>() as u32,
            ..OSVERSIONINFOEXW::default()
        };
        if RtlGetVersion(&mut version) < 0 {
            return Err(io::Error::other("native Windows version query failed"));
        }
        let mut receipt = format!(
            "build={};initial={initial};mode={actual}",
            version.dwBuildNumber
        );
        for (column, row, text) in [
            (1, 1, "A\nB"),
            (1, 3, "C\tD"),
            (5, 4, "E\rF"),
            (10, 4, "G\x08H"),
            (14, 5, "XYZ"),
            (15, 6, "A"),
            (-1, -1, "B"),
            (0, 0, "\x1b[2;5H!"),
        ] {
            if column >= 0 && SetConsoleCursorPosition(output, COORD { X: column, Y: row }) == 0 {
                return Err(io::Error::last_os_error());
            }
            let text: Vec<u16> = text.encode_utf16().collect();
            let mut count = 0;
            if WriteConsoleW(
                output,
                text.as_ptr().cast(),
                text.len() as u32,
                &mut count,
                std::ptr::null(),
            ) == 0
                || count != text.len() as u32
            {
                return Err(io::Error::last_os_error());
            }
            let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
            if GetConsoleScreenBufferInfo(output, &mut info) == 0 {
                return Err(io::Error::last_os_error());
            }
            receipt.push_str(&format!(
                "|{},{}",
                info.dwCursorPosition.X, info.dwCursorPosition.Y
            ));
        }
        if mode & 5 == 5 {
            for sequence in ["\x1b[?7l", "\x1b[?7h"] {
                let sequence: Vec<u16> = sequence.encode_utf16().collect();
                let mut count = 0;
                if WriteConsoleW(
                    output,
                    sequence.as_ptr().cast(),
                    sequence.len() as u32,
                    &mut count,
                    std::ptr::null(),
                ) == 0
                    || count != sequence.len() as u32
                    || GetConsoleMode(output, &mut actual) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                receipt.push_str(&format!("|wrap={actual}"));
            }
        }
        std::fs::write(report, receipt)?;
        if SetConsoleMode(output, 15) == 0 {
            return Err(io::Error::last_os_error());
        }
        let marker: Vec<u16> = "\x1b]2;mode-projection-end:\x07".encode_utf16().collect();
        let mut count = 0;
        if WriteConsoleW(
            output,
            marker.as_ptr().cast(),
            marker.len() as u32,
            &mut count,
            std::ptr::null(),
        ) == 0
            || count != marker.len() as u32
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
