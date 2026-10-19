//! Independent SDK string/attribute writes and physical-cell observations.
use super::mixed_output::{bytes, position};
use std::{io, os::windows::io::AsRawHandle, path::Path};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE};
use windows_sys::Win32::System::Console::{
    CHAR_INFO, COORD, GetStdHandle, ReadConsoleOutputA, ReadConsoleOutputW, SMALL_RECT,
    STD_OUTPUT_HANDLE, SetConsoleCursorPosition, SetConsoleMode, SetConsoleOutputCP,
    SetConsoleTextAttribute, WriteConsoleOutputA, WriteConsoleOutputAttribute,
    WriteConsoleOutputCharacterA, WriteConsoleOutputCharacterW, WriteConsoleOutputW,
};

fn rect() -> SMALL_RECT {
    SMALL_RECT {
        Left: 0,
        Top: 0,
        Right: 7,
        Bottom: 5,
    }
}

fn snapshot(output: HANDLE, report: &mut String) -> io::Result<()> {
    let mut data = [CHAR_INFO::default(); 48];
    let mut region = rect();
    // SAFETY: exact initialized 8x6 target, live synchronous value geometry.
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
    report.push_str("cells:");
    for value in data {
        // SAFETY: Unicode readback initialized the union's UTF-16 field.
        report.push_str(&format!(
            "{:04x}/{:04x},",
            unsafe { value.Char.UnicodeChar },
            value.Attributes
        ));
    }
    report.push('\n');
    Ok(())
}

fn dbcs_roundtrip(output: HANDLE, report: &mut String) -> io::Result<()> {
    if unsafe { SetConsoleOutputCP(936) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut cells = [CHAR_INFO::default(); 2];
    let mut region = SMALL_RECT {
        Left: 0,
        Top: 2,
        Right: 1,
        Bottom: 2,
    };
    // SAFETY: bounded initialized two-cell rectangle; code page is explicit.
    if unsafe {
        ReadConsoleOutputA(
            output,
            cells.as_mut_ptr(),
            COORD { X: 2, Y: 1 },
            COORD { X: 0, Y: 0 },
            &mut region,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    report.push_str("dbcs-read:");
    for cell in cells {
        report.push_str(&format!(
            "{:02x}/{:04x},",
            unsafe { cell.Char.AsciiChar as u8 },
            cell.Attributes
        ));
    }
    report.push('\n');
    region = SMALL_RECT {
        Left: 0,
        Top: 3,
        Right: 1,
        Bottom: 3,
    };
    if unsafe {
        WriteConsoleOutputA(
            output,
            cells.as_ptr(),
            COORD { X: 2, Y: 1 },
            COORD { X: 0, Y: 0 },
            &mut region,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    if unsafe {
        ReadConsoleOutputW(
            output,
            cells.as_mut_ptr(),
            COORD { X: 2, Y: 1 },
            COORD { X: 0, Y: 0 },
            &mut region,
        )
    } == 0
    {
        return Err(io::Error::last_os_error());
    }
    report.push_str("dbcs-written:");
    for cell in cells {
        report.push_str(&format!(
            "{:04x}/{:04x},",
            unsafe { cell.Char.UnicodeChar },
            cell.Attributes
        ));
    }
    report.push('\n');
    Ok(())
}

fn write(
    output: HANDLE,
    label: &str,
    kind: u32,
    data: &[u16],
    x: i16,
    y: i16,
    report: &mut String,
) {
    let mut count = 0xcccc_cccc;
    let coord = COORD { X: x, Y: y };
    let narrow: Vec<u8> = data.iter().map(|unit| *unit as u8).collect();
    // SAFETY: bounded initialized inputs, count and value coordinates; only
    // the private test console is reachable through this owned handle.
    let result = unsafe {
        match kind {
            1 => WriteConsoleOutputCharacterA(
                output,
                narrow.as_ptr(),
                narrow.len() as u32,
                coord,
                &mut count,
            ),
            3 => WriteConsoleOutputAttribute(
                output,
                data.as_ptr(),
                data.len() as u32,
                coord,
                &mut count,
            ),
            _ => WriteConsoleOutputCharacterW(
                output,
                data.as_ptr(),
                data.len() as u32,
                coord,
                &mut count,
            ),
        }
    };
    let error = if result == 0 {
        unsafe { GetLastError() }
    } else {
        0
    };
    report.push_str(&format!("{label}:{result},{error},{count}\n"));
}

pub(super) fn run(path: &Path) -> io::Result<()> {
    // SAFETY: SDK changes apply only to the controlled child's console.
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(output, 15) } == 0
        || unsafe { SetConsoleOutputCP(437) } == 0
        || unsafe { SetConsoleCursorPosition(output, COORD { X: 5, Y: 5 }) } == 0
        || unsafe { SetConsoleTextAttribute(output, 0x2e) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let data: Vec<_> = (0..48_u16)
        .map(|i| {
            let mut value = CHAR_INFO {
                Attributes: 0x10 + i % 8,
                ..CHAR_INFO::default()
            };
            value.Char.UnicodeChar = 0x41 + i;
            value
        })
        .collect();
    let mut region = rect();
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
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    bytes(output, b"\x1b[3")?;
    write(
        output,
        "cross",
        2,
        &[0x78, 0x79, 0x7a, 0x71],
        6,
        0,
        &mut report,
    );
    write(
        output,
        "styles",
        3,
        &[0x4f, 0x2e, 0xc031, 0x17, 0x60],
        7,
        0,
        &mut report,
    );
    write(output, "wide", 2, &[0x4e2d, 0x51], 7, 1, &mut report);
    write(output, "oem", 1, &[0xb3, 0xb4, 0x5a], 2, 3, &mut report);
    if unsafe { SetConsoleOutputCP(65001) } == 0 {
        return Err(io::Error::last_os_error());
    }
    write(
        output,
        "utf8",
        1,
        &[0xe4, 0xb8, 0xad, 0x52],
        5,
        3,
        &mut report,
    );
    write(
        output,
        "utf8-corner",
        1,
        &[0xe4, 0xb8, 0xad],
        7,
        5,
        &mut report,
    );
    write(
        output,
        "utf8-clipped",
        1,
        &[0xe4, 0xb8, 0xad, 0x52, 0x5a],
        5,
        5,
        &mut report,
    );
    write(
        output,
        "literal",
        2,
        &[
            0x1b, 0x5b, 0x33, 0x31, 0x6d, 13, 10, 0, 9, 7, 8, 0x301, 0xfffd,
        ],
        0,
        4,
        &mut report,
    );
    write(output, "end", 2, &[0x21, 0x3f], 7, 5, &mut report);
    for (label, x, y) in [("negative", -1, 0), ("right", 8, 0), ("below", 0, 6)] {
        write(output, label, 2, &[0x58], x, y, &mut report);
    }
    write(output, "empty", 2, &[], -1, -1, &mut report);
    for (label, access) in [("readonly", 0x8000_0000), ("writeonly", 0x4000_0000)] {
        let handle = super::console_file::open("CONOUT$", access)?;
        write(
            handle.as_raw_handle().cast(),
            &format!("{label}-text"),
            2,
            &[0x41],
            0,
            0,
            &mut report,
        );
        write(
            handle.as_raw_handle().cast(),
            &format!("{label}-style"),
            3,
            &[0x10],
            0,
            0,
            &mut report,
        );
    }
    snapshot(output, &mut report)?;
    dbcs_roundtrip(output, &mut report)?;
    position(output, "preserved", &mut report)?;
    bytes(output, b"1mZ")?;
    position(output, "resumed", &mut report)?;
    std::fs::write(path, report)?;
    bytes(output, b"\x1b]2;indexed-end:\x07")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !path.with_extension("opened").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}
