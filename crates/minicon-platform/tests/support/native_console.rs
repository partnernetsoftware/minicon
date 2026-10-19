//! Independent Win32 caller for the platform PTY integration court.
#[cfg(windows)]
mod console_file;
#[cfg(windows)]
mod control;
#[cfg(windows)]
mod echo_output;
#[cfg(windows)]
mod inheritance;
#[cfg(windows)]
mod mixed_output;
#[cfg(windows)]
mod output_modes;
#[cfg(windows)]
mod screen_fill;
#[cfg(windows)]
mod screen_readback;
#[cfg(windows)]
mod text_attributes;
#[cfg(windows)]
mod unicode_output;
#[cfg(not(windows))]
fn main() {
    std::process::exit(2);
}

#[cfg(windows)]
fn main() {
    if let Err(error) = run() {
        eprintln!("native PTY probe: {error}");
        std::process::exit(2);
    }
}

#[cfg(windows)]
fn run() -> std::io::Result<()> {
    use windows_sys::Win32::System::Console::{
        CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetNumberOfConsoleInputEvents,
        GetStdHandle, INPUT_RECORD, ReadConsoleInputW, ReadConsoleW, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetConsoleMode, WriteConsoleInputW,
    };
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    let mode = arguments[0].to_str().unwrap_or("");
    if mode == "unicode-output" || mode == "unicode-output-owned" {
        return unicode_output::run(
            std::path::Path::new(&arguments[1]),
            mode.ends_with("-owned"),
        );
    }
    if mode == "echo-output" || mode == "echo-output-owned" {
        return echo_output::run(
            std::path::Path::new(&arguments[1]),
            mode.ends_with("-owned"),
        );
    }
    if mode == "mixed-output-tail-csi" || mode == "mixed-output-tail-utf8" {
        return mixed_output::tail(std::path::Path::new(&arguments[1]), mode.ends_with("utf8"));
    }
    if mode == "mixed-output" || mode == "mixed-output-budget" {
        return mixed_output::run(
            std::path::Path::new(&arguments[1]),
            mode.ends_with("-budget"),
        );
    }
    if mode == "screen-fill" {
        return screen_fill::run(std::path::Path::new(&arguments[1]));
    }
    if matches!(
        mode,
        "screen-readback"
            | "screen-readback-complex"
            | "screen-readback-owned"
            | "screen-readback-complex-owned"
    ) {
        return screen_readback::run(
            std::path::Path::new(&arguments[1]),
            mode.contains("-complex"),
            mode.ends_with("-owned"),
        );
    }
    if mode == "text-attributes" || mode == "text-attributes-owned" {
        return text_attributes::run(
            std::path::Path::new(&arguments[1]),
            mode.ends_with("-owned"),
        );
    }
    if mode == "output-wide-margin" {
        return output_modes::wide_margin(std::path::Path::new(&arguments[1]));
    }
    if let Some(mode) = mode.strip_prefix("output-modes-") {
        return output_modes::run(
            mode.parse()
                .map_err(|_| std::io::Error::from(std::io::ErrorKind::InvalidInput))?,
            std::path::Path::new(&arguments[1]),
        );
    }
    if mode == "output-orphan-member" {
        let report = std::path::Path::new(&arguments[1]);
        std::fs::write(report.with_extension("orphan-ready"), b"live")?;
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(8);
        while !report.with_extension("opened").exists() {
            if std::time::Instant::now() >= deadline {
                return Err(std::io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
        return std::fs::write(report.with_extension("orphan"), b"finished");
    }
    if mode.starts_with("control-") {
        return control::run(mode, std::path::Path::new(&arguments[1]));
    }
    if mode.starts_with("inherit-") {
        return inheritance::run(mode, std::path::Path::new(&arguments[1]));
    }
    if mode == "objects" {
        return std::fs::write(&arguments[1], objects()?);
    }
    // SAFETY: only this child's native console handles and initialized SDK
    // structures with matching buffer bounds; no host console attachment.
    unsafe {
        let input = GetStdHandle(STD_INPUT_HANDLE);
        let output = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut before = CONSOLE_SCREEN_BUFFER_INFO::default();
        let input_mode = if matches!(mode, "vt-key" | "vt-text-key") {
            0x200
        } else {
            0x98
        };
        if GetConsoleScreenBufferInfo(output, &mut before) == 0
            || SetConsoleMode(input, input_mode) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        println!("native-pty-probe-ready:");
        if mode == "sleep" {
            std::thread::sleep(std::time::Duration::from_secs(60));
            return Ok(());
        }
        let mut records = [INPUT_RECORD::default(); 2];
        let mut count = 0;
        if mode == "vt-text-key" {
            let mut units = [0_u16; 3];
            let mut total = 0;
            while total < units.len() {
                let mut read = 0;
                if ReadConsoleW(
                    input,
                    units[total..].as_mut_ptr().cast(),
                    (units.len() - total) as u32,
                    &mut read,
                    std::ptr::null(),
                ) == 0
                    || read == 0
                {
                    return Err(std::io::Error::last_os_error());
                }
                total += read as usize;
            }
            return std::fs::write(
                &arguments[1],
                format!("{},{},{}", units[0], units[1], units[2]),
            );
        }
        let mut written = 0;
        let mut queued = 0;
        if mode == "write-records" {
            records[0].EventType = 1;
            let key = &mut records[0].Event.KeyEvent;
            key.bKeyDown = 1;
            key.wRepeatCount = 3;
            key.wVirtualKeyCode = 65;
            key.wVirtualScanCode = 30;
            key.uChar.UnicodeChar = 0x4e2d;
            key.dwControlKeyState = 16;
            records[1].EventType = 16;
            records[1].Event.FocusEvent.bSetFocus = 0;
            if WriteConsoleInputW(input, records.as_ptr(), records.len() as u32, &mut written) == 0
                || GetNumberOfConsoleInputEvents(input, &mut queued) == 0
            {
                return Err(std::io::Error::last_os_error());
            }
        }
        if ReadConsoleInputW(
            input,
            records.as_mut_ptr(),
            records.len() as u32,
            &mut count,
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let report = match mode {
            "resize" => {
                if count != 1 || records[0].EventType != 4 {
                    return Err(std::io::Error::other("expected one native window event"));
                }
                let event = records[0].Event.WindowBufferSizeEvent;
                let mut after = CONSOLE_SCREEN_BUFFER_INFO::default();
                if GetConsoleScreenBufferInfo(output, &mut after) == 0 {
                    return Err(std::io::Error::last_os_error());
                }
                format!(
                    "{},{},{},{},{},{}",
                    before.dwSize.X,
                    before.dwSize.Y,
                    event.dwSize.X,
                    event.dwSize.Y,
                    after.dwSize.X,
                    after.dwSize.Y
                )
            }
            "key" | "vt-key" => {
                if count != 1 || records[0].EventType != 1 {
                    return Err(std::io::Error::other("expected one native key event"));
                }
                let key = records[0].Event.KeyEvent;
                format!(
                    "{},{},{},{},{}",
                    key.bKeyDown,
                    key.wRepeatCount,
                    key.wVirtualKeyCode,
                    key.wVirtualScanCode,
                    key.dwControlKeyState
                )
            }
            "write-records" => {
                if count != 2 || records[0].EventType != 1 || records[1].EventType != 16 {
                    return Err(std::io::Error::other("native record roundtrip failed"));
                }
                let key = records[0].Event.KeyEvent;
                format!(
                    "{written},{queued},{count},{},{},{},{}",
                    key.uChar.UnicodeChar,
                    key.wRepeatCount,
                    key.dwControlKeyState,
                    records[1].Event.FocusEvent.bSetFocus
                )
            }
            _ => return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput)),
        };
        std::fs::write(&arguments[1], report)
    }
}

#[cfg(windows)]
fn objects() -> std::io::Result<String> {
    use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
    use std::ptr::{null, null_mut};
    use windows_sys::Win32::Foundation::{
        DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_READ, GENERIC_WRITE, GetLastError,
        INVALID_HANDLE_VALUE, SetLastError,
    };
    use windows_sys::Win32::Storage::FileSystem::{
        CreateFileW, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
    };
    use windows_sys::Win32::System::Console::{
        CONSOLE_CURSOR_INFO, CONSOLE_SCREEN_BUFFER_INFO, GetConsoleCursorInfo, GetConsoleMode,
        GetConsoleScreenBufferInfo, SetConsoleMode, WriteConsoleW,
    };
    use windows_sys::Win32::System::Threading::GetCurrentProcess;
    fn open(name: &str, access: u32, share: u32) -> std::io::Result<OwnedHandle> {
        let path: Vec<u16> = name.encode_utf16().chain([0]).collect();
        // SAFETY: bounded, terminated local console name; successful handle is owned.
        let handle = unsafe {
            CreateFileW(
                path.as_ptr(),
                access,
                share,
                null(),
                OPEN_EXISTING,
                0,
                null_mut(),
            )
        };
        if handle == INVALID_HANDLE_VALUE {
            return Err(std::io::Error::last_os_error());
        }
        Ok(unsafe { OwnedHandle::from_raw_handle(handle) })
    }
    let share = FILE_SHARE_READ | FILE_SHARE_WRITE;
    let reader = open("CONOUT$", GENERIC_READ, share)?;
    let writer = open("CONOUT$", GENERIC_WRITE, share)?;
    let mut fields = Vec::new();
    macro_rules! record {
        ($operation:expr) => {{
            SetLastError(0);
            let ok = $operation;
            fields.push(u32::from(ok != 0));
            fields.push(if ok == 0 { GetLastError() } else { 0 });
        }};
    }
    // SAFETY: every operation uses a live, independently owned local handle
    // and initialized writable SDK storage. No union padding is inspected.
    unsafe {
        let mut mode = 0;
        record!(GetConsoleMode(reader.as_raw_handle(), &mut mode));
        record!(SetConsoleMode(reader.as_raw_handle(), mode));
        record!(GetConsoleMode(writer.as_raw_handle(), &mut mode));
        let mut info = CONSOLE_SCREEN_BUFFER_INFO::default();
        record!(GetConsoleScreenBufferInfo(
            reader.as_raw_handle(),
            &mut info
        ));
        record!(GetConsoleScreenBufferInfo(
            writer.as_raw_handle(),
            &mut info
        ));
        let mut count = 0;
        record!(WriteConsoleW(
            reader.as_raw_handle(),
            [65_u16].as_ptr().cast(),
            1,
            &mut count,
            null()
        ));
        record!(WriteConsoleW(
            writer.as_raw_handle(),
            [66_u16].as_ptr().cast(),
            1,
            &mut count,
            null()
        ));
        let mut cursor = CONSOLE_CURSOR_INFO::default();
        record!(GetConsoleCursorInfo(reader.as_raw_handle(), &mut cursor));
        record!(GetConsoleCursorInfo(writer.as_raw_handle(), &mut cursor));
        let mut duplicate = null_mut();
        if DuplicateHandle(
            GetCurrentProcess(),
            reader.as_raw_handle(),
            GetCurrentProcess(),
            &mut duplicate,
            0,
            0,
            DUPLICATE_SAME_ACCESS,
        ) == 0
        {
            return Err(std::io::Error::last_os_error());
        }
        let duplicate = OwnedHandle::from_raw_handle(duplicate);
        drop(reader);
        record!(GetConsoleMode(duplicate.as_raw_handle(), &mut mode));
        drop(duplicate);
    }
    match open("CONOUT$", GENERIC_READ, 0) {
        Ok(handle) => {
            fields.extend([1, 0]);
            drop(handle);
        }
        Err(error) => fields.extend([0, error.raw_os_error().unwrap_or(0) as u32]),
    }
    let generic = open("CON", GENERIC_READ, share)?;
    let mut input_mode = 0;
    unsafe {
        record!(GetConsoleMode(generic.as_raw_handle(), &mut input_mode));
    }
    drop(generic);
    // More than the host's total simultaneous-handle budget, one at a time:
    // a missed close must become a native create failure rather than a leak.
    for _ in 0..1050 {
        drop(open("CONOUT$", GENERIC_READ, share)?);
    }
    Ok(fields
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(","))
}
