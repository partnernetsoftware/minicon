//! Independent SDK caller shared by the direct-driver and platform PTY courts.
use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{HANDLE, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Console::*;

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn write(handle: HANDLE, text: &str) -> io::Result<()> {
    let units: Vec<_> = text.encode_utf16().collect();
    let mut count = 0;
    // SAFETY: live console handle, bounded UTF-16 input and writable count.
    check(unsafe {
        WriteConsoleW(
            handle,
            units.as_ptr().cast(),
            units.len() as u32,
            &mut count,
            std::ptr::null(),
        )
    })?;
    if count as usize != units.len() {
        return Err(io::Error::other("short SDK write"));
    }
    Ok(())
}
fn snapshot(handle: HANDLE) -> io::Result<(String, (i16, i16))> {
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    let mut text = [0_u16; 10];
    let mut count = 0;
    // SAFETY: initialized SDK structures and ten writable UTF-16 units.
    unsafe {
        check(GetConsoleScreenBufferInfo(handle, &mut info))?;
        check(ReadConsoleOutputCharacterW(
            handle,
            text.as_mut_ptr(),
            text.len() as u32,
            COORD { X: 0, Y: 0 },
            &mut count,
        ))?;
    }
    if count != 10 {
        return Err(io::Error::other("short SDK readback"));
    }
    Ok((
        String::from_utf16_lossy(&text)
            .trim_end_matches(' ')
            .to_owned(),
        (info.dwCursorPosition.Y, info.dwCursorPosition.X),
    ))
}
fn keys(input: HANDLE, edit: bool) -> io::Result<()> {
    let events: &[(u16, u16)] = if edit {
        &[(0x25, 0), (0, 88), (13, 13)]
    } else {
        &[(0, 97), (0, 98)]
    };
    let records: Vec<_> = events
        .iter()
        .map(|&(vk, ch)| {
            let mut record = INPUT_RECORD {
                EventType: KEY_EVENT as u16,
                ..INPUT_RECORD::default()
            };
            record.Event.KeyEvent.bKeyDown = 1;
            record.Event.KeyEvent.wRepeatCount = 1;
            record.Event.KeyEvent.wVirtualKeyCode = vk;
            record.Event.KeyEvent.uChar.UnicodeChar = ch;
            record
        })
        .collect();
    let mut count = 0;
    // SAFETY: private console input and initialized records.
    check(unsafe {
        WriteConsoleInputW(input, records.as_ptr(), records.len() as u32, &mut count)
    })?;
    if count as usize != records.len() {
        return Err(io::Error::other("short SDK input"));
    }
    Ok(())
}
fn wait(deadline: Instant, condition: impl Fn() -> io::Result<bool>) -> io::Result<()> {
    while !condition()? {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

pub(crate) fn run(kind: &str, path: &Path, controlled: bool) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(12);
    // SAFETY: process-owned standard console handles and a fresh output object.
    let (input, base, created) = unsafe {
        (
            GetStdHandle(STD_INPUT_HANDLE),
            GetStdHandle(STD_OUTPUT_HANDLE),
            CreateConsoleScreenBuffer(
                0xc000_0000,
                3,
                std::ptr::null(),
                CONSOLE_TEXTMODE_BUFFER,
                std::ptr::null(),
            ),
        )
    };
    if created == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    // SAFETY: successful creation transfers unique ownership.
    let mut buffer = Some(unsafe { OwnedHandle::from_raw_handle(created) });
    unsafe {
        check(SetConsoleMode(input, 0xa7))?;
        check(SetConsoleMode(base, 15))?;
        check(SetConsoleMode(created, 15))?;
    }
    write(base, "\x1b[2J\x1b[HB>")?;
    unsafe {
        check(SetConsoleActiveScreenBuffer(created))?;
    }
    write(created, "\x1b[2J\x1b[HA>")?;
    let initial = kind == "initial";
    if initial {
        write(created, "ab")?;
    }
    let input_token = input as usize;
    let reader = std::thread::spawn(move || -> io::Result<Vec<u16>> {
        let mut text = [0_u16; 32];
        let mut count = 0;
        let mut control = CONSOLE_READCONSOLE_CONTROL {
            nLength: std::mem::size_of::<CONSOLE_READCONSOLE_CONTROL>() as u32,
            nInitialChars: if initial { 2 } else { 0 },
            dwCtrlWakeupMask: 0,
            dwControlKeyState: 0,
        };
        if initial {
            text[..2].copy_from_slice(&[97, 98]);
        }
        // SAFETY: inherited input stays open until the reader joins.
        check(unsafe {
            ReadConsoleW(
                input_token as HANDLE,
                text.as_mut_ptr().cast(),
                text.len() as u32,
                &mut count,
                std::ptr::addr_of_mut!(control),
            )
        })?;
        if count as usize > text.len() {
            return Err(io::Error::other("oversized SDK read"));
        }
        Ok(text[..count as usize].to_vec())
    });
    if controlled {
        wait(deadline, || Ok(path.with_extension("go").exists()))?;
    } else {
        keys(input, false)?;
        wait(deadline, || Ok(snapshot(created)?.1 == (0, 4)))?;
    }
    if kind == "initial" {
        unsafe {
            check(SetConsoleCursorPosition(created, COORD { X: 9, Y: 0 }))?;
        }
    }
    if kind == "late-prompt" {
        write(created, "LO>")?;
    }
    unsafe {
        check(SetConsoleActiveScreenBuffer(base))?;
    }
    if kind == "close" {
        drop(buffer.take());
    }
    std::fs::write(path.with_extension("switched"), b"switched")?;
    if !controlled {
        keys(input, true)?;
    }
    wait(deadline, || Ok(reader.is_finished()))?;
    let units = reader
        .join()
        .map_err(|_| io::Error::other("SDK reader panicked"))??;
    // All fifteen inactive slots must be reusable once a read releases its
    // otherwise-closed drawing buffer. Hold every new object simultaneously.
    let mut fresh = Vec::new();
    if kind == "close" {
        for _ in 0..15 {
            let handle = unsafe {
                CreateConsoleScreenBuffer(
                    0xc000_0000,
                    3,
                    std::ptr::null(),
                    CONSOLE_TEXTMODE_BUFFER,
                    std::ptr::null(),
                )
            };
            if handle == INVALID_HANDLE_VALUE {
                return Err(io::Error::last_os_error());
            }
            fresh.push(unsafe { OwnedHandle::from_raw_handle(handle) });
        }
    }
    let created_after_read = fresh.len();
    drop(fresh);
    let (base_text, base_cursor) = snapshot(base)?;
    let (read_text, read_cursor) = if let Some(ref buffer) = buffer {
        snapshot(buffer.as_raw_handle())?
    } else {
        (String::new(), (-1, -1))
    };
    std::fs::write(
        path,
        format!(
            "{{\"units\":{units:?},\"created_after_read\":{created_after_read},\"base\":{base_text:?},\"base_cursor\":[{},{}],\"read_buffer\":{read_text:?},\"read_cursor\":[{},{}]}}",
            base_cursor.0, base_cursor.1, read_cursor.0, read_cursor.1
        ),
    )?;
    if let Some(ref buffer) = buffer {
        unsafe {
            check(SetConsoleActiveScreenBuffer(buffer.as_raw_handle()))?;
        }
    }
    let shown = buffer.as_ref().map_or(base, AsRawHandle::as_raw_handle);
    write(shown, "\x1b]2;cooked-buffer-revealed:\x07")?;
    wait(deadline, || Ok(path.with_extension("opened").exists()))?;
    unsafe {
        check(SetConsoleActiveScreenBuffer(base))?;
    }
    write(base, "\x1b]2;cooked-buffer-end:\x07")?;
    Ok(())
}

static CANCEL_EVENT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
unsafe extern "system" fn cancel_handler(event: u32) -> i32 {
    if event > 1 {
        return 0;
    }
    CANCEL_EVENT.store(event + 1, std::sync::atomic::Ordering::Release);
    1
}

pub(crate) fn run_cancel(kind: &str, path: &Path, controlled: bool) -> io::Result<()> {
    // SDK calls below use this isolated client's live console objects and
    // bounded initialized buffers. Only the fresh output object is closed
    // during the read; the inherited input remains open until its thread joins.
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
    let deadline = Instant::now() + Duration::from_secs(15);
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let base = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let created = unsafe {
        CreateConsoleScreenBuffer(
            0xc000_0000,
            3,
            std::ptr::null(),
            CONSOLE_TEXTMODE_BUFFER,
            std::ptr::null(),
        )
    };
    if created == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let buffer = unsafe { OwnedHandle::from_raw_handle(created) };
    unsafe {
        check(SetConsoleMode(input, 0xa7))?;
        check(SetConsoleMode(base, 15))?;
        check(SetConsoleMode(created, 15))?;
        check(SetConsoleCtrlHandler(Some(cancel_handler), 1))?;
    }
    write(base, "\x1b[2J\x1b[HB>")?;
    unsafe {
        check(SetConsoleActiveScreenBuffer(created))?;
    }
    write(created, "\x1b[2J\x1b[HA>")?;
    let token = input as usize;
    let reader = std::thread::spawn(move || {
        let mut text = [0_u16; 32];
        let mut count = 0;
        unsafe {
            SetLastError(0);
            let ok = ReadConsoleW(
                token as HANDLE,
                text.as_mut_ptr().cast(),
                text.len() as u32,
                &mut count,
                std::ptr::null_mut(),
            ) != 0;
            (ok, GetLastError(), count)
        }
    });
    if controlled {
        wait(deadline, || Ok(path.with_extension("go").exists()))?;
    } else {
        keys(input, false)?;
        wait(deadline, || Ok(snapshot(created)?.1 == (0, 4)))?;
    }
    if kind == "hidden" {
        unsafe {
            check(SetConsoleActiveScreenBuffer(base))?;
        }
    }
    drop(buffer);
    std::fs::write(path.with_extension("cancel-ready"), b"closed")?;
    write(base, "\x1b]2;cooked-cancel-ready:\x07")?;
    wait(deadline, || Ok(reader.is_finished()))?;
    let (ok, error, count) = reader
        .join()
        .map_err(|_| io::Error::other("SDK cancellation reader panicked"))?;
    if !controlled {
        wait(deadline, || {
            Ok(CANCEL_EVENT.load(std::sync::atomic::Ordering::Acquire) != 0)
        })?;
    }
    let event = CANCEL_EVENT.load(std::sync::atomic::Ordering::Acquire);
    std::fs::write(
        path.with_extension("cancelled"),
        format!("{ok},{error},{count},{event}"),
    )?;
    wait(deadline, || Ok(path.with_extension("probe").exists()))?;
    let mut mode = 0;
    unsafe {
        check(GetConsoleMode(input, &mut mode))?;
    }
    if mode != 0xa7 {
        return Err(io::Error::other("changed cancellation input mode"));
    }
    std::fs::write(path.with_extension("queried"), b"query-bypassed")?;
    let mut fresh = Vec::new();
    for index in 0..15 {
        let handle = unsafe {
            CreateConsoleScreenBuffer(
                0xc000_0000,
                3,
                std::ptr::null(),
                CONSOLE_TEXTMODE_BUFFER,
                std::ptr::null(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(io::Error::last_os_error());
        }
        fresh.push(unsafe { OwnedHandle::from_raw_handle(handle) });
        if index == 0 {
            std::fs::write(path.with_extension("created"), b"first-open")?;
        }
    }
    let created = fresh.len();
    drop(fresh);
    let (base_text, cursor) = snapshot(base)?;
    std::fs::write(
        path,
        format!(
            "{{\"ok\":{ok},\"error\":{error},\"count\":{count},\"event\":{event},\"created\":{created},\"base\":{base_text:?},\"cursor\":[{},{}]}}",
            cursor.0, cursor.1
        ),
    )?;
    write(base, "\x1b]2;cooked-cancel-end:\x07")?;
    wait(deadline, || Ok(path.with_extension("opened").exists()))?;
    Ok(())
}

fn probe_diagnostic(path: &Path, stage: &str, text: &str) {
    let temporary = path.with_extension(format!("{stage}-tmp"));
    // A detached client cannot rely on stderr to report a native SDK failure.
    let _ = std::fs::write(&temporary, text)
        .and_then(|()| std::fs::rename(&temporary, path.with_extension(stage)));
}

pub(crate) fn run_retire(kind: &str, path: &Path, controlled: bool) -> io::Result<()> {
    let result = run_retire_impl(kind, path, controlled);
    if let Err(error) = &result {
        probe_diagnostic(
            path,
            "read-error",
            &format!("cooked-retire {kind}: {error:?}"),
        );
    }
    result
}

fn run_retire_impl(kind: &str, path: &Path, controlled: bool) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{GetHandleInformation, GetLastError, SetLastError};
    if matches!(kind, "peer" | "exit-peer" | "exit-peer-records") {
        return retire_peer(path, kind != "peer", kind == "exit-peer-records");
    }
    let deadline = Instant::now() + Duration::from_secs(15);
    let base = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let standard_input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let fallback = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")?;
    let input = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONIN$")?;
    let input_token = input.as_raw_handle() as usize;
    let created = unsafe {
        CreateConsoleScreenBuffer(
            0xc000_0000,
            3,
            std::ptr::null(),
            CONSOLE_TEXTMODE_BUFFER,
            std::ptr::null(),
        )
    };
    if created == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let buffer = unsafe { OwnedHandle::from_raw_handle(created) };
    // SDK pointers are bounded initialized buffers; all objects belong to this
    // isolated console. Closing the reader's native file while it is blocked is
    // the behavior under observation, not a Rust reference to a dropped File.
    unsafe {
        check(SetConsoleMode(standard_input, 0xa7))?;
        check(SetConsoleMode(base, 15))?;
        check(SetConsoleMode(created, 15))?;
    }
    write(base, "\x1b[2J\x1b[HB>")?;
    let mut peer = if matches!(kind, "remaining" | "exit") {
        let child = std::process::Command::new(std::env::current_exe()?)
            .arg(if kind == "exit" {
                "cooked-retire-exit-peer-records"
            } else {
                "cooked-retire-peer"
            })
            .arg(path.with_extension("peer"))
            .spawn()?;
        wait(deadline, || Ok(path.with_extension("peer-ready").exists()))?;
        Some(child)
    } else {
        None
    };
    unsafe {
        check(SetConsoleActiveScreenBuffer(created))?;
    }
    write(created, "\x1b[2J\x1b[HA>")?;
    let reader_path = path.to_owned();
    let reader = std::thread::spawn(move || {
        let mut text = [0_u16; 32];
        let mut count = 0;
        unsafe {
            SetLastError(0);
            let ok = ReadConsoleW(
                input_token as HANDLE,
                text.as_mut_ptr().cast(),
                text.len() as u32,
                &mut count,
                std::ptr::null_mut(),
            ) != 0;
            let error = GetLastError();
            probe_diagnostic(
                &reader_path,
                "reader-returned",
                &format!("ok={ok};error={error};count={count}"),
            );
            (ok, error, count, text)
        }
    });
    if controlled {
        wait(deadline, || Ok(path.with_extension("go").exists()))?;
    } else {
        keys(standard_input, false)?;
        wait(deadline, || Ok(snapshot(created)?.1 == (0, 4)))?;
    }
    if kind == "remaining" {
        std::fs::write(path.with_extension("peer-go"), b"second-reader")?;
        wait(deadline, || Ok(path.with_extension("go-two").exists()))?;
    }
    drop(buffer);
    write(base, "\x1b]2;cooked-retire-ready:\x07")?;
    std::fs::write(path.with_extension("retire-ready"), b"retire")?;
    if kind == "exit" {
        // Keep the pending reader and its orphaned screen alive until the
        // controller has also admitted the independent surviving peer read.
        wait(deadline, || Ok(path.with_extension("exit").exists()))?;
        check(unsafe {
            windows_sys::Win32::System::Threading::TerminateProcess(
                windows_sys::Win32::System::Threading::GetCurrentProcess(),
                35,
            )
        })?;
        return Err(io::Error::other("self termination unexpectedly returned"));
    }
    let mut detached = false;
    let closer = if kind != "input-close" {
        probe_diagnostic(path, "disconnect-before", "calling FreeConsole");
        unsafe {
            check(FreeConsole())?;
        }
        probe_diagnostic(path, "disconnect-after", "FreeConsole returned success");
        detached = true;
        None
    } else {
        let closer = std::thread::spawn(move || drop(input));
        wait(deadline, || {
            let mut flags = 0;
            if unsafe { GetHandleInformation(input_token as HANDLE, &mut flags) } == 0 {
                if unsafe { GetLastError() } != 6 {
                    return Err(io::Error::last_os_error());
                }
                Ok(true)
            } else {
                Ok(false)
            }
        })?;
        std::fs::write(path.with_extension("closed"), b"handle-table-entry-removed")?;
        if !controlled {
            keys(standard_input, true)?;
        }
        Some(closer)
    };
    std::fs::write(path.with_extension("retired"), b"returned")?;
    wait(deadline, || Ok(reader.is_finished()))?;
    let (ok, error, count, units) = reader
        .join()
        .map_err(|_| io::Error::other("SDK retired reader panicked"))?;
    if count as usize > units.len() {
        return Err(io::Error::other("oversized retired read"));
    }
    let units = format!("{:?}", &units[..count as usize]);
    if let Some(closer) = closer {
        closer
            .join()
            .map_err(|_| io::Error::other("SDK closer panicked"))?;
    }
    let secondary = if let Some(ref mut peer) = peer {
        if !peer.wait()?.success() {
            return Err(io::Error::other("retirement peer failed"));
        }
        std::fs::read_to_string(path.with_extension("peer"))?
    } else {
        "[]".to_owned()
    };
    let temporary = path.with_extension("retire-tmp");
    std::fs::write(
        &temporary,
        format!(
            "{{\"ok\":{ok},\"error\":{error},\"count\":{count},\"detached\":{detached},\"secondary\":{secondary},\"units\":{units}}}"
        ),
    )?;
    std::fs::rename(temporary, path)?;
    wait(deadline, || Ok(path.with_extension("opened").exists()))?;
    drop(fallback);
    Ok(())
}

fn retire_peer(path: &Path, hold: bool, records: bool) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
    let input = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONIN$")?;
    let output = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")?;
    if snapshot(output.as_raw_handle())? != ("B>".to_owned(), (0, 2)) {
        return Err(io::Error::other(
            "peer must attach before the private edit buffer activates",
        ));
    }
    std::fs::write(path.with_extension("peer-ready"), b"attached-to-base")?;
    wait(Instant::now() + Duration::from_secs(15), || {
        Ok(path.with_extension("peer-go").exists())
    })?;
    if hold && !records {
        check(unsafe { SetConsoleMode(input.as_raw_handle(), 7) })?;
    }
    let mut units = [0_u16; 32];
    let mut count = 0;
    unsafe {
        SetLastError(0);
        if records {
            let mut record = INPUT_RECORD::default();
            check(ReadConsoleInputW(
                input.as_raw_handle(),
                &mut record,
                1,
                &mut count,
            ))?;
            if count != 1
                || record.EventType != KEY_EVENT as u16
                || record.Event.KeyEvent.bKeyDown == 0
            {
                return Err(io::Error::other(
                    "peer must receive one native key-down record",
                ));
            }
            units[0] = record.Event.KeyEvent.uChar.UnicodeChar;
        } else {
            check(ReadConsoleW(
                input.as_raw_handle(),
                units.as_mut_ptr().cast(),
                units.len() as u32,
                &mut count,
                std::ptr::null_mut(),
            ))?;
        }
        if GetLastError() != 0 || count as usize > units.len() {
            return Err(io::Error::other("peer read interrupted or oversized"));
        }
    }
    let temporary = path.with_extension("peer-tmp");
    std::fs::write(&temporary, format!("{:?}", &units[..count as usize]))?;
    std::fs::rename(temporary, path)?;
    if hold {
        wait(Instant::now() + Duration::from_secs(15), || {
            Ok(path.with_extension("opened").exists())
        })?;
    }
    Ok(())
}
