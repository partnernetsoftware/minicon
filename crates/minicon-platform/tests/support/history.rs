//! Native SDK history client; all assertions use Win32 results, not host state.
use super::mixed_output::bytes;
use std::{
    io,
    path::Path,
    time::{Duration, Instant},
};
use windows_sys::Win32::{
    Foundation::{GetLastError, HANDLE, SetLastError},
    System::Console::*,
};

fn check(ok: i32) -> io::Result<()> {
    if ok == 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}
fn require(ok: bool, detail: impl std::fmt::Debug) -> io::Result<()> {
    if ok {
        Ok(())
    } else {
        Err(io::Error::other(format!("history SDK: {detail:?}")))
    }
}
fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain([0]).collect()
}
fn event(vk: u16, ch: u16, repeat: u16) -> INPUT_RECORD {
    let mut r = INPUT_RECORD {
        EventType: KEY_EVENT as u16,
        ..INPUT_RECORD::default()
    };
    r.Event.KeyEvent.bKeyDown = 1;
    r.Event.KeyEvent.wVirtualKeyCode = vk;
    r.Event.KeyEvent.wRepeatCount = repeat;
    r.Event.KeyEvent.uChar.UnicodeChar = ch;
    r
}
fn text(s: &str) -> Vec<INPUT_RECORD> {
    s.encode_utf16().map(|u| event(0, u, 1)).collect()
}
fn wait_child(mut child: std::process::Child) -> io::Result<i32> {
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(status.code().unwrap_or(-1));
        }
        if Instant::now() >= deadline {
            child.kill()?;
            let _ = child.wait();
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}
fn read(
    input: HANDLE,
    mut keys: Vec<INPUT_RECORD>,
    expected: &str,
    report: &mut String,
) -> io::Result<()> {
    keys.push(event(13, 13, 1));
    let mut written = 0;
    check(unsafe { WriteConsoleInputW(input, keys.as_ptr(), keys.len() as u32, &mut written) })?;
    require(written as usize == keys.len(), ("input count", written))?;
    let mut data = [0_u16; 256];
    let mut count = 0;
    check(unsafe {
        ReadConsoleW(
            input,
            data.as_mut_ptr().cast(),
            256,
            &mut count,
            std::ptr::null_mut(),
        )
    })?;
    let expected: Vec<_> = format!("{expected}\r\n").encode_utf16().collect();
    require(
        count as usize == expected.len() && data[..count as usize] == expected,
        ("read", count, &data[..count as usize]),
    )?;
    report.push_str(&format!("read:{:?}\n", &data[..count as usize]));
    Ok(())
}
fn settings(size: u32, buffers: u32, flags: u32) -> io::Result<()> {
    let info = CONSOLE_HISTORY_INFO {
        cbSize: size_of::<CONSOLE_HISTORY_INFO>() as u32,
        HistoryBufferSize: size,
        NumberOfHistoryBuffers: buffers,
        dwFlags: flags,
    };
    check(unsafe { SetConsoleHistoryInfo(&info) })?;
    check_settings(size, buffers, flags)
}
fn check_settings(size: u32, buffers: u32, flags: u32) -> io::Result<()> {
    let mut actual = CONSOLE_HISTORY_INFO {
        cbSize: size_of::<CONSOLE_HISTORY_INFO>() as u32,
        ..CONSOLE_HISTORY_INFO::default()
    };
    check(unsafe { GetConsoleHistoryInfo(&mut actual) })?;
    require(
        (
            actual.HistoryBufferSize,
            actual.NumberOfHistoryBuffers,
            actual.dwFlags,
        ) == (size, buffers, flags),
        (
            "settings",
            actual.HistoryBufferSize,
            actual.NumberOfHistoryBuffers,
            actual.dwFlags,
        ),
    )
}
fn list(exe: &str, expected: &[&str], label: &str, report: &mut String) -> io::Result<()> {
    list_with_length(exe, expected, label, report, None)
}
fn list_with_length(
    exe: &str,
    expected: &[&str],
    label: &str,
    report: &mut String,
    ansi_length: Option<usize>,
) -> io::Result<()> {
    let name = wide(exe);
    let units: Vec<_> = expected
        .iter()
        .flat_map(|s| s.encode_utf16().chain([0]))
        .collect();
    let mut data = [0xaaaa_u16; 256];
    let len = unsafe { GetConsoleCommandHistoryLengthW(name.as_ptr()) };
    let count = unsafe { GetConsoleCommandHistoryW(data.as_mut_ptr(), 512, name.as_ptr()) };
    report.push_str(&format!(
        "{label}:wide-length={len};wide-count={count};units={:?}\n",
        &data[..units.len()]
    ));
    require(
        len as usize == units.len() * 2
            && count == len
            && data[..units.len()] == units
            && data[units.len()] == 0xaaaa,
        (label, "wide", len, count, &data[..units.len() + 1]),
    )?;
    let mut ansi = exe.as_bytes().to_vec();
    ansi.push(0);
    let expected_bytes: Vec<_> = expected
        .iter()
        .flat_map(|s| s.as_bytes().iter().copied().chain([0]))
        .collect();
    let mut data = [0xaa_u8; 512];
    let len = unsafe { GetConsoleCommandHistoryLengthA(ansi.as_ptr()) };
    let count = unsafe { GetConsoleCommandHistoryA(data.as_mut_ptr(), 512, ansi.as_ptr()) };
    require(
        len as usize == ansi_length.unwrap_or(expected_bytes.len())
            && count as usize == expected_bytes.len()
            && data[..expected_bytes.len()] == expected_bytes
            && data[expected_bytes.len()] == 0xaa,
        (label, "ansi", len, count),
    )?;
    report.push_str(&format!(
        "{label}:ansi-length={len};ansi-count={count};bytes={expected_bytes:?}\n"
    ));
    Ok(())
}
pub(super) fn run(kind: &str, path: &Path) -> io::Result<()> {
    let (reference, kind) = kind
        .strip_prefix("reference-")
        .map_or((false, kind), |k| (true, k));
    if reference {
        require(
            super::output_modes::windows_build()? == 20348,
            "reference build",
        )?;
    }
    let input = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    check(unsafe { SetConsoleCP(65001) })?;
    check(unsafe { SetConsoleOutputCP(65001) })?;
    check(unsafe { SetConsoleMode(input, 0xa7) })?;
    check(unsafe { SetConsoleMode(output, 7) })?;
    let image = std::env::current_exe()?;
    let exe = image.file_name().unwrap().to_str().unwrap();
    let mut report = format!("kind={kind}\n");
    if kind == "peer-first" || kind == "peer-reuse" {
        if kind == "peer-first" {
            list(exe, &[], "peer-empty", &mut report)?;
            read(input, text("child"), "child", &mut report)?;
        } else {
            read(input, vec![event(0x26, 0, 1)], "child", &mut report)?;
        }
        list(exe, &["child"], "peer-history", &mut report)?;
    } else {
        settings(3, 4, u32::from(kind == "nodup"))?;
        unsafe { ExpungeConsoleCommandHistoryW(wide(exe).as_ptr()) };
        list(exe, &[], "initial", &mut report)?;
        match kind {
            "basic" | "nodup" | "resize" => {
                for value in ["alpha", "beta", "alpha", "alpha"] {
                    read(input, text(value), value, &mut report)?;
                }
                list(
                    exe,
                    if kind == "nodup" {
                        &["beta", "alpha"]
                    } else {
                        &["alpha", "beta", "alpha"]
                    },
                    "duplicates",
                    &mut report,
                )?;
                read(input, text("gamma"), "gamma", &mut report)?;
                list(
                    &exe.to_uppercase(),
                    &["beta", "alpha", "gamma"],
                    "capacity",
                    &mut report,
                )?;
                if kind == "resize" {
                    check(unsafe { SetConsoleNumberOfCommandsW(2, wide(exe).as_ptr()) })?;
                    list(exe, &["beta", "alpha"], "resize-wide", &mut report)?;
                    let mut name = exe.as_bytes().to_vec();
                    name.push(0);
                    check(unsafe { SetConsoleNumberOfCommandsA(1, name.as_ptr()) })?;
                    list(exe, &["beta"], "resize-ansi", &mut report)?;
                    check(unsafe { SetConsoleNumberOfCommandsW(0, wide(exe).as_ptr()) })?;
                    read(input, text("unstored"), "unstored", &mut report)?;
                    list(exe, &[], "zero-capacity", &mut report)?;
                }
            }
            "unicode" => {
                read(input, text("λ中😀"), "λ中😀", &mut report)?;
                read(input, text("Café"), "Café", &mut report)?;
                // Build 20348 undercounts ANSI UTF-8 storage here. Current
                // Microsoft source trial-converts; the owned backend follows
                // that corrected contract. Check both lengths explicitly.
                list_with_length(
                    exe,
                    &["λ中😀", "Café"],
                    "unicode",
                    &mut report,
                    if reference { Some(10) } else { None },
                )?;
            }
            "alias" => {
                check(unsafe {
                    AddConsoleAliasW(
                        wide("short").as_ptr(),
                        wide("expanded").as_ptr(),
                        wide(exe).as_ptr(),
                    )
                })?;
                read(input, text("short"), "expanded", &mut report)?;
                list(exe, &["short"], "original", &mut report)?;
            }
            "silent" => {
                check(unsafe { SetConsoleMode(input, 3) })?;
                read(input, text("secret"), "secret", &mut report)?;
                list(exe, &[], "silent", &mut report)?;
                check(unsafe { SetConsoleMode(input, 0xa7) })?;
                read(input, text("visible"), "visible", &mut report)?;
                list(exe, &["visible"], "echoed", &mut report)?;
            }
            "raw" | "wake" => {
                if kind == "wake" {
                    read(input, text("visible"), "visible", &mut report)?;
                }
                check(unsafe { SetConsoleMode(input, if kind == "raw" { 1 } else { 0xa7 }) })?;
                let value = if kind == "raw" { "raw" } else { "partial\t" };
                let keys = text(value);
                let mut count = 0;
                check(unsafe {
                    WriteConsoleInputW(input, keys.as_ptr(), keys.len() as u32, &mut count)
                })?;
                require(count as usize == keys.len(), "input record count")?;
                let mut data = [0_u16; 256];
                let mut control = CONSOLE_READCONSOLE_CONTROL {
                    nLength: size_of::<CONSOLE_READCONSOLE_CONTROL>() as u32,
                    nInitialChars: 0,
                    dwCtrlWakeupMask: 1 << 9,
                    dwControlKeyState: 0,
                };
                check(unsafe {
                    ReadConsoleW(
                        input,
                        data.as_mut_ptr().cast(),
                        256,
                        &mut count,
                        if kind == "raw" {
                            std::ptr::null_mut()
                        } else {
                            &mut control
                        },
                    )
                })?;
                let expected: Vec<_> = value.encode_utf16().collect();
                require(
                    count as usize == expected.len()
                        && data[..count as usize] == expected
                        && control.dwControlKeyState == 0,
                    ("raw/wake read", count),
                )?;
                report.push_str(&format!("{kind}-read:{:?}\n", &data[..count as usize]));
                list(
                    exe,
                    if kind == "raw" { &[] } else { &["visible"] },
                    "uncommitted",
                    &mut report,
                )?;
            }
            "queries" => {
                read(input, text("alpha"), "alpha", &mut report)?;
                read(input, text("beta"), "beta", &mut report)?;
                let mut data = [0xaaaa_u16; 4];
                unsafe { SetLastError(0) };
                let count =
                    unsafe { GetConsoleCommandHistoryW(data.as_mut_ptr(), 2, wide(exe).as_ptr()) };
                let error = unsafe { GetLastError() };
                require(
                    count == 0 && error == 111 && data == [0xaaaa; 4],
                    ("small wide", count, error, data),
                )?;
                let mut data = [0xaa_u8; 4];
                unsafe { SetLastError(0) };
                let count = unsafe {
                    GetConsoleCommandHistoryA(data.as_mut_ptr(), 2, format!("{exe}\0").as_ptr())
                };
                let error = unsafe { GetLastError() };
                require(
                    count == 0 && error == 111 && data == [0xaa; 4],
                    ("small ansi", count, error, data),
                )?;
                list("missing-history.exe", &[], "missing", &mut report)?;
                check(unsafe {
                    SetConsoleNumberOfCommandsW(1, wide("missing-history.exe").as_ptr())
                })?;
                list(exe, &["alpha", "beta"], "small-preserved", &mut report)?;
                report.push_str("small-wide:0,111,untouched\nsmall-ansi:0,111,untouched\n");
            }
            "recall" => {
                for value in ["first", "second"] {
                    read(input, text(value), value, &mut report)?;
                }
                read(input, vec![event(0x26, 0, 1)], "second", &mut report)?;
                read(
                    input,
                    vec![event(0x26, 0, 2), event(0, 33, 1)],
                    "first!",
                    &mut report,
                )?;
                read(
                    input,
                    vec![event(0x26, 0, 2), event(0x28, 0, 1)],
                    "first!",
                    &mut report,
                )?;
                list(exe, &["first", "second", "first!"], "recall", &mut report)?;
            }
            "lifecycle" => {
                read(input, text("parent"), "parent", &mut report)?;
                for (index, peer) in ["peer-first", "peer-reuse"].into_iter().enumerate() {
                    let peer_path = path.with_extension(format!("peer-{index}"));
                    let child = std::process::Command::new(&image)
                        .arg(format!("history-{peer}"))
                        .arg(&peer_path)
                        .spawn()?;
                    require(wait_child(child)? == 0, "peer exit")?;
                    report.push_str(&std::fs::read_to_string(peer_path)?);
                    list(exe, &["parent"], "parent-isolated", &mut report)?;
                }
                read(input, vec![event(0x26, 0, 1)], "parent", &mut report)?;
            }
            "cmd" => {
                let image = std::path::PathBuf::from(
                    std::env::var_os("SystemRoot").ok_or(io::ErrorKind::NotFound)?,
                )
                .join("System32")
                .join("cmd.exe");
                let child = std::process::Command::new(image)
                    .args(["/d", "/q"])
                    .env("PROMPT", "HISTORY_CMD_READY$G")
                    .current_dir(path.parent().unwrap())
                    .spawn()?;
                let deadline = Instant::now() + Duration::from_secs(8);
                while !path.with_extension("go").exists() {
                    if Instant::now() >= deadline {
                        return Err(io::ErrorKind::TimedOut.into());
                    }
                    std::thread::sleep(Duration::from_millis(2));
                }
                let mut keys = text("echo history-cmd");
                keys.extend([event(13, 13, 1), event(0x26, 0, 1), event(13, 13, 1)]);
                keys.extend(text("exit /b 27"));
                keys.push(event(13, 13, 1));
                let mut count = 0;
                check(unsafe {
                    WriteConsoleInputW(input, keys.as_ptr(), keys.len() as u32, &mut count)
                })?;
                require(count as usize == keys.len(), "cmd input count")?;
                require(wait_child(child)? == 27, "cmd exit")?;
                report.push_str("cmd-exit:27\n");
            }
            "limits" => {
                read(input, text("retained"), "retained", &mut report)?;
                let info = CONSOLE_HISTORY_INFO {
                    cbSize: 16,
                    HistoryBufferSize: 1025,
                    NumberOfHistoryBuffers: 4,
                    dwFlags: 0,
                };
                unsafe { SetLastError(0) };
                require(
                    unsafe { SetConsoleHistoryInfo(&info) } == 0 && unsafe { GetLastError() } == 8,
                    "configuration bound",
                )?;
                check_settings(3, 4, 0)?;
                list(exe, &["retained"], "configuration-preserved", &mut report)?;
                report.push_str("configuration-bound:8\n");
            }
            _ => return Err(io::ErrorKind::InvalidInput.into()),
        }
        unsafe { ExpungeConsoleCommandHistoryA(format!("{exe}\0").as_ptr()) };
        list(exe, &[], "expunged", &mut report)?;
    }
    std::fs::write(path, &report)?;
    if kind.starts_with("peer-") {
        return Ok(());
    }
    bytes(output, b"\x1b]2;history-end:\x07")?;
    let deadline = Instant::now() + Duration::from_secs(8);
    while !path.with_extension("opened").exists() {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
