//! Native file lifetime across creator exit and an inherited-buffer survivor.
use super::{
    buffers::{checked, create, position, query, stage},
    mixed_output::bytes,
};
use std::{
    fs::{File, OpenOptions},
    io,
    os::windows::io::AsRawHandle,
    path::Path,
    process::{Command, Stdio},
    time::{Duration, Instant},
};
use windows_sys::Win32::{Foundation::HANDLE, System::Console::*};

const ROUNDS: usize = 18;

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

fn current_output() -> io::Result<File> {
    OpenOptions::new().read(true).write(true).open("CONOUT$")
}

fn clients(expected: &[u32]) -> io::Result<()> {
    let mut expected = expected.to_vec();
    expected.sort_unstable();
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let mut ids = [0; 8];
        // SAFETY: a bounded live buffer, querying only this fixture's console.
        let count = unsafe { GetConsoleProcessList(ids.as_mut_ptr(), ids.len() as u32) };
        if count == 0 {
            return Err(io::Error::last_os_error());
        }
        if count as usize > ids.len() {
            return Err(io::Error::other("unexpected console family size"));
        }
        let actual = &mut ids[..count as usize];
        actual.sort_unstable();
        if actual == expected {
            return process_list_contract(&expected);
        }
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!(
                "console family mismatch: {actual:?} != {expected:?}"
            )));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn process_list_contract(expected: &[u32]) -> io::Result<()> {
    use windows_sys::Win32::Foundation::{GetLastError, SetLastError};
    for capacity in 1..=expected.len() + 2 {
        let mut ids = [0xfeed_beef; 8];
        // SAFETY: live array, bounded capacity, fixture console only.
        let count = unsafe { GetConsoleProcessList(ids.as_mut_ptr(), capacity as u32) };
        if count as usize != expected.len() {
            return Err(io::Error::other(format!(
                "process-list count {count} for capacity {capacity}"
            )));
        }
        if capacity < expected.len() {
            if ids != [0xfeed_beef; 8] {
                return Err(io::Error::other("short process-list array was modified"));
            }
        } else {
            let (listed, unused) = ids.split_at_mut(expected.len());
            listed.sort_unstable();
            if listed != expected || unused.iter().any(|&id| id != 0xfeed_beef) {
                return Err(io::Error::other("process-list membership or output bounds"));
            }
        }
    }
    for (null, capacity) in [(false, 0), (true, 1)] {
        let mut id = 0xfeed_beef;
        // SAFETY: deliberately invalid API arguments must fail before writing;
        // the nonnull case uses a live output unit and the zero capacity.
        unsafe {
            SetLastError(0);
        }
        let count = unsafe {
            GetConsoleProcessList(if null { std::ptr::null_mut() } else { &mut id }, capacity)
        };
        let error = unsafe { GetLastError() };
        if count != 0 || error != 87 || id != 0xfeed_beef {
            return Err(io::Error::other(format!(
                "invalid process-list arguments: {count},{error},{id:x}"
            )));
        }
    }
    Ok(())
}

fn configure(output: HANDLE, attribute: u16, size: u32, visible: i32) -> io::Result<()> {
    // SAFETY: initialized value records and private console output handle.
    checked(unsafe { SetConsoleMode(output, 7) })?;
    checked(unsafe { SetConsoleTextAttribute(output, attribute) })?;
    checked(unsafe {
        SetConsoleCursorInfo(
            output,
            &CONSOLE_CURSOR_INFO {
                dwSize: size,
                bVisible: visible,
            },
        )
    })
}

pub(super) fn run(mode: &str, path: &Path) -> io::Result<()> {
    let output = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    if mode == "buffer-family-worker" {
        let buffer = create(0xc000_0000, 3)?;
        let handle = buffer.as_raw_handle();
        configure(handle, 0x2d, 63, 0)?;
        bytes(handle, b"ALT")?;
        checked(unsafe { SetConsoleActiveScreenBuffer(handle) })?;
        std::fs::write(path.with_extension("worker-ready"), b"active")?;
        wait(&path.with_extension("worker-go"))?;
        // Exercise kernel process teardown, without File's explicit drop/close.
        std::process::exit(0);
    }
    if mode == "buffer-family-child" || mode == "buffer-family-child-tail" {
        let mut inherited = String::new();
        query(output, "inherited", &mut inherited)?;
        if !inherited.contains("inherited:row0=\"ALT\"") {
            return Err(io::Error::other(format!(
                "wrong inherited buffer: {inherited}"
            )));
        }
        std::fs::write(path.with_extension("child-ready"), inherited)?;
        wait(&path.with_extension("child-go"))?;
        position(output, 0, 1)?;
        bytes(output, b"CHILD")?;
        let mut receipt = String::new();
        query(output, "survivor", &mut receipt)?;
        std::fs::write(path.with_extension("child-result"), receipt)?;
        wait(&path.with_extension("child-exit"))?;
        // This grammar belongs to the retiring buffer, never its fallback.
        if mode.ends_with("-tail") {
            bytes(output, b"\xe4\xb8")?;
        }
        std::process::exit(0);
    }
    if mode != "buffer-family-root" && mode != "buffer-family-root-tail" {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    checked(unsafe { SetConsoleOutputCP(65001) })?;
    configure(output, 0x1e, 25, 1)?;
    bytes(output, b"BASE")?;
    let mut receipt = format!(
        "build={};root-pid={}\n",
        super::output_modes::windows_build()?,
        std::process::id()
    );
    for round in 0..ROUNDS {
        let member = path
            .parent()
            .ok_or(io::ErrorKind::InvalidInput)?
            .join(format!("family{round}.txt"));
        let mut worker = Command::new(std::env::current_exe()?)
            .arg("buffer-family-worker")
            .arg(&member)
            .stdin(Stdio::inherit())
            .stdout(Stdio::inherit())
            .stderr(Stdio::inherit())
            .spawn()?;
        wait(&member.with_extension("worker-ready"))?;
        let active = current_output()?;
        let mut command = Command::new(std::env::current_exe()?);
        command
            .arg(if mode.ends_with("-tail") {
                "buffer-family-child-tail"
            } else {
                "buffer-family-child"
            })
            .arg(&member)
            .stdin(Stdio::inherit())
            .stdout(Stdio::from(active))
            .stderr(Stdio::inherit());
        let mut child = command.spawn()?;
        drop(command); // Only the child retains this inherited output file.
        wait(&member.with_extension("child-ready"))?;
        std::fs::write(member.with_extension("worker-go"), b"exit")?;
        if !worker.wait()?.success() {
            return Err(io::Error::other("buffer creator failed"));
        }
        clients(&[std::process::id(), child.id()])?;
        if child.try_wait()?.is_some() {
            return Err(io::Error::other("inherited-buffer child retired early"));
        }
        std::fs::write(member.with_extension("child-go"), b"creator-exited")?;
        wait(&member.with_extension("child-result"))?;
        receipt.push_str(&format!(
            "round={round};creator-exit=0;clients=2;creator-pid={};child-pid={}\n",
            worker.id(),
            child.id()
        ));
        receipt.push_str(&std::fs::read_to_string(
            member.with_extension("child-result"),
        )?);
        query(output, "base-held", &mut receipt)?;
        let active = current_output()?;
        query(active.as_raw_handle(), "active-held", &mut receipt)?;
        stage(active.as_raw_handle(), path, round * 2, &receipt)?;
        drop(active);
        std::fs::remove_file(path.with_extension(format!("stage{}", round * 2)))?;
        std::fs::write(member.with_extension("child-exit"), b"last-holder-exit")?;
        if !child.wait()?.success() {
            return Err(io::Error::other("inherited-buffer child failed"));
        }
        clients(&[std::process::id()])?;
        // Exit/CloseHandle returns can precede the final server-side close.
        let deadline = Instant::now() + Duration::from_secs(8);
        loop {
            let active = current_output()?;
            let mut state = String::new();
            query(active.as_raw_handle(), "fallback", &mut state)?;
            if state.contains("fallback:row0=\"BASE\"") {
                receipt.push_str(&format!("round={round};last-exit=0;clients=1\n"));
                receipt.push_str(&state);
                stage(active.as_raw_handle(), path, round * 2 + 1, &receipt)?;
                break;
            }
            drop(active);
            if Instant::now() >= deadline {
                return Err(io::Error::other(format!(
                    "buffer fallback missing: {state}"
                )));
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        std::fs::remove_file(path.with_extension(format!("stage{}", round * 2 + 1)))?;
        for extension in [
            "worker-ready",
            "worker-go",
            "child-ready",
            "child-go",
            "child-result",
            "child-exit",
        ] {
            std::fs::remove_file(member.with_extension(extension))?;
        }
    }
    receipt.push_str(&format!("rounds={ROUNDS}\n"));
    std::fs::write(path, receipt)
}
