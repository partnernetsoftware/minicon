//! Independent SDK reads; spare physical slots expose old-host count overflow.
use std::{
    io,
    os::windows::io::AsRawHandle,
    path::Path,
    time::{Duration, Instant},
};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE, SetLastError};
use windows_sys::Win32::System::Console::*;
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleW, GetProcAddress};
use windows_sys::Win32::System::Threading::WaitForSingleObject;

fn checked(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn key(character: u16, variant: u16) -> INPUT_RECORD {
    INPUT_RECORD {
        EventType: KEY_EVENT as u16,
        Event: INPUT_RECORD_0 {
            KeyEvent: KEY_EVENT_RECORD {
                bKeyDown: 1,
                wRepeatCount: 3,
                wVirtualKeyCode: 65 + variant,
                wVirtualScanCode: 30 + variant,
                uChar: KEY_EVENT_RECORD_0 {
                    UnicodeChar: character,
                },
                dwControlKeyState: 16 + u32::from(variant),
            },
        },
    }
}
fn put(input: HANDLE, records: &[INPUT_RECORD]) -> io::Result<()> {
    let mut count = 0;
    checked(unsafe {
        WriteConsoleInputW(input, records.as_ptr(), records.len() as u32, &mut count)
    })?;
    if count as usize != records.len() {
        return Err(io::Error::other("short native input write"));
    }
    Ok(())
}
fn queued(input: HANDLE) -> io::Result<u32> {
    let mut count = 0;
    checked(unsafe { GetNumberOfConsoleInputEvents(input, &mut count) })?;
    Ok(count)
}
fn get(input: HANDLE, wide: bool, peek: bool, capacity: u32, nowait: bool) -> io::Result<String> {
    if capacity > 128 {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    const CANARY: u16 = 0x7fed;
    let mut records = [INPUT_RECORD::default(); 128];
    for r in &mut records {
        r.EventType = CANARY;
    }
    let mut count = 0;
    // SAFETY: initialized physical storage exceeds the requested test capacity.
    // Exact pinned winconp.h ABI; optional exports must exist, never fallback.
    let ok = unsafe {
        if nowait {
            type ReadEx =
                unsafe extern "system" fn(HANDLE, *mut INPUT_RECORD, u32, *mut u32, u16) -> i32;
            let module: Vec<u16> = "kernel32.dll".encode_utf16().chain([0]).collect();
            let module = GetModuleHandleW(module.as_ptr());
            let name = if wide {
                b"ReadConsoleInputExW\0".as_slice()
            } else {
                b"ReadConsoleInputExA\0".as_slice()
            };
            let proc = GetProcAddress(module, name.as_ptr()).ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::Unsupported,
                    "native record Ex export missing",
                )
            })?;
            let read: ReadEx = std::mem::transmute(proc);
            read(
                input,
                records.as_mut_ptr(),
                capacity,
                &mut count,
                if peek { 3 } else { 2 },
            )
        } else if peek {
            if wide {
                PeekConsoleInputW(input, records.as_mut_ptr(), capacity, &mut count)
            } else {
                PeekConsoleInputA(input, records.as_mut_ptr(), capacity, &mut count)
            }
        } else if wide {
            ReadConsoleInputW(input, records.as_mut_ptr(), capacity, &mut count)
        } else {
            ReadConsoleInputA(input, records.as_mut_ptr(), capacity, &mut count)
        }
    };
    checked(ok)?;
    if count > 128 || capacity > 128 {
        return Err(io::Error::other("unsafe SDK reply length"));
    }
    let over = records[capacity as usize..]
        .iter()
        .filter(|r| r.EventType != CANARY)
        .count();
    let mut text = String::new();
    for r in &records[..count as usize] {
        let value = unsafe {
            match u32::from(r.EventType) {
                KEY_EVENT => {
                    let k = r.Event.KeyEvent;
                    format!(
                        "K:{},{},{},{},{},{}",
                        k.bKeyDown,
                        k.wRepeatCount,
                        k.wVirtualKeyCode,
                        k.wVirtualScanCode,
                        k.uChar.UnicodeChar,
                        k.dwControlKeyState
                    )
                }
                FOCUS_EVENT => format!("F:{}", r.Event.FocusEvent.bSetFocus),
                MOUSE_EVENT => {
                    let m = r.Event.MouseEvent;
                    format!(
                        "M:{},{},{},{},{}",
                        m.dwMousePosition.X,
                        m.dwMousePosition.Y,
                        m.dwButtonState,
                        m.dwControlKeyState,
                        m.dwEventFlags
                    )
                }
                _ => format!("X:{}", r.EventType),
            }
        };
        if !text.is_empty() {
            text.push('|');
        }
        text.push_str(&value);
    }
    Ok(format!(
        "{count}/{capacity}:{text};over={over};q={},{}",
        queued(input)?,
        unsafe { WaitForSingleObject(input, 0) }
    ))
}
fn observe(
    input: HANDLE,
    report: &mut String,
    label: &str,
    wide: bool,
    peek: bool,
    capacity: u32,
) -> io::Result<()> {
    report.push_str(&format!(
        "{label}={}\n",
        get(input, wide, peek, capacity, true).map_err(|error| io::Error::new(
            error.kind(),
            format!("SDK stage {label}: {error}; prior={report}")
        ))?
    ));
    Ok(())
}
pub(super) fn run(mode: &str, path: &Path) -> io::Result<()> {
    let panic_path = path.with_extension("read-panic");
    std::panic::set_hook(Box::new(move |info| {
        let _ = std::fs::write(&panic_path, info.to_string());
    }));
    let result = run_inner(mode, path);
    if let Err(error) = &result {
        let _ = std::fs::write(path.with_extension("read-error"), error.to_string());
    }
    result
}

fn run_inner(mode: &str, path: &Path) -> io::Result<()> {
    let kind = mode
        .strip_prefix("ansi-read-")
        .ok_or(io::ErrorKind::InvalidInput)?;
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    checked(unsafe { SetConsoleMode(input, 0) })?;
    checked(unsafe { FlushConsoleInputBuffer(input) })?;
    checked(unsafe {
        SetConsoleCP(
            if matches!(kind, "single" | "wait" | "pressure" | "behind" | "rights") {
                1252
            } else if kind == "utf8" || kind == "surrogate" {
                65001
            } else {
                932
            },
        )
    })?;
    checked(unsafe { SetConsoleOutputCP(437) })?;
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    match kind {
        "rights" => {
            put(input, &[key(233, 0)])?;
            let wo = super::console_file::open("CONIN$", 0x4000_0000)?;
            for (name, handle) in [
                ("wo", wo.as_raw_handle()),
                ("out", unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }),
                ("bad", INVALID_HANDLE_VALUE),
            ] {
                let mut record = INPUT_RECORD::default();
                let mut count = 999;
                unsafe { SetLastError(0) };
                let ok = unsafe { PeekConsoleInputA(handle, &mut record, 1, &mut count) };
                report.push_str(&format!("{name}={ok},{},{count}\n", unsafe {
                    GetLastError()
                }));
            }
            observe(input, &mut report, "r", false, false, 16)?;
        }
        "single" => {
            let focus = INPUT_RECORD {
                EventType: FOCUS_EVENT as u16,
                Event: INPUT_RECORD_0 {
                    FocusEvent: FOCUS_EVENT_RECORD { bSetFocus: 1 },
                },
            };
            let mouse = INPUT_RECORD {
                EventType: MOUSE_EVENT as u16,
                Event: INPUT_RECORD_0 {
                    MouseEvent: MOUSE_EVENT_RECORD {
                        dwMousePosition: COORD { X: -2, Y: 3 },
                        dwButtonState: 5,
                        dwControlKeyState: 8,
                        dwEventFlags: 1,
                    },
                },
            };
            let mut up = key(65, 1);
            up.Event.KeyEvent.bKeyDown = 0;
            put(input, &[key(233, 0), focus, mouse, up])?;
            observe(input, &mut report, "p1", false, true, 16)?;
            observe(input, &mut report, "p2", false, true, 16)?;
            observe(input, &mut report, "r", false, false, 16)?;
            observe(input, &mut report, "empty", false, true, 16)?;
        }
        "wait" => {
            let reader = super::console_file::open("CONIN$", 0xc000_0000)?;
            let (send, receive) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let result = get(reader.as_raw_handle(), false, false, 1, false);
                let _ = send.send(result);
            });
            put(input, &[key(8364, 0)])?;
            report.push_str(&format!(
                "r={}\n",
                receive
                    .recv_timeout(Duration::from_secs(8))
                    .map_err(|_| io::ErrorKind::TimedOut)??
            ));
        }
        "behind" => {
            checked(unsafe { SetConsoleMode(input, 3) })?;
            put(input, &[key(65, 0), key(66, 1)])?;
            // Three repeats above yield AAABBB before the final C/CR.
            let reader = super::console_file::open("CONIN$", 0xc000_0000)?;
            let (send, receive) = std::sync::mpsc::channel();
            std::thread::spawn(move || {
                let mut text = [0u16; 16];
                let mut count = 0;
                let result = checked(unsafe {
                    ReadConsoleW(
                        reader.as_raw_handle(),
                        text.as_mut_ptr().cast(),
                        16,
                        &mut count,
                        std::ptr::null(),
                    )
                })
                .map(|()| text[..count as usize].to_vec());
                let _ = send.send(result);
            });
            let deadline = Instant::now() + Duration::from_secs(8);
            while queued(input)? != 0 {
                if Instant::now() >= deadline {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            observe(input, &mut report, "query", false, false, 1)?;
            let mut c = key(67, 2);
            c.Event.KeyEvent.wRepeatCount = 1;
            let mut cr = key(13, 3);
            cr.Event.KeyEvent.wRepeatCount = 1;
            put(input, &[c, cr])?;
            report.push_str(&format!(
                "text={:?}\n",
                receive
                    .recv_timeout(Duration::from_secs(8))
                    .map_err(|_| io::ErrorKind::TimedOut)??
            ));
        }
        "pressure" => {
            put(input, &[key(233, 0)])?;
            let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
            checked(unsafe { SetConsoleMode(output, 15) })?;
            for _ in 0..8 {
                let units = [u16::from(b'P'); 8192];
                let mut count = 0;
                checked(unsafe {
                    WriteConsoleW(
                        output,
                        units.as_ptr().cast(),
                        8192,
                        &mut count,
                        std::ptr::null(),
                    )
                })?;
                if count != 8192 {
                    return Err(io::Error::other("short pressure write"));
                }
            }
            observe(input, &mut report, "p", false, true, 1)?;
            observe(input, &mut report, "r", false, false, 1)?;
            std::fs::write(path.with_extension("ready"), &report)?;
            let deadline = Instant::now() + Duration::from_secs(8);
            while !path.with_extension("go").exists() {
                if Instant::now() >= deadline {
                    return Err(io::ErrorKind::TimedOut.into());
                }
                std::thread::sleep(Duration::from_millis(2));
            }
        }
        _ => {
            let initial = if kind == "surrogate" {
                vec![key(0xd83d, 0), key(0xde00, 1)]
            } else if kind == "codepage" {
                vec![key(20013, 0), key(233, 1)]
            } else if matches!(kind, "split" | "utf8" | "switch" | "text") {
                vec![key(20013, 0), key(90, 1)]
            } else {
                vec![key(20013, 0)]
            };
            put(input, &initial)?;
            if kind == "surrogate" {
                observe(input, &mut report, "r", false, false, 16)?;
            } else if kind == "split" || kind == "utf8" {
                let cap = if kind == "utf8" { 2 } else { 1 };
                observe(input, &mut report, "p1", false, true, cap)?;
                observe(input, &mut report, "p2", false, true, cap)?;
                for label in ["r1", "p3", "r2", "r3", "end"] {
                    observe(
                        input,
                        &mut report,
                        label,
                        false,
                        label == "p3" || label == "end",
                        if label == "end" { 16 } else { cap },
                    )?;
                }
            } else {
                let reader = super::console_file::open("CONIN$", 0xc000_0000)?;
                report.push_str(&format!(
                    "first={}\n",
                    get(reader.as_raw_handle(), false, false, 1, true)?
                ));
                let reader = if kind == "close" {
                    drop(reader);
                    None
                } else {
                    Some(reader)
                };
                let remainder = reader.as_ref().map_or(input, |file| file.as_raw_handle());
                match kind {
                    "flush" => checked(unsafe { FlushConsoleInputBuffer(input) })?,
                    "codepage" => checked(unsafe { SetConsoleCP(1252) })?,
                    "switch" => observe(input, &mut report, "wide", true, true, 16)?,
                    "zero" => observe(input, &mut report, "zero", true, true, 0)?,
                    "text" => {
                        let mut unit = 0u16;
                        let mut count = 0;
                        checked(unsafe {
                            ReadConsoleW(
                                input,
                                (&mut unit as *mut u16).cast(),
                                1,
                                &mut count,
                                std::ptr::null(),
                            )
                        })?;
                        report.push_str(&format!("text={count},{unit}\n"));
                    }
                    "tail" | "close" => {}
                    _ => return Err(io::ErrorKind::InvalidInput.into()),
                }
                observe(remainder, &mut report, "p", false, true, 16)?;
                if kind == "flush" || kind == "close" {
                    put(input, &[key(90, 1)])?;
                }
                observe(remainder, &mut report, "rest", false, false, 16)?;
            }
        }
    }
    std::fs::write(path, report)
}
