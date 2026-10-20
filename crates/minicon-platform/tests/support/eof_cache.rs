//! Native cooked EOF/continuation client, independent of host implementation.
use super::mixed_output::bytes;
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::io::AsRawHandle,
    path::Path,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Storage::FileSystem::ReadFile,
    System::{Console::*, Threading::WaitForSingleObject},
};

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn open() -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open("CONIN$")
}
fn put(file: &File, units: &[u16]) -> io::Result<()> {
    let records: Vec<_> = units
        .iter()
        .map(|&u| {
            let mut r = INPUT_RECORD {
                EventType: KEY_EVENT as u16,
                ..INPUT_RECORD::default()
            };
            r.Event.KeyEvent.bKeyDown = 1;
            r.Event.KeyEvent.wRepeatCount = 1;
            r.Event.KeyEvent.uChar.UnicodeChar = u;
            r.Event.KeyEvent.wVirtualKeyCode = if u == 13 { 13 } else { 0 };
            r
        })
        .collect();
    let mut count = 0;
    check(unsafe {
        WriteConsoleInputW(
            file.as_raw_handle(),
            records.as_ptr(),
            records.len() as u32,
            &mut count,
        )
    })?;
    if count as usize != records.len() {
        return Err(io::Error::other("short SDK input write"));
    }
    Ok(())
}
fn read(file: &File, api: &str, capacity: u32, report: &mut String) -> io::Result<()> {
    let mut wide = [0xaaaa_u16; 64];
    let mut narrow = [0xaa_u8; 64];
    let mut count = 0;
    if capacity > 32 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let h = file.as_raw_handle();
    let ok = unsafe {
        match api {
            "wide" => ReadConsoleW(
                h,
                wide.as_mut_ptr().cast(),
                capacity,
                &mut count,
                std::ptr::null_mut(),
            ),
            "ansi" => ReadConsoleA(
                h,
                narrow.as_mut_ptr().cast(),
                capacity,
                &mut count,
                std::ptr::null_mut(),
            ),
            _ => ReadFile(
                h,
                narrow.as_mut_ptr(),
                capacity,
                &mut count,
                std::ptr::null_mut(),
            ),
        }
    };
    check(ok)?;
    if count > capacity {
        return Err(io::Error::other("SDK read exceeded capacity"));
    }
    let data: Vec<u16> = if api == "wide" {
        wide[..count as usize].to_vec()
    } else {
        narrow[..count as usize]
            .iter()
            .copied()
            .map(u16::from)
            .collect()
    };
    let sentinel = if api == "wide" {
        wide[count as usize] == 0xaaaa
    } else {
        narrow[count as usize] == 0xaa
    };
    if !sentinel {
        return Err(io::Error::other("SDK read overwrote tail"));
    }
    let mut queued = 0;
    check(unsafe { GetNumberOfConsoleInputEvents(h, &mut queued) })?;
    let wait = unsafe { WaitForSingleObject(h, 0) };
    report.push_str(&format!(
        "api={api};capacity={capacity};data={data:?};queued={queued};wait={wait}\n"
    ));
    bytes(
        unsafe { GetStdHandle(STD_OUTPUT_HANDLE) },
        b"\x1b]2;eof-cache-step:\x07",
    )?;
    Ok(())
}
pub(super) fn run(kind: &str, path: &Path) -> io::Result<()> {
    let file = open()?;
    check(unsafe { SetConsoleMode(file.as_raw_handle(), 3) })?;
    check(unsafe { SetConsoleCP(65001) })?;
    let mut report = format!(
        "build={}\nkind={kind}\n",
        super::output_modes::windows_build()?
    );
    put(
        &file,
        if kind == "middle" || kind.starts_with("mode") {
            &[65, 26, 66, 13]
        } else {
            &[26, 65, 13]
        },
    )?;
    read(
        &file,
        "file",
        if kind == "fill" { 2 } else { 1 },
        &mut report,
    )?;
    std::fs::write(path, &report)?;
    match kind {
        "short" | "fill" | "wide" | "ansi" => {
            read(
                &file,
                if kind == "wide" {
                    "wide"
                } else if kind == "ansi" {
                    "ansi"
                } else {
                    "file"
                },
                8,
                &mut report,
            )?;
        }
        "middle" => {
            read(&file, "file", 1, &mut report)?;
            read(&file, "file", 8, &mut report)?;
        }
        "duplicate" => {
            let duplicate = file.try_clone()?;
            read(&duplicate, "file", 8, &mut report)?;
        }
        "independent" => {
            let other = open()?;
            put(&other, &[67, 13])?;
            read(&other, "file", 8, &mut report)?;
            read(&file, "file", 8, &mut report)?;
        }
        "flush" => {
            check(unsafe { FlushConsoleInputBuffer(file.as_raw_handle()) })?;
            read(&file, "file", 8, &mut report)?;
        }
        "mode" | "mode-ansi" => {
            check(unsafe { SetConsoleMode(file.as_raw_handle(), 1) })?;
            read(&file, "file", 8, &mut report)?;
            check(unsafe { SetConsoleMode(file.as_raw_handle(), 0) })?;
            put(&file, &[26, 68])?;
            read(
                &file,
                if kind == "mode-ansi" { "ansi" } else { "file" },
                8,
                &mut report,
            )?;
            check(unsafe { SetConsoleMode(file.as_raw_handle(), 3) })?;
        }
        "close" => {
            drop(file);
            let reopened = open()?;
            put(&reopened, &[66, 13])?;
            read(&reopened, "file", 8, &mut report)?;
            return finish(path, report);
        }
        _ => return Err(io::ErrorKind::InvalidInput.into()),
    }
    put(&file, &[66, 13])?;
    read(&file, "file", 8, &mut report)?;
    finish(path, report)
}
fn finish(path: &Path, report: String) -> io::Result<()> {
    std::fs::write(path, report)?;
    bytes(
        unsafe { GetStdHandle(STD_OUTPUT_HANDLE) },
        b"\x1b]2;eof-cache-end:\x07",
    )?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while !path.with_extension("opened").exists() {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
