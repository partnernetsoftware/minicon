//! Actual native readers behind an unread terminal output stream.
use std::os::windows::io::AsRawHandle;
use std::sync::atomic::{AtomicU32, Ordering};
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};
use windows_sys::Win32::Foundation::{GetLastError, HANDLE, SetLastError};
use windows_sys::Win32::Storage::FileSystem::ReadFile;
use windows_sys::Win32::System::{Console::*, Threading::GetThreadIOPendingFlag};

static EVENT: AtomicU32 = AtomicU32::new(0);

pub(crate) fn raw_batch(kind: &str, report: &Path, controlled: bool) -> io::Result<()> {
    let result = raw_batch_impl(kind, report, controlled);
    if let Err(error) = &result {
        publish(report, &format!("raw-batch-error: {error:?}"))?;
    }
    result
}

fn raw_batch_impl(kind: &str, report: &Path, controlled: bool) -> io::Result<()> {
    let (api, case) = kind.split_once('-').ok_or(io::ErrorKind::InvalidInput)?;
    if !matches!(api, "file" | "wide" | "ansi") {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    check(unsafe { SetConsoleMode(input, u32::from(matches!(case, "eof" | "middle-eof"))) })?;
    check(unsafe { SetConsoleCP(65001) })?;
    if unsafe { GetConsoleCP() } != 65001 {
        return Err(io::Error::other(
            "raw fixture input code page did not change",
        ));
    }
    let wide = api == "wide";
    let (units, capacity, second) = match case {
        "ascii" | "pending" => (vec![65, 13, 66], 32, false),
        "capacity" => (vec![65, 66], 2, true),
        "mixed" => (vec![65, 66], 2, true),
        "bmp" => (vec![0x4e2d, 233, 90], if wide { 2 } else { 4 }, true),
        "tail" => (vec![0x4e2d, 88], 1, true),
        "eof" => (vec![26, 65, 66], 2, true),
        "middle-eof" => (vec![65, 26, 66], 32, false),
        "pair" => (
            vec![0x4e2d, 0xd83d, 0xde00, 90],
            if wide { 2 } else { 5 },
            true,
        ),
        "split" => (vec![65, 0xd83d], 32, true),
        _ => return Err(io::ErrorKind::InvalidInput.into()),
    };
    let mut records: Vec<_> = units.into_iter().map(|unit| raw_key(unit, 1)).collect();
    if case == "capacity" {
        records[0] = raw_key(65, 3);
    }
    if case == "mixed" {
        records[1] = raw_key(66, 2);
        let mut up = raw_key(85, 1);
        up.Event.KeyEvent.bKeyDown = 0;
        records.insert(0, up);
        records.insert(
            0,
            INPUT_RECORD {
                EventType: MOUSE_EVENT as u16,
                ..INPUT_RECORD::default()
            },
        );
        records.insert(
            3,
            INPUT_RECORD {
                EventType: FOCUS_EVENT as u16,
                ..INPUT_RECORD::default()
            },
        );
    }
    let deadline = Instant::now() + Duration::from_secs(18);
    let pending = case == "pending";
    if !pending {
        raw_write(input, &records)?;
    }
    let token = input as usize;
    let api_owned = api.to_owned();
    let reader = std::thread::spawn(move || raw_read(token as HANDLE, &api_owned, capacity));
    if pending {
        wait(deadline, || {
            let mut pending = 0;
            check(unsafe { GetThreadIOPendingFlag(reader.as_raw_handle(), &mut pending) })?;
            Ok(pending != 0)
        })?;
        if controlled {
            wait(deadline, || Ok(report.with_extension("go").exists()))?;
        }
        raw_write(input, &records)?;
    }
    wait(deadline, || Ok(reader.is_finished()))?;
    let first = reader
        .join()
        .map_err(|_| io::Error::other("raw batch reader panicked"))??;
    let mut queued = 0;
    check(unsafe { GetNumberOfConsoleInputEvents(input, &mut queued) })?;
    let mut remaining = [INPUT_RECORD::default(); 16];
    let mut count = 0;
    check(unsafe { PeekConsoleInputW(input, remaining.as_mut_ptr(), 16, &mut count) })?;
    let mut peek = Vec::new();
    for record in &remaining[..count as usize] {
        if record.EventType != KEY_EVENT as u16 {
            return Err(io::Error::other("unexpected unconsumed non-key event"));
        }
        let key = unsafe { record.Event.KeyEvent };
        peek.push([
            u32::from(key.wRepeatCount),
            u32::from(unsafe { key.uChar.UnicodeChar }),
            u32::from(key.wVirtualKeyCode),
            u32::from(key.wVirtualScanCode),
            key.dwControlKeyState,
        ]);
    }
    publish(
        &report.with_extension("ready"),
        &format!(
            "first={first:?};queued={queued};peek={peek:?};cp={}",
            unsafe { GetConsoleCP() }
        ),
    )?;
    if case == "split" {
        raw_write(input, &[raw_key(0xde00, 1), raw_key(90, 1)])?;
    }
    let second = if second {
        raw_read(input, api, 32)?
    } else {
        Vec::new()
    };
    publish(
        report,
        &format!("first={first:?};second={second:?};queued={queued};peek={peek:?}"),
    )?;
    wait(deadline, || Ok(report.with_extension("opened").exists()))
}

pub(super) fn raw_key(unit: u16, repeat: u16) -> INPUT_RECORD {
    let mut record = INPUT_RECORD {
        EventType: KEY_EVENT as u16,
        ..INPUT_RECORD::default()
    };
    record.Event.KeyEvent.bKeyDown = 1;
    record.Event.KeyEvent.wRepeatCount = repeat;
    record.Event.KeyEvent.wVirtualKeyCode = if unit < 128 { unit } else { 0 };
    record.Event.KeyEvent.wVirtualScanCode = 30;
    record.Event.KeyEvent.dwControlKeyState = 32;
    record.Event.KeyEvent.uChar.UnicodeChar = unit;
    record
}
pub(super) fn raw_write(input: HANDLE, records: &[INPUT_RECORD]) -> io::Result<()> {
    let mut count = 0;
    check(unsafe {
        WriteConsoleInputW(input, records.as_ptr(), records.len() as u32, &mut count)
    })?;
    if count as usize != records.len() {
        return Err(io::Error::other("short raw batch input write"));
    }
    Ok(())
}
pub(super) fn raw_read(input: HANDLE, api: &str, capacity: u32) -> io::Result<Vec<u16>> {
    let mut count = 0;
    let mut bytes = [0_u8; 64];
    let mut units = [0_u16; 32];
    // SAFETY: initialized arrays hold every permitted fixture capacity, and
    // the console handle remains owned until all synchronous reads return.
    unsafe {
        SetLastError(0);
        let ok = match api {
            "file" => ReadFile(
                input,
                bytes.as_mut_ptr().cast(),
                capacity,
                &mut count,
                std::ptr::null_mut(),
            ),
            "ansi" => ReadConsoleA(
                input,
                bytes.as_mut_ptr().cast(),
                capacity,
                &mut count,
                std::ptr::null_mut(),
            ),
            "wide" => ReadConsoleW(
                input,
                units.as_mut_ptr().cast(),
                capacity,
                &mut count,
                std::ptr::null_mut(),
            ),
            _ => return Err(io::ErrorKind::InvalidInput.into()),
        };
        check(ok)?;
        if GetLastError() != 0 || count > capacity {
            return Err(io::Error::other("invalid raw batch success reply"));
        }
    }
    Ok(if api == "wide" {
        units[..count as usize].to_vec()
    } else {
        bytes[..count as usize]
            .iter()
            .copied()
            .map(u16::from)
            .collect()
    })
}

/// Change LINE_INPUT after a native text request has become a waiting read.
pub(crate) fn mode_wait(kind: &str, report: &Path, controlled: bool) -> io::Result<()> {
    let (api, action) = kind.split_once('-').ok_or(io::ErrorKind::InvalidInput)?;
    let file = match api {
        "file" => true,
        "console" => false,
        _ => return Err(io::ErrorKind::InvalidInput.into()),
    };
    let cooked = action.starts_with("cooked-");
    if !matches!(
        action,
        "cooked-text" | "raw-text" | "cooked-signal" | "raw-signal"
    ) {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let deadline = Instant::now() + Duration::from_secs(18);
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    // SAFETY: this fixture's private console and lifetime-long callback.
    unsafe {
        check(SetConsoleMode(input, if cooked { 3 } else { 1 }))?;
        check(SetConsoleCtrlHandler(None, 0))?;
        check(SetConsoleCtrlHandler(Some(handler), 1))?;
    }
    let token = input as usize;
    let reader = std::thread::spawn(move || {
        let mut count = 0;
        // SAFETY: the original handle remains live until the worker joins;
        // API output pointers refer to bounded initialized arrays.
        unsafe {
            let input = token as HANDLE;
            SetLastError(0);
            if file {
                let mut bytes = [0_u8; 32];
                let ok = ReadFile(
                    input,
                    bytes.as_mut_ptr().cast(),
                    32,
                    &mut count,
                    std::ptr::null_mut(),
                ) != 0;
                let error = GetLastError();
                (
                    ok,
                    error,
                    bytes[..count as usize]
                        .iter()
                        .map(|b| u16::from(*b))
                        .collect::<Vec<_>>(),
                )
            } else {
                let mut units = [0_u16; 32];
                let ok = ReadConsoleW(
                    input,
                    units.as_mut_ptr().cast(),
                    32,
                    &mut count,
                    std::ptr::null_mut(),
                ) != 0;
                let error = GetLastError();
                (ok, error, units[..count as usize].to_vec())
            }
        }
    });
    wait(deadline, || {
        let mut pending = 0;
        check(unsafe { GetThreadIOPendingFlag(reader.as_raw_handle(), &mut pending) })?;
        Ok(pending != 0)
    })?;
    if controlled {
        wait(deadline, || Ok(report.with_extension("go").exists()))?;
    }
    // The independent direct court has admitted the read before this change.
    // The PTY court leaves output free and submits the mode API after pending I/O.
    let changed = if cooked { 1 } else { 3 };
    check(unsafe { SetConsoleMode(input, changed) })?;
    publish(&report.with_extension("ready"), "mode-changed")?;
    if action.ends_with("signal") {
        wait(deadline, || Ok(EVENT.load(Ordering::Acquire) == 1))?;
    }
    if !action.ends_with("signal") || !cooked {
        let mut records = [INPUT_RECORD::default(); 2];
        for (record, unit) in records.iter_mut().zip([90_u16, 13]) {
            record.EventType = KEY_EVENT as u16;
            record.Event.KeyEvent.bKeyDown = 1;
            record.Event.KeyEvent.wRepeatCount = 1;
            record.Event.KeyEvent.uChar.UnicodeChar = unit;
        }
        // Text mode switches also exercise raw batching. The signal case
        // keeps its single later unit as an independent cancellation check.
        let records = &records[..if cooked || action == "raw-text" { 2 } else { 1 }];
        let mut written = 0;
        check(unsafe {
            WriteConsoleInputW(input, records.as_ptr(), records.len() as u32, &mut written)
        })?;
        if written as usize != records.len() {
            return Err(io::Error::other("short mode-wait input write"));
        }
    }
    wait(deadline, || Ok(reader.is_finished()))?;
    let (ok, error, units) = reader
        .join()
        .map_err(|_| io::Error::other("mode-wait reader panicked"))?;
    let mut mode = 0;
    check(unsafe { GetConsoleMode(input, &mut mode) })?;
    publish(
        report,
        &format!(
            "{{\"ok\":{ok},\"error\":{error},\"units\":{units:?},\"event\":{},\"mode\":{mode}}}",
            EVENT.load(Ordering::Acquire)
        ),
    )?;
    wait(deadline, || Ok(report.with_extension("opened").exists()))
}
unsafe extern "system" fn handler(event: u32) -> i32 {
    if event > 1 {
        return 0;
    }
    EVENT.store(event + 1, Ordering::Release);
    1
}
pub(super) fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
pub(super) fn wait(deadline: Instant, condition: impl Fn() -> io::Result<bool>) -> io::Result<()> {
    while !condition()? {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
pub(super) fn publish(path: &Path, text: &str) -> io::Result<()> {
    let temporary = path.with_extension("queue-tmp");
    std::fs::write(&temporary, text)?;
    std::fs::rename(temporary, path)
}
fn write(output: HANDLE, units: &[u16]) -> io::Result<()> {
    let mut count = 0;
    // SAFETY: initialized UTF-16 and live private console output.
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
        return Err(io::Error::other("short queued fixture write"));
    }
    Ok(())
}
#[derive(Clone, Copy)]
enum Api {
    File,
    Console,
    Records,
}
fn read(input: HANDLE, api: Api) -> (bool, u32, u32, u16) {
    let mut count = 0;
    let mut unit = 0;
    // SAFETY: every API receives a bounded live output and count buffer. The
    // console remains privately owned until this synchronous caller returns.
    unsafe {
        SetLastError(0);
        let ok = match api {
            Api::File => {
                let mut bytes = [0_u8; 32];
                let result = ReadFile(
                    input,
                    bytes.as_mut_ptr().cast(),
                    bytes.len() as u32,
                    &mut count,
                    std::ptr::null_mut(),
                );
                if count != 0 {
                    unit = u16::from(bytes[0]);
                }
                result
            }
            Api::Console => {
                let mut units = [0_u16; 32];
                ReadConsoleW(
                    input,
                    units.as_mut_ptr().cast(),
                    units.len() as u32,
                    &mut count,
                    std::ptr::null_mut(),
                )
            }
            Api::Records => {
                let mut records = [INPUT_RECORD::default(); 8];
                let result = ReadConsoleInputW(
                    input,
                    records.as_mut_ptr(),
                    records.len() as u32,
                    &mut count,
                );
                if count != 0 && records[0].EventType == KEY_EVENT as u16 {
                    unit = records[0].Event.KeyEvent.uChar.UnicodeChar;
                }
                result
            }
        } != 0;
        (ok, GetLastError(), count, unit)
    }
}

pub(crate) fn run(kind: &str, report: &Path, controlled: bool) -> io::Result<()> {
    let (action, api) = kind.split_once('-').ok_or(io::ErrorKind::InvalidInput)?;
    let api = match api {
        "file" => Api::File,
        "console" => Api::Console,
        "records" => Api::Records,
        _ => return Err(io::ErrorKind::InvalidInput.into()),
    };
    let deadline = Instant::now() + Duration::from_secs(18);
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    let mode = match api {
        Api::Console => 7,
        Api::File => 1,
        Api::Records => 0,
    };
    unsafe {
        check(SetConsoleMode(input, mode))?;
        check(SetConsoleMode(output, 15))?;
        // The runner can inherit IGNORE_CTRL_C; installing a callback alone
        // does not clear that independently inherited setting.
        check(SetConsoleCtrlHandler(None, 0))?;
        check(SetConsoleCtrlHandler(Some(handler), 1))?;
    }
    // Keep the endpoint alive while the detached root reports through files.
    let keeper = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open("CONOUT$")?;
    let mut peer = if action == "other" {
        write(output, &[66, 62])?;
        let child = std::process::Command::new(std::env::current_exe()?)
            .arg("cooked-retire-peer")
            .arg(report.with_extension("peer"))
            .spawn()?;
        wait(deadline, || {
            Ok(report.with_extension("peer-ready").exists())
        })?;
        Some(child)
    } else {
        None
    };
    let writer = if !controlled {
        let output = output as usize;
        let path = report.to_owned();
        Some(std::thread::spawn(move || -> io::Result<()> {
            let units = [97_u16, 0x4e2d, 0xd83d, 0xde00].repeat(1024);
            for index in 0..32 {
                write(output as HANDLE, &units)?;
                if index == 7 {
                    publish(&path.with_extension("first"), "65536")?;
                }
            }
            Ok(())
        }))
    } else {
        None
    };
    if !controlled {
        wait(deadline, || Ok(report.with_extension("first").exists()))?;
    }
    let token = input as usize;
    let reader = std::thread::spawn(move || read(token as HANDLE, api));
    if controlled {
        wait(deadline, || Ok(report.with_extension("go").exists()))?;
    } else {
        wait(deadline, || {
            let mut pending = 0;
            check(unsafe { GetThreadIOPendingFlag(reader.as_raw_handle(), &mut pending) })?;
            Ok(pending != 0)
        })?;
    }
    if action == "other" {
        publish(&report.with_extension("peer-go"), "read")?;
        wait(deadline, || Ok(report.with_extension("go-two").exists()))?;
    }
    publish(&report.with_extension("ready"), "read-submitted")?;
    if matches!(action, "disconnect" | "other") {
        unsafe {
            check(FreeConsole())?;
        }
    }
    let keyboard_records =
        action == "signal" || matches!(api, Api::Records) && !controlled && action == "interrupt";
    if keyboard_records {
        wait(deadline, || Ok(EVENT.load(Ordering::Acquire) != 0))?;
        if reader.is_finished() {
            return Err(io::Error::other("read must survive this keyboard control"));
        }
        publish(&report.with_extension("signal"), "read-pending")?;
        wait(deadline, || Ok(report.with_extension("records").exists()))?;
        if !controlled {
            let input = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open("CONIN$")?;
            let mut record = INPUT_RECORD {
                EventType: KEY_EVENT as u16,
                ..INPUT_RECORD::default()
            };
            record.Event.KeyEvent.bKeyDown = 1;
            record.Event.KeyEvent.wRepeatCount = 1;
            record.Event.KeyEvent.uChar.UnicodeChar = 90;
            let mut written = 0;
            check(unsafe { WriteConsoleInputW(input.as_raw_handle(), &record, 1, &mut written) })?;
            if written != 1 {
                return Err(io::Error::other("short record wakeup"));
            }
        }
    }
    wait(deadline, || Ok(reader.is_finished()))?;
    let (ok, error, count, unit) = reader
        .join()
        .map_err(|_| io::Error::other("queued reader panicked"))?;
    if !controlled && action == "interrupt" {
        wait(deadline, || Ok(EVENT.load(Ordering::Acquire) != 0))?;
    }
    let mut after = 0;
    if matches!(action, "interrupt" | "signal") {
        unsafe {
            check(GetConsoleMode(input, &mut after))?;
        }
    }
    let secondary = if let Some(ref mut peer) = peer {
        if !peer.wait()?.success() {
            return Err(io::Error::other("queued peer failed"));
        }
        std::fs::read_to_string(report.with_extension("peer"))?
    } else {
        "[]".to_owned()
    };
    let blocked = writer.as_ref().is_some_and(|thread| !thread.is_finished());
    publish(
        report,
        &format!(
            "{{\"ok\":{ok},\"error\":{error},\"count\":{count},\"unit\":{unit},\"event\":{},\"mode\":{after},\"secondary\":{secondary},\"writer_pending\":{blocked}}}",
            EVENT.load(Ordering::Acquire)
        ),
    )?;
    wait(deadline, || Ok(report.with_extension("opened").exists()))?;
    if let Some(writer) = writer {
        let result = writer
            .join()
            .map_err(|_| io::Error::other("queued writer panicked"))?;
        if matches!(action, "interrupt" | "signal") {
            result?;
        }
    }
    drop(keeper);
    Ok(())
}
