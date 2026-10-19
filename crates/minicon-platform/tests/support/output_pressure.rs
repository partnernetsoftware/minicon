//! Native output pressure, independent file objects and handler-query receipts.
use super::*;

fn publish(report: &Path, extension: &str, bytes: &[u8]) -> io::Result<()> {
    let temporary = report.with_extension(format!("{extension}.tmp"));
    std::fs::write(&temporary, bytes)?;
    std::fs::rename(temporary, report.with_extension(extension))
}

pub(super) fn run(
    input: windows_sys::Win32::Foundation::HANDLE,
    report: &Path,
    mode: &str,
) -> io::Result<()> {
    let tail = mode.contains("-tail");
    let budget = mode.ends_with("-budget");
    let reading = mode.ends_with("-read");
    let _unattached = if mode.ends_with("-orphan") {
        use std::os::windows::process::CommandExt;
        use std::process::{Command, Stdio};
        use windows_sys::Win32::System::Threading::DETACHED_PROCESS;
        let child = Command::new(std::env::current_exe()?)
            .arg("output-orphan-member")
            .arg(report)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .creation_flags(DETACHED_PROCESS)
            .spawn()?;
        let deadline = Instant::now() + Duration::from_secs(8);
        while !report.with_extension("orphan-ready").exists() {
            if Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        Some(child)
    } else {
        None
    };
    // SAFETY: the fixture owns this console for the lifetime of its writer.
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) } as usize;
    let writer_report = report.to_owned();
    let writer = std::thread::spawn(move || -> io::Result<()> {
        let mut units: Vec<u16> = [u16::from(b'a'), 0x4e2d, 0xd83d, 0xde00].repeat(1024);
        for index in 0..if tail { 9 } else { 32 } {
            if !budget {
                for (position, group) in units.chunks_exact_mut(4).enumerate() {
                    group[0] = u16::from(b'A' + ((index * 7 + position) % 26) as u8);
                }
            }
            let mut written = 0;
            // SAFETY: live borrowed console handle, initialized UTF-16 buffer.
            if unsafe {
                WriteConsoleW(
                    output as _,
                    units.as_ptr().cast(),
                    units.len() as u32,
                    &mut written,
                    std::ptr::null(),
                )
            } == 0
                || written != units.len() as u32
            {
                return Err(io::Error::last_os_error());
            }
            if index == 8 {
                publish(
                    &writer_report,
                    if budget || reading { "first" } else { "ready" },
                    b"73728",
                )?;
            }
        }
        Ok(())
    });
    let deadline = Instant::now() + Duration::from_secs(15);
    if tail {
        writer
            .join()
            .map_err(|_| io::Error::other("native output writer panicked"))??;
        return std::fs::write(report, b"tail");
    }
    let rejected = std::sync::Arc::new(AtomicU32::new(0));
    let unexpected = std::sync::Arc::new(AtomicU32::new(0));
    let mut additional = Vec::new();
    if budget || reading {
        while !report.with_extension("first").exists() {
            if Instant::now() >= deadline {
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
    }
    let mut read_receipt = String::new();
    if reading {
        publish(report, "ready", b"73728")?;
        let mut units = [0_u16; 8];
        let mut count = 0;
        // SAFETY: the private native input and a live SDK-sized UTF-16 buffer.
        let (ok, error) = unsafe {
            SetLastError(0);
            let ok = ReadConsoleW(
                input,
                units.as_mut_ptr().cast(),
                units.len() as u32,
                &mut count,
                std::ptr::null(),
            ) != 0;
            (ok, GetLastError())
        };
        read_receipt = format!(",{ok},{error},{count}");
    }
    if budget {
        for _ in 0..65 {
            let rejected = std::sync::Arc::clone(&rejected);
            let unexpected = std::sync::Arc::clone(&unexpected);
            additional.push(std::thread::spawn(move || -> io::Result<()> {
                // Sharing STDOUT did not expose independent pending writes in
                // the original fixture. Separate synchronous file objects
                // make the server's request capacity observable.
                let file = super::super::console_file::open("CONOUT$", 0xc000_0000)?;
                let units: Vec<u16> = [u16::from(b'a'), 0x4e2d, 0xd83d, 0xde00].repeat(1024);
                let mut written = 0;
                // SAFETY: the fixture retains its console until every writer joins.
                if unsafe {
                    WriteConsoleW(
                        file.as_raw_handle(),
                        units.as_ptr().cast(),
                        units.len() as u32,
                        &mut written,
                        std::ptr::null(),
                    )
                } == 0
                {
                    let error = io::Error::last_os_error();
                    if error.raw_os_error() != Some(1450) {
                        unexpected
                            .store(error.raw_os_error().unwrap_or(-1) as u32, Ordering::Release);
                        return Err(error);
                    }
                    rejected.fetch_add(1, Ordering::AcqRel);
                } else if written != units.len() as u32 {
                    return Err(io::Error::other("short native output write"));
                }
                Ok(())
            }));
        }
        while rejected.load(Ordering::Acquire) != 2 {
            if unexpected.load(Ordering::Acquire) != 0 {
                std::fs::write(
                    report.with_extension("go"),
                    format!(
                        "unexpected-budget-error:{}",
                        unexpected.load(Ordering::Acquire)
                    ),
                )?;
                return Err(io::Error::other("unexpected budget rejection"));
            }
            if Instant::now() >= deadline {
                std::fs::write(
                    report.with_extension("go"),
                    format!(
                        "budget-timeout:rejected={}",
                        rejected.load(Ordering::Acquire)
                    ),
                )?;
                return Err(io::ErrorKind::TimedOut.into());
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        publish(report, "ready", b"73728")?;
    }
    while EVENT.load(Ordering::Acquire) == 0 {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    let mut mode = 0;
    // SAFETY: query the same private input after handler API reentry completed.
    if unsafe { GetConsoleMode(input, &mut mode) } == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut receipt = format!(
        "{},{mode},{}",
        EVENT.load(Ordering::Acquire),
        QUERY_MODE.load(Ordering::Acquire)
    );
    if budget {
        receipt.push_str(&format!(",{}", rejected.load(Ordering::Acquire)));
    }
    receipt.push_str(&read_receipt);
    publish(report, "go", receipt.as_bytes())?;
    while !report.with_extension("opened").exists() {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    // Two round trips put an actor lap between the consumer's first 4 KiB
    // read and this receipt. With no other reader, staging must have flushed
    // exactly that free prefix before the remaining bytes are consumed.
    for _ in 0..2 {
        // SAFETY: the same live private console input handle.
        if unsafe { GetConsoleMode(input, &mut mode) } == 0 {
            return Err(io::Error::last_os_error());
        }
    }
    publish(report, "opened-ready", &mode.to_le_bytes())?;
    writer
        .join()
        .map_err(|_| io::Error::other("native output writer panicked"))??;
    for thread in additional {
        thread
            .join()
            .map_err(|_| io::Error::other("native output writer panicked"))??;
    }
    std::fs::write(report, receipt)
}
