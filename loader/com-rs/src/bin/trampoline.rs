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
use std::path::{Path, PathBuf};
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

    // `--com-info` is answered by the launcher itself: it identifies the packed
    // artifact without unpacking or running a payload. Only the first argument
    // counts, so a script's own `--com-info` still reaches the payload.
    if argv.get(1).map(|a| a == "--com-info").unwrap_or(false) {
        match table.iter().find(|e| e.key == "__meta__") {
            Some(meta) => {
                let mut src = std::fs::File::open(&exe).map_err(|e| format!("open self: {e}"))?;
                src.seek(SeekFrom::Start(meta.offset))
                    .map_err(|e| format!("seek metadata: {e}"))?;
                let mut raw = vec![0u8; meta.len as usize];
                src.read_exact(&mut raw)
                    .map_err(|e| format!("read metadata: {e}"))?;
                let mut out = std::io::stdout();
                out.write_all(&raw)
                    .map_err(|e| format!("write metadata: {e}"))?;
                out.write_all(b"\n")
                    .map_err(|e| format!("write metadata: {e}"))?;
                out.flush().map_err(|e| format!("write metadata: {e}"))?;
            }
            None => println!("{{\"product\":null}}"),
        }
        return Ok(0);
    }

    let host = minicon_com_rs::host_key();

    let entry = table.iter().find(|e| e.key == host).ok_or_else(|| {
        let keys: Vec<&str> = table
            .iter()
            .map(|e| e.key.as_str())
            .filter(|k| *k != "__meta__")
            .collect();
        format!("no cell for {host}; packed cells: {}", keys.join(", "))
    })?;

    let payload = extract(&exe, entry)?;
    let child = Command::new(&payload).args(&argv[1..]).status();
    cleanup(&payload);
    let child = child.map_err(|e| format!("spawn payload: {e}"))?;

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

/// A sandboxed parent may hand us a temp directory we cannot write. Take the
/// first directory that accepts the payload: explicit override, TMPDIR, the
/// launcher's own directory, then the working directory.
fn extract(exe: &PathBuf, entry: &minicon_com_rs::Entry) -> Result<PathBuf, String> {
    reap_stale(&std::env::temp_dir());
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
        let path = target_path(&dir);
        match write_payload(exe, entry, &path) {
            Ok(()) => {
                if std::env::var_os("MINICON_COM_VERBOSE").is_some() {
                    eprintln!("minicon-com: extracted to {}", path.display());
                }
                return Ok(path);
            }
            Err(e) => {
                last = format!("{}: {e}", path.display());
                cleanup(&path);
            }
        }
    }
    Err(format!("cannot extract payload: {last}"))
}

/// On Unix the payload lives in a private 0700 directory so another local user
/// cannot swap the binary between extraction and exec.
fn target_path(dir: &Path) -> PathBuf {
    #[cfg(unix)]
    {
        let private = dir.join(format!("minicon-com-{}", std::process::id()));
        let _ = std::fs::create_dir_all(&private);
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&private, std::fs::Permissions::from_mode(0o700));
        private.join("payload")
    }
    #[cfg(not(unix))]
    {
        dir.join(format!("minicon-com-{}.exe", std::process::id()))
    }
}

fn write_payload(exe: &PathBuf, entry: &minicon_com_rs::Entry, path: &Path) -> Result<(), String> {
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
        let _ = std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o700));
    }

    Ok(())
}

/// Remove extract leftovers from an earlier run that was killed before it
/// could clean up. Only our own naming pattern, only the shared temp dir, only
/// entries older than a day; `MINICON_COM_REAP=0` turns it off.
fn reap_stale(dir: &Path) {
    if std::env::var_os("MINICON_COM_REAP")
        .map(|v| v == "0")
        .unwrap_or(false)
    {
        return;
    }
    // The sweep walks every temp entry, so it runs at most hourly; without the
    // stamp it costs more than the extraction it protects on a busy temp.
    let stamp = dir.join("minicon-com-reap.stamp");
    if let Ok(meta) = std::fs::metadata(&stamp) {
        if let Ok(modified) = meta.modified() {
            if let Ok(age) = std::time::SystemTime::now().duration_since(modified) {
                if age.as_secs() < 60 * 60 {
                    return;
                }
            }
        }
    }
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let now = std::time::SystemTime::now();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if !name.starts_with("minicon-com-") {
            continue;
        }
        let Ok(meta) = entry.metadata() else { continue };
        let Ok(modified) = meta.modified() else {
            continue;
        };
        let Ok(age) = now.duration_since(modified) else {
            continue;
        };
        if age.as_secs() < 24 * 60 * 60 {
            continue;
        }
        if meta.is_dir() {
            let _ = std::fs::remove_dir_all(entry.path());
        } else {
            let _ = std::fs::remove_file(entry.path());
        }
    }
    let _ = std::fs::write(&stamp, b"");
}
fn cleanup(path: &Path) {
    let _ = std::fs::remove_file(path);
    #[cfg(unix)]
    {
        if let Some(parent) = path.parent() {
            let ours = parent
                .file_name()
                .map(|n| n.to_string_lossy().starts_with("minicon-com-"))
                .unwrap_or(false);
            if ours {
                let _ = std::fs::remove_dir(parent);
            }
        }
    }
}
