//! Independent SDK rectangle packing, clipping and cell-shape receipts.
use super::mixed_output::{bytes, position};
use std::os::windows::io::AsRawHandle;
use std::{io, path::Path};
use windows_sys::Win32::Foundation::{
    GENERIC_READ, GENERIC_WRITE, GetLastError, HANDLE, SetLastError,
};
use windows_sys::Win32::System::Console::{
    CHAR_INFO, CONSOLE_FONT_INFOEX, COORD, GetConsoleFontSize, GetCurrentConsoleFontEx,
    GetStdHandle, ReadConsoleOutputA, ReadConsoleOutputW, SMALL_RECT, STD_OUTPUT_HANDLE,
    SetConsoleCursorPosition, SetConsoleMode, SetConsoleOutputCP, SetConsoleTextAttribute,
    WriteConsoleOutputA, WriteConsoleOutputW,
};

fn cell(unit: u16, attribute: u16) -> CHAR_INFO {
    let mut value = CHAR_INFO {
        Attributes: attribute,
        ..CHAR_INFO::default()
    };
    value.Char.UnicodeChar = unit;
    value
}

fn region(value: SMALL_RECT) -> String {
    format!(
        "{},{},{},{}",
        value.Left, value.Top, value.Right, value.Bottom
    )
}

#[allow(clippy::too_many_arguments)]
fn write(
    output: HANDLE,
    source: &[CHAR_INFO],
    size: COORD,
    origin: COORD,
    mut rect: SMALL_RECT,
    ansi: bool,
    label: &str,
    receipt: &mut String,
) -> io::Result<SMALL_RECT> {
    // SAFETY: explicit allocated source dimensions, private console, live rect.
    let ok = unsafe {
        if ansi {
            WriteConsoleOutputA(output, source.as_ptr(), size, origin, &mut rect)
        } else {
            WriteConsoleOutputW(output, source.as_ptr(), size, origin, &mut rect)
        }
    };
    if ok == 0 {
        return Err(io::Error::other(format!(
            "{label}-write failed: {}; partial={receipt:?}",
            io::Error::last_os_error()
        )));
    }
    receipt.push_str(&format!("{label}-write:{}\n", region(rect)));
    Ok(rect)
}

fn read(
    output: HANDLE,
    size: COORD,
    origin: COORD,
    mut rect: SMALL_RECT,
    ansi: bool,
    label: &str,
    receipt: &mut String,
) -> io::Result<SMALL_RECT> {
    let mut target = vec![cell(0x21, 0x55); size.X as usize * size.Y as usize];
    // SAFETY: initialized target with exact positive dimensions and live rect.
    let ok = unsafe {
        if ansi {
            ReadConsoleOutputA(output, target.as_mut_ptr(), size, origin, &mut rect)
        } else {
            ReadConsoleOutputW(output, target.as_mut_ptr(), size, origin, &mut rect)
        }
    };
    if ok == 0 {
        return Err(io::Error::other(format!(
            "{label}-read failed: {}; partial={receipt:?}",
            io::Error::last_os_error()
        )));
    }
    receipt.push_str(&format!("{label}-read:{}:", region(rect)));
    for value in target {
        // SAFETY: interpretation follows the actual A/W SDK call above.
        let unit = unsafe {
            if ansi {
                u16::from(value.Char.AsciiChar as u8)
            } else {
                value.Char.UnicodeChar
            }
        };
        receipt.push_str(&format!("{unit:04x}/{:04x},", value.Attributes));
    }
    receipt.push('\n');
    Ok(rect)
}

fn rect(left: i16, top: i16, right: i16, bottom: i16) -> SMALL_RECT {
    SMALL_RECT {
        Left: left,
        Top: top,
        Right: right,
        Bottom: bottom,
    }
}

pub(super) fn run(report: &Path) -> io::Result<()> {
    // SAFETY: controlled private console and in-bounds initial cursor.
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if unsafe { SetConsoleMode(output, 15) } == 0
        || unsafe { SetConsoleOutputCP(437) } == 0
        || unsafe { SetConsoleCursorPosition(output, COORD { X: 7, Y: 6 }) } == 0
        || unsafe { SetConsoleTextAttribute(output, 0x2e) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    let mut receipt = format!("build={}\n", super::output_modes::windows_build()?);
    bytes(output, b"\x1b[3")?;
    let source: Vec<_> = (0..12).map(|i| cell(0x41 + i, i + 1)).collect();
    write(
        output,
        &source,
        COORD { X: 4, Y: 3 },
        COORD { X: 1, Y: 1 },
        rect(-1, -1, 2, 1),
        false,
        "clip",
        &mut receipt,
    )?;
    read(
        output,
        COORD { X: 5, Y: 3 },
        COORD { X: 1, Y: 1 },
        rect(-1, -1, 2, 1),
        false,
        "clip",
        &mut receipt,
    )?;
    let source: Vec<_> = (0..6).map(|i| cell(0x61 + i, 0x1e + i)).collect();
    write(
        output,
        &source,
        COORD { X: 3, Y: 2 },
        COORD { X: 0, Y: 0 },
        rect(2, 2, 4, 3),
        false,
        "block",
        &mut receipt,
    )?;
    read(
        output,
        COORD { X: 3, Y: 2 },
        COORD { X: 0, Y: 0 },
        rect(2, 2, 4, 3),
        false,
        "block",
        &mut receipt,
    )?;
    let source = [cell(0x41, 0x1e), cell(0xb3, 0x2d), cell(0x42, 0x4f)];
    write(
        output,
        &source,
        COORD { X: 3, Y: 1 },
        COORD { X: 0, Y: 0 },
        rect(0, 4, 2, 4),
        true,
        "ansi",
        &mut receipt,
    )?;
    read(
        output,
        COORD { X: 3, Y: 1 },
        COORD { X: 0, Y: 0 },
        rect(0, 4, 2, 4),
        true,
        "ansi",
        &mut receipt,
    )?;
    read(
        output,
        COORD { X: 3, Y: 1 },
        COORD { X: 0, Y: 0 },
        rect(0, 4, 2, 4),
        false,
        "ansi-wide",
        &mut receipt,
    )?;
    let source = [
        cell(0x4e2d, 0x107),
        cell(0x4e2d, 0x207),
        cell(0x51, 7),
        cell(0x52, 7),
    ];
    write(
        output,
        &source,
        COORD { X: 4, Y: 1 },
        COORD { X: 0, Y: 0 },
        rect(0, 5, 3, 5),
        false,
        "wide",
        &mut receipt,
    )?;
    read(
        output,
        COORD { X: 4, Y: 1 },
        COORD { X: 0, Y: 0 },
        rect(0, 5, 3, 5),
        false,
        "wide",
        &mut receipt,
    )?;
    read(
        output,
        COORD { X: 1, Y: 1 },
        COORD { X: 0, Y: 0 },
        rect(1, 5, 1, 5),
        false,
        "tail",
        &mut receipt,
    )?;
    read(
        output,
        COORD { X: 1, Y: 1 },
        COORD { X: 0, Y: 0 },
        rect(0, 5, 0, 5),
        false,
        "lead",
        &mut receipt,
    )?;
    let mut outside_rect = rect(20, 20, 21, 21);
    let mut outside_cells = [cell(0x21, 0x55); 4];
    // SAFETY: initialized bounded data; intentionally disjoint rectangle.
    unsafe { SetLastError(0x0bad_f00d) };
    let outside_read = unsafe {
        ReadConsoleOutputW(
            output,
            outside_cells.as_mut_ptr(),
            COORD { X: 2, Y: 2 },
            COORD { X: 0, Y: 0 },
            &mut outside_rect,
        )
    };
    let outside_error = if outside_read == 0 {
        unsafe { GetLastError() }
    } else {
        0
    };
    let unchanged = outside_cells
        .iter()
        .all(|value| unsafe { value.Char.UnicodeChar == 0x21 } && value.Attributes == 0x55);
    receipt.push_str(&format!(
        "outside-read:{outside_read},{outside_error},{};{unchanged}\n",
        region(outside_rect)
    ));
    outside_rect = rect(20, 20, 21, 21);
    unsafe { SetLastError(0x0bad_f00d) };
    let outside_write = unsafe {
        WriteConsoleOutputW(
            output,
            outside_cells.as_ptr(),
            COORD { X: 2, Y: 2 },
            COORD { X: 0, Y: 0 },
            &mut outside_rect,
        )
    };
    let outside_error = if outside_write == 0 {
        unsafe { GetLastError() }
    } else {
        0
    };
    receipt.push_str(&format!(
        "outside-write:{outside_write},{outside_error},{}\n",
        region(outside_rect)
    ));
    write(
        output,
        &[cell(0x21, 5), cell(0x3f, 6)],
        COORD { X: 2, Y: 1 },
        COORD { X: 0, Y: 0 },
        rect(15, 7, 16, 7),
        false,
        "corner",
        &mut receipt,
    )?;
    read(
        output,
        COORD { X: 1, Y: 1 },
        COORD { X: 0, Y: 0 },
        rect(15, 7, 15, 7),
        false,
        "corner",
        &mut receipt,
    )?;
    let read_only = super::console_file::open("CONOUT$", GENERIC_READ)?;
    let write_only = super::console_file::open("CONOUT$", GENERIC_WRITE)?;
    let mut denied_region = rect(0, 0, 0, 0);
    let source = cell(0x58, 7);
    // SAFETY: initialized single-cell source; the handle deliberately lacks WRITE.
    unsafe { SetLastError(0x0bad_f00d) };
    let written = unsafe {
        WriteConsoleOutputW(
            read_only.as_raw_handle().cast(),
            &source,
            COORD { X: 1, Y: 1 },
            COORD { X: 0, Y: 0 },
            &mut denied_region,
        )
    };
    let write_error = if written == 0 {
        unsafe { GetLastError() }
    } else {
        0
    };
    let mut target = cell(0x21, 0x55);
    denied_region = rect(0, 0, 0, 0);
    // SAFETY: initialized single-cell target; the handle deliberately lacks READ.
    unsafe { SetLastError(0x0bad_f00d) };
    let read = unsafe {
        ReadConsoleOutputW(
            write_only.as_raw_handle().cast(),
            &mut target,
            COORD { X: 1, Y: 1 },
            COORD { X: 0, Y: 0 },
            &mut denied_region,
        )
    };
    let read_error = if read == 0 {
        unsafe { GetLastError() }
    } else {
        0
    };
    receipt.push_str(&format!(
        "access:{written},{write_error},{read},{read_error}\n"
    ));
    let mut font = CONSOLE_FONT_INFOEX {
        cbSize: std::mem::size_of::<CONSOLE_FONT_INFOEX>() as u32,
        ..CONSOLE_FONT_INFOEX::default()
    };
    // SAFETY: initialized SDK-sized font record and private output handle.
    if unsafe { GetCurrentConsoleFontEx(output, 0, &mut font) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let font_size = unsafe { GetConsoleFontSize(output, font.nFont) };
    let name_end = font
        .FaceName
        .iter()
        .position(|unit| *unit == 0)
        .unwrap_or(font.FaceName.len());
    let name = String::from_utf16_lossy(&font.FaceName[..name_end]);
    receipt.push_str(&format!(
        "font-consistent:{}\nfont:{},{},{},{:x},{},{}\n",
        font_size.X >= 0
            && font_size.Y > 0
            && font_size.X == font.dwFontSize.X
            && font_size.Y == font.dwFontSize.Y,
        font.nFont,
        font_size.X,
        font_size.Y,
        font.FontFamily,
        font.FontWeight,
        name
    ));
    unsafe { SetLastError(0x0bad_f00d) };
    let invalid_font = unsafe { GetConsoleFontSize(output, u32::MAX) };
    let font_error = unsafe { GetLastError() };
    receipt.push_str(&format!(
        "font-invalid:{},{},{font_error}\n",
        invalid_font.X, invalid_font.Y
    ));
    position(output, "preserved", &mut receipt)?;
    bytes(output, b"1mZ")?;
    position(output, "resumed", &mut receipt)?;
    std::fs::write(report, receipt)?;
    bytes(output, b"\x1b]2;rect-end:\x07")?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !report.with_extension("opened").exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}
