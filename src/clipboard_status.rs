//! Bounded, generation-safe clipboard type and size status.
//!
//! The clipboard changes from any process, so its type and length are re-read,
//! and reading the clipboard every frame would contend with whoever holds it.
//! Two cheaper signals decide when to read:
//!
//! - `clipboard::change_count()`, which on Windows is a system counter read
//!   without opening the clipboard, so polling it every frame is free;
//! - MiniCon's own writes, which all go through [`set_text`] here and are
//!   known without reading anything -- the only signal on hosts that have no
//!   counter (macOS via pbcopy/pbpaste, X11), where focus also forces a
//!   refresh.

use std::sync::{
    Mutex,
    atomic::{AtomicU64, Ordering},
};

use minicon_platform::{
    clipboard::{self, ClipboardObserved, ClipboardObservedRead, ClipboardObservedReadPoll},
    contract::clipboard::ClipboardError,
};

/// The most text the status bar will retain to measure a length. Beyond it
/// the readout says "over this" rather than holding megabytes to count them.
pub(crate) const MEASURE_LIMIT_BYTES: usize = 8 * 1024 * 1024;

/// What the readout shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipboardLength {
    /// Not known yet, or the clipboard holds no text.
    None,
    /// The text's length in characters.
    Chars(usize),
    /// More than `MEASURE_LIMIT_BYTES` of text.
    Over,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ClipboardKind {
    Checking,
    Unavailable,
    Empty,
    Text,
    Image,
    TextAndImage,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ClipboardSummary {
    pub(crate) kind: ClipboardKind,
    pub(crate) length: ClipboardLength,
}

impl ClipboardSummary {
    const CHECKING: Self = Self {
        kind: ClipboardKind::Checking,
        length: ClipboardLength::None,
    };
    const UNAVAILABLE: Self = Self {
        kind: ClipboardKind::Unavailable,
        length: ClipboardLength::None,
    };
}

static OWN_WRITE_GENERATION: AtomicU64 = AtomicU64::new(0);
static OWN_WRITE_FACTS: Mutex<(usize, Option<u64>)> = Mutex::new((0, None));

/// Put text on the clipboard. Every MiniCon copy goes through here so the
/// status bar knows the new length without reading it back.
pub(crate) fn set_text(text: &str) -> bool {
    if clipboard::set_text(text).is_err() {
        return false;
    }
    if let Ok(mut facts) = OWN_WRITE_FACTS.lock() {
        *facts = (text.chars().count(), clipboard::change_count());
    }
    OWN_WRITE_GENERATION.fetch_add(1, Ordering::Release);
    true
}

pub(crate) struct ClipboardStatus {
    summary: ClipboardSummary,
    seen_change: Option<u64>,
    seen_own_write: u64,
    pending: Option<ClipboardObservedRead>,
    pending_generation: u64,
    generation: u64,
    /// Hosts without a counter still read once at start and on focus.
    refresh_requested: bool,
}

impl ClipboardStatus {
    pub(crate) fn new() -> Self {
        Self {
            summary: ClipboardSummary::CHECKING,
            seen_change: None,
            seen_own_write: OWN_WRITE_GENERATION.load(Ordering::Acquire),
            pending: None,
            pending_generation: 0,
            generation: 0,
            refresh_requested: true,
        }
    }

    pub(crate) fn summary(&self) -> ClipboardSummary {
        self.summary
    }

    /// Re-read on the next poll even if no counter moved -- on focus, where
    /// another program may have changed the clipboard while MiniCon was in
    /// the background on a host that cannot say so.
    pub(crate) fn request_refresh(&mut self) {
        self.generation = self.generation.wrapping_add(1);
        self.refresh_requested = true;
        self.summary = ClipboardSummary::CHECKING;
    }

    /// Advances the readout. Returns true when what it shows changed, so the
    /// caller can repaint only then. `start_read` is given the byte limit and
    /// must start an asynchronous read that wakes the window when done.
    pub(crate) fn poll(
        &mut self,
        start_read: impl FnOnce(usize) -> Option<ClipboardObservedRead>,
    ) -> bool {
        let before = self.summary;
        let current_change = clipboard::change_count();

        // MiniCon's own write: its length is already known.
        let own = OWN_WRITE_GENERATION.load(Ordering::Acquire);
        if own != self.seen_own_write {
            self.seen_own_write = own;
            let facts = OWN_WRITE_FACTS.lock().ok().map(|facts| *facts);
            self.seen_change = current_change;
            self.pending = None;
            if facts
                .is_none_or(|(_, own_change)| own_change.is_some() && own_change != current_change)
            {
                self.generation = self.generation.wrapping_add(1);
                self.refresh_requested = true;
                self.summary = ClipboardSummary::CHECKING;
            } else {
                self.refresh_requested = false;
                self.summary = ClipboardSummary {
                    kind: ClipboardKind::Text,
                    length: ClipboardLength::Chars(facts.expect("facts checked above").0),
                };
            }
        }

        if current_change.is_some() && current_change != self.seen_change {
            self.seen_change = current_change;
            self.generation = self.generation.wrapping_add(1);
            self.refresh_requested = true;
            self.summary = ClipboardSummary::CHECKING;
        }

        if let Some(read) = &self.pending {
            match read.try_poll() {
                ClipboardObservedReadPoll::Pending => return self.summary != before,
                ClipboardObservedReadPoll::Ready(result) => {
                    self.pending = None;
                    self.apply_observation(result, current_change);
                }
            }
        }
        if self.pending.is_none() && self.refresh_requested {
            self.refresh_requested = false;
            self.pending_generation = self.generation;
            self.pending = start_read(MEASURE_LIMIT_BYTES);
            if self.pending.is_none() {
                self.summary = ClipboardSummary::UNAVAILABLE;
            }
        }
        self.summary != before
    }

    fn apply_observation(
        &mut self,
        result: Result<ClipboardObserved, ClipboardError>,
        current_change: Option<u64>,
    ) {
        if self.pending_generation != self.generation || self.refresh_requested {
            return;
        }
        match result {
            Ok(observed)
                if observed.change_count.is_none() || observed.change_count == current_change =>
            {
                self.summary = classify(observed);
            }
            Err(ClipboardError::Failed { code, .. }) if code == "clipboard_changed_during_read" => {
                self.generation = self.generation.wrapping_add(1);
                self.refresh_requested = true;
                self.summary = ClipboardSummary::CHECKING;
            }
            Ok(_) => {
                self.generation = self.generation.wrapping_add(1);
                self.refresh_requested = true;
                self.summary = ClipboardSummary::CHECKING;
            }
            Err(_) => self.summary = ClipboardSummary::UNAVAILABLE,
        }
    }
}

fn classify(observed: ClipboardObserved) -> ClipboardSummary {
    if observed.types.is_empty() {
        return if matches!(&observed.text, Ok(value) if !value.is_empty()) {
            ClipboardSummary::UNAVAILABLE
        } else {
            ClipboardSummary {
                kind: ClipboardKind::Empty,
                length: ClipboardLength::None,
            }
        };
    }
    let image = observed.types.iter().any(|name| image_type(name));
    let text_advertised = observed.types.iter().any(|name| text_type(name));
    let text = text_advertised || matches!(&observed.text, Ok(value) if !value.is_empty());
    if text
        && matches!(&observed.text, Err(ClipboardError::Failed { code, .. }) if code != "clipboard_too_large")
    {
        return ClipboardSummary::UNAVAILABLE;
    }
    if text && matches!(&observed.text, Err(ClipboardError::Unsupported { .. })) {
        return ClipboardSummary::UNAVAILABLE;
    }
    let kind = match (text, image) {
        (true, true) => ClipboardKind::TextAndImage,
        (true, false) => ClipboardKind::Text,
        (false, true) => ClipboardKind::Image,
        (false, false) => ClipboardKind::Other,
    };
    let length = if text {
        match observed.text {
            Ok(value) => ClipboardLength::Chars(value.chars().count()),
            Err(ClipboardError::Failed { code, .. }) if code == "clipboard_too_large" => {
                ClipboardLength::Over
            }
            Err(_) => ClipboardLength::None,
        }
    } else {
        ClipboardLength::None
    };
    ClipboardSummary { kind, length }
}

fn text_type(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "cf_unicodetext"
            | "cf_text"
            | "cf_oemtext"
            | "utf8_string"
            | "string"
            | "text"
            | "«class utf8»"
            | "«class ut16»"
            | "«class utxt»"
            | "unicode text"
    ) || name.starts_with("text/")
        || name.contains("plain-text")
        || name == "public.html"
}

fn image_type(name: &str) -> bool {
    let name = name.to_ascii_lowercase();
    matches!(
        name.as_str(),
        "cf_bitmap"
            | "cf_dib"
            | "cf_dibv5"
            | "cf_tiff"
            | "cf_enhmetafile"
            | "png"
            | "jpeg"
            | "jfif"
            | "gif"
            | "bmp"
            | "«class pngf»"
            | "«class tiff»"
            | "«class jpeg»"
            | "«class pict»"
    ) || name.starts_with("image/")
        || name.starts_with("public.image")
        || matches!(
            name.as_str(),
            "public.png" | "public.jpeg" | "public.tiff" | "public.heic"
        )
}

/// Render a localized type and honest size. Image bytes are never read merely
/// for this status, so image size remains explicitly unknown.
pub(crate) fn readout(summary: ClipboardSummary, chinese: bool, compact: bool) -> Option<String> {
    let prefix = if chinese && compact {
        "剪"
    } else if chinese {
        "剪贴板"
    } else {
        "Clip"
    };
    let text_amount = match summary.length {
        ClipboardLength::None => if chinese { "字数?" } else { "chars?" }.to_owned(),
        ClipboardLength::Chars(n) if n > 999_999 => if chinese {
            ">999999 字"
        } else {
            ">999999 chars"
        }
        .to_owned(),
        ClipboardLength::Chars(n) if compact && chinese => format!("{n}字"),
        ClipboardLength::Chars(n) if compact => format!("{n}c"),
        ClipboardLength::Chars(n) if chinese => format!("{n:>7} 字"),
        ClipboardLength::Chars(n) => format!("{n:>7} chars"),
        ClipboardLength::Over => ">8 MiB".to_owned(),
    };
    let body = match summary.kind {
        ClipboardKind::Checking => if chinese { "检测中" } else { "checking" }.to_owned(),
        ClipboardKind::Unavailable => if chinese { "不可用" } else { "unavailable" }.to_owned(),
        ClipboardKind::Empty => if chinese { "空" } else { "Empty" }.to_owned(),
        ClipboardKind::Other => if chinese { "其他类型" } else { "Other" }.to_owned(),
        ClipboardKind::Text => format!(
            "{} {text_amount}",
            if chinese && compact {
                "文"
            } else if chinese {
                "文本"
            } else if compact {
                "T"
            } else {
                "Text"
            }
        ),
        ClipboardKind::Image => format!(
            "{} {}",
            if chinese && compact {
                "图"
            } else if chinese {
                "图片"
            } else if compact {
                "I"
            } else {
                "Image"
            },
            if compact {
                "?"
            } else if chinese {
                "大小?"
            } else {
                "size ?"
            }
        ),
        ClipboardKind::TextAndImage => {
            if compact {
                format!(
                    "{} {text_amount}/{}",
                    if chinese { "文+图" } else { "T+I" },
                    if chinese { "图?" } else { "I?" }
                )
            } else {
                format!(
                    "{} {text_amount} · {}",
                    if chinese {
                        "文本+图片"
                    } else {
                        "Text+Image"
                    },
                    if chinese {
                        "图片大小?"
                    } else {
                        "image size ?"
                    }
                )
            }
        }
    };
    Some(format!("{prefix} {body}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use minicon_platform::contract::clipboard::ClipboardResult;

    fn observation(types: &[&str], text: ClipboardResult<&str>) -> ClipboardObserved {
        ClipboardObserved {
            types: types.iter().map(|name| (*name).to_owned()).collect(),
            text: text.map(str::to_owned),
            change_count: None,
        }
    }

    /// The readout is fixed-width for any length it can show, so the status
    /// bar never jitters as the clipboard changes.
    #[test]
    fn the_text_count_keeps_one_width_and_mixed_formats_stay_visible() {
        let widths: Vec<usize> = [0_usize, 7, 42, 1234, 999_999, 5_000_000]
            .iter()
            .filter_map(|&chars| {
                readout(
                    ClipboardSummary {
                        kind: ClipboardKind::Text,
                        length: ClipboardLength::Chars(chars),
                    },
                    false,
                    false,
                )
            })
            .map(|text| text.chars().count())
            .collect();
        assert!(
            widths.windows(2).all(|pair| pair[0] == pair[1]),
            "{widths:?}"
        );
        let mixed = ClipboardSummary {
            kind: ClipboardKind::TextAndImage,
            length: ClipboardLength::Chars(12),
        };
        let shown = readout(mixed, false, false).unwrap();
        assert!(
            shown.contains("Text+Image")
                && shown.contains("12 chars")
                && shown.contains("image size ?")
        );
        let chinese = readout(mixed, true, true).unwrap();
        assert!(chinese.contains("文+图") && chinese.contains("12字"));
    }

    /// An own write is reflected on the next poll without any read, and a
    /// later poll with nothing new changes nothing.
    #[test]
    fn an_own_write_is_shown_without_reading_the_clipboard() {
        let mut status = ClipboardStatus::new();
        status.refresh_requested = false;
        status.seen_change = clipboard::change_count();
        if let Ok(mut facts) = OWN_WRITE_FACTS.lock() {
            *facts = (11, clipboard::change_count());
        }
        OWN_WRITE_GENERATION.fetch_add(1, Ordering::Release);
        let mut reads = 0;
        assert!(status.poll(|_| {
            reads += 1;
            None
        }));
        assert_eq!(
            status.summary(),
            ClipboardSummary {
                kind: ClipboardKind::Text,
                length: ClipboardLength::Chars(11)
            }
        );
        assert_eq!(reads, 0, "an own write was read back");
        assert!(!status.poll(|_| None));
    }

    /// The own-write signal is only as good as its coverage: a copy path that
    /// wrote the clipboard directly would leave the readout stale on every
    /// host without a change counter. Every write in MiniCon goes through
    /// `set_text` here.
    #[test]
    fn no_other_source_writes_the_clipboard_directly() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        // A cross-compiled test binary run on another machine (a Windows
        // court) has no source tree to read; this checks source, not behaviour.
        if !root.is_dir() {
            eprintln!("skipped: no source tree at {}", root.display());
            return;
        }
        let mut offenders = Vec::new();
        for entry in std::fs::read_dir(&root).expect("src is readable") {
            let path = entry.expect("entry").path();
            if path.extension().and_then(|e| e.to_str()) != Some("rs")
                || path.file_name().and_then(|n| n.to_str()) == Some("clipboard_status.rs")
            {
                continue;
            }
            let source = std::fs::read_to_string(&path).expect("readable");
            for (number, line) in source.lines().enumerate() {
                if line.contains("clipboard::set_text(")
                    && !line.contains("clipboard_status::set_text(")
                {
                    offenders.push(format!("{}:{}", path.display(), number + 1));
                }
            }
        }
        assert!(
            offenders.is_empty(),
            "direct clipboard writes: {offenders:?}"
        );
    }

    /// Characters, not bytes: the user reads the readout as "how much text".
    #[test]
    fn advertised_types_preserve_image_mixed_empty_and_failed_text_reads() {
        assert_eq!(
            classify(observation(
                &["public.png", "public.utf8-plain-text"],
                Ok("中文ab")
            )),
            ClipboardSummary {
                kind: ClipboardKind::TextAndImage,
                length: ClipboardLength::Chars(4)
            }
        );
        assert_eq!(
            classify(observation(
                &["CF_DIBV5"],
                Err(ClipboardError::failed("clipboard_unavailable", "no text"))
            ))
            .kind,
            ClipboardKind::Image
        );
        assert_eq!(
            classify(observation(
                &["«class PNGf»"],
                Err(ClipboardError::failed("clipboard_unavailable", "no text"))
            ))
            .kind,
            ClipboardKind::Image
        );
        assert_eq!(
            classify(observation(
                &["PNG"],
                Err(ClipboardError::failed("clipboard_unavailable", "no text"))
            ))
            .kind,
            ClipboardKind::Image
        );
        assert_eq!(
            classify(observation(&[], Ok(""))).kind,
            ClipboardKind::Empty
        );
        assert_eq!(
            classify(observation(
                &[],
                Err(ClipboardError::failed("clipboard_unavailable", "empty"))
            ))
            .kind,
            ClipboardKind::Empty
        );
        assert_eq!(
            classify(observation(&[], Ok("new content"))).kind,
            ClipboardKind::Unavailable
        );
        assert_eq!(
            classify(observation(&["CF_HDROP"], Ok(""))).kind,
            ClipboardKind::Other
        );
        assert_eq!(
            classify(observation(
                &["CF_UNICODETEXT"],
                Err(ClipboardError::failed("clipboard_busy", "busy"))
            )),
            ClipboardSummary::UNAVAILABLE
        );
        assert_eq!(
            classify(observation(
                &["text/plain"],
                Err(ClipboardError::failed("clipboard_too_large", "big"))
            ))
            .length,
            ClipboardLength::Over
        );
    }

    #[test]
    fn a_late_observation_cannot_replace_a_newer_clipboard_summary() {
        let mut status = ClipboardStatus::new();
        status.refresh_requested = false;
        status.generation = 2;
        status.pending_generation = 1;
        status.apply_observation(Ok(observation(&["PNG"], Ok(""))), None);
        assert_eq!(status.summary(), ClipboardSummary::CHECKING);
        status.pending_generation = 2;
        status.apply_observation(Ok(observation(&["PNG"], Ok(""))), None);
        assert_eq!(status.summary().kind, ClipboardKind::Image);

        status.pending_generation = 2;
        let mut changed = observation(&["text/plain"], Ok("old"));
        changed.change_count = Some(7);
        status.apply_observation(Ok(changed), Some(8));
        assert_eq!(status.summary(), ClipboardSummary::CHECKING);
        assert!(status.refresh_requested);
        assert_eq!(status.generation, 3);
    }

    /// Run only on a disposable hosted desktop. The original clipboard must
    /// be empty, so exact restoration is another empty type inventory.
    #[cfg(any(windows, target_os = "macos", target_os = "linux"))]
    #[test]
    #[ignore = "requires an isolated CI desktop and explicit clipboard-court flag"]
    fn native_empty_text_and_png_status_restore_empty_clipboard() {
        assert_eq!(
            std::env::var("MINICON_NATIVE_CLIPBOARD_COURT").as_deref(),
            Ok("1")
        );
        assert_eq!(std::env::var("GITHUB_ACTIONS").as_deref(), Ok("true"));
        let original = clipboard::available_types().expect("native clipboard format listing");
        assert!(
            original.is_empty(),
            "BLOCKED: hosted clipboard was not empty before the native court"
        );
        struct RestoreEmpty;
        impl Drop for RestoreEmpty {
            fn drop(&mut self) {
                let _ = clipboard::clear_with_timeout(std::time::Duration::from_secs(5));
            }
        }
        let _restore = RestoreEmpty;
        let observe = || {
            let (sender, receiver) = std::sync::mpsc::sync_channel(1);
            let pending = clipboard::read_observed_async(MEASURE_LIMIT_BYTES, move || {
                let _ = sender.send(());
            })
            .expect("start native clipboard observation");
            receiver
                .recv_timeout(std::time::Duration::from_secs(5))
                .expect("native clipboard observation wake");
            match pending.try_poll() {
                ClipboardObservedReadPoll::Ready(Ok(observed)) => classify(observed),
                ClipboardObservedReadPoll::Ready(Err(error)) => {
                    panic!("native clipboard observation failed: {error:?}")
                }
                ClipboardObservedReadPoll::Pending => panic!("wake arrived before result"),
            }
        };
        assert_eq!(observe().kind, ClipboardKind::Empty);
        clipboard::set_text("中文a").expect("write native text");
        assert_eq!(
            observe(),
            ClipboardSummary {
                kind: ClipboardKind::Text,
                length: ClipboardLength::Chars(3),
            }
        );
        const ONE_PIXEL_PNG: &[u8] = &[
            137, 80, 78, 71, 13, 10, 26, 10, 0, 0, 0, 13, 73, 72, 68, 82, 0, 0, 0, 1, 0, 0, 0, 1,
            8, 4, 0, 0, 0, 181, 28, 12, 2, 0, 0, 0, 11, 73, 68, 65, 84, 120, 218, 99, 252, 255, 31,
            0, 2, 235, 1, 245, 141, 27, 146, 0, 0, 0, 0, 73, 69, 78, 68, 174, 66, 96, 130,
        ];
        #[cfg(windows)]
        let image_format = "PNG";
        #[cfg(target_os = "macos")]
        let image_format = "«class PNGf»";
        #[cfg(target_os = "linux")]
        let image_format = "image/png";
        clipboard::set_type(image_format, ONE_PIXEL_PNG).expect("write native image format");
        assert_eq!(observe().kind, ClipboardKind::Image);
        clipboard::clear_with_timeout(std::time::Duration::from_secs(5))
            .expect("restore empty clipboard");
        assert_eq!(
            clipboard::available_types().expect("verify native empty clipboard"),
            original
        );
    }
}
