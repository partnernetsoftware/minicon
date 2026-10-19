//! Actual Win32 control-handler receipts, independent of the console output.
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
use windows_sys::Win32::System::Console::{
    GetStdHandle, ReadConsoleW, STD_INPUT_HANDLE, SetConsoleCtrlHandler, SetConsoleMode,
};

static EVENT: AtomicU32 = AtomicU32::new(0);

unsafe extern "system" fn handler(event: u32) -> i32 {
    if event == 0 {
        EVENT.store(event + 1, Ordering::Release);
        1
    } else {
        0
    }
}

pub(super) fn run(mode: &str, report: &Path) -> io::Result<()> {
    let raw = mode == "control-raw";
    let reading = raw || mode == "control-read";
    // SAFETY: this controlled child's own console; callback touches only an
    // atomic and remains valid for the entire process lifetime.
    unsafe {
        // The spawning test runner may inherit IGNORE_CTRL_C from its shell.
        // Installing a handler does not clear that independently inherited flag.
        if SetConsoleCtrlHandler(None, 0) == 0 {
            return Err(io::Error::last_os_error());
        }
        if mode != "control-default" && SetConsoleCtrlHandler(Some(handler), 1) == 0 {
            return Err(io::Error::last_os_error());
        }
        let input = GetStdHandle(STD_INPUT_HANDLE);
        if SetConsoleMode(input, if raw { 0 } else { 7 }) == 0 {
            return Err(io::Error::last_os_error());
        }
        println!("control-probe-ready:");
        let mut result = String::new();
        if reading {
            let mut units = [0_u16; 16];
            let mut count = 0;
            SetLastError(0);
            let ok = ReadConsoleW(
                input,
                units.as_mut_ptr().cast(),
                units.len() as u32,
                &mut count,
                std::ptr::null(),
            ) != 0;
            // Interrupted ReadConsoleW succeeds with LastError=995.
            let error = GetLastError();
            result = format!("{ok},{error},{count},{}", units[0]);
            if raw {
                return std::fs::write(
                    report,
                    format!("{result}|{}", EVENT.load(Ordering::Acquire)),
                );
            }
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        while EVENT.load(Ordering::Acquire) == 0 {
            if Instant::now() >= deadline {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "native control handler did not execute",
                ));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        std::fs::write(
            report,
            format!("{result}|{}", EVENT.load(Ordering::Acquire)),
        )?;
    }
    Ok(())
}
