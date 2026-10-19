//! Two native buffers, each retaining an independent main/alternate VT pair.
use super::{
    buffers::{checked, create, query, wait_stage},
    mixed_output::bytes,
};
use std::{
    io,
    os::windows::io::AsRawHandle,
    path::Path,
    time::{Duration, Instant},
};
use windows_sys::Win32::{Foundation::HANDLE, System::Console::*};

fn stage(output: HANDLE, path: &Path, n: usize, report: &mut String) -> io::Result<()> {
    query(output, &format!("stage{n}"), report)?;
    std::fs::write(path, report)?;
    let marker: Vec<_> = format!("alt-stage-{n}:").encode_utf16().collect();
    let mut count = 0;
    // A literal native edit fences the peer without consuming client grammar.
    checked(unsafe {
        WriteConsoleOutputCharacterW(
            output,
            marker.as_ptr(),
            marker.len() as u32,
            COORD { X: 0, Y: 7 },
            &mut count,
        )
    })?;
    if count as usize != marker.len() {
        return Err(io::Error::other("short VT-layer marker"));
    }
    wait_stage(path, n)
}

pub(super) fn run(path: &Path) -> io::Result<()> {
    let base = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
    checked(unsafe { SetConsoleMode(base, 7) })?;
    bytes(base, b"BASE\x1b[?1049h\x1b[HVTBASE")?;
    let mut report = format!("build={}\n", super::output_modes::windows_build()?);
    stage(base, path, 0, &mut report)?;
    let other = create(0xc000_0000, 3)?;
    let active = other.as_raw_handle();
    checked(unsafe { SetConsoleMode(active, 7) })?;
    bytes(active, b"NATIVE")?;
    checked(unsafe { SetConsoleActiveScreenBuffer(active) })?;
    bytes(base, b"CARRY\x1b[3")?;
    stage(active, path, 1, &mut report)?;
    bytes(active, b"\x1b[?1049h\x1b[HVTA")?;
    stage(active, path, 2, &mut report)?;
    checked(unsafe { SetConsoleActiveScreenBuffer(base) })?;
    stage(base, path, 3, &mut report)?;
    bytes(base, b"1mZ")?;
    stage(base, path, 4, &mut report)?;
    bytes(base, b"\x1b[?1049l")?;
    stage(base, path, 5, &mut report)?;
    checked(unsafe { SetConsoleActiveScreenBuffer(active) })?;
    stage(active, path, 6, &mut report)?;
    bytes(active, b"\x1b[?1049l")?;
    stage(active, path, 7, &mut report)?;
    drop(other);
    let deadline = Instant::now() + Duration::from_secs(8);
    loop {
        let handle = super::console_file::open("CONOUT$", 0xc000_0000)?;
        let mut state = String::new();
        query(handle.as_raw_handle(), "fallback", &mut state)?;
        if state.contains("fallback:row0=\"BASE\"") {
            break;
        }
        drop(handle);
        if Instant::now() >= deadline {
            return Err(io::Error::other(format!("VT-layer fallback: {state}")));
        }
        std::thread::sleep(Duration::from_millis(2));
    }
    stage(base, path, 8, &mut report)
}
