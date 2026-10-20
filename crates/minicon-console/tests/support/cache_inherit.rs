//! Actual parent/child console clients. No host state or expected values enter here.
use super::queued_reads::{check, publish, raw_key, raw_read, raw_write, wait};
use std::{
    fs::OpenOptions,
    io,
    os::windows::io::AsRawHandle,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{FILETIME, HANDLE},
    System::{Console::*, Threading::GetProcessTimes},
};

fn creation(handle: HANDLE) -> io::Result<u64> {
    let (mut born, mut exit, mut kernel, mut user) = (
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
        FILETIME::default(),
    );
    check(unsafe { GetProcessTimes(handle, &mut born, &mut exit, &mut kernel, &mut user) })?;
    Ok(u64::from(born.dwHighDateTime) << 32 | u64::from(born.dwLowDateTime))
}
pub(crate) fn run(kind: &str, report: &Path, child: bool) -> io::Result<()> {
    let result = run_inner(kind, report, child);
    if let Err(error) = &result {
        publish(report, &format!("inherit-cache-error: {error:?}"))?;
    }
    result
}
fn run_inner(kind: &str, report: &Path, child: bool) -> io::Result<()> {
    let parts: Vec<_> = kind.split('-').collect();
    if parts.len() != 3
        || !matches!(parts[0], "wide" | "file" | "utf8")
        || !matches!(parts[1], "stdio" | "file")
        || !matches!(parts[2], "exit" | "detach")
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let (api, source, lifetime) = (parts[0], parts[1], parts[2]);
    let api = if api == "wide" { "wide" } else { "file" };
    let open = || OpenOptions::new().read(true).write(true).open("CONIN$");
    let deadline = Instant::now() + Duration::from_secs(24);
    if child {
        let inherited = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        let mut mode = 0;
        check(unsafe { GetConsoleMode(inherited, &mut mode) })?;
        publish(&report.with_extension("ready"), "child-attached")?;
        wait(deadline, || Ok(report.with_extension("go").exists()))?;
        check(unsafe { GetConsoleMode(inherited, &mut mode) })?;
        let other = open()?;
        let b = other.as_raw_handle();
        raw_write(b, &[raw_key(90, 1)])?;
        let other_data = raw_read(b, api, 32)?;
        raw_write(b, &[raw_key(88, 1)])?;
        let rest = raw_read(inherited, api, 32)?;
        let mut queued = 0;
        check(unsafe { GetNumberOfConsoleInputEvents(b, &mut queued) })?;
        let after = if queued != 0 {
            raw_read(b, api, 32)?
        } else {
            Vec::new()
        };
        let first = std::fs::read_to_string(report.with_extension("first"))?;
        let final_wait =
            unsafe { windows_sys::Win32::System::Threading::WaitForSingleObject(b, 0) };
        publish(
            report,
            &format!(
                "first={first};other={other_data:?};rest={rest:?};after={after:?};mode={mode};final_wait={final_wait}"
            ),
        )?;
        return wait(deadline, || Ok(report.with_extension("opened").exists()));
    }
    let original = if source == "file" {
        Some(open()?)
    } else {
        None
    };
    let input = original.as_ref().map_or_else(
        || unsafe { GetStdHandle(STD_INPUT_HANDLE) },
        |file| file.as_raw_handle(),
    );
    check(unsafe { SetConsoleMode(input, 3) })?;
    check(unsafe { SetConsoleCP(65001) })?;
    let units: &[u16] = if parts[0] == "utf8" {
        &[0x4e2d, 233, 13]
    } else {
        &[97, 98, 99, 13]
    };
    raw_write(
        input,
        &units.iter().map(|u| raw_key(*u, 1)).collect::<Vec<_>>(),
    )?;
    let first = raw_read(input, api, 1)?;
    publish(&report.with_extension("first"), &format!("{first:?}"))?;
    check(unsafe { SetConsoleMode(input, 1) })?;
    let mut command = Command::new(std::env::current_exe()?);
    command
        .arg(format!("cache-child-{kind}"))
        .arg(report)
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit());
    if let Some(ref file) = original {
        command.stdin(Stdio::from(file.try_clone()?));
    } else {
        command.stdin(Stdio::inherit());
    }
    let mut descendant = command.spawn()?;
    drop(command);
    drop(original);
    wait(deadline, || Ok(report.with_extension("ready").exists()))?;
    // The controller acquires a read-only handle while the child is held, and
    // verifies this creation stamp before allowing any creator retirement.
    let stamp = creation(descendant.as_raw_handle())?;
    publish(
        &report.with_extension("family"),
        &format!("{},{}", descendant.id(), stamp),
    )?;
    wait(deadline, || Ok(report.with_extension("observed").exists()))?;
    if lifetime == "detach" {
        check(unsafe { FreeConsole() })?;
        let mut mode = 0;
        let attached = unsafe { GetConsoleMode(GetStdHandle(STD_INPUT_HANDLE), &mut mode) } != 0;
        publish(
            &report.with_extension("detached"),
            &format!("available={attached}"),
        )?;
        if attached {
            return Err(io::Error::other("creator remains attached"));
        }
        if !descendant.wait()?.success() {
            return Err(io::Error::other("inherited client failed"));
        }
    }
    Ok(())
}
