//! Independent SDK fill/readback and projection fixture.
use std::os::windows::io::AsRawHandle;
use std::{io, path::Path};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, COORD, FillConsoleOutputAttribute, FillConsoleOutputCharacterA,
    FillConsoleOutputCharacterW, GetConsoleScreenBufferInfo, GetStdHandle,
    ReadConsoleOutputAttribute, ReadConsoleOutputCharacterW, STD_OUTPUT_HANDLE,
    SetConsoleCursorPosition, SetConsoleMode, SetConsoleOutputCP, SetConsoleTextAttribute,
    WriteConsoleW,
};

pub(super) fn run(report: &Path) -> io::Result<()> {
    // SAFETY: only this child's console; SDK buffers are initialized and bounded.
    unsafe {
        let output = GetStdHandle(STD_OUTPUT_HANDLE);
        if SetConsoleMode(output, 31) == 0
            || SetConsoleOutputCP(437) == 0
            || SetConsoleTextAttribute(output, 0x1e) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let text: Vec<u16> = "abcdefghijklmnopqrstuvwx".encode_utf16().collect();
        let mut count = 0;
        if WriteConsoleW(
            output,
            text.as_ptr().cast(),
            text.len() as u32,
            &mut count,
            std::ptr::null(),
        ) == 0
            || SetConsoleCursorPosition(output, COORD { X: 5, Y: 3 }) == 0
            || SetConsoleTextAttribute(output, 0x2d) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut receipt = format!("build={}\n", super::output_modes::windows_build()?);
        for (kind, element, x, y, length) in [
            (2, b'Z' as u16, 10, 0, 5),
            (3, 0x4f, 11, 0, 3),
            (2, 0x4e2d, 1, 1, 5),
            (2, b'Q' as u16, 2, 1, 1),
            (2, 0x1b, 0, 2, 2),
            (2, 0, 2, 2, 1),
            (2, 9, 3, 2, 1),
            (1, 0xb3, 4, 2, 2),
            (2, b'!' as u16, 11, 7, 10),
            (2, 0x4e2d, 11, 3, 3),
            (2, 0x301, 7, 2, 2),
            (2, 0xfffd, 9, 2, 1),
            (2, b'A' as u16, -1, 0, 2),
            (3, 7, 12, 0, 2),
            (2, 32, 0, 8, 2),
            (2, 32, 0, 0, 0),
        ] {
            let coord = COORD { X: x, Y: y };
            let mut written = 0xcccc_cccc;
            let result = match kind {
                1 => {
                    FillConsoleOutputCharacterA(output, element as i8, length, coord, &mut written)
                }
                3 => FillConsoleOutputAttribute(output, element, length, coord, &mut written),
                _ => FillConsoleOutputCharacterW(output, element, length, coord, &mut written),
            };
            let error = if result == 0 { GetLastError() } else { 0 };
            receipt.push_str(&format!(
                "fill:{kind},{element:04x},{x},{y},{length}:{result},{error},{written}\n"
            ));
            for row in [0, 1, 2, 3, 4, 7] {
                let mut chars = [0xcccc_u16; 12];
                let mut attrs = [0xcccc_u16; 12];
                let mut chars_read = 0;
                let mut attrs_read = 0;
                if ReadConsoleOutputCharacterW(
                    output,
                    chars.as_mut_ptr(),
                    12,
                    COORD { X: 0, Y: row },
                    &mut chars_read,
                ) == 0
                    || ReadConsoleOutputAttribute(
                        output,
                        attrs.as_mut_ptr(),
                        12,
                        COORD { X: 0, Y: row },
                        &mut attrs_read,
                    ) == 0
                {
                    return Err(io::Error::last_os_error());
                }
                receipt.push_str(&format!(
                    "row:{row}:{:04x?}:{:04x?}\n",
                    &chars[..chars_read as usize],
                    &attrs[..attrs_read as usize]
                ));
            }
        }
        let read =
            super::console_file::open("CONOUT$", windows_sys::Win32::Foundation::GENERIC_READ)?;
        for kind in [2, 3] {
            let mut written = 0xcccc_cccc;
            let result = if kind == 2 {
                FillConsoleOutputCharacterW(
                    read.as_raw_handle(),
                    88,
                    1,
                    COORD { X: 0, Y: 0 },
                    &mut written,
                )
            } else {
                FillConsoleOutputAttribute(
                    read.as_raw_handle(),
                    7,
                    1,
                    COORD { X: 0, Y: 0 },
                    &mut written,
                )
            };
            let error = if result == 0 { GetLastError() } else { 0 };
            receipt.push_str(&format!("access:{kind}:{result},{error},{written}\n"));
        }
        let write =
            super::console_file::open("CONOUT$", windows_sys::Win32::Foundation::GENERIC_WRITE)?;
        let mut written = 0xcccc_cccc;
        let result = FillConsoleOutputCharacterW(
            write.as_raw_handle(),
            32,
            0,
            COORD { X: 0, Y: 0 },
            &mut written,
        );
        let error = if result == 0 { GetLastError() } else { 0 };
        receipt.push_str(&format!("write-only:{result},{error},{written}\n"));
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        if GetConsoleScreenBufferInfo(output, &mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
        receipt.push_str(&format!(
            "cursor={},{};attribute={:04x}\n",
            info.dwCursorPosition.X, info.dwCursorPosition.Y, info.wAttributes
        ));
        std::fs::write(report, receipt)?;
        let marker: Vec<u16> = "\x1b]2;fill-end:\x07".encode_utf16().collect();
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
