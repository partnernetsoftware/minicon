//! The container a packed `.com` carries after its launcher.
//!
//! Layout (all offsets absolute from the start of the file):
//!
//! ```text
//! [ launcher bytes ][ payload blobs ... ][ table ][ footer ]
//!                                              ^ 24 bytes at EOF:
//!     magic "MINICOM1" (8) | table offset (u64 LE) | table length (u64 LE)
//! ```
//!
//! Table entries are `key[32] | offset u64 | length u64`, where `key` is a
//! NUL-padded `{os}-{isa}` name such as `win-x86_64`. Appending is safe for
//! every loader that matters: PE, ELF and Mach-O all ignore trailing bytes.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const FOOTER_MAGIC: &[u8; 8] = b"MINICOM1";
pub const FOOTER_LEN: u64 = 24;
pub const KEY_LEN: usize = 32;
pub const ENTRY_LEN: u64 = (KEY_LEN + 8 + 8) as u64;

/// The `{os}-{isa}` cell this build of the launcher runs.
pub const fn host_key() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "win-x86_64"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "win-aarch64"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "lnx-x86_64"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "lnx-aarch64"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "osx-x86_64"
    }
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "osx-aarch64"
    }
}

#[derive(Clone, Debug)]
pub struct Entry {
    pub key: String,
    pub offset: u64,
    pub len: u64,
}

/// Read the container table from a packed file.
pub fn read_table(path: &Path) -> Result<Vec<Entry>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let size = f
        .metadata()
        .map_err(|e| format!("stat {}: {e}", path.display()))?
        .len();
    if size < FOOTER_LEN {
        return Err("file is too small to be packed".into());
    }
    f.seek(SeekFrom::End(-(FOOTER_LEN as i64)))
        .map_err(|e| format!("seek: {e}"))?;
    let mut footer = [0u8; FOOTER_LEN as usize];
    f.read_exact(&mut footer)
        .map_err(|e| format!("read footer: {e}"))?;
    if &footer[0..8] != FOOTER_MAGIC {
        return Err("no MINICOM1 footer (this binary was not packed)".into());
    }
    let table_off = u64::from_le_bytes(footer[8..16].try_into().unwrap());
    let table_len = u64::from_le_bytes(footer[16..24].try_into().unwrap());
    if table_len % ENTRY_LEN != 0
        || table_off < FOOTER_LEN
        || table_off
            .saturating_add(table_len)
            .saturating_add(FOOTER_LEN)
            != size
    {
        return Err("container table is malformed".into());
    }
    f.seek(SeekFrom::Start(table_off))
        .map_err(|e| format!("seek table: {e}"))?;
    let mut raw = vec![0u8; table_len as usize];
    f.read_exact(&mut raw)
        .map_err(|e| format!("read table: {e}"))?;
    let mut out = Vec::new();
    for chunk in raw.chunks(ENTRY_LEN as usize) {
        let key_end = chunk[..KEY_LEN]
            .iter()
            .position(|b| *b == 0)
            .unwrap_or(KEY_LEN);
        let key = String::from_utf8_lossy(&chunk[..key_end]).into_owned();
        let offset = u64::from_le_bytes(chunk[KEY_LEN..KEY_LEN + 8].try_into().unwrap());
        let len = u64::from_le_bytes(chunk[KEY_LEN + 8..KEY_LEN + 16].try_into().unwrap());
        // A cell must live before the table and carry at least one byte; a
        // corrupt file must fail here, not with an out-of-range read later.
        if key.is_empty()
            || len == 0
            || offset
                .checked_add(len)
                .map(|e| e > table_off)
                .unwrap_or(true)
        {
            return Err(format!("container entry '{key}' is out of bounds"));
        }
        out.push(Entry { key, offset, len });
    }
    Ok(out)
}
