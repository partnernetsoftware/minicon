//! Private child-family fixture. Results use files outside the tested console.
use super::console_file::open as open_console;
use std::io;
use std::io::Read;
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};
use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
use windows_sys::Win32::System::Console::{
    FreeConsole, GetConsoleMode, GetStdHandle, ReadConsoleW, STD_INPUT_HANDLE, STD_OUTPUT_HANDLE,
    WriteConsoleW,
};

fn wait(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(io::Error::from(io::ErrorKind::TimedOut));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}

pub(super) fn run(mode: &str, report: &Path) -> io::Result<()> {
    if mode == "inherit-close-tail" {
        return close_tail(report);
    }
    if matches!(
        mode,
        "inherit-parent-exit"
            | "inherit-parent-free"
            | "inherit-parent-readonly"
            | "inherit-parent-tail"
    ) {
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg(if mode == "inherit-parent-readonly" {
                "inherit-child-readonly"
            } else if mode == "inherit-parent-tail" {
                "inherit-child-tail"
            } else {
                "inherit-child"
            })
            .arg(report)
            .stdin(Stdio::inherit())
            .stderr(Stdio::inherit());
        if mode == "inherit-parent-readonly" {
            command.stdout(Stdio::from(open_console("CONOUT$", 0x8000_0000)?));
        } else {
            command.stdout(Stdio::inherit());
        }
        let mut partial = if mode == "inherit-parent-tail" {
            let file = open_console("CONIN$", 0xc000_0000)?;
            command.stdin(Stdio::from(file.try_clone()?));
            Some(file)
        } else {
            None
        };
        let _child = command.spawn()?;
        wait(&report.with_extension("ready"))?;
        println!("inherit-parent-ready:");
        if let Some(file) = partial.as_mut() {
            use std::os::windows::io::AsRawHandle;
            use windows_sys::Win32::System::Console::{SetConsoleCP, SetConsoleMode};
            // Configure only this fixture's own console before its partial read.
            if unsafe { SetConsoleCP(65001) } == 0
                || unsafe { SetConsoleMode(file.as_raw_handle(), 0) } == 0
            {
                return Err(io::Error::last_os_error());
            }
            println!("inherit-tail-send:");
            let mut first = [0];
            file.read_exact(&mut first)?;
            std::fs::write(report.with_extension("first"), first[0].to_string())?;
        }
        if mode == "inherit-parent-free" {
            // Detach only this controlled parent's own console.
            if unsafe { FreeConsole() } == 0 {
                return Err(io::Error::last_os_error());
            }
            std::fs::write(report.with_extension("detached"), b"detached")?;
        }
        return Ok(());
    }
    if !matches!(
        mode,
        "inherit-child" | "inherit-child-readonly" | "inherit-child-tail"
    ) {
        return Err(io::Error::from(io::ErrorKind::InvalidInput));
    }
    std::fs::write(report.with_extension("ready"), b"connected")?;
    wait(&report.with_extension("go"))?;
    // SAFETY: operations use this controlled child's actual inherited handles;
    // buffers are local and lengths match their allocations.
    unsafe {
        let input = GetStdHandle(STD_INPUT_HANDLE);
        let output = GetStdHandle(STD_OUTPUT_HANDLE);
        let mut native_mode = 0;
        SetLastError(0);
        let input_ok = GetConsoleMode(input, &mut native_mode) != 0;
        let input_error = if input_ok { 0 } else { GetLastError() };
        SetLastError(0);
        let output_ok = GetConsoleMode(output, &mut native_mode) != 0;
        let output_error = if output_ok { 0 } else { GetLastError() };
        std::fs::write(
            report.with_extension("opened"),
            format!("{input_ok},{input_error},{output_ok},{output_error}"),
        )?;
        std::fs::write(report.with_extension("opened-ready"), b"published")?;
        if !input_ok || !output_ok {
            std::fs::write(
                report,
                format!("query-failed:{input_ok},{input_error},{output_ok},{output_error}"),
            )?;
            return Err(io::Error::other("inherited console handle rejected"));
        }
        if mode == "inherit-child-tail" {
            use windows_sys::Win32::Storage::FileSystem::ReadFile;
            let mut bytes = [0_u8; 2];
            let mut total = 0;
            while total < bytes.len() {
                let mut count = 0;
                if ReadFile(
                    input,
                    bytes[total..].as_mut_ptr(),
                    (bytes.len() - total) as u32,
                    &mut count,
                    std::ptr::null_mut(),
                ) == 0
                    || count == 0
                {
                    return Err(io::Error::last_os_error());
                }
                total += count as usize;
            }
            std::fs::write(report, format!("{},{}", bytes[0], bytes[1]))?;
            return Ok(());
        }
        if mode == "inherit-child-readonly" {
            let units: Vec<u16> = "forbidden-write".encode_utf16().collect();
            let mut count = 0;
            SetLastError(0);
            let ok = WriteConsoleW(
                output,
                units.as_ptr().cast(),
                units.len() as u32,
                &mut count,
                std::ptr::null(),
            ) != 0;
            let error = if ok { 0 } else { GetLastError() };
            std::fs::write(report, format!("{ok},{error},{count}"))?;
            return Ok(());
        }
        let prompt: Vec<u16> = "inherit-child-ready:".encode_utf16().collect();
        let mut written = 0;
        if WriteConsoleW(
            output,
            prompt.as_ptr().cast(),
            prompt.len() as u32,
            &mut written,
            std::ptr::null(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        let mut units = [0_u16; 32];
        let mut count = 0;
        if ReadConsoleW(
            input,
            units.as_mut_ptr().cast(),
            units.len() as u32,
            &mut count,
            std::ptr::null(),
        ) == 0
        {
            return Err(io::Error::last_os_error());
        }
        if count as usize > units.len() {
            return Err(io::Error::from(io::ErrorKind::InvalidData));
        }
        let text = String::from_utf16(&units[..count as usize]).map_err(io::Error::other)?;
        let reply: Vec<u16> = format!("inherit-value:{text}").encode_utf16().collect();
        let mut reply_written = 0;
        let ok = WriteConsoleW(
            output,
            reply.as_ptr().cast(),
            reply.len() as u32,
            &mut reply_written,
            std::ptr::null(),
        ) != 0;
        let error = if ok { 0 } else { GetLastError() };
        std::fs::write(
            report,
            format!("{ok},{error},{count},{reply_written},{text}"),
        )?;
        if !ok {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

fn close_tail(report: &Path) -> io::Result<()> {
    use std::os::windows::io::AsRawHandle;
    use windows_sys::Win32::Foundation::WAIT_TIMEOUT;
    use windows_sys::Win32::System::Console::{
        GetNumberOfConsoleInputEvents, SetConsoleCP, SetConsoleMode,
    };
    use windows_sys::Win32::System::Threading::WaitForSingleObject;
    let mut file = open_console("CONIN$", 0xc000_0000)?;
    // SAFETY: private fixture console and live SDK handles; no foreign attach.
    if unsafe { SetConsoleCP(65001) } == 0
        || unsafe { SetConsoleMode(file.as_raw_handle(), 0) } == 0
    {
        return Err(io::Error::last_os_error());
    }
    println!("inherit-tail-send:");
    let mut first = [0];
    file.read_exact(&mut first)?;
    drop(file);
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut queued = 0;
    let observed = loop {
        if unsafe { GetNumberOfConsoleInputEvents(input, &mut queued) } == 0 {
            return Err(io::Error::last_os_error());
        }
        let event = unsafe { WaitForSingleObject(input, 0) };
        if event == WAIT_TIMEOUT || Instant::now() >= deadline {
            break event;
        }
        std::thread::sleep(Duration::from_millis(2));
    };
    std::fs::write(report, format!("{},{observed},{queued}", first[0]))
}
