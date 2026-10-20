//! Finite native TUI producer; raw API payload records are separate from PTY output.
use super::mixed_output::bytes;
use super::queued_reads::{check, publish};
use std::{
    fs::File,
    io::{self, Write},
    path::Path,
};
use windows_sys::Win32::{Foundation::HANDLE, Storage::FileSystem::ReadFile, System::Console::*};

fn info(output: HANDLE) -> io::Result<CONSOLE_SCREEN_BUFFER_INFO> {
    let mut value = CONSOLE_SCREEN_BUFFER_INFO::default();
    check(unsafe { GetConsoleScreenBufferInfo(output, &mut value) })?;
    Ok(value)
}
fn emit(output: HANDLE, record: &mut File, value: &str, wide: bool) -> io::Result<()> {
    let units: Vec<u16> = value.encode_utf16().collect();
    let payload = if wide {
        units
            .iter()
            .flat_map(|u| u.to_le_bytes())
            .collect::<Vec<_>>()
    } else {
        value.as_bytes().to_vec()
    };
    record.write_all(&[if wide { b'W' } else { b'F' }])?;
    record.write_all(&(payload.len() as u32).to_le_bytes())?;
    record.write_all(&payload)?;
    if wide {
        let mut count = 0;
        check(unsafe {
            WriteConsoleW(
                output,
                units.as_ptr().cast(),
                units.len() as u32,
                &mut count,
                std::ptr::null(),
            )
        })?;
        if count as usize != units.len() {
            return Err(io::Error::other("short TUI wide write"));
        }
    } else {
        bytes(output, &payload)?;
    }
    Ok(())
}
fn split(output: HANDLE, record: &mut File, value: &str) -> io::Result<()> {
    let mut offset = 0;
    for length in [1, 2, 7, 3, 13].into_iter().cycle() {
        let end = (offset + length).min(value.len());
        if end == offset {
            break;
        }
        let payload = &value.as_bytes()[offset..end];
        record.write_all(b"F")?;
        record.write_all(&(payload.len() as u32).to_le_bytes())?;
        record.write_all(payload)?;
        bytes(output, payload)?;
        offset = end;
    }
    Ok(())
}
fn snapshot(output: HANDLE, phase: usize, rows: u16, cols: u16) -> io::Result<String> {
    let v = info(output)?;
    let mut report = format!(
        "phase={phase};size={},{};view={},{},{},{};cursor={},{}\n",
        v.dwSize.X,
        v.dwSize.Y,
        v.srWindow.Left,
        v.srWindow.Top,
        v.srWindow.Right,
        v.srWindow.Bottom,
        v.dwCursorPosition.Y,
        v.dwCursorPosition.X
    );
    if v.srWindow.Right - v.srWindow.Left + 1 != cols as i16
        || v.srWindow.Bottom - v.srWindow.Top + 1 != rows as i16
    {
        return Err(io::Error::other(format!(
            "TUI geometry did not commit: {report}"
        )));
    }
    for row in 0..rows {
        let mut units = [0u16; 10];
        let mut count = 0;
        check(unsafe {
            ReadConsoleOutputCharacterW(
                output,
                units.as_mut_ptr(),
                10,
                COORD {
                    X: 0,
                    Y: row as i16,
                },
                &mut count,
            )
        })?;
        if count != 10 {
            return Err(io::Error::other("short TUI native readback"));
        }
        report.push_str(&format!("row{row}={units:?}\n"));
    }
    Ok(report)
}
fn frame(phase: usize, n: usize, rows: u16, cols: u16, combining: bool) -> String {
    let mut text = String::from("\x1b[?7l\x1b[?25l");
    let accent = if combining { "\u{301}" } else { "" };
    for row in 0..rows {
        text.push_str(&format!(
            "\x1b[{};1H\x1b[2K\x1b[38;2;100;150;200mP{phase}F{n:03}R{row:02}: λ中😀e{accent}",
            row + 1
        ));
        if n.is_multiple_of(2) {
            text.push_str(&"X".repeat(cols as usize - 20));
        }
    }
    text.push_str(&format!("\x1b[0m\x1b[?25h\x1b[{rows};{cols}H"));
    text
}
fn run_impl(kind: &str, path: &Path) -> io::Result<()> {
    let (kind, combining) = kind
        .strip_suffix("-plain")
        .map_or((kind, true), |kind| (kind, false));
    let (kind, resize) = kind.split_once('-').ok_or(io::ErrorKind::InvalidInput)?;
    if !matches!(kind, "burst" | "mixed") || !matches!(resize, "resize" | "stable") {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let resize = resize == "resize";
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    check(unsafe { SetConsoleMode(output, 7) })?;
    check(unsafe { SetConsoleMode(input, 0) })?;
    check(unsafe { SetConsoleOutputCP(65001) })?;
    check(unsafe { SetConsoleCP(65001) })?;
    for phase in 0..5 {
        let (rows, cols) = if resize && phase >= 2 {
            if phase == 2 { (12, 56) } else { (6, 32) }
        } else {
            (8, 40)
        };
        let mut record = File::create(path.with_extension(format!("stage{phase}")))?;
        if phase == 0 {
            emit(
                output,
                &mut record,
                "\x1b[?1049l\x1b[0m\x1b[?7l\x1b[2J\x1b[HMAIN-RESTORE\x1b[2;3H",
                false,
            )?;
        } else if phase == 4 {
            emit(output, &mut record, "\x1b[?1049l", false)?;
        } else {
            if phase == 1 {
                emit(output, &mut record, "\x1b[?1049h", false)?;
            }
            if kind == "burst" {
                let mut value = String::new();
                for n in 0..128 {
                    value.push_str(&frame(phase, n, rows, cols, combining));
                }
                emit(output, &mut record, &value, false)?;
            } else {
                for n in 0..32 {
                    let value = frame(phase, n, rows, cols, combining);
                    if n.is_multiple_of(2) {
                        split(output, &mut record, &value)?;
                    } else {
                        emit(output, &mut record, &value, true)?;
                    }
                }
            }
        }
        record.flush()?;
        drop(record);
        publish(path, &snapshot(output, phase, rows, cols)?)?;
        bytes(output, format!("\x1b]2;tui-redraw-{phase}\x07").as_bytes())?;
        let mut command = [0u8; 1];
        let mut count = 0;
        check(unsafe {
            ReadFile(
                input,
                command.as_mut_ptr(),
                1,
                &mut count,
                std::ptr::null_mut(),
            )
        })?;
        if count != 1 || command[0] != if phase == 4 { b'q' } else { b'n' } {
            return Err(io::Error::other("TUI phase input mismatch"));
        }
    }
    std::process::exit(29)
}
pub(super) fn run(kind: &str, path: &Path) -> io::Result<()> {
    let result = run_impl(kind, path);
    if let Err(error) = &result {
        publish(path, &format!("TUI failure: {error:?}"))?;
    }
    result
}
