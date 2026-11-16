//! Minimal PNG encoder used by the `webgl.*` native software backend.
//!
//! Writes 8-bit RGBA PNGs with **stored** (uncompressed) deflate blocks, so the
//! encoder needs no third-party crate and stays deterministic. That also makes
//! the output easy to verify: a reader can walk the blocks and check the Adler-32.

const SIGNATURE: [u8; 8] = [0x89, 0x50, 0x4E, 0x47, 0x0D, 0x0A, 0x1A, 0x0A];

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = 0xFFFF_FFFFu32;
    for byte in bytes {
        crc ^= *byte as u32;
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
        }
    }
    !crc
}

fn adler32(bytes: &[u8]) -> u32 {
    let mut a: u32 = 1;
    let mut b: u32 = 0;
    for byte in bytes {
        a = (a + *byte as u32) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    let mut crc_input = Vec::with_capacity(4 + data.len());
    crc_input.extend_from_slice(kind);
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc_input);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

fn push_stored_zlib(out: &mut Vec<u8>, raw: &[u8]) {
    out.push(0x78);
    out.push(0x01);
    let mut offset = 0usize;
    while offset < raw.len() {
        let take = (raw.len() - offset).min(65_535);
        let final_block = offset + take >= raw.len();
        out.push(if final_block { 0x01 } else { 0x00 });
        out.extend_from_slice(&(take as u16).to_le_bytes());
        out.extend_from_slice(&(!(take as u16)).to_le_bytes());
        out.extend_from_slice(&raw[offset..offset + take]);
        offset += take;
    }
    if raw.is_empty() {
        out.push(0x01);
        out.extend_from_slice(&0u16.to_le_bytes());
        out.extend_from_slice(&0xFFFFu16.to_le_bytes());
    }
    out.extend_from_slice(&adler32(raw).to_be_bytes());
}

/// Encode an RGBA framebuffer (width * height * 4 bytes) as a PNG file.
pub fn encode_rgba(width: u32, height: u32, pixels: &[u8]) -> Result<Vec<u8>, String> {
    if width == 0 || height == 0 {
        return Err("png: zero-sized image".to_string());
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or_else(|| "png: image is too large".to_string())?;
    if pixels.len() != expected {
        return Err(format!(
            "png: expected {} bytes, got {}",
            expected,
            pixels.len()
        ));
    }
    let mut raw = Vec::with_capacity(expected + height as usize);
    for row in pixels.chunks(width as usize * 4) {
        raw.push(0); // filter: none
        raw.extend_from_slice(row);
    }
    let mut out = Vec::new();
    out.extend_from_slice(&SIGNATURE);
    let mut ihdr = Vec::with_capacity(13);
    ihdr.extend_from_slice(&width.to_be_bytes());
    ihdr.extend_from_slice(&height.to_be_bytes());
    ihdr.push(8); // bit depth
    ihdr.push(6); // colour type: RGBA
    ihdr.push(0); // compression
    ihdr.push(0); // filter
    ihdr.push(0); // interlace
    push_chunk(&mut out, b"IHDR", &ihdr);
    let mut idat = Vec::new();
    push_stored_zlib(&mut idat, &raw);
    push_chunk(&mut out, b"IDAT", &idat);
    push_chunk(&mut out, b"IEND", &[]);
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn read_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_be_bytes([
            bytes[offset],
            bytes[offset + 1],
            bytes[offset + 2],
            bytes[offset + 3],
        ])
    }

    fn parse_chunks(png: &[u8]) -> Vec<(&'static [u8; 4], Vec<u8>)> {
        let mut chunks = Vec::new();
        let mut offset = 8usize;
        while offset + 8 <= png.len() {
            let len = read_u32(png, offset) as usize;
            let kind: [u8; 4] = png[offset + 4..offset + 8].try_into().expect("kind");
            let data = png[offset + 8..offset + 8 + len].to_vec();
            let crc = read_u32(png, offset + 8 + len);
            let mut crc_input = Vec::new();
            crc_input.extend_from_slice(&kind);
            crc_input.extend_from_slice(&data);
            assert_eq!(crc, super::crc32(&crc_input), "chunk CRC must match");
            let name: &'static [u8; 4] = match &kind {
                b"IHDR" => b"IHDR",
                b"IDAT" => b"IDAT",
                b"IEND" => b"IEND",
                other => panic!("unexpected chunk {other:?}"),
            };
            chunks.push((name, data));
            offset += 12 + len;
        }
        chunks
    }

    #[test]
    fn encodes_a_readable_rgba_png() {
        let pixels = vec![255u8, 0, 0, 255, 0, 255, 0, 128];
        let png = encode_rgba(2, 1, &pixels).expect("encode");
        assert_eq!(&png[0..8], &SIGNATURE);
        let chunks = parse_chunks(&png);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].0, b"IHDR");
        assert_eq!(read_u32(&chunks[0].1, 0), 2);
        assert_eq!(read_u32(&chunks[0].1, 4), 1);
        assert_eq!(chunks[1].0, b"IDAT");
        assert_eq!(chunks[2].0, b"IEND");
        // IDAT = zlib header (2) + stored block header (5) + raw rows + adler (4).
        let idat = &chunks[1].1;
        assert_eq!(idat[0], 0x78);
        assert_eq!(idat[1], 0x01);
        let raw_len = 1 + 8; // one filter byte + one RGBA row
        assert_eq!(idat.len(), 2 + 5 + raw_len + 4);
        let stored = &idat[7..7 + raw_len];
        assert_eq!(stored[0], 0, "filter byte");
        assert_eq!(&stored[1..5], &pixels[0..4]);
        assert_eq!(adler32(stored), read_u32(idat, idat.len() - 4));
    }

    #[test]
    fn rejects_mismatched_buffers() {
        assert!(encode_rgba(0, 1, &[]).is_err());
        assert!(encode_rgba(1, 1, &[0, 0, 0]).is_err());
    }
}
