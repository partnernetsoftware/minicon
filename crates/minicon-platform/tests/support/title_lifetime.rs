//! Real parent/child console title lifecycle, observed through SDK and VT.
use super::{
    buffers::{checked, wait_stage},
    mixed_output::bytes,
};
use std::{
    io,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::System::Console::*;

fn wait(path: &Path) -> io::Result<()> {
    let deadline = Instant::now() + Duration::from_secs(8);
    while !path.exists() {
        if Instant::now() >= deadline {
            return Err(io::ErrorKind::TimedOut.into());
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    Ok(())
}
fn set(title: &str) -> io::Result<()> {
    let title: Vec<_> = title.encode_utf16().chain(Some(0)).collect();
    checked(unsafe { SetConsoleTitleW(title.as_ptr()) })
}
fn value(original: bool) -> String {
    let mut data = [0; 512];
    let count = unsafe {
        if original {
            GetConsoleOriginalTitleW(data.as_mut_ptr(), data.len() as u32)
        } else {
            GetConsoleTitleW(data.as_mut_ptr(), data.len() as u32)
        }
    };
    data[..(count as usize).min(511)]
        .iter()
        .map(|u| format!("{u:x}"))
        .collect::<Vec<_>>()
        .join(",")
}
fn stage(path: &Path, n: usize, report: &mut String) -> io::Result<()> {
    report.push_str(&format!(
        "stage{n}:current={};original={}\n",
        value(false),
        value(true)
    ));
    std::fs::write(path, report)?;
    let marker: Vec<_> = format!("life-title-{n}:").encode_utf16().collect();
    let mut written = 0;
    checked(unsafe {
        WriteConsoleOutputCharacterW(
            GetStdHandle(STD_OUTPUT_HANDLE),
            marker.as_ptr(),
            marker.len() as u32,
            COORD { X: 0, Y: 7 },
            &mut written,
        )
    })?;
    if written as usize != marker.len() {
        return Err(io::Error::other("short lifecycle fence"));
    }
    wait_stage(path, n)
}
pub(super) fn run(mode: &str, path: &Path) -> io::Result<()> {
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    checked(unsafe { SetConsoleMode(output, 7) })?;
    checked(unsafe { SetConsoleOutputCP(65001) })?;
    if mode.ends_with("-child") {
        if mode.contains("native") {
            set("child-native")?;
        } else {
            bytes(output, b"\x1b]2;child-vt\x07")?;
        }
        std::fs::write(path.with_extension("ready"), b"title-set")?;
        wait(&path.with_extension("go"))?;
        if mode.contains("vt") {
            bytes(output, b"\x1b[3")?;
        }
        return Ok(());
    }
    bytes(output, b"ROOT")?;
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    stage(path, 0, &mut report)?;
    for (round, kind) in ["native", "vt"].iter().enumerate() {
        set(if round == 0 { "parent" } else { "parent-2" })?;
        let child_path = path.with_file_name(format!("title-child-{kind}.txt"));
        let mut child = Command::new(std::env::current_exe()?)
            .arg(format!("title-life-{kind}-child"))
            .arg(&child_path)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        wait(&child_path.with_extension("ready"))?;
        stage(path, 1 + round * 2, &mut report)?;
        std::fs::write(child_path.with_extension("go"), b"exit")?;
        if !child.wait()?.success() {
            return Err(io::Error::other("title child failed"));
        }
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let mut ids = [0; 4];
            let count = unsafe { GetConsoleProcessList(ids.as_mut_ptr(), 4) };
            if count == 1 && ids[0] == std::process::id() {
                break;
            }
            if Instant::now() >= deadline {
                return Err(io::Error::other("title child still attached"));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        stage(path, 2 + round * 2, &mut report)?;
    }
    set("after-child")?;
    bytes(output, b"1mZ")?;
    stage(path, 5, &mut report)?;
    set("last-title")?;
    stage(path, 6, &mut report)?;
    Ok(())
}
