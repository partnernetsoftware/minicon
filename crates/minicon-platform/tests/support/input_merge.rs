//! Independent queue/coalescing callers. Record readback always uses W.
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE, SetLastError};
use windows_sys::Win32::System::Console::*;
use windows_sys::Win32::System::Threading::WaitForSingleObject;

fn checked(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn key(ch: u16, repeat: u16, scan: u16, controls: u32, vk: u16, down: i32) -> INPUT_RECORD {
    INPUT_RECORD {
        EventType: KEY_EVENT as u16,
        Event: INPUT_RECORD_0 {
            KeyEvent: KEY_EVENT_RECORD {
                bKeyDown: down,
                wRepeatCount: repeat,
                wVirtualKeyCode: vk,
                wVirtualScanCode: scan,
                uChar: KEY_EVENT_RECORD_0 { UnicodeChar: ch },
                dwControlKeyState: controls,
            },
        },
    }
}
fn a(repeat: u16) -> INPUT_RECORD {
    key(65, repeat, 30, 16, 65, 1)
}
fn focus() -> INPUT_RECORD {
    INPUT_RECORD {
        EventType: FOCUS_EVENT as u16,
        Event: INPUT_RECORD_0 {
            FocusEvent: FOCUS_EVENT_RECORD { bSetFocus: 0 },
        },
    }
}
fn resize(x: i16, y: i16) -> INPUT_RECORD {
    INPUT_RECORD {
        EventType: WINDOW_BUFFER_SIZE_EVENT as u16,
        Event: INPUT_RECORD_0 {
            WindowBufferSizeEvent: WINDOW_BUFFER_SIZE_RECORD {
                dwSize: COORD { X: x, Y: y },
            },
        },
    }
}
fn mouse(x: i16, y: i16, buttons: u32, controls: u32, flags: u32) -> INPUT_RECORD {
    INPUT_RECORD {
        EventType: MOUSE_EVENT as u16,
        Event: INPUT_RECORD_0 {
            MouseEvent: MOUSE_EVENT_RECORD {
                dwMousePosition: COORD { X: x, Y: y },
                dwButtonState: buttons,
                dwControlKeyState: controls,
                dwEventFlags: flags,
            },
        },
    }
}
fn put(input: HANDLE, events: &[INPUT_RECORD], ansi: bool) -> io::Result<u32> {
    let mut count = 0;
    checked(unsafe {
        if ansi {
            WriteConsoleInputA(input, events.as_ptr(), events.len() as u32, &mut count)
        } else {
            WriteConsoleInputW(input, events.as_ptr(), events.len() as u32, &mut count)
        }
    })?;
    Ok(count)
}
fn queued(input: HANDLE) -> io::Result<u32> {
    let mut n = 0;
    checked(unsafe { GetNumberOfConsoleInputEvents(input, &mut n) })?;
    Ok(n)
}
fn records(input: HANDLE, peek: bool) -> io::Result<Vec<INPUT_RECORD>> {
    let n = queued(input)?;
    if n > 4096 {
        return Err(io::Error::other("fixture record budget"));
    }
    if n == 0 {
        return Ok(Vec::new());
    }
    let mut data = vec![INPUT_RECORD::default(); n as usize];
    let mut count = 0;
    checked(unsafe {
        if peek {
            PeekConsoleInputW(input, data.as_mut_ptr(), n, &mut count)
        } else {
            ReadConsoleInputW(input, data.as_mut_ptr(), n, &mut count)
        }
    })?;
    if count > n {
        return Err(io::Error::other("reply exceeded requested count"));
    }
    data.truncate(count as usize);
    Ok(data)
}
fn one(record: &INPUT_RECORD) -> String {
    // SAFETY: records originate in the SDK; each initialized union is selected
    // by its SDK tag, and unknown tags remain explicit instead of reinterpreted.
    unsafe {
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
            WINDOW_BUFFER_SIZE_EVENT => {
                let r = record.Event.WindowBufferSizeEvent.dwSize;
                format!("R:{},{}", r.X, r.Y)
            }
            FOCUS_EVENT => format!("F:{}", record.Event.FocusEvent.bSetFocus),
            _ => format!("X:{}", record.EventType),
        }
    }
}
fn describe(records: &[INPUT_RECORD]) -> String {
    if records.len() <= 16 {
        return records.iter().map(one).collect::<Vec<_>>().join("|");
    }
    let focus = records
        .iter()
        .filter(|r| r.EventType == FOCUS_EVENT as u16)
        .count();
    format!(
        "{}..{};focus={focus}",
        one(&records[0]),
        one(records.last().unwrap())
    )
}
fn finish(input: HANDLE, report: &mut String) -> io::Result<()> {
    let peek = records(input, true)?;
    let text = describe(&peek);
    if describe(&records(input, true)?) != text {
        return Err(io::Error::other("peek changed queue"));
    }
    report.push_str(&format!(
        "queued={},{}\npeek={text}\n",
        queued(input)?,
        unsafe { WaitForSingleObject(input, 0) }
    ));
    let read = records(input, false)?;
    report.push_str(&format!(
        "read={}\nempty={},{}\n",
        describe(&read),
        queued(input)?,
        unsafe { WaitForSingleObject(input, 0) }
    ));
    Ok(())
}
fn write_result(input: HANDLE, event: INPUT_RECORD, report: &mut String, label: &str) {
    let mut count = 999;
    unsafe { SetLastError(0) };
    let ok = unsafe { WriteConsoleInputW(input, &event, 1, &mut count) };
    report.push_str(&format!(
        "{label}={ok},{},{count},{}\n",
        unsafe { GetLastError() },
        queued(input).unwrap()
    ));
}
fn size_result(output: HANDLE, x: i16, y: i16, report: &mut String, label: &str) -> io::Result<()> {
    unsafe { SetLastError(0) };
    let ok = unsafe { SetConsoleScreenBufferSize(output, COORD { X: x, Y: y }) };
    let error = unsafe { GetLastError() };
    let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
    checked(unsafe { GetConsoleScreenBufferInfo(output, &mut info) })?;
    report.push_str(&format!(
        "{label}={ok},{error},{},{}\n",
        info.dwSize.X, info.dwSize.Y
    ));
    Ok(())
}
pub(super) fn run(mode: &str, path: &Path) -> io::Result<()> {
    let kind = mode
        .strip_prefix("input-merge-")
        .ok_or(io::ErrorKind::InvalidInput)?;
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    checked(unsafe { SetConsoleMode(input, 0) })?;
    checked(unsafe { FlushConsoleInputBuffer(input) })?;
    checked(unsafe { SetConsoleCP(if kind == "dbcs" { 932 } else { 1252 }) })?;
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    let mut writes = Vec::new();
    let batches: Vec<Vec<INPUT_RECORD>> = match kind {
        "keys" => vec![vec![a(2)], vec![key(65, 3, 30, 16, 90, 1)]],
        "batch" => vec![vec![a(1), a(1)], vec![a(4)], vec![a(1), a(1)]],
        "keyup" => vec![
            vec![a(1)],
            vec![key(65, 2, 30, 16, 65, 0)],
            vec![key(65, 3, 30, 16, 65, 0)],
        ],
        "identity" => vec![
            vec![a(1)],
            vec![key(65, 1, 31, 16, 65, 1)],
            vec![key(65, 1, 31, 17, 65, 1)],
            vec![key(66, 1, 31, 17, 65, 1)],
        ],
        "ime" => vec![
            vec![key(65, 2, 30, 0x800010, 65, 1)],
            vec![key(65, 3, 31, 0x800010, 66, 1)],
        ],
        "surrogate" => vec![
            vec![key(0xd83d, 1, 30, 16, 65, 1)],
            vec![key(0xd83d, 2, 30, 16, 65, 1)],
            vec![key(0xde00, 1, 30, 16, 65, 1)],
            vec![key(0xde00, 2, 30, 16, 65, 1)],
        ],
        "mouse" => vec![
            vec![mouse(1, 2, 1, 16, 1)],
            vec![mouse(3, 4, 2, 8, 1)],
            vec![mouse(5, 6, 2, 8, 0)],
            vec![mouse(7, 8, 2, 8, 1)],
            vec![mouse(9, 10, 3, 4, 4)],
        ],
        "resize" => vec![
            vec![resize(80, 24)],
            vec![resize(120, 40)],
            vec![resize(120, 41)],
        ],
        "focus" => vec![vec![focus()], vec![focus()], vec![focus()]],
        "barrier" => vec![vec![a(1)], vec![focus()], vec![a(1)]],
        "ansi" => vec![
            vec![key(233, 2, 30, 16, 65, 1)],
            vec![key(233, 3, 30, 16, 65, 1)],
        ],
        "max" => vec![vec![a(65534)], vec![a(1)]],
        "overflow" => vec![vec![a(65535)], vec![a(1)]],
        _ => Vec::new(),
    };
    for batch in batches {
        writes.push(put(input, &batch, kind == "ansi")?);
    }
    if kind == "dbcs" {
        writes.push(put(input, &[key(20013, 2, 30, 16, 65, 1)], false)?);
        writes.push(put(
            input,
            &[key(0x92, 3, 30, 16, 65, 1), key(0x86, 1, 31, 0, 66, 1)],
            true,
        )?);
        writes.push(put(input, &[key(0x92, 4, 30, 16, 65, 1)], true)?);
        writes.push(put(input, &[key(0x86, 1, 31, 0, 66, 1)], true)?);
    } else if kind == "capacity" {
        let mut fill = vec![focus(); 4095];
        fill.push(a(2));
        writes.push(put(input, &fill, false)?);
        write_result(input, a(3), &mut report, "merge");
        write_result(input, key(66, 1, 30, 16, 65, 1), &mut report, "other");
        write_result(input, a(65531), &mut report, "overflow");
    } else if kind == "resize-capacity" {
        checked(unsafe { SetConsoleMode(input, 8) })?;
        let mut fill = vec![focus(); 4095];
        fill.push(resize(80, 24));
        put(input, &fill, false)?;
        let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        size_result(output, 100, 40, &mut report, "resize1")?;
        size_result(output, 110, 41, &mut report, "resize2")?;
        finish(input, &mut report)?;
        put(input, &vec![focus(); 4096], false)?;
        size_result(output, 110, 41, &mut report, "same")?;
        size_result(output, 120, 42, &mut report, "reject")?;
    } else if kind == "transport" || kind == "native-key" {
        println!("input-merge-ready:");
        std::fs::write(path.with_extension("ready"), b"input")?;
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let data = records(input, true)?;
            let total: u32 = data
                .iter()
                .filter(|r| r.EventType == KEY_EVENT as u16)
                .map(|r| u32::from(unsafe { r.Event.KeyEvent.wRepeatCount }))
                .sum();
            if total >= 1 {
                std::fs::write(path.with_extension("first"), b"first-record")?;
            }
            if total >= 2 {
                break;
            }
            if Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        if kind == "transport" {
            report.push_str(&format!(
                "queued={}\npeek={}\n",
                queued(input)?,
                describe(&records(input, true)?)
            ));
            let mut text = Vec::new();
            while text.len() < 2 {
                let mut units = [0u16; 2];
                let mut count = 0;
                checked(unsafe {
                    ReadConsoleW(
                        input,
                        units.as_mut_ptr().cast(),
                        (2 - text.len()) as u32,
                        &mut count,
                        std::ptr::null(),
                    )
                })?;
                text.extend_from_slice(&units[..count as usize]);
            }
            report.push_str(&format!("text={text:?}\nempty={}\n", queued(input)?));
            return std::fs::write(path, report);
        }
    } else if !matches!(
        kind,
        "keys"
            | "batch"
            | "keyup"
            | "identity"
            | "ime"
            | "surrogate"
            | "mouse"
            | "resize"
            | "focus"
            | "barrier"
            | "ansi"
            | "max"
            | "overflow"
    ) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    if !writes.is_empty() {
        report.push_str(&format!("writes={writes:?}\n"));
    }
    finish(input, &mut report)?;
    std::fs::write(path, report)
}
