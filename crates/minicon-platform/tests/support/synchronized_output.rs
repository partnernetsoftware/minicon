//! Independent SDK producer: never decides when the consumer should present.
use super::queued_reads::{check, publish};
use super::tui_redraw::{emit, snapshot, split};
use std::{fs::File, io, path::Path};
use windows_sys::Win32::{Foundation::HANDLE, Storage::FileSystem::ReadFile, System::Console::*};

fn read(input: HANDLE, expected: &[u8], wide: bool) -> io::Result<Vec<u16>> {
    let mut received = Vec::with_capacity(expected.len());
    for expected in expected {
        let mut count = 0;
        let mut byte = 0u8;
        let mut unit = 0u16;
        if wide {
            check(unsafe {
                ReadConsoleW(
                    input,
                    (&mut unit as *mut u16).cast(),
                    1,
                    &mut count,
                    std::ptr::null(),
                )
            })?;
        } else {
            check(unsafe { ReadFile(input, &mut byte, 1, &mut count, std::ptr::null_mut()) })?;
            unit = u16::from(byte);
        }
        if count != 1 || unit != u16::from(*expected) {
            return Err(io::Error::other(format!(
                "synchronized native read: count={count};unit={unit};expected={expected}"
            )));
        }
        received.push(unit);
    }
    Ok(received)
}

fn output(output: HANDLE, record: &mut File, value: &str, wide: bool) -> io::Result<()> {
    if wide {
        emit(output, record, value, true)
    } else {
        split(output, record, value)
    }
}

fn checkpoint(
    output_handle: HANDLE,
    record: &mut File,
    path: &Path,
    phase: usize,
    resized: bool,
    wide: bool,
    window: bool,
) -> io::Result<()> {
    let (rows, cols) = if window {
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        check(unsafe { GetConsoleScreenBufferInfo(output_handle, &mut info) })?;
        (
            (info.srWindow.Bottom - info.srWindow.Top + 1) as u16,
            (info.srWindow.Right - info.srWindow.Left + 1) as u16,
        )
    } else if resized {
        (12, 56)
    } else {
        (8, 40)
    };
    let native = snapshot(output_handle, phase, rows, cols)?;
    publish(&path.with_extension(format!("native{phase}")), &native)?;
    output(
        output_handle,
        record,
        &format!("\x1b]2;native-sync-{phase}\x07"),
        wide,
    )
}

pub(super) fn run(kind: &str, path: &Path) -> io::Result<()> {
    let result = run_impl(kind, path);
    if let Err(error) = &result {
        publish(&path.with_extension("failure"), &format!("{error:?}"))?;
    }
    result
}
fn run_impl(kind: &str, path: &Path) -> io::Result<()> {
    let (api, kind) = kind.split_once('-').ok_or(io::ErrorKind::InvalidInput)?;
    let (window, kind) = kind
        .strip_prefix("window-")
        .map_or((false, kind), |kind| (true, kind));
    if !matches!(api, "file" | "wide")
        || !matches!(kind, "end" | "alt" | "timeout" | "quota" | "resize" | "eof")
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let wide = api == "wide";
    if window {
        publish(&path.with_extension("pid"), &std::process::id().to_string())?;
    }
    let output_handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    check(unsafe { SetConsoleMode(output_handle, 7) })?;
    check(unsafe { SetConsoleMode(input, 0) })?;
    check(unsafe { SetConsoleOutputCP(65001) })?;
    check(unsafe { SetConsoleCP(65001) })?;
    let mut record = File::create(path.with_extension("source0"))?;
    output(
        output_handle,
        &mut record,
        "\x1b[?1049l\x1b[0m\x1b[?7l\x1b[2J\x1b[HMAIN-BASE\x1b[2;3H",
        wide,
    )?;
    checkpoint(output_handle, &mut record, path, 0, false, wide, window)?;
    drop(record);
    let _ = read(input, b"a", wide)?;

    let mut record = File::create(path.with_extension("source1"))?;
    // Begin is fragmented across native writes even in the wide API journey.
    for part in ["\x1b[?20", "26h"] {
        output(output_handle, &mut record, part, wide)?;
    }
    if kind == "alt" {
        output(output_handle, &mut record, "\x1b[?1049h", wide)?;
    }
    output(
        output_handle,
        &mut record,
        "\x1b[?7l\x1b[?25l\x1b[2J\x1b[HSYNC-FRAME: λ中😀\x1b[2;1H\x1b[38;2;12;34;56mROW-TWO\x1b[0m\x1b[3;6H",
        wide,
    )?;
    output(output_handle, &mut record, "\x1b[6n\x1b[?2026$p", wide)?;
    // Successful reads prove the replies traversed the owned native API.
    let received = read(input, b"\x1b[3;6R\x1b[?2026;0$y", wide)?;
    publish(
        &path.with_extension("replies"),
        &format!("api={api};units={received:?}\n"),
    )?;
    checkpoint(output_handle, &mut record, path, 1, false, wide, window)?;
    drop(record);

    let _ = read(input, if kind == "eof" { b"q" } else { b"e" }, wide)?;
    if kind != "eof" {
        let mut record = File::create(path.with_extension("source2"))?;
        if matches!(kind, "end" | "alt") {
            for part in ["\x1b[?202", "6l"] {
                output(output_handle, &mut record, part, wide)?;
            }
        } else if kind == "quota" {
            // Semantically idempotent output exhausts the consumer's quota.
            output(output_handle, &mut record, &"\x1b[?7l".repeat(900), wide)?;
        }
        checkpoint(
            output_handle,
            &mut record,
            path,
            2,
            kind == "resize",
            wide,
            window,
        )?;
        drop(record);
        let _ = read(input, b"q", wide)?;
    }
    let mut record = File::create(path.with_extension("source3"))?;
    if kind == "alt" {
        output(output_handle, &mut record, "\x1b[?1049l\x1b[?25h", wide)?;
    }
    checkpoint(
        output_handle,
        &mut record,
        path,
        3,
        kind == "resize",
        wide,
        window,
    )?;
    std::process::exit(35)
}
