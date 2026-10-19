//! Native console API probe. Results use files so a broken console path cannot
//! accidentally supply its own expected output. Used by the integration court.
#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("console probe: {error}");
        std::process::exit(2);
    }
}

#[cfg(not(windows))]
fn main() {
    eprintln!("minicon-console-probe requires Windows");
    std::process::exit(2);
}

#[cfg(windows)]
fn run() -> std::io::Result<()> {
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError, WAIT_OBJECT_0};
    use windows_sys::Win32::Storage::FileSystem::ReadFile;
    use windows_sys::Win32::System::Console::{
        GetNumberOfConsoleInputEvents, GetStdHandle, INPUT_RECORD, PeekConsoleInputW,
        ReadConsoleInputW, ReadConsoleW, STD_INPUT_HANDLE, SetConsoleCP, SetConsoleMode,
    };
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let args: Vec<_> = std::env::args_os().skip(1).collect();
    if args.len() != 2 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidInput,
            "usage: minicon-console-probe MODE RESULT_PATH",
        ));
    }
    let mode = args[0].to_str().unwrap_or("");
    let result = std::path::Path::new(&args[1]);
    if mode == "job-child" {
        std::fs::write(result.with_extension("ready"), b"ready")?;
        std::thread::sleep(std::time::Duration::from_secs(60));
        return Ok(());
    }
    if mode == "job-tree" {
        let child = std::process::Command::new(std::env::current_exe()?)
            .arg("job-child")
            .arg(result)
            .spawn()?;
        std::fs::write(result, format!("{{\"child_pid\":{}}}", child.id()))?;
        std::thread::sleep(std::time::Duration::from_secs(60));
        return Ok(());
    }
    // SAFETY: console APIs operate on this fixture's own inherited input handle;
    // all buffers below are live and their explicit lengths match allocation.
    let report = unsafe {
        let input = GetStdHandle(STD_INPUT_HANDLE);
        let input_mode = if matches!(
            mode,
            "raw-bytes"
                | "raw-emoji"
                | "records"
                | "wait-records"
                | "peek-records"
                | "peek-existing"
        ) {
            0x98
        } else {
            7
        };
        if SetConsoleMode(input, input_mode) == 0 {
            return Err(std::io::Error::last_os_error());
        }
        match mode {
            "concurrent-peek" => return concurrent_peek(input, result),
            "count-events" => {
                let mut count = 0;
                SetLastError(0);
                let ok = GetNumberOfConsoleInputEvents(input, &mut count) != 0;
                let error = GetLastError();
                let wait = WaitForSingleObject(input, 0);
                format!("{{\"ok\":{ok},\"error\":{error},\"count\":{count},\"wait\":{wait}}}")
            }
            "line" | "cancel" | "initial-line" | "wakeup" => {
                let mut units = [0_u16; 32];
                let mut count = 0;
                let mut control =
                    windows_sys::Win32::System::Console::CONSOLE_READCONSOLE_CONTROL {
                        nLength: std::mem::size_of::<
                            windows_sys::Win32::System::Console::CONSOLE_READCONSOLE_CONTROL,
                        >() as u32,
                        nInitialChars: if mode == "initial-line" { 4 } else { 0 },
                        dwCtrlWakeupMask: if mode == "wakeup" { 1 << 9 } else { 0 },
                        dwControlKeyState: 0,
                    };
                if mode == "initial-line" {
                    units[..4].copy_from_slice(&[112, 114, 101, 45]);
                }
                SetLastError(0);
                let ok = ReadConsoleW(
                    input,
                    units.as_mut_ptr().cast(),
                    units.len() as u32,
                    &mut count,
                    if matches!(mode, "initial-line" | "wakeup") {
                        &mut control
                    } else {
                        null()
                    },
                ) != 0;
                let error = GetLastError();
                if count as usize > units.len() {
                    return Err(std::io::Error::other("invalid native read count"));
                }
                format!(
                    "{{\"ok\":{ok},\"error\":{error},\"count\":{count},\"controls\":{},\"units\":{:?}}}",
                    control.dwControlKeyState,
                    &units[..count as usize]
                )
            }
            "raw-bytes" | "raw-emoji" => {
                if SetConsoleCP(65001) == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                let mut bytes = Vec::new();
                let mut error = 0;
                let mut ok = true;
                for _ in 0..if mode == "raw-emoji" { 4 } else { 3 } {
                    let mut byte = 0_u8;
                    let mut count = 0;
                    SetLastError(0);
                    if ReadFile(input, &mut byte, 1, &mut count, null_mut()) == 0 {
                        ok = false;
                        error = GetLastError();
                        break;
                    }
                    if count != 1 {
                        ok = false;
                        break;
                    }
                    bytes.push(byte);
                }
                format!("{{\"ok\":{ok},\"error\":{error},\"bytes\":{bytes:?}}}")
            }
            "records" | "wait-records" | "peek-records" | "peek-existing" => {
                let mut records = [INPUT_RECORD::default(); 4];
                let mut peek_count = 0;
                if mode == "peek-records"
                    && PeekConsoleInputW(
                        input,
                        records.as_mut_ptr(),
                        records.len() as u32,
                        &mut peek_count,
                    ) == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                let mut wait = WAIT_OBJECT_0;
                if matches!(mode, "wait-records" | "peek-existing") {
                    std::fs::write(result.with_extension("ready"), b"ready")?;
                    wait = WaitForSingleObject(input, 3000);
                }
                if mode == "peek-existing"
                    && PeekConsoleInputW(
                        input,
                        records.as_mut_ptr(),
                        records.len() as u32,
                        &mut peek_count,
                    ) == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                let mut count = 0;
                SetLastError(0);
                let ok = wait == WAIT_OBJECT_0
                    && ReadConsoleInputW(
                        input,
                        records.as_mut_ptr(),
                        records.len() as u32,
                        &mut count,
                    ) != 0;
                let error = GetLastError();
                if count as usize > records.len() {
                    return Err(std::io::Error::other("invalid native record count"));
                }
                let mut decoded = Vec::new();
                for record in &records[..count as usize] {
                    // Read only the active union variant, never struct padding.
                    let fields = match record.EventType {
                        1 => {
                            let key = record.Event.KeyEvent;
                            vec![
                                1,
                                key.bKeyDown as u32,
                                key.wRepeatCount as u32,
                                key.wVirtualKeyCode as u32,
                                key.wVirtualScanCode as u32,
                                key.uChar.UnicodeChar as u32,
                                key.dwControlKeyState,
                            ]
                        }
                        2 => {
                            let mouse = record.Event.MouseEvent;
                            vec![
                                2,
                                mouse.dwMousePosition.X as u32,
                                mouse.dwMousePosition.Y as u32,
                                mouse.dwButtonState,
                                mouse.dwControlKeyState,
                                mouse.dwEventFlags,
                            ]
                        }
                        4 => {
                            let resize = record.Event.WindowBufferSizeEvent;
                            vec![4, resize.dwSize.X as u32, resize.dwSize.Y as u32]
                        }
                        16 => vec![16, record.Event.FocusEvent.bSetFocus as u32],
                        _ => vec![record.EventType as u32],
                    };
                    decoded.push(fields);
                }
                format!(
                    "{{\"ok\":{ok},\"error\":{error},\"wait\":{wait},\"peek_count\":{peek_count},\"records\":{decoded:?}}}"
                )
            }
            _ => {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    "unknown probe mode",
                ));
            }
        }
    };
    std::fs::write(result, report)
}

#[cfg(windows)]
fn concurrent_peek(
    input: windows_sys::Win32::Foundation::HANDLE,
    result: &std::path::Path,
) -> std::io::Result<()> {
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
    use windows_sys::Win32::System::Console::{INPUT_RECORD, PeekConsoleInputW, ReadConsoleW};
    let raw_input = input as usize;
    let (send, receive) = std::sync::mpsc::channel();
    let worker = std::thread::spawn(move || {
        let mut units = [0_u16; 16];
        let mut count = 0;
        // SAFETY: handle remains live in this process; output storage is owned
        // by this thread and used only after the synchronous read returns.
        let (ok, error) = unsafe {
            SetLastError(0);
            let ok = ReadConsoleW(
                raw_input as _,
                units.as_mut_ptr().cast(),
                units.len() as u32,
                &mut count,
                std::ptr::null(),
            ) != 0;
            (ok, GetLastError())
        };
        let _ = send.send((ok, error, count, units));
    });
    let deadline = Instant::now() + Duration::from_secs(5);
    // Controller creates this only after observing the actual deferred read.
    while !result.with_extension("go").exists() {
        if Instant::now() > deadline {
            return Err(std::io::Error::from(std::io::ErrorKind::TimedOut));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut records = [INPUT_RECORD::default(); 4];
    let mut peek_count = 0;
    // SAFETY: actual Win32 peek, with a live handle and matching buffer length.
    let (peek_ok, peek_error) = unsafe {
        SetLastError(0);
        let ok = PeekConsoleInputW(
            input,
            records.as_mut_ptr(),
            records.len() as u32,
            &mut peek_count,
        ) != 0;
        (ok, GetLastError())
    };
    std::fs::write(result.with_extension("peek"), b"peek-returned")?;
    let (ok, error, count, units) = receive
        .recv_timeout(Duration::from_secs(5))
        .map_err(|_| std::io::Error::from(std::io::ErrorKind::TimedOut))?;
    worker
        .join()
        .map_err(|_| std::io::Error::other("read worker panicked"))?;
    if count as usize > units.len() {
        return Err(std::io::Error::other("invalid native read count"));
    }
    std::fs::write(
        result,
        format!(
            "{{\"peek_ok\":{peek_ok},\"peek_error\":{peek_error},\"peek_count\":{peek_count},\"ok\":{ok},\"error\":{error},\"units\":{:?}}}",
            &units[..count as usize]
        ),
    )
}
