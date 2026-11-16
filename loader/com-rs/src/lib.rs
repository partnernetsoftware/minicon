//! The container a packed `.com` carries after its launcher.
//!
//! Layout (all offsets absolute from the start of the file):
//!
//! ```text
//! [ launcher bytes ][ payload blobs ... ][ table ][ footer ]
//!                                              ^ 24 bytes:
//!     magic "MINICOM1" (8) | table offset (u64 LE) | table length (u64 LE)
//! ```
//!
//! Table entries are `key[32] | offset u64 | length u64`, where `key` is a
//! NUL-padded `{os}-{isa}` name such as `win-x86_64`. Appending is safe for
//! every loader that matters: PE, ELF and Mach-O all ignore trailing bytes.
//!
//! Authenticode coexistence: signing a PE appends the certificate-table blob
//! at the end of the file, *after* this footer. `read_table` therefore never
//! assumes the footer is the last 24 bytes. For a PE it first asks the
//! optional header's security data directory where the certificate overlay
//! begins, and looks for the footer immediately before it; otherwise (and as
//! a fallback) it scans a bounded window backwards from that boundary. The
//! strict shape rule stays: the table must end exactly where the footer
//! begins, so stray "MINICOM1" bytes inside a certificate cannot pass.

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

pub const FOOTER_MAGIC: &[u8; 8] = b"MINICOM1";
pub const FOOTER_LEN: u64 = 24;
pub const KEY_LEN: usize = 32;
pub const ENTRY_LEN: u64 = (KEY_LEN + 8 + 8) as u64;

/// How far back from the container boundary the footer may hide. Signature
/// overlays are a few KiB; 1 MiB leaves room for padding or debug data.
pub const MAX_OVERLAY_SCAN: u64 = 1 << 20;

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

/// Where the pack's own container must end: the start of the Authenticode
/// certificate overlay when the PE carries one, or the end of the file.
pub fn overlay_start(f: &mut std::fs::File, size: u64) -> u64 {
    let mut head = [0u8; 0x200];
    if f.rewind().is_err() || f.read_exact(&mut head).is_err() {
        return size;
    }
    if head.len() < 0x40 || &head[0..2] != b"MZ" {
        return size;
    }
    let pe = u32::from_le_bytes(head[0x3c..0x40].try_into().unwrap()) as usize;
    if pe + 0x18 + 0x70 > head.len() || &head[pe..pe + 4] != b"PE\0\0" {
        return size;
    }
    let opt = pe + 0x18;
    let magic = u16::from_le_bytes([head[opt], head[opt + 1]]);
    let dirs = match magic {
        0x20b => opt + 112,
        0x10b => opt + 96,
        _ => return size,
    };
    // Data directory 4 (security): its "RVA" is a file offset by definition.
    let sec = dirs + 4 * 8;
    if sec + 8 > head.len() {
        return size;
    }
    let cert_off = u32::from_le_bytes(head[sec..sec + 4].try_into().unwrap()) as u64;
    let cert_len = u32::from_le_bytes(head[sec + 4..sec + 8].try_into().unwrap()) as u64;
    if cert_off > 0 && cert_len > 0 && cert_off.checked_add(cert_len) == Some(size) {
        return cert_off;
    }
    size
}

/// Parse and strictly validate the footer read at `at` within `buf`, whose
/// absolute file offset is `footer_off`. Returns `(table_off, table_len)`
/// when the table ends exactly at the footer.
fn footer_at(buf: &[u8], at: usize, footer_off: u64) -> Option<(u64, u64)> {
    if footer_off < FOOTER_LEN {
        return None;
    }
    let i = at;
    if i + FOOTER_LEN as usize > buf.len() || &buf[i..i + 8] != FOOTER_MAGIC {
        return None;
    }
    let table_off = u64::from_le_bytes(buf[i + 8..i + 16].try_into().unwrap());
    let table_len = u64::from_le_bytes(buf[i + 16..i + 24].try_into().unwrap());
    if table_len == 0 || table_len % ENTRY_LEN != 0 {
        return None;
    }
    if table_off == 0 || table_off.checked_add(table_len) != Some(footer_off) {
        return None;
    }
    Some((table_off, table_len))
}

/// Locate the footer: try the last 24 bytes of the container region first,
/// then scan backwards over at most [`MAX_OVERLAY_SCAN`] bytes.
fn find_footer(f: &mut std::fs::File, size: u64) -> Result<(u64, u64, u64), String> {
    let end = overlay_start(f, size);
    if end < FOOTER_LEN {
        return Err("file is too small to be packed".into());
    }

    let mut tail = [0u8; FOOTER_LEN as usize];
    if f.seek(SeekFrom::Start(end - FOOTER_LEN)).is_ok() && f.read_exact(&mut tail).is_ok() {
        if let Some((table_off, table_len)) = footer_at(&tail, 0, end - FOOTER_LEN) {
            return Ok((end - FOOTER_LEN, table_off, table_len));
        }
    }

    let window = MAX_OVERLAY_SCAN.min(end - FOOTER_LEN);
    let start = end - FOOTER_LEN - window;
    let mut buf = vec![0u8; (window + FOOTER_LEN) as usize];
    f.seek(SeekFrom::Start(start))
        .map_err(|e| format!("seek scan window: {e}"))?;
    f.read_exact(&mut buf)
        .map_err(|e| format!("read scan window: {e}"))?;
    for i in (0..=window as usize).rev() {
        if let Some((table_off, table_len)) = footer_at(&buf, i, start + i as u64) {
            return Ok((start + i as u64, table_off, table_len));
        }
    }
    Err("no MINICOM1 footer (this binary was not packed)".into())
}

/// Read the container table from a packed file.
pub fn read_table(path: &Path) -> Result<Vec<Entry>, String> {
    let mut f = std::fs::File::open(path).map_err(|e| format!("open {}: {e}", path.display()))?;
    let size = f
        .metadata()
        .map_err(|e| format!("stat {}: {e}", path.display()))?
        .len();
    let (footer_off, table_off, table_len) = find_footer(&mut f, size)?;
    if footer_off.saturating_add(FOOTER_LEN) > size {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn table_bytes(entries: &[(&str, u64, u64)]) -> Vec<u8> {
        let mut v = Vec::new();
        for (key, off, len) in entries {
            let mut k = [0u8; KEY_LEN];
            k[..key.len()].copy_from_slice(key.as_bytes());
            v.extend_from_slice(&k);
            v.extend_from_slice(&off.to_le_bytes());
            v.extend_from_slice(&len.to_le_bytes());
        }
        v
    }

    fn footer(table_off: u64, table_len: u64) -> Vec<u8> {
        let mut v = FOOTER_MAGIC.to_vec();
        v.extend_from_slice(&table_off.to_le_bytes());
        v.extend_from_slice(&table_len.to_le_bytes());
        v
    }

    /// A fresh scratch dir per call: parallel test threads must not share
    /// one file name, and a file a scanner still holds must not block
    /// another test's write on Windows.
    fn scratch(name: &str) -> std::path::PathBuf {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQ: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "com-rs-test-{}-{}",
            std::process::id(),
            SEQ.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&dir).unwrap();
        dir.join(name)
    }

    /// launcher(64) + one cell + table + footer, then `extra` trailing bytes.
    fn packed_with(extra: &[u8]) -> std::path::PathBuf {
        let path = scratch("probe.com");
        let launcher = vec![0x90u8; 64];
        let cell = b"payload-bytes".to_vec();
        let table = table_bytes(&[("win-x86_64", 64, cell.len() as u64)]);
        let table_off = (launcher.len() + cell.len()) as u64;
        let mut file = Vec::new();
        file.extend_from_slice(&launcher);
        file.extend_from_slice(&cell);
        file.extend_from_slice(&table);
        file.extend_from_slice(&footer(table_off, table.len() as u64));
        file.extend_from_slice(extra);
        std::fs::write(&path, &file).unwrap();
        path
    }

    #[test]
    fn footer_at_eof_round_trips() {
        let path = packed_with(b"");
        let table = read_table(&path).unwrap();
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].key, "win-x86_64");
        assert_eq!(table[0].offset, 64);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn certificate_overlay_after_footer_still_reads() {
        // signtool shape: an arbitrary certificate blob follows the footer.
        let path = packed_with(&[0xa5u8; 4096]);
        let table = read_table(&path).unwrap();
        assert_eq!(table.len(), 1);
        assert_eq!(table[0].key, "win-x86_64");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn security_directory_bounds_the_search() {
        // A PE whose security directory points at a blob that itself
        // contains a fake footer; the real footer before the blob must win.
        // The launcher is large enough that the real container lives beyond
        // the 0x200 PE header this test patches in place.
        let mut file = vec![0x90u8; 0x400];
        let cell = b"payload-bytes".to_vec();
        let table = table_bytes(&[("win-x86_64", file.len() as u64, cell.len() as u64)]);
        let table_off = (file.len() + cell.len()) as u64;
        file.extend_from_slice(&cell);
        file.extend_from_slice(&table);
        file.extend_from_slice(&footer(table_off, table.len() as u64));

        let cert_blob = {
            let table = table_bytes(&[("fake-cell", 1, 1)]);
            let mut blob = table.clone();
            blob.extend_from_slice(&footer(1, table.len() as u64));
            blob.extend_from_slice(&[0xc3u8; 512]);
            blob
        };
        let cert_off = file.len() as u64;

        // Minimal PE header patched onto the file's own head: MZ,
        // e_lfanew -> PE\0\0, PE32+ optional header, 16 data directories,
        // security (index 4) = (cert_off, blob len).
        file[0..2].copy_from_slice(b"MZ");
        file[0x3c..0x40].copy_from_slice(&0x80u32.to_le_bytes());
        file[0x80..0x84].copy_from_slice(b"PE\0\0");
        file[0x98..0x9a].copy_from_slice(&0x20bu16.to_le_bytes());
        let dirs = 0x80 + 0x18 + 112;
        file[dirs + 32..dirs + 36].copy_from_slice(&(cert_off as u32).to_le_bytes());
        file[dirs + 36..dirs + 40].copy_from_slice(&(cert_blob.len() as u32).to_le_bytes());
        file.extend_from_slice(&cert_blob);

        let path = scratch("secdir.com");
        std::fs::write(&path, &file).unwrap();
        let table = read_table(&path).unwrap();
        assert_eq!(
            table.len(),
            1,
            "real footer must win over the fake one in the blob"
        );
        assert_eq!(table[0].key, "win-x86_64");
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn corrupted_footer_is_rejected() {
        let path = packed_with(b"");
        let mut file = std::fs::read(&path).unwrap();
        let magic_at = file.len() - 24;
        file[magic_at + 1] ^= 0xff;
        std::fs::write(&path, &file).unwrap();
        assert!(read_table(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn dangling_footer_without_table_is_rejected() {
        // A footer whose table does not end exactly at the footer must fail,
        // even though the magic bytes exist.
        let path = scratch("dangling.com");
        let mut file = vec![0x90u8; 64];
        let table = table_bytes(&[("win-x86_64", 1, 1)]);
        let table_off = file.len() as u64;
        file.extend_from_slice(&table);
        file.extend_from_slice(&[0u8; 8]); // gap breaks table_end == footer_off
        file.extend_from_slice(&footer(table_off, table.len() as u64));
        std::fs::write(&path, &file).unwrap();
        assert!(read_table(&path).is_err());
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn entry_out_of_bounds_is_rejected() {
        let path = scratch("oob.com");
        let mut file = vec![0x90u8; 64];
        let table = table_bytes(&[("win-x86_64", 0xffff_0000, 0xffff_0000)]);
        let table_off = file.len() as u64;
        file.extend_from_slice(&table);
        file.extend_from_slice(&footer(table_off, table.len() as u64));
        std::fs::write(&path, &file).unwrap();
        assert!(read_table(&path).is_err());
        std::fs::remove_file(&path).ok();
    }
}
