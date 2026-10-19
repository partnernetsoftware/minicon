//! Independent Win32 caller for fragmented client output/native projection.
use std::{io, path::Path};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::Storage::FileSystem::WriteFile;
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, COORD, FillConsoleOutputCharacterW, GetConsoleScreenBufferInfo,
    GetStdHandle, ReadConsoleOutputCharacterW, STD_OUTPUT_HANDLE, SetConsoleCursorPosition,
    SetConsoleMode, SetConsoleOutputCP, SetConsoleTextAttribute, WriteConsoleW,
};

pub(super) fn bytes(output: HANDLE, value: &[u8]) -> io::Result<()> {
    let mut written = 0;
    // SAFETY: live private console and initialized bounded byte buffer.
    if unsafe {
        WriteFile(
            output,
            value.as_ptr(),
            value.len() as u32,
            &mut written,
            std::ptr::null_mut(),
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if written != value.len() as u32 {
        return Err(io::Error::other("short client write"));
    }
    Ok(())
}

fn cursor(output: HANDLE, x: i16, y: i16) -> io::Result<()> {
    // SAFETY: private console and value coordinate.
    if unsafe { SetConsoleCursorPosition(output, COORD { X: x, Y: y }) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn attribute(output: HANDLE, value: u16) -> io::Result<()> {
    // SAFETY: private console and value attributes.
    if unsafe { SetConsoleTextAttribute(output, value) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn fill(output: HANDLE, x: i16, y: i16, value: u8) -> io::Result<()> {
    let mut written = 0;
    // SAFETY: private console, one character and live count buffer.
    if unsafe {
        FillConsoleOutputCharacterW(
            output,
            u16::from(value),
            1,
            COORD { X: x, Y: y },
            &mut written,
        )
    } == 0
        || written != 1
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

pub(super) fn tail(report: &Path, utf8: bool) -> io::Result<()> {
    // SAFETY: controlled private console; the final write intentionally leaves
    // input grammar incomplete, then exits without further producer output.
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(output, 15) } == 0 || unsafe { SetConsoleOutputCP(65001) } == 0 {
        return Err(io::Error::last_os_error());
    }
    bytes(output, b"tail:")?;
    bytes(output, if utf8 { &[0xe4, 0xb8] } else { b"\x1b[3" })?;
    std::fs::write(report, b"accepted")
}

pub(super) fn position(output: HANDLE, phase: &str, receipt: &mut String) -> io::Result<()> {
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: initialized SDK-sized output buffer.
    if unsafe { GetConsoleScreenBufferInfo(output, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    receipt.push_str(&format!(
        "{phase}:{},{};{:04x}\n",
        info.dwCursorPosition.X, info.dwCursorPosition.Y, info.wAttributes
    ));
    Ok(())
}

pub(super) fn run(report: &Path, budget: bool) -> io::Result<()> {
    // SAFETY: this controlled child's private console.
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(output, 15) } == 0 || unsafe { SetConsoleOutputCP(65001) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut receipt = format!("build={}\n", super::output_modes::windows_build()?);
    if budget {
        let mut fragment = vec![b'1'; 65540];
        fragment[0] = 27;
        fragment[1] = b'[';
        let mut written = 0xcccc_cccc;
        // SAFETY: bounded controlled buffer and count; synchronous console write.
        let result = unsafe {
            WriteFile(
                output,
                fragment.as_ptr(),
                fragment.len() as u32,
                &mut written,
                std::ptr::null_mut(),
            )
        };
        let error = if result == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        receipt.push_str(&format!("raw-limit:{result},{error},{written}\n"));
        bytes(output, b"RECOVER")?;
        let wide: Vec<u16> = fragment.iter().copied().map(u16::from).collect();
        let mut written = 0xcccc_cccc;
        // SAFETY: initialized UTF-16 units and a live count buffer.
        let result = unsafe {
            WriteConsoleW(
                output,
                wide.as_ptr().cast(),
                wide.len() as u32,
                &mut written,
                std::ptr::null(),
            )
        };
        let error = if result == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        // WriteConsoleW does not promise to update the caller's count on error.
        receipt.push_str(&format!("wide-limit:{result},{error}\n"));
        bytes(output, b"OK")?;
        position(output, "recovery", &mut receipt)?;
    } else {
        bytes(output, b"\x1b[3")?;
        fill(output, 0, 2, b'F')?;
        bytes(output, b"1mA")?;
        position(output, "csi", &mut receipt)?;
        bytes(output, &[0xe4])?;
        attribute(output, 0x2e)?;
        cursor(output, 2, 1)?;
        bytes(output, &[0xb8, 0xad])?;
        position(output, "utf8", &mut receipt)?;
        bytes(output, b"\x1b[2;\n")?;
        position(output, "control", &mut receipt)?;
        fill(output, 4, 3, b'X')?;
        bytes(output, b"3HB")?;
        bytes(output, b"\x1b]2;native-")?;
        cursor(output, 5, 4)?;
        fill(output, 7, 4, b'!')?;
        bytes(output, b"title\x07C")?;
        bytes(output, b"\x1bPqDCS")?;
        fill(output, 8, 4, b'D')?;
        bytes(output, b"\x1b\\")?;
        bytes(output, b"\x1b[99")?;
        fill(output, 9, 4, b'N')?;
        bytes(output, b"\x18R")?;
        bytes(output, b"\x1b(")?;
        attribute(output, 0x1f)?;
        bytes(output, b"BT")?;
        bytes(output, &[0xf0, 0x9f])?;
        fill(output, 10, 4, b'Z')?;
        bytes(output, &[0x98, 0x80, b'Q'])?;
        position(output, "strings", &mut receipt)?;
        cursor(output, 0, 7)?;
        bytes(output, b"\x1b[1;\n")?;
        fill(output, 0, 7, b'J')?;
        bytes(output, b"HK")?;
        position(output, "scroll", &mut receipt)?;
        for (x, y) in [
            (0, 0),
            (2, 0),
            (0, 1),
            (4, 2),
            (5, 3),
            (6, 3),
            (7, 3),
            (10, 3),
            (0, 7),
        ] {
            let mut value = [0_u16; 1];
            let mut count = 0;
            // SAFETY: one initialized output unit and count, private console.
            if unsafe {
                ReadConsoleOutputCharacterW(
                    output,
                    value.as_mut_ptr(),
                    1,
                    COORD { X: x, Y: y },
                    &mut count,
                )
            } == 0
                || count != 1
            {
                return Err(io::Error::last_os_error());
            }
            receipt.push_str(&format!("cell:{x},{y}:{:04x}\n", value[0]));
        }
    }
    std::fs::write(report, receipt)?;
    bytes(output, b"\x1b]2;mixed-end:\x07")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !report.with_extension("opened").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}
