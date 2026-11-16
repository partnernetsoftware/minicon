//! Packer side: append per-host cells, metadata and the table to a launcher.
//!
//! ```text
//! com-pack pack --trampoline <launcher> --out <file.com>
//!               [--subsystem gui|console] [--meta key=value ...]
//!               key=path [key=path ...]
//! com-pack list <file.com>
//! com-pack info <file.com>
//! com-pack verify <file.com>
//! ```
//!
//! `info` reads the `__meta__` entry the packer writes: product, version and
//! whatever the caller recorded, so a packed file can be identified without
//! launching a payload.

use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

const META_KEY: &str = "__meta__";

/// One thing to append: a file on disk, or bytes the packer already holds.
enum Source {
    File(PathBuf),
    Bytes(Vec<u8>),
}

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
    "usage: com-pack pack --trampoline <launcher> --out <file.com> [--subsystem gui|console] [--meta key=value ...] key=path...\n       com-pack list <file.com>\n       com-pack info <file.com>\n       com-pack inspect <file.com>\n       com-pack verify <file.com>".into()
}

fn run() -> Result<(), String> {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("pack") => pack(&args[1..]),
        Some("verify") => {
            let path = args.get(1).ok_or_else(usage)?;
            verify(Path::new(path))
        }
        Some("inspect") => {
            let path = args.get(1).ok_or_else(usage)?;
            inspect(Path::new(path))
        }
        Some("info") => {
            let path = args.get(1).ok_or_else(usage)?;
            info(Path::new(path))
        }
        Some("list") => {
            let path = args.get(1).ok_or_else(usage)?;
            for entry in minicon_com_rs::read_table(Path::new(path))? {
                println!(
                    "{:<16} {:>9} bytes at {}",
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
    let mut subsystem: Option<u16> = None;
    let mut meta: Vec<(String, String)> = Vec::new();
    let mut cells: Vec<(String, Source)> = Vec::new();

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
            "--subsystem" => {
                subsystem = Some(match args.get(i + 1).map(String::as_str) {
                    Some("gui") => 2,
                    Some("console") => 3,
                    _ => return Err("--subsystem takes gui or console".into()),
                });
                i += 2;
            }
            "--meta" => {
                let spec = args.get(i + 1).ok_or_else(usage)?;
                let (key, value) = spec.split_once('=').ok_or_else(usage)?;
                meta.push((key.to_string(), value.to_string()));
                i += 2;
            }
            spec => {
                let (key, path) = spec.split_once('=').ok_or_else(usage)?;
                cells.push((key.to_string(), Source::File(PathBuf::from(path))));
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
    match launcher_kind(&launcher) {
        LauncherKind::Unknown => {
            return Err(format!(
                "launcher {} is not a PE, ELF or Mach-O executable",
                trampoline.display()
            ))
        }
        LauncherKind::Pe => {
            if let Some(subsystem) = subsystem {
                patch_pe_subsystem(&mut launcher, subsystem)?;
            }
        }
        _ => {
            if subsystem.is_some() {
                return Err("--subsystem applies only to a PE launcher".into());
            }
        }
    }

    // Metadata rides in the table under a reserved key, so the container format
    // stays byte-compatible with v1 readers that only look up their host cell.
    if !meta.is_empty() {
        let mut json = String::from("{");
        for (n, (key, value)) in meta.iter().enumerate() {
            if n > 0 {
                json.push(',');
            }
            json.push_str(&format!(
                "\"{}\":\"{}\"",
                json_escape(key),
                json_escape(value)
            ));
        }
        json.push('}');
        cells.push((META_KEY.to_string(), Source::Bytes(json.into_bytes())));
    }

    let tmp = out.with_file_name(format!(
        ".{}.tmp{}",
        out.file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| "com".into()),
        std::process::id()
    ));
    let mut file = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .open(&tmp)
        .map_err(|e| format!("create {}: {e}", tmp.display()))?;
    file.write_all(&launcher)
        .map_err(|e| format!("write: {e}"))?;

    let mut table: Vec<(String, u64, u64)> = Vec::new();
    let mut pos = launcher.len() as u64;
    for (key, source) in &cells {
        if key.len() >= minicon_com_rs::KEY_LEN {
            return Err(format!("cell key '{key}' is too long"));
        }
        let pad = (8 - (pos % 8)) % 8;
        if pad > 0 {
            file.write_all(&vec![0u8; pad as usize])
                .map_err(|e| format!("pad: {e}"))?;
            pos += pad;
        }
        let data: Vec<u8> = match source {
            Source::Bytes(bytes) => bytes.clone(),
            Source::File(path) => {
                let mut data = Vec::new();
                std::fs::File::open(path)
                    .map_err(|e| format!("open {}: {e}", path.display()))?
                    .read_to_end(&mut data)
                    .map_err(|e| format!("read {}: {e}", path.display()))?;
                data
            }
        };
        file.write_all(&data)
            .map_err(|e| format!("write cell: {e}"))?;
        table.push((key.clone(), pos, data.len() as u64));
        pos += data.len() as u64;
        println!(
            "cell {key:<14} {:>9} bytes @ {}",
            data.len(),
            pos - data.len() as u64
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
    drop(file);

    // Publish atomically: a failed pack must not leave a half-written artifact
    // under the final name.
    std::fs::rename(&tmp, &out).map_err(|e| {
        let _ = std::fs::remove_file(&tmp);
        format!("publish {}: {e}", out.display())
    })?;

    // Prove the result is readable before claiming success.
    let check = minicon_com_rs::read_table(&out)?;
    if check.len() != table.len() {
        return Err("packed table does not round-trip".into());
    }
    verify(&out)?;
    let size = std::fs::metadata(&out)
        .map_err(|e| format!("stat out: {e}"))?
        .len();
    println!("wrote {} ({size} bytes)", out.display());
    Ok(())
}

fn json_escape(value: &str) -> String {
    let mut out = String::with_capacity(value.len());
    for c in value.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out
}

#[derive(PartialEq)]
enum LauncherKind {
    Pe,
    Elf,
    MachO,
    Unknown,
}

fn launcher_kind(bytes: &[u8]) -> LauncherKind {
    if bytes.len() >= 2 && &bytes[0..2] == b"MZ" {
        LauncherKind::Pe
    } else if bytes.len() >= 4 && &bytes[0..4] == b"\x7fELF" {
        LauncherKind::Elf
    } else if bytes.len() >= 4
        && matches!(
            &bytes[0..4],
            b"\xcf\xfa\xed\xfe" | b"\xce\xfa\xed\xfe" | b"\xca\xfe\xba\xbe"
        )
    {
        LauncherKind::MachO
    } else {
        LauncherKind::Unknown
    }
}

/// Rewrite the PE optional header's Subsystem so a GUI product does not flash a
/// console window before its payload appears (and a console product keeps one).
fn patch_pe_subsystem(launcher: &mut [u8], subsystem: u16) -> Result<(), String> {
    if launcher.len() < 0x40 || &launcher[0..2] != b"MZ" {
        return Err("launcher is not a PE".into());
    }
    let e = u32::from_le_bytes(launcher[0x3c..0x40].try_into().unwrap()) as usize;
    if e + 0x18 + 0x46 > launcher.len() {
        return Err("PE headers are truncated".into());
    }
    if &launcher[e..e + 4] != b"PE\0\0" {
        return Err("bad PE signature".into());
    }
    let opt = e + 0x18;
    let magic = u16::from_le_bytes([launcher[opt], launcher[opt + 1]]);
    if magic != 0x10b && magic != 0x20b {
        return Err("unknown PE optional-header magic".into());
    }
    launcher[opt + 68..opt + 70].copy_from_slice(&subsystem.to_le_bytes());
    Ok(())
}

/// Human-readable structure: launcher format/subsystem, cells, metadata.
fn inspect(path: &Path) -> Result<(), String> {
    let bytes = std::fs::read(path).map_err(|e| format!("read {}: {e}", path.display()))?;
    let kind = match launcher_kind(&bytes) {
        LauncherKind::Pe => "PE",
        LauncherKind::Elf => "ELF",
        LauncherKind::MachO => "Mach-O",
        LauncherKind::Unknown => "unknown",
    };
    println!("file     : {} ({} bytes)", path.display(), bytes.len());
    if kind == "PE" {
        match pe_subsystem(&bytes) {
            Some(2) => println!("launcher : PE, subsystem gui"),
            Some(3) => println!("launcher : PE, subsystem console"),
            Some(other) => println!("launcher : PE, subsystem {other}"),
            None => println!("launcher : PE, subsystem unknown"),
        }
    } else {
        println!("launcher : {kind}");
    }
    let table = minicon_com_rs::read_table(path)?;
    for entry in &table {
        if entry.key == META_KEY {
            continue;
        }
        println!(
            "cell     : {:<14} {:>9} bytes @ {}",
            entry.key, entry.len, entry.offset
        );
    }
    if table.iter().any(|e| e.key == META_KEY) {
        let mut file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
        let meta = table.iter().find(|e| e.key == META_KEY).unwrap();
        file.seek(SeekFrom::Start(meta.offset))
            .map_err(|e| format!("seek: {e}"))?;
        let mut raw = vec![0u8; meta.len as usize];
        file.read_exact(&mut raw)
            .map_err(|e| format!("read: {e}"))?;
        println!("metadata : {}", String::from_utf8_lossy(&raw));
    } else {
        println!("metadata : none");
    }
    Ok(())
}

fn pe_subsystem(bytes: &[u8]) -> Option<u16> {
    if bytes.len() < 0x40 || &bytes[0..2] != b"MZ" {
        return None;
    }
    let e = u32::from_le_bytes(bytes[0x3c..0x40].try_into().ok()?) as usize;
    if e + 0x18 + 70 > bytes.len() {
        return None;
    }
    Some(u16::from_le_bytes([
        bytes[e + 0x18 + 68],
        bytes[e + 0x18 + 69],
    ]))
}
fn info(path: &Path) -> Result<(), String> {
    let table = minicon_com_rs::read_table(path)?;
    let Some(meta) = table.iter().find(|e| e.key == META_KEY) else {
        return Err("no metadata in this file".into());
    };
    let mut file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    file.seek(SeekFrom::Start(meta.offset))
        .map_err(|e| format!("seek: {e}"))?;
    let mut raw = vec![0u8; meta.len as usize];
    file.read_exact(&mut raw)
        .map_err(|e| format!("read: {e}"))?;
    println!("{}", String::from_utf8_lossy(&raw));
    Ok(())
}

/// Check that every cell's executable header matches the OS/arch its key names.
fn verify(path: &Path) -> Result<(), String> {
    let table = minicon_com_rs::read_table(path)?;
    let mut file = std::fs::File::open(path).map_err(|e| format!("open: {e}"))?;
    let mut bad = 0usize;
    for entry in &table {
        if entry.key == META_KEY {
            println!("ok   {:<14} metadata ({} bytes)", entry.key, entry.len);
            continue;
        }
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
