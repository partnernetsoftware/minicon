//! What the status bar says about the clipboard: the length of its current
//! text.
//!
//! The clipboard changes from any process, so the length has to be re-read,
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

use agenterm_platform::{
    clipboard::{self, ClipboardTextRead, ClipboardTextReadPoll},
    contract::clipboard::{ClipboardError, ClipboardResult},
};

/// The most the status bar will read to measure a length. Beyond it the
/// readout says "over this" rather than holding megabytes to count them.
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

static OWN_WRITE_GENERATION: AtomicU64 = AtomicU64::new(0);
static OWN_WRITE_CHARS: Mutex<usize> = Mutex::new(0);

/// Put text on the clipboard. Every MiniCon copy goes through here so the
/// status bar knows the new length without reading it back.
pub(crate) fn set_text(text: &str) -> bool {
    if clipboard::set_text(text).is_err() {
        return false;
    }
    if let Ok(mut chars) = OWN_WRITE_CHARS.lock() {
        *chars = text.chars().count();
    }
    OWN_WRITE_GENERATION.fetch_add(1, Ordering::Release);
    true
}

pub(crate) struct ClipboardStatus {
    length: ClipboardLength,
    seen_change: Option<u64>,
    seen_own_write: u64,
    pending: Option<ClipboardTextRead>,
    /// Hosts without a counter still read once at start and on focus.
    refresh_requested: bool,
}

impl ClipboardStatus {
    pub(crate) fn new() -> Self {
        Self {
            length: ClipboardLength::None,
            seen_change: None,
            seen_own_write: OWN_WRITE_GENERATION.load(Ordering::Acquire),
            pending: None,
            refresh_requested: true,
        }
    }

    pub(crate) fn length(&self) -> ClipboardLength {
        self.length
    }

    /// Re-read on the next poll even if no counter moved -- on focus, where
    /// another program may have changed the clipboard while MiniCon was in
    /// the background on a host that cannot say so.
    pub(crate) fn request_refresh(&mut self) {
        self.refresh_requested = true;
    }

    /// Advances the readout. Returns true when what it shows changed, so the
    /// caller can repaint only then. `start_read` is given the byte limit and
    /// must start an asynchronous read that wakes the window when done.
    pub(crate) fn poll(
        &mut self,
        start_read: impl FnOnce(usize) -> Option<ClipboardTextRead>,
    ) -> bool {
        let before = self.length;

        // MiniCon's own write: its length is already known.
        let own = OWN_WRITE_GENERATION.load(Ordering::Acquire);
        if own != self.seen_own_write {
            self.seen_own_write = own;
            if let Ok(chars) = OWN_WRITE_CHARS.lock() {
                self.length = ClipboardLength::Chars(*chars);
            }
            // The write moved the counter too; do not read it back.
            self.seen_change = clipboard::change_count();
            self.pending = None;
            self.refresh_requested = false;
            return self.length != before;
        }

        if let Some(read) = &self.pending {
            match read.try_poll() {
                ClipboardTextReadPoll::Pending => return false,
                ClipboardTextReadPoll::Ready(result) => {
                    self.pending = None;
                    self.length = length_of(result);
                }
            }
            return self.length != before;
        }

        let change = clipboard::change_count();
        let moved = change.is_some() && change != self.seen_change;
        if moved || self.refresh_requested {
            self.seen_change = change;
            self.refresh_requested = false;
            self.pending = start_read(MEASURE_LIMIT_BYTES);
        }
        self.length != before
    }
}

fn length_of(result: ClipboardResult<String>) -> ClipboardLength {
    match result {
        Ok(text) if text.is_empty() => ClipboardLength::None,
        Ok(text) => ClipboardLength::Chars(text.chars().count()),
        // Every adapter reports a read over the bound with this code; any
        // other failure (no text, clipboard busy) leaves nothing to show.
        Err(ClipboardError::Failed { code, .. }) if code == "clipboard_too_large" => {
            ClipboardLength::Over
        }
        Err(_) => ClipboardLength::None,
    }
}

/// The status bar's text for a length, fixed-width so the bar does not
/// reflow as it changes.
pub(crate) fn readout(length: ClipboardLength, chinese: bool) -> Option<String> {
    let label = if chinese { "剪贴板" } else { "Clip" };
    match length {
        ClipboardLength::None => None,
        ClipboardLength::Chars(chars) if chars > 999_999 => Some(format!("{label} >999999")),
        ClipboardLength::Chars(chars) => Some(format!("{label} {chars:>7}")),
        ClipboardLength::Over => Some(format!("{label} {:>7}", ">8MB")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The readout is fixed-width for any length it can show, so the status
    /// bar never jitters as the clipboard changes.
    #[test]
    fn the_readout_keeps_one_width() {
        let widths: Vec<usize> = [0_usize, 7, 42, 1234, 999_999, 5_000_000]
            .iter()
            .filter_map(|&chars| readout(ClipboardLength::Chars(chars), false))
            .chain(readout(ClipboardLength::Over, false))
            .map(|text| text.chars().count())
            .collect();
        assert!(
            widths.windows(2).all(|pair| pair[0] == pair[1]),
            "{widths:?}"
        );
        assert_eq!(readout(ClipboardLength::None, true), None);
        assert_eq!(
            readout(ClipboardLength::Chars(12), true).as_deref(),
            Some("剪贴板      12")
        );
    }

    /// An own write is reflected on the next poll without any read, and a
    /// later poll with nothing new changes nothing.
    #[test]
    fn an_own_write_is_shown_without_reading_the_clipboard() {
        let mut status = ClipboardStatus::new();
        status.refresh_requested = false;
        status.seen_change = clipboard::change_count();
        if let Ok(mut chars) = OWN_WRITE_CHARS.lock() {
            *chars = 11;
        }
        OWN_WRITE_GENERATION.fetch_add(1, Ordering::Release);
        let mut reads = 0;
        assert!(status.poll(|_| {
            reads += 1;
            None
        }));
        assert_eq!(status.length(), ClipboardLength::Chars(11));
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
    fn length_counts_characters() {
        assert_eq!(
            length_of(Ok("中文ab".to_owned())),
            ClipboardLength::Chars(4)
        );
        assert_eq!(length_of(Ok(String::new())), ClipboardLength::None);
        assert_eq!(
            length_of(Err(ClipboardError::failed("clipboard_too_large", "big"))),
            ClipboardLength::Over
        );
    }
}
