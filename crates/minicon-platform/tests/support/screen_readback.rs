//! Independent native screen readback observations.
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, COORD, GetConsoleScreenBufferInfo, GetStdHandle,
    ReadConsoleOutputAttribute, ReadConsoleOutputCharacterA, ReadConsoleOutputCharacterW,
    STD_OUTPUT_HANDLE, SetConsoleCursorPosition, SetConsoleMode, SetConsoleOutputCP,
    SetConsoleTextAttribute, WriteConsoleW,
};

pub(super) fn run(report: &Path, complex: bool, owned: bool) -> io::Result<()> {
    // SAFETY: this private console, initialized fixed-size SDK buffers and
    // lengths checked against those buffers; owned file handles stay live.
    unsafe {
        let output = GetStdHandle(STD_OUTPUT_HANDLE);
        if SetConsoleMode(output, 31) == 0 {
            return Err(io::Error::last_os_error());
        }
        let text = if complex {
            "A中😀e\u{301}Z"
        } else {
            "A中QZ"
        };
        for (row, attribute, text) in [(0, 0x1e, text), (1, 0x2d, "tail")] {
            if SetConsoleTextAttribute(output, attribute) == 0
                || SetConsoleCursorPosition(output, COORD { X: 0, Y: row }) == 0
            {
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
        }
        let write =
            super::console_file::open("CONOUT$", windows_sys::Win32::Foundation::GENERIC_WRITE)?;
        let read =
            super::console_file::open("CONOUT$", windows_sys::Win32::Foundation::GENERIC_READ)?;
        let reader = read.as_raw_handle();
        let mut receipt = format!("build={}\n", super::output_modes::windows_build()?);
        for (x, y, length) in [
            (0, 0, 12),
            (1, 0, 1),
            (2, 0, 4),
            (3, 0, 1),
            (4, 0, 3),
            (5, 0, 4),
            (10, 0, 6),
            (11, 7, 12),
            (-1, 0, 2),
            (12, 0, 2),
            (0, 8, 2),
            (0, 0, 0),
        ] {
            let coord = COORD { X: x, Y: y };
            let mut buffer = [0xcccc_u16; 32];
            let mut count = 0xcccc_cccc;
            let result =
                ReadConsoleOutputCharacterW(reader, buffer.as_mut_ptr(), length, coord, &mut count);
            let error = if result == 0 { GetLastError() } else { 0 };
            receipt.push_str(&format!(
                "W:{x},{y},{length}:{result},{error},{count}:{:04x?}\n",
                &buffer[..(count as usize).min(32)]
            ));
            let mut count = 0xcccc_cccc;
            let result =
                ReadConsoleOutputAttribute(reader, buffer.as_mut_ptr(), length, coord, &mut count);
            let error = if result == 0 { GetLastError() } else { 0 };
            receipt.push_str(&format!(
                "T:{x},{y},{length}:{result},{error},{count}:{:04x?}\n",
                &buffer[..(count as usize).min(32)]
            ));
        }
        for code_page in [65001, 437] {
            if SetConsoleOutputCP(code_page) == 0 {
                return Err(io::Error::last_os_error());
            }
            for (x, length) in [(0, 12), (1, 1), (2, 4), (3, 1), (4, 3)] {
                let mut buffer = [0xcc_u8; 32];
                let mut count = 0xcccc_cccc;
                let result = ReadConsoleOutputCharacterA(
                    reader,
                    buffer.as_mut_ptr(),
                    length,
                    COORD { X: x, Y: 0 },
                    &mut count,
                );
                let error = if result == 0 { GetLastError() } else { 0 };
                receipt.push_str(&format!(
                    "A{code_page}:{x},{length}:{result},{error},{count}:{:02x?}\n",
                    &buffer[..(count as usize).min(32)]
                ));
            }
        }
        for attributes in [false, true] {
            let mut buffer = [0xcccc_u16; 2];
            let mut count = 0xcccc_cccc;
            let result = if attributes {
                ReadConsoleOutputAttribute(
                    write.as_raw_handle(),
                    buffer.as_mut_ptr(),
                    2,
                    COORD { X: 0, Y: 0 },
                    &mut count,
                )
            } else {
                ReadConsoleOutputCharacterW(
                    write.as_raw_handle(),
                    buffer.as_mut_ptr(),
                    2,
                    COORD { X: 0, Y: 0 },
                    &mut count,
                )
            };
            let error = if result == 0 { GetLastError() } else { 0 };
            receipt.push_str(&format!("access:{attributes}:{result},{error},{count}\n"));
        }
        if owned {
            let mut buffer = vec![0_u8; 1024 * 1024 + 1];
            let mut count = 0xcccc_cccc;
            let result = ReadConsoleOutputCharacterA(
                reader,
                buffer.as_mut_ptr(),
                buffer.len() as u32,
                COORD { X: 0, Y: 0 },
                &mut count,
            );
            let error = if result == 0 { GetLastError() } else { 0 };
            receipt.push_str(&format!("budget:{result},{error},{count}\n"));
        }
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        if GetConsoleScreenBufferInfo(output, &mut info) == 0 {
            return Err(io::Error::last_os_error());
        }
        receipt.push_str(&format!(
            "cursor={},{};attribute={:04x}\n",
            info.dwCursorPosition.X, info.dwCursorPosition.Y, info.wAttributes
        ));
        std::fs::write(report, receipt)
    }
}
