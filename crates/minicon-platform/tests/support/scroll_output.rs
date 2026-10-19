//! Independent native block-move calls and full CHAR_INFO snapshots.
use super::mixed_output::{bytes, position};
use std::os::windows::io::AsRawHandle;
use std::{io, path::Path};
use windows_sys::Win32::Foundation::GetLastError;
use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::System::Console::{
    CHAR_INFO, COORD, GetStdHandle, ReadConsoleOutputW, SMALL_RECT, STD_OUTPUT_HANDLE,
    ScrollConsoleScreenBufferA, ScrollConsoleScreenBufferW, SetConsoleCursorPosition,
    SetConsoleMode, SetConsoleOutputCP, SetConsoleTextAttribute, WriteConsoleOutputW,
};

fn rect(left: i16, top: i16, right: i16, bottom: i16) -> SMALL_RECT {
    SMALL_RECT {
        Left: left,
        Top: top,
        Right: right,
        Bottom: bottom,
    }
}

fn cell(unit: u16, attribute: u16) -> CHAR_INFO {
    let mut value = CHAR_INFO {
        Attributes: attribute,
        ..CHAR_INFO::default()
    };
    value.Char.UnicodeChar = unit;
    value
}

fn seed(output: HANDLE) -> io::Result<()> {
    let data: Vec<_> = (0..48_u16).map(|i| cell(0x41 + i, 0x1e + i / 8)).collect();
    let mut region = rect(0, 0, 7, 5);
    // SAFETY: exact 8x6 initialized cells, private console and live region.
    if unsafe {
        WriteConsoleOutputW(
            output,
            data.as_ptr(),
            COORD { X: 8, Y: 6 },
            COORD { X: 0, Y: 0 },
            &mut region,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    Ok(())
}

fn snapshot(output: HANDLE, label: &str, report: &mut String) -> io::Result<()> {
    let mut data = [CHAR_INFO::default(); 48];
    let mut region = rect(0, 0, 7, 5);
    // SAFETY: exact 8x6 initialized target and bounded native region.
    if unsafe {
        ReadConsoleOutputW(
            output,
            data.as_mut_ptr(),
            COORD { X: 8, Y: 6 },
            COORD { X: 0, Y: 0 },
            &mut region,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    report.push_str(label);
    report.push(':');
    for value in data {
        // SAFETY: ReadConsoleOutputW wrote Unicode CHAR_INFO cells.
        report.push_str(&format!(
            "{:04x}/{:04x},",
            unsafe { value.Char.UnicodeChar },
            value.Attributes
        ));
    }
    report.push('\n');
    Ok(())
}

pub(super) fn run(path: &Path) -> io::Result<()> {
    // SAFETY: controlled client's private console, bounded initial cursor.
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(output, 15) } == 0
        || unsafe { SetConsoleOutputCP(437) } == 0
        || unsafe { SetConsoleCursorPosition(output, COORD { X: 6, Y: 5 }) } == 0
        || unsafe { SetConsoleTextAttribute(output, 0x2e) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut report = String::new();
    bytes(output, b"\x1b[3")?;
    for (label, source, destination, clip, fill, ansi) in [
        (
            "right",
            rect(0, 0, 5, 1),
            COORD { X: 2, Y: 0 },
            None,
            cell(0x2e, 0x4f),
            false,
        ),
        (
            "left",
            rect(2, 0, 7, 1),
            COORD { X: 0, Y: 0 },
            None,
            cell(0x2e, 0x4f),
            false,
        ),
        (
            "clip",
            rect(0, 0, 5, 2),
            COORD { X: 2, Y: 1 },
            Some(rect(1, 0, 5, 2)),
            cell(0x5f, 0x2e),
            false,
        ),
        (
            "negative",
            rect(-2, -1, 3, 1),
            COORD { X: 0, Y: 1 },
            None,
            cell(0x2e, 0x4f),
            false,
        ),
        (
            "offscreen",
            rect(1, 1, 3, 2),
            COORD { X: 20, Y: 20 },
            None,
            cell(0x23, 0x3a),
            false,
        ),
        (
            "ansi",
            rect(0, 0, 3, 0),
            COORD { X: 2, Y: 0 },
            None,
            cell(0xb3, 0x2e),
            true,
        ),
    ] {
        seed(output)?;
        let clip_pointer = clip.as_ref().map_or(std::ptr::null(), std::ptr::from_ref);
        // SAFETY: initialized value rectangles/fill, bounded console; the
        // optional clipping value lives through this synchronous SDK call.
        let ok = unsafe {
            if ansi {
                ScrollConsoleScreenBufferA(output, &source, clip_pointer, destination, &fill)
            } else {
                ScrollConsoleScreenBufferW(output, &source, clip_pointer, destination, &fill)
            }
        };
        if ok == 0 {
            return Err(io::Error::other(format!(
                "{label}: {}",
                io::Error::last_os_error()
            )));
        }
        snapshot(output, label, &mut report)?;
    }
    let source = rect(0, 0, 0, 0);
    let fill = cell(0x2e, 0x4f);
    for (label, access) in [("read-only", 0x8000_0000), ("write-only", 0x4000_0000)] {
        let handle = super::console_file::open("CONOUT$", access)?;
        // SAFETY: private owned console file; initialized value arguments.
        let ok = unsafe {
            ScrollConsoleScreenBufferW(
                handle.as_raw_handle().cast(),
                &source,
                std::ptr::null(),
                COORD { X: 0, Y: 0 },
                &fill,
            )
        };
        let error = if ok == 0 {
            unsafe { GetLastError() }
        } else {
            0
        };
        report.push_str(&format!("{label}:{ok},{error}\n"));
    }
    position(output, "preserved", &mut report)?;
    bytes(output, b"1mZ")?;
    position(output, "resumed", &mut report)?;
    std::fs::write(path, report)?;
    bytes(output, b"\x1b]2;scroll-end:\x07")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !path.with_extension("opened").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}
