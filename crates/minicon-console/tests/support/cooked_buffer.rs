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
