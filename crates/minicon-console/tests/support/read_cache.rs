//! Independent native file-object/cache consumer; no host types are imported.
use super::queued_reads::{check, publish, raw_key, raw_read, raw_write, wait};
use std::{
    fs::OpenOptions,
    io,
    os::windows::io::AsRawHandle,
    path::Path,
    time::{Duration, Instant},
};
use windows_sys::Win32::System::{Console::*, Threading::WaitForSingleObject};

pub(crate) fn run(kind: &str, report: &Path) -> io::Result<()> {
    let result = run_inner(kind, report);
    if let Err(error) = &result {
        publish(report, &format!("cache-error: {error:?}"))?;
    }
    result
}
fn run_inner(kind: &str, report: &Path) -> io::Result<()> {
    let (api, lifetime) = kind.split_once('-').ok_or(io::ErrorKind::InvalidInput)?;
    if !matches!(api, "wide" | "file" | "mixed" | "utf8")
        || !matches!(lifetime, "share" | "flush" | "close")
    {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    let open = || OpenOptions::new().read(true).write(true).open("CONIN$");
    let original = open()?;
    let duplicate = original.try_clone()?;
    let other = open()?;
    let handle = original.as_raw_handle();
    check(unsafe { SetConsoleMode(handle, 3) })?;
    check(unsafe { SetConsoleCP(65001) })?;
    if unsafe { GetConsoleCP() } != 65001 {
        return Err(io::Error::other("cache fixture input code page differs"));
    }
    let units: &[u16] = if api == "utf8" {
        &[0x4e2d, 233, 13]
    } else {
        &[97, 98, 99, 13]
    };
    raw_write(
        handle,
        &units
            .iter()
            .map(|unit| raw_key(*unit, 1))
            .collect::<Vec<_>>(),
    )?;
    // This shared SDK wrapper bounds the actual API buffer; cooked/raw behavior
    // is selected by console mode, not the helper's name.
    let first = raw_read(
        handle,
        if matches!(api, "wide" | "mixed") {
            "wide"
        } else {
            "file"
        },
        1,
    )?;
    let b = other.as_raw_handle();
    check(unsafe { SetConsoleMode(b, 1) })?;
    if lifetime == "flush" {
        check(unsafe { FlushConsoleInputBuffer(b) })?;
    }
    drop(original);
    let duplicate = if lifetime == "close" {
        drop(duplicate);
        None
    } else {
        Some(duplicate)
    };
    raw_write(b, &[raw_key(90, 1)])?;
    let other_data = raw_read(b, if api == "wide" { "wide" } else { "file" }, 32)?;
    let continuation_wait = unsafe { WaitForSingleObject(b, 0) };
    publish(
        &report.with_extension("ready"),
        &format!("first={first:?};other={other_data:?};wait={continuation_wait}"),
    )?;
    raw_write(b, &[raw_key(88, 1)])?;
    let rest = if let Some(ref duplicate) = duplicate {
        raw_read(
            duplicate.as_raw_handle(),
            if api == "wide" { "wide" } else { "file" },
            32,
        )?
    } else {
        Vec::new()
    };
    let mut queued = 0;
    check(unsafe { GetNumberOfConsoleInputEvents(b, &mut queued) })?;
    // Missing X is an observable failure (not an invitation to hang a second
    // API call). Positive cases always perform that final native read.
    let after = if queued != 0 {
        let reopened = if lifetime == "close" {
            Some(open()?)
        } else {
            None
        };
        raw_read(
            reopened.as_ref().map_or(b, |file| file.as_raw_handle()),
            if api == "wide" { "wide" } else { "file" },
            32,
        )?
    } else {
        Vec::new()
    };
    let final_wait = unsafe { WaitForSingleObject(b, 0) };
    publish(
        report,
        &format!(
            "first={first:?};other={other_data:?};rest={rest:?};after={after:?};continuation_wait={continuation_wait};final_wait={final_wait}"
        ),
    )?;
    wait(Instant::now() + Duration::from_secs(18), || {
        Ok(report.with_extension("opened").exists())
    })
}
