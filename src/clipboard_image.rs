//! Composer image/screenshot paste, file-path fallback (`F1` in
//! `plan/plan-carried-debt.md`, decided shape in
//! `prd/PRD_02_25_con_workspace.md`'s "Screenshot/image paste into the
//! composer").
//!
//! `agenterm_platform::clipboard` already exposes a generic
//! `available_types()`/`get_type()` pair across every OS adapter; no new
//! platform-crate API is needed. What's MiniCon-owned is picking the right
//! host type name and turning the bytes into a temp file whose path the
//! composer can insert like any other pasted text.

use std::{
    io::Write,
    path::PathBuf,
    time::{SystemTime, UNIX_EPOCH},
};

use agenterm_platform::contract::clipboard::MAX_CLIPBOARD_TYPE_BYTES;

/// Host-native clipboard type names that carry image bytes, tried in order
/// against whatever `available_types()` actually reports. Names are the
/// host's own spelling (macOS UTIs, Windows registered formats, X11/Wayland
/// MIME types) -- see `crates/agenterm-platform/src/adapters/*/clipboard.rs`
/// in the `agenterm` repo.
const IMAGE_TYPE_CANDIDATES: &[(&str, &str)] = &[
    ("public.png", "png"),
    ("image/png", "png"),
    ("public.jpeg", "jpg"),
    ("image/jpeg", "jpg"),
    ("CF_DIBV5", "bmp"),
    ("CF_DIB", "bmp"),
];

/// Picks the first candidate type name this clipboard actually reports, in
/// [`IMAGE_TYPE_CANDIDATES`] order. `available_types` reporting `Unsupported`
/// (a host with no way to enumerate types) is not treated as "no image" --
/// callers that need a fallback should still try `get_type` directly, which
/// this function does not do.
fn matching_image_type(available: &[String]) -> Option<(&'static str, &'static str)> {
    IMAGE_TYPE_CANDIDATES
        .iter()
        .copied()
        .find(|(name, _)| available.iter().any(|available| available == name))
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
    let available = agenterm_platform::clipboard::available_types().ok()?;
    let (type_name, extension) = matching_image_type(&available)?;
    let bytes = agenterm_platform::clipboard::get_type(type_name, MAX_CLIPBOARD_TYPE_BYTES).ok()?;
    write_temp_image_file(&bytes, extension).ok()
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

    #[test]
    fn matching_image_type_prefers_earlier_candidates_in_order() {
        let available = vec!["image/jpeg".to_owned(), "public.png".to_owned()];
        assert_eq!(
            matching_image_type(&available),
            Some(("public.png", "png")),
            "public.png is listed before image/jpeg in IMAGE_TYPE_CANDIDATES"
        );
    }

    #[test]
    fn matching_image_type_is_none_for_text_only_types() {
        let available = vec!["text/plain".to_owned(), "UTF8_STRING".to_owned()];
        assert_eq!(matching_image_type(&available), None);
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
