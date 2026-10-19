//! Independent Win32 caller for the platform PTY integration court.
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
        CONSOLE_SCREEN_BUFFER_INFO, GetConsoleScreenBufferInfo, GetStdHandle, INPUT_RECORD,
        ReadConsoleInputW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleMode,
    };
    let arguments: Vec<_> = std::env::args_os().skip(1).collect();
    if arguments.len() != 2 {
        return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput));
    }
    let mode = arguments[0].to_str().unwrap_or("");
    // SAFETY: only this child's native console handles and initialized SDK
    // structures with matching buffer bounds; no host console attachment.
    unsafe {
        let input = GetStdHandle(STD_INPUT_HANDLE);
        let output = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut before = CONSOLE_SCREEN_BUFFER_INFO::default();
        if GetConsoleScreenBufferInfo(output, &mut before) == 0 || SetConsoleMode(input, 0x98) == 0
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
            "key" => {
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
            _ => return Err(std::io::Error::from(std::io::ErrorKind::InvalidInput)),
        };
        std::fs::write(&arguments[1], report)
    }
}
