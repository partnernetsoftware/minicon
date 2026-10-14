//! Composer image/screenshot paste, file-path fallback (`F1` in
//! `plan/plan-carried-debt.md`, decided shape in
//! `prd/PRD_02_25_con_workspace.md`'s "Screenshot/image paste into the
//! composer").
//!
//! `minicon_platform::clipboard` already exposes a generic
//! `available_types()`/`get_type()` pair across every OS adapter; no new
//! platform-crate API is needed. What's MiniCon-owned is picking the right
//! host type name and turning the bytes into a temp file whose path the
//! composer can insert like any other pasted text.

use std::{
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use minicon_platform::contract::clipboard::MAX_CLIPBOARD_TYPE_BYTES;

/// One host-native clipboard type that carries image bytes, tried in order
/// against whatever `available_types()` actually reports. Names are the
/// host's own spelling (macOS UTIs, Windows registered formats, X11/Wayland
/// MIME types) -- see `crates/minicon-platform/src/adapters/*/clipboard.rs`
/// in the `agenterm` repo.
struct ImageTypeCandidate {
    name: &'static str,
    extension: &'static str,
    conversion: NeedsConversion,
}

/// How a candidate's raw clipboard bytes become a file on disk.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum NeedsConversion {
    /// The payload already is a self-contained file (png/jpeg); write it
    /// verbatim under `extension`.
    None,
    /// A headerless Windows DIB (`CF_DIB`/`CF_DIBV5`); re-encode the pixels to
    /// PNG, falling back to the wrapped-BMP form if that fails.
    Dib,
}

const IMAGE_TYPE_CANDIDATES: &[ImageTypeCandidate] = &[
    // Windows programs (snipping tool, browsers, Office) often register a
    // `PNG` format holding a complete PNG file -- cheapest and already small,
    // so prefer it over everything else, including the bare DIBs below.
    ImageTypeCandidate {
        name: "PNG",
        extension: "png",
        conversion: NeedsConversion::None,
    },
    ImageTypeCandidate {
        name: "public.png",
        extension: "png",
        conversion: NeedsConversion::None,
    },
    ImageTypeCandidate {
        name: "image/png",
        extension: "png",
        conversion: NeedsConversion::None,
    },
    ImageTypeCandidate {
        name: "public.jpeg",
        extension: "jpg",
        conversion: NeedsConversion::None,
    },
    ImageTypeCandidate {
        name: "image/jpeg",
        extension: "jpg",
        conversion: NeedsConversion::None,
    },
    ImageTypeCandidate {
        name: "CF_DIBV5",
        extension: "bmp",
        conversion: NeedsConversion::Dib,
    },
    ImageTypeCandidate {
        name: "CF_DIB",
        extension: "bmp",
        conversion: NeedsConversion::Dib,
    },
];

/// Picks the first candidate type name this clipboard actually reports, in
/// [`IMAGE_TYPE_CANDIDATES`] order. `available_types` reporting `Unsupported`
/// (a host with no way to enumerate types) is not treated as "no image" --
/// callers that need a fallback should still try `get_type` directly, which
/// this function does not do.
fn matching_image_type(available: &[String]) -> Option<&'static ImageTypeCandidate> {
    IMAGE_TYPE_CANDIDATES.iter().find(|candidate| {
        available
            .iter()
            .any(|available| available.as_str() == candidate.name)
    })
}

/// If the clipboard currently carries an image (checked through
/// `available_types`, host-native type names only), reads it and saves it to
/// a fresh temp file. Returns the absolute path as a `String`, ready to
/// insert into the composer draft exactly like pasted text.
///
/// Returns `None` when the clipboard holds no recognized image type, when the
/// host can't enumerate types at all, or when the read/write itself fails --
/// none of those are errors worth surfacing to the composer; the caller falls
/// back to a normal text paste.
pub fn image_paste_as_temp_file_path() -> Option<String> {
    let available = minicon_platform::clipboard::available_types().ok()?;
    let candidate = matching_image_type(&available)?;
    let bytes =
        minicon_platform::clipboard::get_type(candidate.name, MAX_CLIPBOARD_TYPE_BYTES).ok()?;
    let (bytes, extension) = image_file_bytes(candidate, bytes)?;
    write_temp_image_file(&bytes, extension).ok()
}

/// Turns the raw clipboard bytes for `candidate` into `(file bytes, extension)`
/// for a real file on disk: self-contained formats (png/jpeg) pass through
/// untouched, headerless Windows DIBs are re-encoded to PNG (smaller, so the
/// downstream model reads it more easily), falling back to a wrapped BMP only
/// if the DIB cannot be decoded -- a BMP the caller can still open beats `None`.
fn image_file_bytes(
    candidate: &ImageTypeCandidate,
    bytes: Vec<u8>,
) -> Option<(Vec<u8>, &'static str)> {
    match candidate.conversion {
        NeedsConversion::None => Some((bytes, candidate.extension)),
        NeedsConversion::Dib => {
            if let Some(png) = dib_to_png_file_bytes(&bytes) {
                return Some((png, "png"));
            }
            dib_to_bmp_file_bytes(&bytes).map(|bmp| (bmp, "bmp"))
        }
    }
}

fn read_u16_le(bytes: &[u8], offset: usize) -> Option<u16> {
    let slice = bytes.get(offset..offset.checked_add(2)?)?;
    Some(u16::from_le_bytes(slice.try_into().ok()?))
}

fn read_u32_le(bytes: &[u8], offset: usize) -> Option<u32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(u32::from_le_bytes(slice.try_into().ok()?))
}

fn read_i32_le(bytes: &[u8], offset: usize) -> Option<i32> {
    let slice = bytes.get(offset..offset.checked_add(4)?)?;
    Some(i32::from_le_bytes(slice.try_into().ok()?))
}

/// Info-header size and the number of palette/mask bytes that follow it inside
/// a headerless DIB, or `None` when the header is too short or inconsistent
/// with the buffer. The BMP wrapper and the PNG decoder must agree on where the
/// colour table ends, so both go through this one calculator.
fn dib_header_and_palette_bytes(dib: &[u8]) -> Option<(usize, usize)> {
    const BITMAPINFOHEADER_SIZE: usize = 40;
    const BI_BITFIELDS: u32 = 3;
    const BITFIELDS_MASK_BYTES: usize = 12; // 3 DWORD colour masks

    // First u32 of a DIB is its info-header size (40/108/124, and other
    // reasonable values); the fields read below all live at BITMAPINFOHEADER
    // offsets, so reject anything smaller or claiming more than we hold.
    let header_size = read_u32_le(dib, 0)? as usize;
    if header_size < BITMAPINFOHEADER_SIZE || header_size > dib.len() {
        return None;
    }
    let bit_count = read_u16_le(dib, 14)?;
    let compression = read_u32_le(dib, 16)?;
    let clr_used = read_u32_le(dib, 32)?;

    let color_count: u32 = if clr_used != 0 {
        clr_used
    } else if bit_count <= 8 {
        1u32 << u32::from(bit_count)
    } else {
        0
    };
    let mut palette_bytes = 4usize.checked_mul(color_count as usize)?;

    // A 40-byte BITMAPINFOHEADER with BI_BITFIELDS keeps its colour masks
    // right after the header instead of inside it: 3 DWORDs, or 4 when an
    // alpha mask is present (hosts report that via biClrUsed==3/4, which the
    // `color_count` branch above already folds in). biClrUsed==0 leaves them
    // uncounted, so add the 3 DWORDs here. V4/V5 headers carry their masks
    // inside the header and must not add them again.
    if header_size == BITMAPINFOHEADER_SIZE && compression == BI_BITFIELDS && clr_used == 0 {
        palette_bytes = palette_bytes.checked_add(BITFIELDS_MASK_BYTES)?;
    }
    Some((header_size, palette_bytes))
}

/// Wraps a headerless Windows DIB (a `CF_DIB`/`CF_DIBV5` payload) into a
/// complete BMP file: a 14-byte `BITMAPFILEHEADER` followed by the DIB bytes
/// verbatim. Returns `None` for anything too short or malformed to describe
/// safely (never panics).
pub(crate) fn dib_to_bmp_file_bytes(dib: &[u8]) -> Option<Vec<u8>> {
    const FILE_HEADER_SIZE: usize = 14;

    let (header_size, palette_bytes) = dib_header_and_palette_bytes(dib)?;

    let off_bits = FILE_HEADER_SIZE
        .checked_add(header_size)?
        .checked_add(palette_bytes)?;
    let file_size = FILE_HEADER_SIZE.checked_add(dib.len())?;
    if off_bits > file_size || file_size > u32::MAX as usize {
        return None;
    }

    let mut out = Vec::with_capacity(file_size);
    out.extend_from_slice(b"BM");
    out.extend_from_slice(&(file_size as u32).to_le_bytes());
    out.extend_from_slice(&0u16.to_le_bytes()); // bfReserved1
    out.extend_from_slice(&0u16.to_le_bytes()); // bfReserved2
    out.extend_from_slice(&(off_bits as u32).to_le_bytes());
    out.extend_from_slice(dib);
    Some(out)
}

/// Decodes a headerless Windows DIB (`CF_DIB`/`CF_DIBV5`) and re-encodes its
/// pixels as a complete PNG file (always RGBA8). Returns `None` for anything
/// this deliberately narrow reader does not handle -- non-`BI_RGB` compression,
/// bit depths other than 8/24/32, zero/negative dimensions, or a buffer too
/// short for the pixels it claims. Every step is checked; it never panics.
pub(crate) fn dib_to_png_file_bytes(dib: &[u8]) -> Option<Vec<u8>> {
    const BI_RGB: u32 = 0;

    let (header_size, palette_bytes) = dib_header_and_palette_bytes(dib)?;
    let width_i = read_i32_le(dib, 4)?;
    let height_i = read_i32_le(dib, 8)?;
    let bit_count = read_u16_le(dib, 14)?;
    let compression = read_u32_le(dib, 16)?;

    if compression != BI_RGB || width_i <= 0 || height_i == 0 {
        return None;
    }
    if bit_count != 8 && bit_count != 24 && bit_count != 32 {
        return None;
    }

    let top_down = height_i < 0;
    let width = width_i as usize;
    let height = height_i.unsigned_abs() as usize;

    // DIB rows are padded up to a 4-byte boundary.
    let row_bits = width.checked_mul(usize::from(bit_count))?;
    let stride = row_bits.checked_add(31)? / 32 * 4;
    let pixels_offset = header_size.checked_add(palette_bytes)?;
    let pixels_end = pixels_offset.checked_add(stride.checked_mul(height)?)?;
    if pixels_end > dib.len() {
        return None;
    }

    let mut rgba = Vec::with_capacity(width.checked_mul(height)?.checked_mul(4)?);
    for out_row in 0..height {
        // bottom-up (positive height) stores the image's bottom row first.
        let src_row = if top_down {
            out_row
        } else {
            height - 1 - out_row
        };
        let row_start = pixels_offset.checked_add(src_row.checked_mul(stride)?)?;
        let row = dib.get(row_start..row_start.checked_add(stride)?)?;
        match bit_count {
            24 => {
                for x in 0..width {
                    let p = row.get(x * 3..x * 3 + 3)?;
                    rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                }
            }
            32 => {
                for x in 0..width {
                    let p = row.get(x * 4..x * 4 + 4)?;
                    rgba.extend_from_slice(&[p[2], p[1], p[0], p[3]]);
                }
            }
            _ => {
                // 8bpp: one palette index per pixel; palette entries are BGRA.
                for x in 0..width {
                    let entry = usize::from(*row.get(x)?)
                        .checked_mul(4)
                        .and_then(|offset| header_size.checked_add(offset))
                        .and_then(|offset| dib.get(offset..offset.checked_add(4)?));
                    match entry {
                        Some(e) => rgba.extend_from_slice(&[e[2], e[1], e[0], 255]),
                        None => rgba.extend_from_slice(&[0, 0, 0, 255]),
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut out, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().ok()?;
        writer.write_image_data(&rgba).ok()?;
        writer.finish().ok()?;
    }
    Some(out)
}

fn write_temp_image_file(bytes: &[u8], extension: &str) -> std::io::Result<String> {
    let unique = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_nanos())
        .unwrap_or(0);
    let path: PathBuf = std::env::temp_dir().join(format!(
        "minicon-paste-{}-{unique}.{extension}",
        std::process::id()
    ));
    let mut file = std::fs::File::create(&path)?;
    file.write_all(bytes)?;
    path.into_os_string()
        .into_string()
        .map_err(|_| std::io::Error::other("temp image path is not valid UTF-8"))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Builds a minimal DIB with a `header_size`-byte info header and `tail`
    /// trailing bytes (palette/masks/pixels, caller-controlled).
    fn make_dib(
        header_size: u32,
        bit_count: u16,
        compression: u32,
        clr_used: u32,
        tail: usize,
    ) -> Vec<u8> {
        let mut dib = vec![0u8; header_size as usize + tail];
        dib[0..4].copy_from_slice(&header_size.to_le_bytes());
        dib[14..16].copy_from_slice(&bit_count.to_le_bytes());
        dib[16..20].copy_from_slice(&compression.to_le_bytes());
        dib[32..36].copy_from_slice(&clr_used.to_le_bytes());
        dib
    }

    fn le_u32(bytes: &[u8], offset: usize) -> u32 {
        u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
    }

    #[test]
    fn matching_image_type_prefers_earlier_candidates_in_order() {
        let available = vec!["image/jpeg".to_owned(), "public.png".to_owned()];
        let matched = matching_image_type(&available).expect("public.png is present");
        assert_eq!(
            matched.name, "public.png",
            "public.png is listed before image/jpeg in IMAGE_TYPE_CANDIDATES"
        );
    }

    #[test]
    fn matching_image_type_is_none_for_text_only_types() {
        let available = vec!["text/plain".to_owned(), "UTF8_STRING".to_owned()];
        assert!(matching_image_type(&available).is_none());
    }

    #[test]
    fn matching_image_type_prefers_the_windows_png_format_first() {
        let available = vec!["CF_DIB".to_owned(), "PNG".to_owned()];
        let matched = matching_image_type(&available).expect("PNG is present");
        assert_eq!(
            matched.name, "PNG",
            "the PNG candidate must win over CF_DIB"
        );
        assert_eq!(matched.extension, "png");
        assert_eq!(matched.conversion, NeedsConversion::None);
    }

    #[test]
    fn matching_image_type_marks_windows_dib_variants_for_conversion() {
        let matched = matching_image_type(&["CF_DIBV5".to_owned()]).expect("CF_DIBV5 is present");
        assert_eq!(matched.extension, "bmp");
        assert_eq!(
            matched.conversion,
            NeedsConversion::Dib,
            "CF_DIBV5 must be re-encoded from a headerless DIB"
        );
    }

    #[test]
    fn image_file_bytes_leaves_self_contained_png_untouched() {
        let candidate = IMAGE_TYPE_CANDIDATES
            .iter()
            .find(|candidate| candidate.name == "public.png")
            .expect("png candidate");
        assert_eq!(candidate.conversion, NeedsConversion::None);
        let png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a];
        assert_eq!(image_file_bytes(candidate, png.clone()), Some((png, "png")));
    }

    /// Assembles a 40-byte `BITMAPINFOHEADER` DIB. `height` may be negative
    /// (top-down). `tail` holds palette/masks then pixels, in on-disk order.
    fn make_bi_rgb_dib(
        width: i32,
        height: i32,
        bit_count: u16,
        clr_used: u32,
        tail: &[u8],
    ) -> Vec<u8> {
        let mut dib = make_dib(40, bit_count, 0, clr_used, tail.len());
        dib[4..8].copy_from_slice(&width.to_le_bytes());
        dib[8..12].copy_from_slice(&height.to_le_bytes());
        dib[40..].copy_from_slice(tail);
        dib
    }

    /// Decodes our own PNG output back to RGBA8 so tests can assert on real
    /// pixels rather than trusting the encoder blindly.
    fn decode_png_rgba(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
        let decoder = png::Decoder::new(bytes);
        let mut reader = decoder.read_info().expect("valid PNG");
        let mut buf = vec![0; reader.output_buffer_size()];
        let info = reader.next_frame(&mut buf).expect("PNG frame");
        buf.truncate(info.buffer_size());
        (info.width, info.height, buf)
    }

    fn rgba_at(rgba: &[u8], width: u32, x: u32, y: u32) -> [u8; 4] {
        let offset = ((y * width + x) * 4) as usize;
        rgba[offset..offset + 4].try_into().unwrap()
    }

    // A two-row 2x2 image: the top row is C/D, the bottom row is A/B. On-disk
    // row bytes are BGR with the row padded to 4 bytes.
    const TOP_ROW_BGR: [u8; 8] = [90, 80, 70, 120, 110, 100, 0, 0];
    const BOTTOM_ROW_BGR: [u8; 8] = [30, 20, 10, 60, 50, 40, 0, 0];
    const TOP_ROW_RGBA: [[u8; 4]; 2] = [[70, 80, 90, 255], [100, 110, 120, 255]];
    const BOTTOM_ROW_RGBA: [[u8; 4]; 2] = [[10, 20, 30, 255], [40, 50, 60, 255]];

    #[test]
    fn dib_to_png_24bpp_bottom_up_reverses_rows_and_channel_order() {
        // bottom-up: on-disk row 0 is the image's bottom row.
        let mut tail = Vec::from(BOTTOM_ROW_BGR);
        tail.extend_from_slice(&TOP_ROW_BGR);
        let dib = make_bi_rgb_dib(2, 2, 24, 0, &tail);

        let png = dib_to_png_file_bytes(&dib).expect("24bpp DIB converts");
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "PNG signature");

        let (width, height, rgba) = decode_png_rgba(&png);
        assert_eq!((width, height), (2, 2));
        // Row 0 of the PNG is the image top (C/D), i.e. the DIB's last row.
        assert_eq!(rgba_at(&rgba, width, 0, 0), TOP_ROW_RGBA[0]);
        assert_eq!(rgba_at(&rgba, width, 1, 0), TOP_ROW_RGBA[1]);
        assert_eq!(rgba_at(&rgba, width, 0, 1), BOTTOM_ROW_RGBA[0]);
        assert_eq!(rgba_at(&rgba, width, 1, 1), BOTTOM_ROW_RGBA[1]);
    }

    #[test]
    fn dib_to_png_24bpp_top_down_keeps_first_row_at_png_row_zero() {
        // negative height => on-disk row 0 is already the image top row.
        let mut tail = Vec::from(TOP_ROW_BGR);
        tail.extend_from_slice(&BOTTOM_ROW_BGR);
        let dib = make_bi_rgb_dib(2, -2, 24, 0, &tail);

        let png = dib_to_png_file_bytes(&dib).expect("top-down DIB converts");
        let (width, height, rgba) = decode_png_rgba(&png);
        assert_eq!((width, height), (2, 2));
        assert_eq!(rgba_at(&rgba, width, 0, 0), TOP_ROW_RGBA[0]);
        assert_eq!(rgba_at(&rgba, width, 0, 1), BOTTOM_ROW_RGBA[0]);
    }

    #[test]
    fn dib_to_png_32bpp_reorders_bgra_to_rgba_and_keeps_alpha() {
        // one pixel: B=1 G=2 R=3 A=4 -> RGBA 3,2,1,4.
        let dib = make_bi_rgb_dib(1, 1, 32, 0, &[1, 2, 3, 4]);
        let png = dib_to_png_file_bytes(&dib).expect("32bpp DIB converts");
        let (width, height, rgba) = decode_png_rgba(&png);
        assert_eq!((width, height), (1, 1));
        assert_eq!(rgba_at(&rgba, width, 0, 0), [3, 2, 1, 4]);
    }

    #[test]
    fn dib_to_png_8bpp_reads_the_palette() {
        // two BGRA palette entries, biClrUsed=2; pixel index 1 -> second entry.
        let mut tail = vec![5, 6, 7, 0, 15, 16, 17, 0];
        tail.extend_from_slice(&[1, 0, 0, 0]); // 1 pixel, 4-byte padded row
        let dib = make_bi_rgb_dib(1, 1, 8, 2, &tail);

        let png = dib_to_png_file_bytes(&dib).expect("8bpp DIB converts");
        let (width, height, rgba) = decode_png_rgba(&png);
        assert_eq!((width, height), (1, 1));
        assert_eq!(
            rgba_at(&rgba, width, 0, 0),
            [17, 16, 15, 255],
            "index 1 must resolve through the palette"
        );
    }

    #[test]
    fn dib_to_png_rejects_unsupported_or_malformed_dibs() {
        // 16bpp is out of scope.
        assert_eq!(
            dib_to_png_file_bytes(&make_bi_rgb_dib(1, 1, 16, 0, &[0; 4])),
            None
        );
        // RLE8 compression (code 1) is out of scope.
        let mut rle = make_bi_rgb_dib(1, 1, 8, 0, &[0; 8]);
        rle[16..20].copy_from_slice(&1u32.to_le_bytes());
        assert_eq!(dib_to_png_file_bytes(&rle), None);
        // Buffer shorter than the pixels it claims.
        assert_eq!(
            dib_to_png_file_bytes(&make_bi_rgb_dib(4, 4, 24, 0, &[])),
            None
        );
        // Zero width / height and empty input.
        assert_eq!(
            dib_to_png_file_bytes(&make_bi_rgb_dib(0, 1, 24, 0, &[])),
            None
        );
        assert_eq!(dib_to_png_file_bytes(&[]), None);
    }

    #[test]
    fn image_file_bytes_falls_back_to_bmp_when_dib_cannot_become_png() {
        let candidate = IMAGE_TYPE_CANDIDATES
            .iter()
            .find(|candidate| candidate.name == "CF_DIB")
            .expect("CF_DIB candidate");
        // 16bpp DIB: dib_to_png_file_bytes refuses it, so the BMP wrapper runs.
        let dib = make_bi_rgb_dib(1, 1, 16, 0, &[0; 4]);
        let (bytes, extension) =
            image_file_bytes(candidate, dib).expect("DIB still yields a usable BMP");
        assert_eq!(extension, "bmp");
        assert_eq!(&bytes[..2], b"BM");
    }

    #[test]
    fn image_file_bytes_reencodes_a_decodable_dib_as_png() {
        let candidate = IMAGE_TYPE_CANDIDATES
            .iter()
            .find(|candidate| candidate.name == "CF_DIB")
            .expect("CF_DIB candidate");
        let dib = make_bi_rgb_dib(1, 1, 24, 0, &[1, 2, 3, 0]);
        let (bytes, extension) = image_file_bytes(candidate, dib).expect("24bpp DIB");
        assert_eq!(extension, "png");
        assert_eq!(&bytes[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn dib_to_bmp_wraps_24bpp_bitmapinfoheader_with_correct_header() {
        let dib = make_dib(40, 24, 0, 0, 12);
        let bmp = dib_to_bmp_file_bytes(&dib).expect("valid 24bpp DIB");
        assert_eq!(&bmp[0..2], b"BM", "bfType magic");
        assert_eq!(le_u32(&bmp, 2) as usize, 14 + dib.len(), "bfSize");
        assert_eq!(le_u32(&bmp, 10) as usize, 14 + 40, "bfOffBits, no palette");
        assert_eq!(&bmp[14..], &dib[..], "DIB payload is copied verbatim");
    }

    #[test]
    fn dib_to_bmp_counts_8bpp_palette_in_off_bits() {
        // biClrUsed==0 and 8bpp => 1<<8 = 256 colours => 1024 palette bytes.
        let dib = make_dib(40, 8, 0, 0, 1024 + 8);
        let bmp = dib_to_bmp_file_bytes(&dib).expect("valid 8bpp DIB");
        assert_eq!(
            le_u32(&bmp, 10) as usize,
            14 + 40 + 1024,
            "bfOffBits must include the palette"
        );
    }

    #[test]
    fn dib_to_bmp_honours_explicit_bi_clr_used() {
        // biClrUsed==2 => 8 palette bytes, not the 1<<8 default.
        let dib = make_dib(40, 8, 0, 2, 8 + 4);
        let bmp = dib_to_bmp_file_bytes(&dib).expect("valid DIB");
        assert_eq!(le_u32(&bmp, 10) as usize, 14 + 40 + 8, "bfOffBits");
    }

    #[test]
    fn dib_to_bmp_counts_bitfields_masks_when_bi_clr_used_is_zero() {
        // 32bpp BI_BITFIELDS, biClrUsed==0 => 3 DWORD masks => 12 extra bytes.
        let dib = make_dib(40, 32, 3, 0, 12 + 16);
        let bmp = dib_to_bmp_file_bytes(&dib).expect("valid BI_BITFIELDS DIB");
        assert_eq!(
            le_u32(&bmp, 10) as usize,
            14 + 40 + 12,
            "bfOffBits must reserve the trailing mask DWORDs"
        );
    }

    #[test]
    fn dib_to_bmp_rejects_short_or_malformed_input() {
        assert_eq!(dib_to_bmp_file_bytes(&[]), None);
        assert_eq!(dib_to_bmp_file_bytes(&[1, 2, 3]), None);
        // Header claims 40 bytes but the buffer only holds 4.
        assert_eq!(dib_to_bmp_file_bytes(&40u32.to_le_bytes()), None);
        // Absurd header size.
        let mut absurd = make_dib(40, 24, 0, 0, 0);
        absurd[0..4].copy_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(dib_to_bmp_file_bytes(&absurd), None);
    }

    #[test]
    fn write_temp_image_file_round_trips_exact_bytes_with_the_named_extension() {
        let bytes = [0x89, b'P', b'N', b'G', 0x0d, 0x0a];
        let path = write_temp_image_file(&bytes, "png").expect("temp file write");
        assert!(
            path.ends_with(".png"),
            "path must carry its extension: {path}"
        );
        let read_back = std::fs::read(&path).expect("read back the temp file");
        assert_eq!(read_back, bytes);
        std::fs::remove_file(&path).expect("clean up the temp file");
    }
}
