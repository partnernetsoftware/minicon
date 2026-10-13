//! Minimal readers for the two archive shapes a release publishes.
//!
//! Releases ship one archive per platform plus a SHA-256 sidecar, and the
//! updater has to get the executable out of that archive. The product's size
//! story argues against a general archive crate, and the issue that asked for
//! this feature recorded the choice explicitly (option (a): a small pure-Rust
//! reader for the exact formats shipped). Only the two shapes this repository
//! actually publishes are read — a deflated `.zip` entry and a gzipped tar
//! member — and anything else is a typed refusal, never a guess.
//!
//! Both readers take the whole archive as one slice because the download
//! ceiling already bounded it; a streaming API would buy nothing here.

use std::io::Read;

/// Why an archive could not produce the expected executable.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ArchiveError {
    /// The bytes are not the format the caller named, or are cut short.
    Malformed,
    /// The archive is readable but holds no entry with the expected name.
    EntryMissing,
    /// The entry carries a compression method this reader does not implement.
    UnsupportedCompression,
    /// The inflated bytes did not match the size the archive declared.
    SizeMismatch,
}

impl std::fmt::Display for ArchiveError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let text = match self {
            Self::Malformed => "the archive is malformed",
            Self::EntryMissing => "the archive does not contain the expected executable",
            Self::UnsupportedCompression => "the archive uses an unsupported compression method",
            Self::SizeMismatch => "the archive entry did not decode to its declared size",
        };
        formatter.write_str(text)
    }
}

/// Reads the single entry named `expected` out of a `.zip`.
///
/// The central directory is the authority — it is what a real zip tool reads,
/// and walking the local headers instead would find entries a truncated or
/// hand-made archive never registered. A Zip64 marker is refused rather than
/// truncated to 32 bits, because the archives this repository publishes are
/// always small and a silent wrap would be a wrong answer, not a slow one.
pub fn read_zip(bytes: &[u8], expected: &str) -> Result<Vec<u8>, ArchiveError> {
    let end = find_zip_end(bytes).ok_or(ArchiveError::Malformed)?;
    let entries = u16::from_le_bytes(read_array(bytes, end + 10)?);
    let directory = u32::from_le_bytes(read_array(bytes, end + 16)?) as usize;
    let mut cursor = directory;
    for _ in 0..entries {
        if read_array::<4>(bytes, cursor)? != [0x50, 0x4b, 0x01, 0x02] {
            return Err(ArchiveError::Malformed);
        }
        let method = u16::from_le_bytes(read_array(bytes, cursor + 10)?);
        let compressed = u32::from_le_bytes(read_array(bytes, cursor + 20)?);
        let uncompressed = u32::from_le_bytes(read_array(bytes, cursor + 24)?);
        let name_length = u16::from_le_bytes(read_array(bytes, cursor + 28)?) as usize;
        let extra_length = u16::from_le_bytes(read_array(bytes, cursor + 30)?) as usize;
        let comment_length = u16::from_le_bytes(read_array(bytes, cursor + 32)?) as usize;
        let local = u32::from_le_bytes(read_array(bytes, cursor + 42)?) as usize;
        let name = read_slice(bytes, cursor + 46, name_length)?;
        let name = std::str::from_utf8(name).map_err(|_| ArchiveError::Malformed)?;
        if compressed == u32::MAX || uncompressed == u32::MAX {
            return Err(ArchiveError::UnsupportedCompression);
        }
        if base_name(name) == expected {
            let local_name = u16::from_le_bytes(read_array(bytes, local + 26)?) as usize;
            let local_extra = u16::from_le_bytes(read_array(bytes, local + 28)?) as usize;
            let start = local + 30 + local_name + local_extra;
            let data = read_slice(bytes, start, compressed as usize)?;
            return inflate(data, method, uncompressed as usize);
        }
        cursor = cursor + 46 + name_length + extra_length + comment_length;
    }
    Err(ArchiveError::EntryMissing)
}

/// Reads the single member named `expected` out of a `.tar.gz`.
pub fn read_tar_gz(bytes: &[u8], expected: &str) -> Result<Vec<u8>, ArchiveError> {
    let mut tar = Vec::new();
    flate2::read::GzDecoder::new(bytes)
        .read_to_end(&mut tar)
        .map_err(|_| ArchiveError::Malformed)?;
    let mut cursor = 0usize;
    while cursor + 512 <= tar.len() {
        let header = read_slice(&tar, cursor, 512)?;
        // A zero block ends the archive; the earlier blocks are padding.
        if header.iter().all(|byte| *byte == 0) {
            break;
        }
        let raw_name = read_slice(header, 0, 100)?;
        let end = raw_name
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(raw_name.len());
        let name = std::str::from_utf8(&raw_name[..end]).map_err(|_| ArchiveError::Malformed)?;
        let size = parse_tar_size(read_slice(header, 124, 12)?);
        let typeflag = read_array::<1>(header, 156)?[0];
        let data = read_slice(&tar, cursor + 512, size)?;
        if matches!(typeflag, b'0' | 0) && base_name(name) == expected {
            return Ok(data.to_vec());
        }
        cursor += 512 + size.div_ceil(512) * 512;
    }
    Err(ArchiveError::EntryMissing)
}

fn inflate(data: &[u8], method: u16, expected: usize) -> Result<Vec<u8>, ArchiveError> {
    let mut out = Vec::with_capacity(expected);
    match method {
        0 => out.extend_from_slice(data),
        8 => {
            flate2::read::DeflateDecoder::new(data)
                .read_to_end(&mut out)
                .map_err(|_| ArchiveError::Malformed)?;
        }
        _ => return Err(ArchiveError::UnsupportedCompression),
    }
    if out.len() != expected {
        return Err(ArchiveError::SizeMismatch);
    }
    Ok(out)
}

/// The offset of the end-of-central-directory record.
///
/// Its trailing comment is the only variable-length part of a zip, so the
/// signature is searched for from the back within the largest comment a 16-bit
/// length can describe.
fn find_zip_end(bytes: &[u8]) -> Option<usize> {
    let floor = bytes.len().saturating_sub(22 + u16::MAX as usize);
    (floor..bytes.len().saturating_sub(3))
        .rev()
        .find(|offset| read_array::<4>(bytes, *offset) == Ok([0x50, 0x4b, 0x05, 0x06]))
}

/// A tar size field: octal ASCII, or the base-256 form a tar writes for a size
/// that does not fit in octal (the high bit of the first byte marks it).
///
/// A field this reader cannot read is zero rather than an error: tar pads a
/// header's tail with NULs, and treating padding as a malformed size would
/// refuse archives that are perfectly valid.
fn parse_tar_size(raw: &[u8]) -> usize {
    if raw.first().is_some_and(|byte| byte & 0x80 != 0) {
        return raw[1..].iter().fold(0usize, |value, byte| {
            value.saturating_mul(256).saturating_add(usize::from(*byte))
        });
    }
    raw.iter()
        .copied()
        .take_while(|byte| matches!(byte, b'0'..=b'7'))
        .fold(0usize, |value, byte| {
            value
                .saturating_mul(8)
                .saturating_add(usize::from(byte - b'0'))
        })
}

fn base_name(path: &str) -> &str {
    path.rsplit(['/', '\\']).next().unwrap_or(path)
}

fn read_slice(bytes: &[u8], offset: usize, length: usize) -> Result<&[u8], ArchiveError> {
    let end = offset.checked_add(length).ok_or(ArchiveError::Malformed)?;
    bytes.get(offset..end).ok_or(ArchiveError::Malformed)
}

fn read_array<const N: usize>(bytes: &[u8], offset: usize) -> Result<[u8; N], ArchiveError> {
    read_slice(bytes, offset, N)?
        .try_into()
        .map_err(|_| ArchiveError::Malformed)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PAYLOAD: &[u8] = b"minicon-update-fixture-payload\n";

    #[test]
    fn a_deflated_zip_entry_is_read_by_name() {
        let archive = include_bytes!("../tests/assets/update-archive.zip");
        assert_eq!(read_zip(archive, "minicon.exe").unwrap(), PAYLOAD);
    }

    #[test]
    fn a_gzipped_tar_member_is_read_by_name() {
        let archive = include_bytes!("../tests/assets/update-archive.tar.gz");
        assert_eq!(read_tar_gz(archive, "minicon").unwrap(), PAYLOAD);
    }

    #[test]
    fn an_archive_without_the_expected_entry_is_a_typed_refusal() {
        let zip = include_bytes!("../tests/assets/update-archive.zip");
        assert_eq!(
            read_zip(zip, "minicon").unwrap_err(),
            ArchiveError::EntryMissing
        );
        let tar = include_bytes!("../tests/assets/update-archive.tar.gz");
        assert_eq!(
            read_tar_gz(tar, "minicon.exe").unwrap_err(),
            ArchiveError::EntryMissing
        );
    }

    #[test]
    fn bytes_that_are_not_an_archive_are_refused() {
        assert_eq!(
            read_zip(b"not a zip at all", "minicon.exe").unwrap_err(),
            ArchiveError::Malformed
        );
        assert_eq!(
            read_tar_gz(b"not gzip at all", "minicon").unwrap_err(),
            ArchiveError::Malformed
        );
    }
}
