//! Launcher side of a packed `.com`: pick this host's cell, run it, and hand
//! back its exact exit code.
//!
//! Design notes that matter (both are defects of the C trampoline this
//! replaces):
//!   * The payload is spawned with **inherited** stdio, so a parent that gave us
//!     pipes still captures the output.
//!   * The payload's exit code is returned verbatim; no POSIX status decoding
//!     is applied on Windows.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::PathBuf;
use std::process::Command;

fn main() {
    let code = match run() {
        Ok(code) => code,
        Err(message) => {
            eprintln!("minicon-com: {message}");
            3
        }
    };
    std::process::exit(code);
}

fn run() -> Result<i32, String> {
    let argv: Vec<std::ffi::OsString> = std::env::args_os().collect();
    let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
    let table = minicon_com_rs::read_table(&exe)?;
    let host = minicon_com_rs::host_key();

    let entry = table.iter().find(|e| e.key == host).ok_or_else(|| {
        let keys: Vec<&str> = table.iter().map(|e| e.key.as_str()).collect();
        format!("no cell for {host}; packed cells: {}", keys.join(", "))
    })?;

    let payload = extract(&exe, entry)?;
    let child = Command::new(&payload)
        .args(&argv[1..])
        .status()
        .map_err(|e| format!("spawn {}: {e}", payload.display()))?;

    let _ = std::fs::remove_file(&payload);

    Ok(match child.code() {
        Some(code) => code,
        #[cfg(unix)]
        None => {
            use std::os::unix::process::ExitStatusExt;
            128 + child.signal().unwrap_or(0)
        }
        #[cfg(not(unix))]
        None => 1,
    })
}

fn extract(exe: &PathBuf, entry: &minicon_com_rs::Entry) -> Result<PathBuf, String> {
    let name = if cfg!(windows) {
        format!("minicon-com-{}.exe", std::process::id())
    } else {
        format!("minicon-com-{}", std::process::id())
    };

    // A sandboxed parent may hand us a temp directory we cannot write. Take the
    // first directory that actually accepts the payload: explicit override,
    // TMPDIR, the launcher's own directory, then the working directory.
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(dir) = std::env::var("MINICON_COM_EXTRACT") {
        if !dir.is_empty() {
            candidates.push(PathBuf::from(dir));
        }
    }
    candidates.push(std::env::temp_dir());
    if let Some(parent) = exe.parent() {
        candidates.push(parent.to_path_buf());
    }
    if let Ok(cwd) = std::env::current_dir() {
        candidates.push(cwd);
    }

    let mut last = String::from("no writable extraction directory");
    for dir in candidates {
        let path = dir.join(&name);
        match write_payload(exe, entry, &path) {
            Ok(()) => {
                if std::env::var_os("MINICON_COM_VERBOSE").is_some() {
                    eprintln!("minicon-com: extracted to {}", path.display());
                }
                return Ok(path);
            }
            Err(e) => last = format!("{}: {e}", path.display()),
        }
    }
    Err(format!("cannot extract payload: {last}"))
}

fn write_payload(
    exe: &PathBuf,
    entry: &minicon_com_rs::Entry,
    path: &PathBuf,
) -> Result<(), String> {
    let mut src = std::fs::File::open(exe).map_err(|e| format!("open self: {e}"))?;
    src.seek(SeekFrom::Start(entry.offset))
        .map_err(|e| format!("seek cell: {e}"))?;
    let mut data = vec![0u8; entry.len as usize];
    src.read_exact(&mut data)
        .map_err(|e| format!("read cell: {e}"))?;

    let mut out = std::fs::File::create(path).map_err(|e| format!("create: {e}"))?;
    out.write_all(&data).map_err(|e| format!("write: {e}"))?;
    out.flush().map_err(|e| format!("flush: {e}"))?;
    drop(out);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755));
    }

    Ok(())
}
