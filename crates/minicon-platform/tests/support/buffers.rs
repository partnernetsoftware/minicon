//! Independent SDK caller for native buffer identity, publication and lifetime.
use super::mixed_output::bytes;
use std::{
    fs::File,
    io,
    os::windows::io::{AsRawHandle, FromRawHandle},
    path::Path,
};
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, GetLastError, HANDLE, INVALID_HANDLE_VALUE,
};
use windows_sys::Win32::System::{Console::*, Threading::GetCurrentProcess};

pub(super) fn checked(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(super) fn create(access: u32, share: u32) -> io::Result<File> {
    // SAFETY: a private text buffer; no borrowed pointers or inheritable handle.
    let handle =
        unsafe { CreateConsoleScreenBuffer(access, share, std::ptr::null(), 1, std::ptr::null()) };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(unsafe { File::from_raw_handle(handle) })
}

fn duplicate(handle: HANDLE) -> io::Result<File> {
    let mut result = std::ptr::null_mut();
    let process = unsafe { GetCurrentProcess() };
    checked(unsafe {
        DuplicateHandle(
            process,
            handle,
            process,
            &mut result,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        )
    })?;
    Ok(unsafe { File::from_raw_handle(result) })
}

pub(super) fn query(handle: HANDLE, label: &str, report: &mut String) -> io::Result<()> {
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    let mut cursor = CONSOLE_CURSOR_INFO::default();
    let mut mode = 0;
    checked(unsafe { GetConsoleScreenBufferInfo(handle, &mut info) })?;
    checked(unsafe { GetConsoleCursorInfo(handle, &mut cursor) })?;
    checked(unsafe { GetConsoleMode(handle, &mut mode) })?;
    report.push_str(&format!(
        "{label}:size={},{};view={},{},{},{};pos={},{};attr={:04x};mode={mode};cursor={},{}\n",
        info.dwSize.X,
        info.dwSize.Y,
        info.srWindow.Left,
        info.srWindow.Top,
        info.srWindow.Right,
        info.srWindow.Bottom,
        info.dwCursorPosition.X,
        info.dwCursorPosition.Y,
        info.wAttributes,
        cursor.dwSize,
        cursor.bVisible
    ));
    for row in 0..3 {
        let mut data = [0u16; 16];
        let mut count = 0;
        checked(unsafe {
            ReadConsoleOutputCharacterW(
                handle,
                data.as_mut_ptr(),
                16,
                COORD { X: 0, Y: row },
                &mut count,
            )
        })?;
        report.push_str(&format!(
            "{label}:row{row}={:?}\n",
            String::from_utf16_lossy(&data[..count as usize]).trim_end()
        ));
    }
    Ok(())
}

pub(super) fn position(handle: HANDLE, x: i16, y: i16) -> io::Result<()> {
    checked(unsafe { SetConsoleCursorPosition(handle, COORD { X: x, Y: y }) })
}

pub(super) fn stage(handle: HANDLE, path: &Path, n: usize, report: &str) -> io::Result<()> {
    std::fs::write(path, report)?;
    bytes(handle, format!("\x1b]2;buffers-stage-{n}:\x07").as_bytes())?;
    wait_stage(path, n)
}

pub(super) fn wait_stage(path: &Path, n: usize) -> io::Result<()> {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
    while !path.with_extension(format!("stage{n}")).exists() {
        if std::time::Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(std::time::Duration::from_millis(2));
    }
    Ok(())
}

pub(super) fn run(path: &Path) -> io::Result<()> {
    let base = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    checked(unsafe { SetConsoleMode(base, 7) })?;
    checked(unsafe { SetConsoleTextAttribute(base, 0x1e) })?;
    bytes(base, b"BASE")?;
    checked(unsafe { SetConsoleScreenBufferSize(base, COORD { X: 32, Y: 12 }) })?;
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    let first = create(0xc000_0000, 3)?;
    let first_handle = first.as_raw_handle();
    query(first_handle, "first-new", &mut report)?;
    checked(unsafe { SetConsoleMode(first_handle, 7) })?;
    checked(unsafe { SetConsoleTextAttribute(first_handle, 0x2d) })?;
    checked(unsafe {
        SetConsoleCursorInfo(
            first_handle,
            &CONSOLE_CURSOR_INFO {
                dwSize: 61,
                bVisible: 0,
            },
        )
    })?;
    bytes(first_handle, b"FIRST")?;
    let second = create(0xc000_0000, 3)?;
    let second_handle = second.as_raw_handle();
    query(second_handle, "second-new", &mut report)?;
    checked(unsafe { SetConsoleMode(second_handle, 7) })?;
    bytes(second_handle, b"SECOND")?;
    query(base, "base-inactive-writes", &mut report)?;
    query(first_handle, "first-written", &mut report)?;
    stage(base, path, 0, &report)?;
    checked(unsafe { SetConsoleActiveScreenBuffer(first_handle) })?;
    position(base, 0, 1)?;
    bytes(base, b"HIDDEN")?;
    let reopened = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")?;
    query(reopened.as_raw_handle(), "reopened-first", &mut report)?;
    let dupe = duplicate(first_handle)?;
    drop(first);
    position(dupe.as_raw_handle(), 0, 2)?;
    bytes(dupe.as_raw_handle(), b"DUP")?;
    query(dupe.as_raw_handle(), "duplicate-first", &mut report)?;
    drop(reopened);
    stage(dupe.as_raw_handle(), path, 1, &report)?;
    checked(unsafe { SetConsoleActiveScreenBuffer(second_handle) })?;
    drop(dupe);
    stage(second_handle, path, 2, &report)?;
    checked(unsafe { SetConsoleActiveScreenBuffer(base) })?;
    position(second_handle, 0, 1)?;
    bytes(second_handle, b"MORE")?;
    query(base, "base-restored", &mut report)?;
    stage(base, path, 3, &report)?;
    checked(unsafe { SetConsoleActiveScreenBuffer(second_handle) })?;
    stage(second_handle, path, 4, &report)?;
    drop(second);
    let fallback = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")?;
    query(fallback.as_raw_handle(), "active-close", &mut report)?;
    drop(fallback);
    for _ in 0..24 {
        drop(create(0xc000_0000, 3)?);
    }
    report.push_str("recycled=24\n");
    let readonly = create(0x8000_0000, 0)?;
    let ok = unsafe { SetConsoleActiveScreenBuffer(readonly.as_raw_handle()) };
    report.push_str(&format!(
        "readonly-active:{ok},{}\n",
        if ok == 0 {
            unsafe { GetLastError() }
        } else {
            0
        }
    ));
    // An exclusive handle to another inactive buffer must not constrain the
    // active buffer's share domain (keep that handle alive during the open).
    let isolated = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")?;
    query(isolated.as_raw_handle(), "share-isolated", &mut report)?;
    drop(isolated);
    drop(readonly);
    query(base, "final-base", &mut report)?;
    stage(base, path, 5, &report)?;
    Ok(())
}

pub(super) fn pressure(path: &Path) -> io::Result<()> {
    let base = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    checked(unsafe { SetConsoleMode(base, 7) })?;
    bytes(base, b"BASE")?;
    let active = create(0xc000_0000, 3)?;
    bytes(active.as_raw_handle(), b"ALT")?;
    checked(unsafe { SetConsoleActiveScreenBuffer(active.as_raw_handle()) })?;
    for n in 0..8 {
        bytes(
            active.as_raw_handle(),
            format!("\x1b]2;{n};{}\x07", "x".repeat(8192)).as_bytes(),
        )?;
    }
    // Native CloseHandle may return before the server's finalizer. Observe
    // actual CONOUT binding/readback rather than treating that return as an
    // authoritative server-side close or activation receipt.
    use std::os::windows::io::IntoRawHandle;
    std::fs::write(path.with_extension("go"), b"before-close")?;
    checked(unsafe { windows_sys::Win32::Foundation::CloseHandle(active.into_raw_handle()) })?;
    std::fs::write(path.with_extension("go"), b"closed")?;
    let observer = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")?;
    let mut receipt = String::new();
    std::fs::write(path.with_extension("go"), b"opened")?;
    query(observer.as_raw_handle(), "held", &mut receipt)?;
    let mut clients = [0; 4];
    let count = unsafe { GetConsoleProcessList(clients.as_mut_ptr(), clients.len() as u32) };
    if count != 1 || clients[0] != std::process::id() {
        return Err(io::Error::other("backpressured process-list query"));
    }
    receipt.push_str("held-clients=1\n");
    let mut cells = [CHAR_INFO::default(); 3];
    let mut region = SMALL_RECT {
        Left: 0,
        Top: 0,
        Right: 2,
        Bottom: 0,
    };
    checked(unsafe {
        ReadConsoleOutputW(
            observer.as_raw_handle(),
            cells.as_mut_ptr(),
            COORD { X: 3, Y: 1 },
            COORD { X: 0, Y: 0 },
            &mut region,
        )
    })?;
    let text = String::from_utf16_lossy(
        &cells
            .iter()
            .map(|c| unsafe { c.Char.UnicodeChar })
            .collect::<Vec<_>>(),
    );
    receipt.push_str(&format!("held-rect={text}\n"));
    std::fs::write(path.with_extension("go"), b"queried")?;
    drop(observer);
    super::queued_reads::publish(&path.with_extension("ready"), &receipt)?;
    bytes(base, b"\x1b]2;buffers-pressure-end:\x07")?;
    std::fs::write(path, b"closed-after-drain")
}
