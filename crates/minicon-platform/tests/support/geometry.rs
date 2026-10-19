//! Independent native geometry observations; parent PTY size is separate.
use super::mixed_output::bytes;
use std::{io, os::windows::io::AsRawHandle, path::Path};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::System::Console::{
    CONSOLE_SCREEN_BUFFER_INFO, COORD, GetConsoleScreenBufferInfo, GetLargestConsoleWindowSize,
    GetStdHandle, SMALL_RECT, STD_OUTPUT_HANDLE, SetConsoleMode, SetConsoleScreenBufferSize,
    SetConsoleWindowInfo,
};

fn query(output: HANDLE, label: &str, report: &mut String) -> io::Result<()> {
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    // SAFETY: private controlled console, initialized SDK output structure.
    if unsafe { GetConsoleScreenBufferInfo(output, &mut info) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let largest = unsafe { GetLargestConsoleWindowSize(output) };
    let error = if largest.X == 0 && largest.Y == 0 {
        unsafe { GetLastError() }
    } else {
        0
    };
    report.push_str(&format!(
        "{label}:buf={},{};view={},{},{},{};cursor={},{};max={},{};largest={},{},{error}\n",
        info.dwSize.X,
        info.dwSize.Y,
        info.srWindow.Left,
        info.srWindow.Top,
        info.srWindow.Right,
        info.srWindow.Bottom,
        info.dwCursorPosition.X,
        info.dwCursorPosition.Y,
        info.dwMaximumWindowSize.X,
        info.dwMaximumWindowSize.Y,
        largest.X,
        largest.Y
    ));
    Ok(())
}

fn buffer(output: HANDLE, x: i16, y: i16, label: &str, report: &mut String) -> io::Result<()> {
    let ok = unsafe { SetConsoleScreenBufferSize(output, COORD { X: x, Y: y }) };
    let error = if ok == 0 {
        unsafe { GetLastError() }
    } else {
        0
    };
    report.push_str(&format!("set-buffer:{label}:{ok},{error}\n"));
    query(unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }, label, report)
}

fn window(
    output: HANDLE,
    absolute: i32,
    values: [i16; 4],
    label: &str,
    report: &mut String,
) -> io::Result<()> {
    let rect = SMALL_RECT {
        Left: values[0],
        Top: values[1],
        Right: values[2],
        Bottom: values[3],
    };
    let ok = unsafe { SetConsoleWindowInfo(output, absolute, &rect) };
    let error = if ok == 0 {
        unsafe { GetLastError() }
    } else {
        0
    };
    report.push_str(&format!("set-window:{label}:{ok},{error}\n"));
    query(unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }, label, report)
}

pub(super) fn run(path: &Path) -> io::Result<()> {
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(output, 15) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    query(output, "initial", &mut report)?;
    bytes(output, b"baseline\x1b[3;5HX")?;
    buffer(output, 32, 16, "grow", &mut report)?;
    window(output, 1, [0, 0, 19, 9], "window20x10", &mut report)?;
    window(output, 0, [1, 1, 1, 1], "relative-shift", &mut report)?;
    buffer(output, 40, 20, "grow-again", &mut report)?;
    window(output, 1, [2, 1, 17, 8], "offset16x8", &mut report)?;
    buffer(output, 16, 8, "buffer16x8", &mut report)?;
    for (label, x, y) in [
        ("negative", -1, 8),
        ("zero", 0, 1),
        ("tiny", 1, 1),
        ("short-max", 32767, 8),
    ] {
        buffer(output, x, y, label, &mut report)?;
    }
    for (label, rect) in [
        ("negative-window", [-1, 0, 14, 7]),
        ("reversed-window", [2, 2, 1, 3]),
        ("one-cell", [0, 0, 0, 0]),
        ("restore", [0, 0, 15, 7]),
    ] {
        window(output, 1, rect, label, &mut report)?;
    }
    for (label, access) in [("readonly", 0x8000_0000), ("writeonly", 0x4000_0000)] {
        let handle = super::console_file::open("CONOUT$", access)?;
        let raw = handle.as_raw_handle().cast();
        let largest = unsafe { GetLargestConsoleWindowSize(raw) };
        let error = if largest.X == 0 && largest.Y == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        report.push_str(&format!(
            "access-largest:{label}:{},{},{error}\n",
            largest.X, largest.Y
        ));
        buffer(raw, 16, 8, label, &mut report)?;
        window(raw, 1, [0, 0, 15, 7], label, &mut report)?;
    }
    std::fs::write(path, report)?;
    bytes(output, b"\x1b]2;geometry-end:\x07")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !path.with_extension("opened").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}

pub(super) fn wrap(path: &Path, mode: u32) -> io::Result<()> {
    use windows_sys::Win32::System::Console::{ReadConsoleOutputCharacterW, WriteConsoleW};
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(output, mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut report = String::new();
    buffer(output, 32, 16, "grow", &mut report)?;
    let text: Vec<_> = "0123456789abcdefghijklmnopqrstuvwxyzABCD"
        .encode_utf16()
        .collect();
    let mut count = 0;
    for (label, part) in [("mid", &text[..20]), ("end", &text[20..])] {
        if unsafe {
            WriteConsoleW(
                output,
                part.as_ptr().cast(),
                part.len() as u32,
                &mut count,
                std::ptr::null(),
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        query(output, label, &mut report)?;
    }
    query(output, "written", &mut report)?;
    for row in 0..3 {
        let mut cells = [0; 32];
        let mut count = 0;
        if unsafe {
            ReadConsoleOutputCharacterW(
                output,
                cells.as_mut_ptr(),
                32,
                COORD { X: 0, Y: row },
                &mut count,
            )
        } == 0
        {
            return Err(io::Error::last_os_error());
        }
        report.push_str(&format!(
            "row{row}:{:?}\n",
            String::from_utf16_lossy(&cells[..count as usize])
        ));
    }
    std::fs::write(path, report)?;
    if unsafe { SetConsoleMode(output, 7) } == 0 {
        return Err(io::Error::last_os_error());
    }
    bytes(output, b"\x1b]2;geometry-wrap-end:\x07")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !path.with_extension("opened").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}

pub(super) fn parent_resize(path: &Path) -> io::Result<()> {
    use windows_sys::Win32::System::Console::{
        INPUT_RECORD, ReadConsoleInputW, STD_INPUT_HANDLE, WINDOW_BUFFER_SIZE_EVENT,
    };
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(input, 15) } == 0 || unsafe { SetConsoleMode(output, 7) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    buffer(output, 32, 16, "grow", &mut report)?;
    bytes(output, b"\x1b]2;geometry-parent-ready:\x07")?;
    loop {
        let mut event = INPUT_RECORD::default();
        let mut count = 0;
        // SAFETY: initialized one-record input and count in this private console.
        if unsafe { ReadConsoleInputW(input, &mut event, 1, &mut count) } == 0 {
            return Err(io::Error::last_os_error());
        }
        if count == 1 && event.EventType == WINDOW_BUFFER_SIZE_EVENT as u16 {
            let size = unsafe { event.Event.WindowBufferSizeEvent.dwSize };
            report.push_str(&format!("event:{},{}\n", size.X, size.Y));
            if size.X == 24 {
                break;
            }
        }
    }
    query(output, "parent-resized", &mut report)?;
    std::fs::write(path, report)?;
    bytes(output, b"\x1b]2;geometry-parent-done:\x07")?;
    Ok(())
}
