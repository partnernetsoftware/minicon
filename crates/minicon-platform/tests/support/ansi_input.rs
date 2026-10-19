//! Independent ANSI input-record SDK caller; readback deliberately stays W.
use std::io;
use std::os::windows::io::AsRawHandle;
use std::path::Path;
use windows_sys::Win32::Foundation::{GetLastError, HANDLE, INVALID_HANDLE_VALUE, SetLastError};
use windows_sys::Win32::System::Console::*;
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
fn focus() -> INPUT_RECORD {
    INPUT_RECORD {
        EventType: FOCUS_EVENT as u16,
        Event: INPUT_RECORD_0 {
            FocusEvent: FOCUS_EVENT_RECORD { bSetFocus: 1 },
        },
    }
}
fn put(input: HANDLE, records: &[INPUT_RECORD], ansi: bool) -> io::Result<u32> {
    let mut count = 0;
    // SAFETY: initialized input records and this fixture's live console file.
    checked(unsafe {
        if ansi {
            WriteConsoleInputA(input, records.as_ptr(), records.len() as u32, &mut count)
        } else {
            WriteConsoleInputW(input, records.as_ptr(), records.len() as u32, &mut count)
        }
    })?;
    Ok(count)
}
fn queued(input: HANDLE) -> io::Result<u32> {
    let mut count = 0;
    checked(unsafe { GetNumberOfConsoleInputEvents(input, &mut count) })?;
    Ok(count)
}
fn snapshot(input: HANDLE, peek: bool) -> io::Result<String> {
    let mut records = [INPUT_RECORD::default(); 16];
    let mut count = 0;
    checked(unsafe {
        if peek {
            PeekConsoleInputW(input, records.as_mut_ptr(), 16, &mut count)
        } else {
            ReadConsoleInputW(input, records.as_mut_ptr(), 16, &mut count)
        }
    })?;
    if count > 16 {
        return Err(io::Error::other("oversized native record reply"));
    }
    let mut text = String::new();
    for record in &records[..count as usize] {
        // SAFETY: EventType selects the initialized SDK union member.
        let item = unsafe {
            match u32::from(record.EventType) {
                KEY_EVENT => {
                    let k = record.Event.KeyEvent;
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
                FOCUS_EVENT => format!("F:{}", record.Event.FocusEvent.bSetFocus),
                MOUSE_EVENT => {
                    let m = record.Event.MouseEvent;
                    format!(
                        "M:{},{},{},{},{}",
                        m.dwMousePosition.X,
                        m.dwMousePosition.Y,
                        m.dwButtonState,
                        m.dwControlKeyState,
                        m.dwEventFlags
                    )
                }
                _ => return Err(io::Error::other("unexpected input record")),
            }
        };
        if !text.is_empty() {
            text.push('|');
        }
        text.push_str(&item);
    }
    Ok(text)
}
fn finish(input: HANDLE, report: &mut String) -> io::Result<()> {
    report.push_str(&format!("queued={},{}\n", queued(input)?, unsafe {
        WaitForSingleObject(input, 0)
    }));
    let peek = snapshot(input, true)?;
    report.push_str(&format!("peek={peek}\n"));
    if snapshot(input, true)? != peek {
        return Err(io::Error::other("peek changed records"));
    }
    report.push_str(&format!("read={}\n", snapshot(input, false)?));
    report.push_str(&format!("empty={},{}\n", queued(input)?, unsafe {
        WaitForSingleObject(input, 0)
    }));
    Ok(())
}
pub(super) fn run(mode: &str, path: &Path) -> io::Result<()> {
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    checked(unsafe { SetConsoleMode(input, 0) })?;
    checked(unsafe { FlushConsoleInputBuffer(input) })?;
    checked(unsafe { SetConsoleOutputCP(437) })?;
    let kind = mode
        .strip_prefix("ansi-input-")
        .ok_or(io::ErrorKind::InvalidInput)?;
    checked(unsafe {
        SetConsoleCP(if kind == "single" {
            1252
        } else if kind == "oem" {
            437
        } else if kind == "utf8" {
            65001
        } else {
            932
        })
    })?;
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    match kind {
        "single" => {
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
            let count = put(input, &[key(0xaae9, 0), focus(), mouse, up], true)?;
            report.push_str(&format!("written={count}\n"));
            finish(input, &mut report)?;
        }
        "oem" => {
            report.push_str(&format!("written={}\n", put(input, &[key(0x82, 0)], true)?));
            finish(input, &mut report)?;
        }
        "pair" | "invalid" | "utf8" => {
            let records = if kind == "utf8" {
                vec![key(0xe4, 0), key(0xb8, 1), key(0xad, 2)]
            } else {
                vec![
                    key(0x92, 0),
                    key(if kind == "pair" { 0x86 } else { 0x20 }, 1),
                ]
            };
            report.push_str(&format!("written={}\n", put(input, &records, true)?));
            finish(input, &mut report)?;
        }
        "split" | "rights" | "codepage" | "pending" | "capacity" => {
            let writer = super::console_file::open("CONIN$", 0x4000_0000)?;
            report.push_str(&format!(
                "lead={},{},{}\n",
                put(writer.as_raw_handle(), &[key(0x92, 0)], true)?,
                queued(input)?,
                unsafe { WaitForSingleObject(input, 0) }
            ));
            drop(writer);
            report.push_str(&format!(
                "zero={},{},{}\n",
                put(input, &[], true)?,
                queued(input)?,
                unsafe { WaitForSingleObject(input, 0) }
            ));
            if kind == "split" {
                put(input, &[focus()], false)?;
                checked(unsafe { FlushConsoleInputBuffer(input) })?;
            }
            if kind == "rights" {
                let ro = super::console_file::open("CONIN$", 0x8000_0000)?;
                for (name, handle) in [
                    ("ro", ro.as_raw_handle()),
                    ("out", unsafe { GetStdHandle(STD_OUTPUT_HANDLE) }),
                    ("bad", INVALID_HANDLE_VALUE),
                ] {
                    let mut count = 999;
                    unsafe { SetLastError(0) };
                    let ok = unsafe { WriteConsoleInputA(handle, &key(0x86, 1), 1, &mut count) };
                    report.push_str(&format!(
                        "{name}={ok},{},{count},{}\n",
                        unsafe { GetLastError() },
                        queued(input)?
                    ));
                }
            }
            if kind == "codepage" || kind == "capacity" {
                checked(unsafe { SetConsoleCP(1252) })?;
            }
            if kind == "capacity" {
                put(input, &vec![focus(); 4095], false)?;
                let mut count = 999;
                unsafe { SetLastError(0) };
                let ok = unsafe { WriteConsoleInputA(input, &key(0x86, 1), 1, &mut count) };
                report.push_str(&format!(
                    "full={ok},{},{count},{}\n",
                    unsafe { GetLastError() },
                    queued(input)?
                ));
                checked(unsafe { FlushConsoleInputBuffer(input) })?;
                checked(unsafe { SetConsoleCP(932) })?;
                // A two-byte pair must fit one remaining queue slot.
                put(input, &vec![focus(); 4095], false)?;
                report.push_str(&format!("fit={}\n", put(input, &[key(0x86, 1)], true)?));
                checked(unsafe { FlushConsoleInputBuffer(input) })?;
                put(input, &vec![focus(); 4095], false)?;
                report.push_str(&format!(
                    "pair-fit={}\n",
                    put(input, &[key(0x92, 0), key(0x86, 1)], true)?
                ));
                checked(unsafe { FlushConsoleInputBuffer(input) })?;
                put(input, &[key(0x92, 0)], true)?;
            }
            if kind == "pending" {
                let reader = super::console_file::open("CONIN$", 0xc000_0000)?;
                let (send, receive) = std::sync::mpsc::channel();
                std::thread::spawn(move || {
                    let mut unit = 0_u16;
                    let mut count = 0;
                    let result = checked(unsafe {
                        ReadConsoleW(
                            reader.as_raw_handle(),
                            (&mut unit as *mut u16).cast(),
                            1,
                            &mut count,
                            std::ptr::null(),
                        )
                    })
                    .map(|()| (count, unit));
                    let _ = send.send(result);
                });
                // API read may already wait or may consume the following key.
                // Either order must deliver exactly the converted character.
                report.push_str(&format!("written={}\n", put(input, &[key(0x86, 1)], true)?));
                let (count, unit) = receive
                    .recv_timeout(std::time::Duration::from_secs(8))
                    .map_err(|_| io::ErrorKind::TimedOut)??;
                report.push_str(&format!("text={count},{unit}\n"));
            } else {
                report.push_str(&format!("written={}\n", put(input, &[key(0x86, 1)], true)?));
                finish(input, &mut report)?;
            }
        }
        "nonkey" => {
            report.push_str(&format!(
                "batch={}\n",
                put(input, &[key(0x92, 0), focus(), key(90, 2)], true)?
            ));
            finish(input, &mut report)?;
            put(input, &[key(0x92, 0)], true)?;
            report.push_str(&format!(
                "split={}\n",
                put(input, &[focus(), key(90, 2)], true)?
            ));
            finish(input, &mut report)?;
        }
        _ => return Err(io::ErrorKind::InvalidInput.into()),
    }
    std::fs::write(path, report)
}
