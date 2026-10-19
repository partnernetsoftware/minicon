//! Actual Win32 control-handler receipts, independent of the console output.
use std::io;
use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
use windows_sys::Win32::System::Console::{
    GenerateConsoleCtrlEvent, GetConsoleMode, GetNumberOfConsoleInputEvents, GetStdHandle,
    ReadConsoleW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, SetConsoleCtrlHandler, SetConsoleMode,
    WriteConsoleW,
};

static EVENT: AtomicU32 = AtomicU32::new(0);
static HANDLER_IO: AtomicBool = AtomicBool::new(false);

unsafe extern "system" fn handler(event: u32) -> i32 {
    if event <= 1 {
        if HANDLER_IO.load(Ordering::Acquire) {
            // Deliberately issue synchronous API requests from the real native
            // callback thread. No stdio locks, allocation or file reporting.
            let mut mode = 0;
            let message = b"handler-native-write:";
            let mut units = [0_u16; 32];
            for (target, byte) in units.iter_mut().zip(message) {
                *target = u16::from(*byte);
            }
            let mut count = 0;
            let ok = unsafe {
                GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut mode) != 0
                    && WriteConsoleW(
                        GetStdHandle(STD_OUTPUT_HANDLE),
                        units.as_ptr().cast(),
                        message.len() as u32,
                        &mut count,
                        std::ptr::null(),
                    ) != 0
            };
            if !ok || mode != 7 || count != message.len() as u32 {
                EVENT.store(99, Ordering::Release);
                return 1;
            }
        }
        EVENT.store(event + 1, Ordering::Release);
        1
    } else {
        0
    }
}

pub(super) fn run(mode: &str, report: &Path) -> io::Result<()> {
    let raw = mode == "control-raw";
    let reading = raw || mode == "control-read";
    HANDLER_IO.store(mode == "control-handler-io", Ordering::Release);
    // SAFETY: this controlled child's own console; the callback remains valid
    // for the process lifetime and uses fixed buffers for its optional API I/O.
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
        if SetConsoleMode(
            input,
            if raw || mode.starts_with("control-generate-") {
                0
            } else {
                7
            },
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        println!("control-probe-ready:");
        let mut result = String::new();
        if mode == "control-group-parent" {
            return group_parent(report);
        }
        if mode == "control-group-child" {
            std::fs::write(report.with_extension("ready"), b"connected")?;
        }
        if mode == "control-generate-invalid" {
            SetLastError(0);
            let invalid = GenerateConsoleCtrlEvent(2, 0) != 0;
            let invalid_error = GetLastError();
            SetLastError(0);
            let unknown = GenerateConsoleCtrlEvent(1, u32::MAX) != 0;
            let unknown_error = GetLastError();
            return std::fs::write(
                report,
                format!(
                    "{invalid},{invalid_error}|{unknown},{unknown_error}|{}",
                    EVENT.load(Ordering::Acquire)
                ),
            );
        }
        if matches!(mode, "control-generate-c" | "control-generate-break") {
            SetLastError(0);
            let ok = GenerateConsoleCtrlEvent(u32::from(mode == "control-generate-break"), 0) != 0;
            let error = GetLastError();
            result = format!("{ok},{error}");
            if !ok {
                return std::fs::write(report, format!("{result}|0"));
            }
        }
        if mode == "control-full-queue" {
            let deadline = Instant::now() + Duration::from_secs(8);
            let mut queued = 0;
            loop {
                if GetNumberOfConsoleInputEvents(input, &mut queued) == 0 {
                    return Err(io::Error::last_os_error());
                }
                if queued == 4096 {
                    break;
                }
                if Instant::now() >= deadline {
                    return Err(io::Error::new(
                        io::ErrorKind::TimedOut,
                        "queue never filled",
                    ));
                }
                std::thread::sleep(Duration::from_millis(2));
            }
            std::fs::write(report.with_extension("ready"), queued.to_string())?;
            result = queued.to_string();
        }
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
        if mode == "control-full-queue" {
            let mut remaining = 0;
            if GetNumberOfConsoleInputEvents(input, &mut remaining) == 0 {
                return Err(io::Error::last_os_error());
            }
            result.push_str(&format!(",{remaining}"));
        }
        std::fs::write(
            report,
            format!("{result}|{}", EVENT.load(Ordering::Acquire)),
        )?;
    }
    Ok(())
}

fn group_parent(report: &Path) -> io::Result<()> {
    use std::os::windows::process::CommandExt;
    use std::process::{Command, Stdio};
    use windows_sys::Win32::System::Threading::CREATE_NEW_PROCESS_GROUP;
    let child_report = report.with_extension("child");
    let mut child = Command::new(std::env::current_exe()?)
        .arg("control-group-child")
        .arg(&child_report)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .creation_flags(CREATE_NEW_PROCESS_GROUP)
        .spawn()?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while !report.with_extension("ready").exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // SAFETY: target group is the actual child just created in our console;
    // no host-console broadcast or PID enumeration is involved.
    let (ok, error) = unsafe {
        SetLastError(0);
        let ok = GenerateConsoleCtrlEvent(1, child.id()) != 0;
        (ok, GetLastError())
    };
    if !ok {
        return Err(io::Error::from_raw_os_error(error as i32));
    }
    let status = loop {
        if let Some(status) = child.try_wait()? {
            break status;
        }
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    if !status.success() {
        return Err(io::Error::other("group child failed"));
    }
    std::fs::write(
        report,
        format!(
            "{ok},{error}|child:{}|parent:{}",
            std::fs::read_to_string(child_report)?,
            EVENT.load(Ordering::Acquire)
        ),
    )
}
