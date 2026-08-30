//! Linux load-time import gate for the shipped ELF.
//!
//! `libxkbcommon-x11` is dlopened by winit through `xkbcommon-dl`, so the
//! product can bundle it without growing the ELF NEEDED table. This gate still
//! refuses a regression where the linker starts requiring the host SONAME at
//! process start.

#![cfg(target_os = "linux")]

use std::path::PathBuf;
use std::process::Command;

const FORBIDDEN_LOAD_TIME_SONAMES: &[&str] = &["libxkbcommon-x11.so.0", "libxkbcommon-x11.so"];

fn shipped_binary() -> PathBuf {
    if let Some(path) = std::env::var_os("MINICON_TEST_BINARY") {
        let path = PathBuf::from(path);
        assert!(
            path.is_file(),
            "MINICON_TEST_BINARY is missing at {}",
            path.display()
        );
        return path;
    }
    let mut path = std::env::current_exe().expect("test executable path");
    path.pop();
    path.pop();
    path.push("minicon");
    assert!(
        path.is_file(),
        "minicon is missing at {}; build it with `cargo build --bin minicon`",
        path.display()
    );
    path
}

fn needed_libraries(path: &PathBuf) -> Vec<String> {
    let output = Command::new("readelf")
        .args(["-d", &path.display().to_string()])
        .output()
        .expect("readelf for NEEDED entries");
    assert!(
        output.status.success(),
        "readelf failed for {}: {}",
        path.display(),
        String::from_utf8_lossy(&output.stderr)
    );
    let mut needed = Vec::new();
    for line in String::from_utf8_lossy(&output.stdout).lines() {
        if let Some((_, soname)) = line.split_once("(NEEDED)") {
            let soname = soname
                .split_whitespace()
                .next()
                .unwrap_or("")
                .trim_matches(['[', ']']);
            if !soname.is_empty() {
                needed.push(soname.to_owned());
            }
        }
    }
    assert!(
        !needed.is_empty(),
        "no NEEDED libraries parsed from {}; an empty table would make this gate assert nothing",
        path.display()
    );
    needed
}

#[test]
fn elf_does_not_require_libxkbcommon_x11_at_load_time() {
    let path = shipped_binary();
    let libraries = needed_libraries(&path);
    let blocking: Vec<&String> = libraries
        .iter()
        .filter(|library| {
            FORBIDDEN_LOAD_TIME_SONAMES
                .iter()
                .any(|forbidden| library.contains(forbidden))
        })
        .collect();
    assert!(
        blocking.is_empty(),
        "minicon statically requires {blocking:?} at load time. The X11 keyboard \
         bridge must stay dlopen-owned so MiniCon can stage its own copy when the \
         host omits libxkbcommon-x11-0."
    );
}
