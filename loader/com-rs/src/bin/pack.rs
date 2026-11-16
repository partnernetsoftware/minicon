//! Packer side: append per-host cells and the table to a launcher binary.
//!
//! ```text
//! com-pack pack --trampoline <launcher> --out <file.com> key=path [key=path ...]
//! com-pack list <file.com>
//! ```

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

fn main() {
    match run() {
        Ok(()) => {}
        Err(message) => {
            eprintln!("com-pack: {message}");
            std::process::exit(2);
        }
    }
}

fn usage() -> String {
    "usage: com-pack pack --trampoline <launcher> --out <file.com> key=path...\n       com-pack list <file.com>".into()
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("pack") => pack(&args[1..]),
        Some("verify") => {
            let path = args.get(1).ok_or_else(usage)?;
            verify(Path::new(path))
        }
        Some("list") => {
            let path = args.get(1).ok_or_else(usage)?;
            for entry in minicon_com_rs::read_table(Path::new(path))? {
                println!(
                    "{:<16} {:>8} bytes at {}",
                    entry.key, entry.len, entry.offset
                );
            }
            Ok(())
        }
        _ => Err(usage()),
    }
}

fn pack(args: &[String]) -> Result<(), String> {
    let mut trampoline: Option<PathBuf> = None;
    let mut out: Option<PathBuf> = None;
    let mut cells: Vec<(String, PathBuf)> = Vec::new();

    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--trampoline" => {
                trampoline = Some(PathBuf::from(args.get(i + 1).ok_or_else(usage)?));
                i += 2;
            }
            "--out" => {
                out = Some(PathBuf::from(args.get(i + 1).ok_or_else(usage)?));
                i += 2;
            }
            spec => {
                let (key, path) = spec.split_once('=').ok_or_else(usage)?;
                cells.push((key.to_string(), PathBuf::from(path)));
                i += 1;
            }
        }
    }

    let trampoline = trampoline.ok_or_else(usage)?;
    let out = out.ok_or_else(usage)?;
    if cells.is_empty() {
        return Err("no cells given".into());
    }

    // Read the launcher first: a mistyped --trampoline must not leave a
    // truncated output file behind that later looks like a real artifact.
    let mut launcher = Vec::new();
    std::fs::File::open(&trampoline)
        .map_err(|e| format!("open launcher {}: {e}", trampoline.display()))?
        .read_to_end(&mut launcher)
        .map_err(|e| format!("read launcher: {e}"))?;
    if launcher.len() < 64 {
        return Err(format!("launcher {} is too small", trampoline.display()));
    }

    let mut file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(true)
        .open(&out)
        .map_err(|e| format!("create {}: {e}", out.display()))?;
    file.write_all(&launcher)
        .map_err(|e| format!("write: {e}"))?;

    let mut table: Vec<(String, u64, u64)> = Vec::new();
    let mut pos = launcher.len() as u64;
    for (key, path) in &cells {
        if key.len() >= minicon_com_rs::KEY_LEN {
            return Err(format!("cell key '{key}' is too long"));
        }
        let pad = (8 - (pos % 8)) % 8;
        if pad > 0 {
            file.write_all(&vec![0u8; pad as usize])
                .map_err(|e| format!("pad: {e}"))?;
            pos += pad;
        }
        let mut data = Vec::new();
        std::fs::File::open(path)
            .map_err(|e| format!("open {}: {e}", path.display()))?
            .read_to_end(&mut data)
            .map_err(|e| format!("read {}: {e}", path.display()))?;
        file.write_all(&data)
            .map_err(|e| format!("write cell: {e}"))?;
        table.push((key.clone(), pos, data.len() as u64));
        pos += data.len() as u64;
        println!(
            "cell {key:<14} {:>9} bytes @ {}",
            data.len(),
            table.last().unwrap().1
        );
    }

    let table_off = pos;
    for (key, offset, len) in &table {
        let mut k = [0u8; minicon_com_rs::KEY_LEN];
        k[..key.len()].copy_from_slice(key.as_bytes());
        file.write_all(&k).map_err(|e| format!("write key: {e}"))?;
        file.write_all(&offset.to_le_bytes())
            .map_err(|e| format!("write offset: {e}"))?;
        file.write_all(&len.to_le_bytes())
            .map_err(|e| format!("write len: {e}"))?;
        pos += minicon_com_rs::ENTRY_LEN;
    }
    let table_len = pos - table_off;
    file.write_all(minicon_com_rs::FOOTER_MAGIC)
        .map_err(|e| format!("write magic: {e}"))?;
    file.write_all(&table_off.to_le_bytes())
        .map_err(|e| format!("write table off: {e}"))?;
    file.write_all(&table_len.to_le_bytes())
        .map_err(|e| format!("write table len: {e}"))?;
    file.flush().map_err(|e| format!("flush: {e}"))?;

    // Prove the result is readable before claiming success.
    drop(file);
    let check = minicon_com_rs::read_table(&out)?;
    if check.len() != table.len() {
        return Err("packed table does not round-trip".into());
    }
    let mut probe = std::fs::File::open(&out).map_err(|e| format!("reopen: {e}"))?;
    for entry in &check {
        probe
            .seek(SeekFrom::Start(entry.offset))
            .map_err(|e| format!("probe seek: {e}"))?;
        let mut head = vec![0u8; entry.len.min(2) as usize];
        probe
            .read_exact(&mut head)
            .map_err(|e| format!("probe read: {e}"))?;
        if entry.len >= 2 && head != b"MZ" && head != b"\x7fE" {
            eprintln!(
                "com-pack: warning: cell {} does not start like an executable",
                entry.key
            );
        }
    }
    let size = std::fs::metadata(&out)
        .map_err(|e| format!("stat out: {e}"))?
        .len();
    verify(&out)?;
    println!("wrote {} ({size} bytes)", out.display());
    Ok(())
}

/// Check that every cell's executable header matches the OS/arch its key names.
fn verify(path: &Path) -> Result<(), String> {
    let table = minicon_com_rs::read_table(path)?;
    let mut file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut bad = 0usize;
    for entry in &table {
        let head_len = entry.len.min(1024) as usize;
        let mut head = vec![0u8; head_len];
        file.seek(SeekFrom::Start(entry.offset))
            .map_err(|e| format!("seek: {e}"))?;
        file.read_exact(&mut head)
            .map_err(|e| format!("read: {e}"))?;
        let (want, got) = match expected_arch(&entry.key) {
            None => (String::from("any (unknown key)"), describe(&head)),
            Some(want) => (want, describe_for(&entry.key, &head)),
        };
        let ok = expected_arch(&entry.key).is_none() || want == got;
        if ok {
            println!("ok   {:<14} {}", entry.key, got);
        } else {
            bad += 1;
            println!("FAIL {:<14} expected {}, got {}", entry.key, want, got);
        }
    }
    if bad > 0 {
        return Err(format!("{bad} cell(s) do not match their key"));
    }
    Ok(())
}

fn expected_arch(key: &str) -> Option<String> {
    Some(match key {
        "win-x86_64" => "PE machine 0x8664".into(),
        "win-aarch64" => "PE machine 0xaa64".into(),
        "lnx-x86_64" => "ELF machine 0x3e".into(),
        "lnx-aarch64" => "ELF machine 0xb7".into(),
        "osx-x86_64" => "Mach-O cpu 0x01000007".into(),
        "osx-aarch64" => "Mach-O cpu 0x0100000c".into(),
        _ => return None,
    })
}

fn describe(head: &[u8]) -> String {
    if head.len() >= 2 && &head[0..2] == b"MZ" {
        return "PE".into();
    }
    if head.len() >= 4 && &head[0..4] == b"\x7fELF" {
        return format!(
            "ELF machine 0x{:x}",
            u16::from_le_bytes([head[18], head[19]])
        );
    }
    if head.len() >= 4 && (&head[0..4] == b"\xcf\xfa\xed\xfe" || &head[0..4] == b"\xce\xfa\xed\xfe")
    {
        return format!(
            "Mach-O cpu 0x{:08x}",
            u32::from_le_bytes([head[4], head[5], head[6], head[7]])
        );
    }
    "unknown header".into()
}

fn describe_for(key: &str, head: &[u8]) -> String {
    if key.starts_with("win-") && head.len() >= 2 && &head[0..2] == b"MZ" {
        let pe = u32::from_le_bytes([head[0x3c], head[0x3d], head[0x3e], head[0x3f]]) as usize;
        if pe + 6 <= head.len() {
            let machine = u16::from_le_bytes([head[pe + 4], head[pe + 5]]);
            return format!("PE machine 0x{machine:04x}");
        }
    }
    describe(head)
}
