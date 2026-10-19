//! Independent native cursor properties and fenced VT observations.
use super::mixed_output::{bytes, position};
use std::{io, os::windows::io::AsRawHandle, path::Path};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::System::Console::{
    CONSOLE_CURSOR_INFO, COORD, GetConsoleCursorInfo, GetStdHandle, STD_INPUT_HANDLE,
    STD_OUTPUT_HANDLE, SetConsoleCursorInfo, SetConsoleMode, WriteConsoleOutputCharacterW,
};

fn query(output: HANDLE) -> io::Result<CONSOLE_CURSOR_INFO> {
    let mut info = CONSOLE_CURSOR_INFO::default();
    // SAFETY: initialized SDK record and this controlled console's handle.
    if unsafe { GetConsoleCursorInfo(output, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(info)
}

fn set(output: HANDLE, size: u32, visible: i32) -> io::Result<()> {
    let info = CONSOLE_CURSOR_INFO {
        dwSize: size,
        bVisible: visible,
    };
    // SAFETY: initialized value record, synchronous SDK call and private console.
    if unsafe { SetConsoleCursorInfo(output, &info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn stage(output: HANDLE, path: &Path, label: &str, report: &mut String) -> io::Result<()> {
    let info = query(output)?;
    report.push_str(&format!("{label}:{},{}\n", info.dwSize, info.bVisible));
    position(output, label, report)?;
    let marker = format!("cursor-{label}:");
    if label == "partial" {
        let text: Vec<_> = marker.encode_utf16().collect();
        let mut written = 0;
        // SAFETY: bounded initialized literal cells, live count and private handle.
        if unsafe {
            WriteConsoleOutputCharacterW(
                output,
                text.as_ptr(),
                text.len() as u32,
                COORD { X: 0, Y: 7 },
                &mut written,
            )
        } == 0
            || written != text.len() as u32
        {
            return Err(io::Error::last_os_error());
        }
    } else {
        bytes(output, format!("\x1b]2;{marker}\x07").as_bytes())?;
    }
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !path.with_extension(label).exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}

pub(super) fn run(path: &Path) -> io::Result<()> {
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(output, 15) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    // Establish a painted, visible cursor before the bulk transitions. A cold
    // system renderer can otherwise coalesce its first hide with startup.
    set(output, 25, 1)?;
    bytes(output, b"\x1b[2 q\x1b[?25h\x1b[HP")?;
    stage(output, path, "prime", &mut report)?;
    bytes(output, b"\x1b[2J\x1b[H")?;
    for size in 1..=100 {
        for visible in [0, 2] {
            set(output, size, visible)?;
            let actual = query(output)?;
            if actual.dwSize != size || actual.bVisible != i32::from(visible != 0) {
                return Err(io::Error::other("native cursor property roundtrip"));
            }
        }
    }
    report.push_str("bulk:200\n");
    for size in [0, 101, u32::MAX] {
        let before = query(output)?;
        let info = CONSOLE_CURSOR_INFO {
            dwSize: size,
            bVisible: 0,
        };
        let ok = unsafe { SetConsoleCursorInfo(output, &info) };
        let error = if ok == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        let after = query(output)?;
        report.push_str(&format!(
            "invalid:{size}:{ok},{error},{}\n",
            before.dwSize == after.dwSize && before.bVisible == after.bVisible
        ));
    }
    for (label, access, size) in [
        ("readonly", 0x8000_0000, 39),
        ("writeonly", 0x4000_0000, 39),
        ("readonly-invalid", 0x8000_0000, 0),
    ] {
        let handle = super::console_file::open("CONOUT$", access)?;
        let info = CONSOLE_CURSOR_INFO {
            dwSize: size,
            bVisible: 0,
        };
        let ok = unsafe { SetConsoleCursorInfo(handle.as_raw_handle().cast(), &info) };
        let error = if ok == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        report.push_str(&format!("{label}:{ok},{error}\n"));
    }
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let info = CONSOLE_CURSOR_INFO {
        dwSize: 25,
        bVisible: 1,
    };
    let ok = unsafe { SetConsoleCursorInfo(input, &info) };
    let error = if ok == 0 {
        unsafe { GetLastError() }
    } else {
        0
    };
    report.push_str(&format!("input:{ok},{error}\n"));
    bytes(output, b"\x1b[6 q\x1b[31;44m\x1b[2;3HX")?;
    set(output, 63, 0)?;
    stage(output, path, "bar-hide", &mut report)?;
    set(output, 63, 1)?;
    stage(output, path, "bar-show", &mut report)?;
    bytes(output, b"\x1b[4 q\x1b[?25l")?;
    stage(output, path, "underline", &mut report)?;
    let size = query(output)?.dwSize;
    set(output, size, 0)?;
    stage(output, path, "same-hide", &mut report)?;
    set(output, 100, 1)?;
    stage(output, path, "new-show", &mut report)?;
    bytes(output, b"\x1b[3")?;
    set(output, 25, 0)?;
    stage(output, path, "partial", &mut report)?;
    bytes(output, b"2mZ")?;
    stage(output, path, "resumed", &mut report)?;
    set(output, 75, 1)?;
    bytes(output, b"\x1b[2 q")?;
    stage(output, path, "final", &mut report)?;
    std::fs::write(path, report)?;
    Ok(())
}
