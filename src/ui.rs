//! Pure geometry and hit-testing for the lightweight console host UI.
//!
//! This module deliberately contains no window-host, PTY, or product authority
//! state. Proven rules can therefore move into a shared frontend contract
//! without importing `minicon` lifecycle policy.

pub const SIDEBAR_WIDTH_DIP: f64 = 224.0;
pub const TREE_HEADER_HEIGHT_DIP: f64 = 32.0;
pub const TREE_ROW_HEIGHT_DIP: f64 = 30.0;
pub const COMPOSER_HEIGHT_DIP: f64 = 96.0;
pub const SIDEBAR_MIN_WIDTH_DIP: f64 = 224.0;
pub const SIDEBAR_MAX_WIDTH_DIP: f64 = 480.0;
pub const TERMINAL_MIN_WIDTH_DIP: f64 = 320.0;
pub const SIDEBAR_RESIZE_GRIP_DIP: f64 = 6.0;
pub const TERMINAL_SCROLLBAR_WIDTH_DIP: f64 = 12.0;

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub struct Rect {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
}

impl Rect {
    pub fn contains(self, x: u32, y: u32) -> bool {
        x >= self.x
            && y >= self.y
            && x < self.x.saturating_add(self.width)
            && y < self.y.saturating_add(self.height)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    pub sidebar: Rect,
    pub tree_header_height: u32,
    pub tree_row_height: u32,
    pub composer: Rect,
    pub composer_input: Rect,
    pub composer_send: Rect,
    /// Directly under [`Self::composer_send`], same column.
    ///
    /// Stacked rather than placed side by side: the pair shares one strip of
    /// width the input would otherwise have, and taking a second column from a
    /// single-line input is what makes a long command stop fitting.
    pub composer_newline: Rect,
    pub zoom_out: Rect,
    pub zoom_reset: Rect,
    pub zoom_in: Rect,
    /// Opens a new root terminal; first tool in the header strip.
    pub new_root: Rect,
    /// Opens the shortcut and feature guide.
    pub help: Rect,
    /// The two language entries, left of the size controls.
    pub language_chinese: Rect,
    pub language_english: Rect,
}

impl Layout {
    #[cfg(test)]
    pub fn new(width: u32, height: u32, scale: f64) -> Self {
        Self::with_sidebar_width(width, height, scale, SIDEBAR_WIDTH_DIP)
    }

    pub fn with_sidebar_width(width: u32, height: u32, scale: f64, sidebar_dip: f64) -> Self {
        let scale = scale.max(1.0);
        let maximum = clamp_f64(
            f64::from(width) / scale - TERMINAL_MIN_WIDTH_DIP,
            SIDEBAR_MIN_WIDTH_DIP,
            SIDEBAR_MAX_WIDTH_DIP,
        );
        let sidebar_width = dip(
            clamp_f64(sidebar_dip, SIDEBAR_MIN_WIDTH_DIP, maximum),
            scale,
        )
        .min(width);
        let composer_height = dip(COMPOSER_HEIGHT_DIP, scale).min(height);
        let composer = Rect {
            x: sidebar_width,
            y: height.saturating_sub(composer_height),
            width: width.saturating_sub(sidebar_width),
            height: composer_height,
        };
        let padding = dip(12.0, scale);
        let label_height = dip(24.0, scale);
        let send_width =
            dip(76.0, scale).min(composer.width.saturating_sub(padding.saturating_mul(2)));
        let control_y = composer.y.saturating_add(label_height);
        let control_height = composer
            .height
            .saturating_sub(label_height)
            .saturating_sub(padding);
        let button_x = width.saturating_sub(padding).saturating_sub(send_width);
        let button_gap = dip(6.0, scale);
        let send_height = control_height.saturating_sub(button_gap) / 2;
        let send = Rect {
            x: button_x,
            y: control_y,
            width: send_width,
            height: send_height,
        };
        let newline = Rect {
            x: button_x,
            y: control_y
                .saturating_add(send_height)
                .saturating_add(button_gap),
            width: send_width,
            height: control_height
                .saturating_sub(send_height)
                .saturating_sub(button_gap),
        };
        let input_x = composer.x.saturating_add(padding);
        let input = Rect {
            x: input_x,
            y: control_y,
            width: send
                .x
                .saturating_sub(dip(8.0, scale))
                .saturating_sub(input_x),
            height: control_height,
        };
        let tool_size = dip(24.0, scale).min(sidebar_width / 2);
        let tool_y = dip(4.0, scale);
        let group_gap = dip(4.0, scale);
        let new_root = Rect {
            x: dip(4.0, scale),
            y: tool_y,
            width: tool_size,
            height: tool_size,
        };
        let help = Rect {
            x: new_root.x.saturating_add(tool_size),
            y: tool_y,
            width: tool_size,
            height: tool_size,
        };
        let language_chinese = Rect {
            x: help.x.saturating_add(tool_size).saturating_add(group_gap),
            y: tool_y,
            width: tool_size,
            height: tool_size,
        };
        let language_english = Rect {
            x: language_chinese.x.saturating_add(tool_size),
            y: tool_y,
            width: tool_size,
            height: tool_size,
        };
        let zoom_out = Rect {
            x: language_english
                .x
                .saturating_add(tool_size)
                .saturating_add(group_gap),
            y: tool_y,
            width: tool_size,
            height: tool_size,
        };
        let zoom_reset = Rect {
            x: zoom_out.x.saturating_add(tool_size),
            y: tool_y,
            width: tool_size,
            height: tool_size,
        };
        let zoom_in = Rect {
            x: zoom_reset.x.saturating_add(tool_size),
            y: tool_y,
            width: tool_size,
            height: tool_size,
        };
        Self {
            sidebar: Rect {
                x: 0,
                y: 0,
                width: sidebar_width,
                height,
            },
            tree_header_height: dip(TREE_HEADER_HEIGHT_DIP, scale),
            tree_row_height: dip(TREE_ROW_HEIGHT_DIP, scale).max(1),
            composer,
            composer_input: input,
            composer_send: send,
            composer_newline: newline,
            zoom_out,
            zoom_reset,
            zoom_in,
            new_root,
            help,
            language_chinese,
            language_english,
        }
    }

    pub fn tree_capacity(self) -> usize {
        self.sidebar
            .height
            .saturating_sub(self.tree_header_height)
            .checked_div(self.tree_row_height)
            .unwrap_or(0) as usize
    }

    /// Primary action on the zero-tab greeting page. The button is centred in
    /// the terminal side of the window and keeps the sidebar header available.
    pub fn empty_new_terminal(self, width: u32, height: u32, scale: f64) -> Rect {
        let button_width = dip(184.0, scale).min(width.saturating_sub(self.sidebar.width));
        let button_height = dip(42.0, scale).min(height);
        let content_width = width.saturating_sub(self.sidebar.width);
        Rect {
            x: self
                .sidebar
                .width
                .saturating_add(content_width.saturating_sub(button_width) / 2),
            y: height.saturating_div(2).saturating_add(dip(24.0, scale)),
            width: button_width,
            height: button_height,
        }
    }

    pub fn sidebar_resize_grip(self, scale: f64) -> Rect {
        let width = dip(SIDEBAR_RESIZE_GRIP_DIP, scale).min(self.sidebar.width);
        Rect {
            x: self.sidebar.width.saturating_sub(width),
            y: 0,
            width,
            height: self.sidebar.height,
        }
    }

    pub fn tree_close_rect(self, visible_row: usize, scale: f64) -> Rect {
        let size = dip(20.0, scale).min(self.tree_row_height);
        Rect {
            x: self
                .sidebar
                .width
                .saturating_sub(dip(8.0, scale))
                .saturating_sub(size),
            y: self
                .tree_header_height
                .saturating_add(
                    u32::try_from(visible_row)
                        .unwrap_or(u32::MAX)
                        .saturating_mul(self.tree_row_height),
                )
                .saturating_add((self.tree_row_height.saturating_sub(size)) / 2),
            width: size,
            height: size,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeHit {
    Outside,
    Background,
    NewRoot,
    Help,
    ZoomOut,
    ZoomReset,
    ZoomIn,
    Language(UiLanguage),
    Select(usize),
    Close(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComposerHit {
    Outside,
    Input,
    Send,
    Newline,
}

/// The language MiniCon labels itself in.
///
/// Only the host UI is translated — the tab column, the send strip and the
/// paste failure. Everything a child process prints belongs to that process
/// and is passed through untouched, which is the line this must not cross: a
/// terminal that rewrote program output would be lying about what ran.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum UiLanguage {
    #[default]
    English,
    Chinese,
}

impl UiLanguage {
    /// The stable identifier used by the control surface, so automation can
    /// read and assert the language without matching a display label.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::Chinese => "zh",
        }
    }

    /// The label painted on this language's own entry. Each is written in the
    /// language it selects: a switch labelled in the language you are leaving
    /// is unreadable to the person who needs it.
    #[must_use]
    pub const fn entry_label(self) -> &'static str {
        match self {
            Self::English => "En",
            Self::Chinese => "中",
        }
    }

    #[must_use]
    pub const fn strings(self) -> HostUiStrings {
        match self {
            Self::English => HostUiStrings {
                paste_failed: "PASTE FAILED",
                send: "Send",
                send_hint: "(ctrl-o)",
                newline: "New Line",
                newline_hint: "(Enter)",
                send_to: "SEND TO @",
                empty_title: "READY FOR A NEW TERMINAL",
                new_terminal: "NEW TERMINAL",
                new_terminal_hint: "Ctrl+Shift+T",
            },
            Self::Chinese => HostUiStrings {
                paste_failed: "貼上失敗",
                send: "送出",
                send_hint: "(ctrl-o)",
                newline: "換行",
                newline_hint: "(Enter)",
                send_to: "送往 @",
                empty_title: "準備開啟新終端",
                new_terminal: "新建終端",
                new_terminal_hint: "Ctrl+Shift+T",
            },
        }
    }

    #[must_use]
    pub const fn help_lines(self) -> [&'static str; 8] {
        match self {
            Self::English => [
                "KEYBOARD + FEATURES",
                "Ctrl+Shift+T   New terminal",
                "Ctrl+Shift+W   Close tab",
                "Ctrl+Shift+I   Focus input",
                "Ctrl+O         Send input",
                "Enter          Soft newline",
                "Ctrl+[ / ]     Previous / next tab",
                "Tab tree · multiline composer · PTY",
            ],
            Self::Chinese => [
                "快捷鍵與功能",
                "Ctrl+Shift+T   新建終端",
                "Ctrl+Shift+W   關閉標籤",
                "Ctrl+Shift+I   聚焦輸入區",
                "Ctrl+O         送出輸入",
                "Enter          軟換行",
                "Ctrl+[ / ]     上一個 / 下一個標籤",
                "標籤樹 · 多行輸入 · PTY 終端",
            ],
        }
    }
}

/// Strings MiniCon writes on its own host UI.
///
/// A struct rather than a lookup by key: a missing translation is then a
/// compile error instead of a blank label discovered by a user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HostUiStrings {
    pub paste_failed: &'static str,
    pub send: &'static str,
    pub send_hint: &'static str,
    pub newline: &'static str,
    pub newline_hint: &'static str,
    pub send_to: &'static str,
    pub empty_title: &'static str,
    pub new_terminal: &'static str,
    pub new_terminal_hint: &'static str,
}

pub fn tree_hit(
    layout: Layout,
    x: u32,
    y: u32,
    scroll_offset: usize,
    item_count: usize,
    scale: f64,
) -> TreeHit {
    if !layout.sidebar.contains(x, y) {
        return TreeHit::Outside;
    }
    if layout.new_root.contains(x, y) {
        return TreeHit::NewRoot;
    }
    if layout.help.contains(x, y) {
        return TreeHit::Help;
    }
    if layout.language_chinese.contains(x, y) {
        return TreeHit::Language(UiLanguage::Chinese);
    }
    if layout.language_english.contains(x, y) {
        return TreeHit::Language(UiLanguage::English);
    }
    if layout.zoom_out.contains(x, y) {
        return TreeHit::ZoomOut;
    }
    if layout.zoom_reset.contains(x, y) {
        return TreeHit::ZoomReset;
    }
    if layout.zoom_in.contains(x, y) {
        return TreeHit::ZoomIn;
    }
    if y < layout.tree_header_height {
        return TreeHit::Background;
    }
    let visible_row = ((y - layout.tree_header_height) / layout.tree_row_height) as usize;
    let index = scroll_offset.saturating_add(visible_row);
    if index >= item_count {
        return TreeHit::Background;
    }
    if layout.tree_close_rect(visible_row, scale).contains(x, y) {
        TreeHit::Close(index)
    } else {
        TreeHit::Select(index)
    }
}

pub fn composer_hit(layout: Layout, x: u32, y: u32) -> ComposerHit {
    if layout.composer_send.contains(x, y) {
        ComposerHit::Send
    } else if layout.composer_newline.contains(x, y) {
        ComposerHit::Newline
    } else if layout.composer.contains(x, y) {
        ComposerHit::Input
    } else {
        ComposerHit::Outside
    }
}

pub fn clamp_tree_scroll(offset: usize, item_count: usize, capacity: usize) -> usize {
    offset.min(item_count.saturating_sub(capacity.max(1)))
}

pub fn scroll_tree(offset: usize, delta_rows: isize, item_count: usize, capacity: usize) -> usize {
    let next = if delta_rows < 0 {
        offset.saturating_sub(delta_rows.unsigned_abs())
    } else {
        offset.saturating_add(delta_rows as usize)
    };
    clamp_tree_scroll(next, item_count, capacity)
}

pub fn reveal_tree_index(offset: usize, index: usize, item_count: usize, capacity: usize) -> usize {
    let capacity = capacity.max(1);
    let next = if index < offset {
        index
    } else if index >= offset.saturating_add(capacity) {
        index.saturating_add(1).saturating_sub(capacity)
    } else {
        offset
    };
    clamp_tree_scroll(next, item_count, capacity)
}

fn dip(value: f64, scale: f64) -> u32 {
    minicon_core::numeric::round_f64(value * scale.max(1.0)).max(0.0) as u32
}

pub fn terminal_scrollbar_width(scale: f64) -> u32 {
    dip(TERMINAL_SCROLLBAR_WIDTH_DIP, scale).max(1)
}

pub fn sidebar_width_from_pointer(pointer_x: f64, client_width: f64) -> f64 {
    let maximum = clamp_f64(
        client_width - TERMINAL_MIN_WIDTH_DIP,
        SIDEBAR_MIN_WIDTH_DIP,
        SIDEBAR_MAX_WIDTH_DIP,
    );
    clamp_f64(pointer_x, SIDEBAR_MIN_WIDTH_DIP, maximum)
}

#[allow(clippy::manual_clamp)] // Callers establish ordered bounds; avoid fmt panic glue.
fn clamp_f64(value: f64, minimum: f64, maximum: f64) -> f64 {
    if value.is_nan() || value < minimum {
        minimum
    } else if value > maximum {
        maximum
    } else {
        value
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TerminalViewport {
    pub width: u32,
    pub height: u32,
    pub left: u32,
    pub top: u32,
    pub bottom_inset: u32,
    pub scale: f64,
    pub rows: usize,
}

pub fn terminal_scrollbar_geometry(
    viewport: TerminalViewport,
    offset: usize,
    maximum: usize,
) -> minicon_core::scrollbar::ScrollbarGeometry {
    minicon_core::scrollbar::terminal_scrollbar_geometry(
        minicon_core::scrollbar::ScrollbarRect {
            left: viewport.left.min(i32::MAX as u32) as i32,
            top: viewport.top.min(i32::MAX as u32) as i32,
            right: viewport.width.min(i32::MAX as u32) as i32,
            bottom: viewport
                .height
                .saturating_sub(viewport.bottom_inset)
                .min(i32::MAX as u32) as i32,
        },
        terminal_scrollbar_width(viewport.scale).min(i32::MAX as u32) as i32,
        viewport.rows,
        offset,
        maximum,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// `Rect::contains` is half-open on both axes, and the saturating add means
    /// an extreme origin plus width cannot wrap past zero and swallow unrelated
    /// pixels. A degenerate rect contains nothing.
    #[test]
    fn rect_contains_is_half_open_and_never_wraps() {
        let rect = Rect {
            x: 10,
            y: 20,
            width: 5,
            height: 4,
        };
        // Corners: the near corner is inside, the far corner is not.
        assert!(rect.contains(10, 20), "the origin is inside");
        assert!(rect.contains(14, 23), "the last cell is inside");
        assert!(!rect.contains(15, 23), "x == x + width is outside");
        assert!(!rect.contains(14, 24), "y == y + height is outside");
        assert!(!rect.contains(9, 20), "left of the rect is outside");
        assert!(!rect.contains(10, 19), "above the rect is outside");

        // Zero-sized rects cover nothing, whatever is probed.
        let flat = Rect {
            x: 5,
            y: 5,
            width: 0,
            height: 0,
        };
        for (x, y) in [(5, 5), (0, 0), (6, 6)] {
            assert!(!flat.contains(x, y), "a zero rect contains ({x},{y})");
        }

        // An origin near the ceiling plus a large width must not wrap. The
        // right edge saturates at `u32::MAX`, and the half-open bound then
        // *excludes* `u32::MAX` itself — a one-pixel shrink at the extreme,
        // chosen over wrapping to zero and swallowing everything.
        let ceiling = Rect {
            x: u32::MAX - 1,
            y: 0,
            width: 100,
            height: 1,
        };
        assert!(ceiling.contains(u32::MAX - 1, 0));
        assert!(
            !ceiling.contains(u32::MAX, 0),
            "the saturated right edge is exclusive"
        );
        assert!(
            !ceiling.contains(0, 0),
            "the saturated rect must not wrap to zero"
        );
    }

    #[test]
    fn layout_separates_sidebar_terminal_and_composer_controls() {
        let layout = Layout::new(1200, 800, 1.0);
        assert_eq!(layout.sidebar.width, 224);
        assert_eq!(layout.composer.y, 704);
        assert!(layout.composer_input.width > layout.composer_send.width);
        assert!(layout.composer_input.x >= layout.composer.x);
        assert!(layout.composer_input.x + layout.composer_input.width < layout.composer_send.x);
    }

    /// New-root, help, two language entries and three size controls never overlap —
    /// seven tool rects share one header row, and
    /// an overlap would make one of them unreachable.
    #[test]
    fn the_header_tools_are_ordered_and_disjoint() {
        for (width, scale) in [(1000, 1.0), (760, 1.0), (1600, 1.5), (2400, 2.0)] {
            let layout = Layout::new(width, 600, scale);
            let tools = [
                layout.new_root,
                layout.help,
                layout.language_chinese,
                layout.language_english,
                layout.zoom_out,
                layout.zoom_reset,
                layout.zoom_in,
            ];
            for pair in tools.windows(2) {
                assert!(
                    pair[0].x + pair[0].width <= pair[1].x,
                    "tools overlap at {width}x{scale}: {pair:?}"
                );
            }
            assert!(
                tools[6].x + tools[6].width <= layout.sidebar.width,
                "the rightmost tool leaves the sidebar at {width}x{scale}"
            );
            assert!(
                tools[0].y + tools[0].height <= layout.tree_header_height,
                "tools spill into the tab rows at {width}x{scale}"
            );
        }
    }

    /// The header's seven tools lay out at their own size regardless of how
    /// narrow the frame is, so a frame narrower than roughly two hundred pixels
    /// has header controls with area outside the frame — `zoom_in` is the last
    /// one out, at `x = 156` with a 24-pixel box. That is the current behaviour
    /// and it is recorded rather than asserted away: the ordering test only
    /// covers wide frames, so a reader should not assume narrow ones are safe.
    /// What is pinned here is the exact boundary and the fact that widening the
    /// frame fixes it, so a future layout change has a number to beat.
    #[test]
    fn a_narrow_frame_overflows_the_header_row_and_widening_it_does_not() {
        // A probe established the boundary: 176 wide still clips `zoom_in`,
        // 200 wide does not.
        let clipped = Layout::new(176, 80, 1.0);
        assert!(
            clipped.zoom_in.x + clipped.zoom_in.width > clipped.sidebar.width,
            "the last tool overflows a 176-wide frame: {:?} vs sidebar {}",
            clipped.zoom_in,
            clipped.sidebar.width
        );
        let fits = Layout::new(200, 80, 1.0);
        assert!(
            fits.zoom_in.x + fits.zoom_in.width <= fits.sidebar.width,
            "a 200-wide frame holds the whole row: {:?} vs sidebar {}",
            fits.zoom_in,
            fits.sidebar.width
        );
        // Even where the row overflows, the frame itself is still partitioned:
        // the sidebar and composer never claim more than the frame, and the
        // composer's own children stay inside it.
        for (width, height) in [(176u32, 80u32), (150, 60), (100, 40)] {
            let layout = Layout::new(width, height, 1.0);
            assert!(layout.sidebar.width <= width, "the sidebar fits the frame");
            assert!(layout.sidebar.height <= height);
            assert!(
                layout.composer.x + layout.composer.width <= width,
                "the composer strip fits the frame at {width}x{height}"
            );
            assert!(layout.composer.y + layout.composer.height <= height);
        }
    }

    #[test]
    fn new_root_header_button_has_its_own_hit_target() {
        let layout = Layout::new(1000, 500, 1.0);
        assert_eq!(
            tree_hit(
                layout,
                layout.new_root.x + layout.new_root.width / 2,
                layout.new_root.y + layout.new_root.height / 2,
                0,
                1,
                1.0,
            ),
            TreeHit::NewRoot
        );
    }

    #[test]
    fn help_header_button_has_its_own_hit_target() {
        let layout = Layout::new(1200, 800, 1.0);
        assert_eq!(
            tree_hit(
                layout,
                layout.help.x + layout.help.width / 2,
                layout.help.y + layout.help.height / 2,
                0,
                1,
                1.0,
            ),
            TreeHit::Help
        );
    }

    #[test]
    fn each_language_entry_is_hit_separately() {
        let layout = Layout::new(1000, 500, 1.0);
        assert_eq!(
            tree_hit(
                layout,
                layout.language_chinese.x + 1,
                layout.language_chinese.y + 1,
                0,
                4,
                1.0
            ),
            TreeHit::Language(UiLanguage::Chinese)
        );
        assert_eq!(
            tree_hit(
                layout,
                layout.language_english.x + 1,
                layout.language_english.y + 1,
                0,
                4,
                1.0
            ),
            TreeHit::Language(UiLanguage::English)
        );
    }

    /// Every entry is labelled in the language it selects, and every language
    /// has a complete set of host UI strings. A struct rather than a map is
    /// what makes the second half a compile error instead of a blank label.
    #[test]
    fn each_language_labels_itself_and_translates_every_string() {
        for language in [UiLanguage::English, UiLanguage::Chinese] {
            assert!(!language.entry_label().is_empty());
            assert!(!language.tag().is_empty());
            let strings = language.strings();
            assert!(!strings.paste_failed.is_empty());
            assert!(!strings.send.is_empty());
            assert!(!strings.send_hint.is_empty());
            assert!(!strings.newline.is_empty());
            assert!(!strings.newline_hint.is_empty());
            assert!(!strings.send_to.is_empty());
        }
        assert_eq!(UiLanguage::Chinese.entry_label(), "中");
        assert_eq!(UiLanguage::English.entry_label(), "En");
        assert_eq!(UiLanguage::English.strings().send, "Send");
        assert_eq!(UiLanguage::English.strings().send_hint, "(ctrl-o)");
        assert_eq!(UiLanguage::English.strings().newline, "New Line");
        assert_eq!(UiLanguage::English.strings().newline_hint, "(Enter)");
        assert_ne!(
            UiLanguage::English.strings().send,
            UiLanguage::Chinese.strings().send,
            "a language that translates nothing is a switch with no effect"
        );
        // The tab identifier travels with the label, so a translated prefix
        // must still leave the `@` that names the tab.
        assert!(UiLanguage::Chinese.strings().send_to.ends_with('@'));
        assert!(UiLanguage::English.strings().send_to.ends_with('@'));
    }

    #[test]
    fn english_is_the_default_language() {
        assert_eq!(UiLanguage::default(), UiLanguage::English);
        assert_eq!(UiLanguage::default().tag(), "en");
    }

    #[test]
    fn tree_hit_distinguishes_rows_close_buttons_and_background() {
        let layout = Layout::new(1000, 500, 1.0);
        assert_eq!(tree_hit(layout, 220, 10, 0, 4, 1.0), TreeHit::Background);
        assert_eq!(
            tree_hit(
                layout,
                layout.zoom_out.x + 1,
                layout.zoom_out.y + 1,
                0,
                4,
                1.0
            ),
            TreeHit::ZoomOut
        );
        assert_eq!(
            tree_hit(
                layout,
                layout.zoom_reset.x + 1,
                layout.zoom_reset.y + 1,
                0,
                4,
                1.0
            ),
            TreeHit::ZoomReset
        );
        assert_eq!(
            tree_hit(
                layout,
                layout.zoom_in.x + 1,
                layout.zoom_in.y + 1,
                0,
                4,
                1.0
            ),
            TreeHit::ZoomIn
        );
        assert_eq!(tree_hit(layout, 20, 40, 1, 4, 1.0), TreeHit::Select(1));
        let close = layout.tree_close_rect(0, 1.0);
        assert_eq!(
            tree_hit(layout, close.x + 1, close.y + 1, 1, 4, 1.0),
            TreeHit::Close(1)
        );
        assert_eq!(tree_hit(layout, 20, 490, 0, 1, 1.0), TreeHit::Background);
        assert_eq!(tree_hit(layout, 500, 40, 0, 4, 1.0), TreeHit::Outside);
    }

    /// The header tools each own their hit before the tree rows are considered,
    /// including the ones the first test did not probe. A click on any of them
    /// must never fall through to a row underneath.
    #[test]
    fn every_header_tool_wins_over_the_rows_beneath_it() {
        let layout = Layout::new(1000, 500, 1.0);
        for (rect, expected) in [
            (layout.new_root, TreeHit::NewRoot),
            (layout.help, TreeHit::Help),
            (
                layout.language_chinese,
                TreeHit::Language(UiLanguage::Chinese),
            ),
            (
                layout.language_english,
                TreeHit::Language(UiLanguage::English),
            ),
        ] {
            let hit = tree_hit(layout, rect.x, rect.y, 0, 4, 1.0);
            assert_eq!(hit, expected, "corner of {expected:?} must hit it");
        }
    }

    /// The first tree row starts exactly at `tree_header_height`: one pixel
    /// above is background and the boundary pixel itself is row zero. Each row
    /// owns `tree_row_height` pixels, so the boundary between row 0 and row 1
    /// decides which index a click selects.
    #[test]
    fn rows_begin_at_the_header_boundary_and_partition_by_row_height() {
        let layout = Layout::new(1000, 500, 1.0);
        let header = layout.tree_header_height;
        let row_h = layout.tree_row_height;
        // One pixel above the first row is still background.
        assert_eq!(
            tree_hit(layout, 20, header - 1, 0, 8, 1.0),
            TreeHit::Background
        );
        // The boundary pixel is the first row.
        assert_eq!(tree_hit(layout, 20, header, 0, 8, 1.0), TreeHit::Select(0));
        // The last pixel of row 0 is still row 0; the next pixel is row 1.
        assert_eq!(
            tree_hit(layout, 20, header + row_h - 1, 0, 8, 1.0),
            TreeHit::Select(0)
        );
        assert_eq!(
            tree_hit(layout, 20, header + row_h, 0, 8, 1.0),
            TreeHit::Select(1)
        );
        // A scroll offset shifts which item a visible row maps to.
        assert_eq!(
            tree_hit(layout, 20, header + row_h, 5, 8, 1.0),
            TreeHit::Select(6)
        );
        // Past the last item is background, not a negative or wrapped index.
        assert_eq!(
            tree_hit(layout, 20, header + row_h * 8, 0, 8, 1.0),
            TreeHit::Background
        );
    }

    #[test]
    fn scrolling_is_bounded_and_reveals_active_rows() {
        assert_eq!(scroll_tree(0, 3, 10, 4), 3);
        assert_eq!(scroll_tree(3, -9, 10, 4), 0);
        assert_eq!(scroll_tree(5, 9, 10, 4), 6);
        assert_eq!(reveal_tree_index(0, 8, 10, 4), 5);
        assert_eq!(reveal_tree_index(5, 2, 10, 4), 2);
    }

    /// A tree that fits, an empty tree, or a zero-height viewport must all pin
    /// the scroll to zero rather than underflowing or scrolling into blank
    /// rows; a zero capacity is treated as one visible row, not as "show
    /// nothing".
    /// `tree_capacity` is the single "visible rows" input the scroll functions
    /// take. It is the sidebar height minus the header, divided by the row
    /// height, and must be zero rather than a panic when the header is taller
    /// than the sidebar or the row height is degenerate.
    #[test]
    fn tree_capacity_divides_the_sidebar_below_the_header() {
        let layout = Layout::new(1000, 500, 1.0);
        let usable = layout.sidebar.height - layout.tree_header_height;
        assert_eq!(
            layout.tree_capacity(),
            (usable / layout.tree_row_height) as usize
        );
        // A sidebar shorter than its header has no room for a row.
        let cramped = Layout::with_sidebar_width(1000, layout.tree_header_height, 1.0, 224.0);
        assert_eq!(cramped.tree_capacity(), 0, "no room means no rows");
        // A taller window shows more rows, never fewer.
        let taller = Layout::new(1000, 900, 1.0);
        assert!(taller.tree_capacity() >= layout.tree_capacity());
    }

    /// The scroll offset and the capacity are consumed together, so whatever
    /// the pair, the result must stay inside the reachable range: never past
    /// `item_count - capacity` (a blank strip below the last row) and never a
    /// panic when the capacity exceeds the item count or is zero.
    #[test]
    fn scroll_and_capacity_stay_within_the_reachable_range() {
        for item_count in [0_usize, 1, 2, 5, 40] {
            for capacity in [0_usize, 1, 2, 3, 7, 64] {
                let effective = capacity.max(1);
                let maximum = item_count.saturating_sub(effective);

                // Every reveal of the last item bottoms the window out.
                let offset =
                    reveal_tree_index(0, item_count.saturating_sub(1), item_count, capacity);
                assert!(
                    offset <= maximum,
                    "reveal left {offset} past the max {maximum} (items {item_count}, capacity {capacity})"
                );

                // Clamping an already-too-large offset (a window shrunk after
                // the offset was set) brings it back in range.
                let clamped = clamp_tree_scroll(usize::MAX, item_count, capacity);
                assert_eq!(clamped, maximum, "a stale offset must be clamped");

                // Scrolling never moves outside the range either, whichever
                // direction the delta points.
                for delta in [isize::MIN, -5, -1, 0, 1, 5, isize::MAX] {
                    let moved = scroll_tree(clamped, delta, item_count, capacity);
                    assert!(
                        moved <= maximum,
                        "scroll {delta} left {moved} past the max {maximum}"
                    );
                }
            }
        }
    }

    /// The scroll functions take the capacity the layout actually reports, so a
    /// capacity change in the layout cannot silently break the scroll: revealing
    /// the last item bottoms out at `items - capacity` and the revealed window
    /// still contains that item.
    #[test]
    fn the_layout_capacity_drives_the_scroll_functions() {
        let layout = Layout::new(1000, 500, 1.0);
        let capacity = layout.tree_capacity();
        assert!(capacity > 0, "this window must show at least one row");
        let items = capacity + 10;
        // Revealing the last item must bottom out at `items - capacity`.
        assert_eq!(
            reveal_tree_index(0, items - 1, items, capacity),
            items - capacity
        );
        // The revealed window contains the last item.
        let offset = reveal_tree_index(0, items - 1, items, capacity);
        assert!(offset < items && items - 1 < offset + capacity);
    }

    #[test]
    fn scrolling_handles_empty_and_degenerate_viewports() {
        // The whole tree fits: no scrolling is possible.
        for offset in [0, 1, 5] {
            assert_eq!(
                scroll_tree(offset, 3, 3, 4),
                0,
                "a fitting tree cannot scroll"
            );
            assert_eq!(clamp_tree_scroll(offset, 3, 4), 0);
        }
        // An empty tree has nothing to scroll to.
        for capacity in [0, 1, 8] {
            assert_eq!(
                scroll_tree(9, 2, 0, capacity),
                0,
                "an empty tree stays at zero"
            );
            assert_eq!(reveal_tree_index(4, 0, 0, capacity), 0);
        }
        // A zero-height viewport still reveals one row (capacity floored to 1),
        // so the offset can move freely up to `item_count - 1`.
        assert_eq!(
            scroll_tree(0, 5, 10, 0),
            5,
            "one visible row, but the offset still moves"
        );
        assert_eq!(
            reveal_tree_index(0, 3, 10, 0),
            3,
            "the row is the whole viewport"
        );
        assert_eq!(
            clamp_tree_scroll(9, 10, 0),
            9,
            "with capacity 1 the max offset is n-1"
        );
        assert_eq!(clamp_tree_scroll(99, 10, 0), 9, "and it is bounded there");
    }

    /// `reveal_tree_index` scrolls exactly when the row is outside the window,
    /// including the two boundary rows, and never past the end. The last row of
    /// a long tree must end up visible with the window bottomed out.
    #[test]
    fn reveal_scrolls_at_the_window_edges_and_bottoms_out() {
        // index == offset + capacity is the first row past the window.
        assert_eq!(reveal_tree_index(0, 4, 10, 4), 1);
        // index == offset is already visible.
        assert_eq!(reveal_tree_index(0, 0, 10, 4), 0);
        assert_eq!(reveal_tree_index(3, 3, 10, 4), 3);
        // index == offset + capacity - 1 is the last visible row.
        assert_eq!(reveal_tree_index(0, 3, 10, 4), 0);
        // The final row of the tree: the window bottoms out, not past the end.
        assert_eq!(reveal_tree_index(0, 9, 10, 4), 6);
        assert_eq!(reveal_tree_index(6, 9, 10, 4), 6, "already visible");
    }

    /// `scroll_tree` takes a signed delta; the most-negative value must scroll
    /// to the top instead of overflowing `unsigned_abs`.
    #[test]
    fn scroll_tree_handles_the_most_negative_delta() {
        assert_eq!(scroll_tree(3, isize::MIN, 10, 4), 0);
        assert_eq!(
            scroll_tree(3, isize::MAX, 10, 4),
            6,
            "a huge down-scroll clamps"
        );
    }

    #[test]
    fn composer_hit_has_a_distinct_send_action() {
        let layout = Layout::new(1000, 500, 1.0);
        assert_eq!(
            composer_hit(
                layout,
                layout.composer_input.x + 1,
                layout.composer_input.y + 1
            ),
            ComposerHit::Input
        );
        assert_eq!(
            composer_hit(
                layout,
                layout.composer_send.x + 1,
                layout.composer_send.y + 1
            ),
            ComposerHit::Send
        );
        assert_eq!(
            composer_hit(
                layout,
                layout.composer_newline.x + 1,
                layout.composer_newline.y + 1
            ),
            ComposerHit::Newline
        );
        assert_eq!(composer_hit(layout, 500, 100), ComposerHit::Outside);
    }

    /// The control buttons sit inside the composer strip, so the only reason a
    /// click on one is not `Input` is the priority chain. Pin that: a point in
    /// both the strip and a button resolves to the button, the boundary pixel
    /// just past a button falls back to the strip, and the gap between the
    /// input area and the buttons is still `Input` rather than dead space.
    #[test]
    fn composer_buttons_take_priority_inside_their_strip() {
        let layout = Layout::new(1200, 800, 1.0);
        // Buttons live inside the composer strip.
        assert!(
            layout
                .composer
                .contains(layout.composer_send.x, layout.composer_send.y)
        );
        // A point in the send button is Send, not Input, even though the strip
        // also contains it.
        assert_eq!(
            composer_hit(layout, layout.composer_send.x, layout.composer_send.y),
            ComposerHit::Send
        );
        // One pixel past the send button's right edge is no longer Send.
        let past = layout.composer_send.x + layout.composer_send.width;
        assert_ne!(
            composer_hit(layout, past, layout.composer_send.y),
            ComposerHit::Send
        );
        // The gap between the input area and the send button is part of the
        // strip, so it is Input rather than Outside.
        let gap_x = layout.composer_input.x + layout.composer_input.width + 1;
        assert!(
            gap_x < layout.composer_send.x,
            "the layout must keep a gap: input ends at {} send starts at {}",
            layout.composer_input.x + layout.composer_input.width,
            layout.composer_send.x
        );
        assert_eq!(
            composer_hit(layout, gap_x, layout.composer_input.y + 1),
            ComposerHit::Input,
            "the gap between input and send belongs to the strip"
        );
        // The newline button is below the send button and takes its own point.
        assert_eq!(
            composer_hit(layout, layout.composer_newline.x, layout.composer_newline.y),
            ComposerHit::Newline
        );
        // The very first pixel of the strip, left of the input inset, is still
        // the strip (padding) rather than outside it.
        assert_eq!(
            composer_hit(
                layout,
                layout.composer.x,
                layout.composer.y + layout.composer.height - 1
            ),
            ComposerHit::Input,
            "the strip's own corner is Input"
        );
    }

    #[test]
    fn the_two_composer_buttons_stack_without_overlapping() {
        let layout = Layout::new(1000, 500, 1.0);
        let send = layout.composer_send;
        let newline = layout.composer_newline;
        // Same column: the pair replaced one control and must not take a
        // second bite out of the input's width.
        assert_eq!(send.x, newline.x);
        assert_eq!(send.width, newline.width);
        // Newline sits below with a gap, and neither box is empty -- a
        // zero-height button is unclickable while still passing a hit test
        // written against its rect.
        assert!(send.height > 0 && newline.height > 0);
        assert!(newline.y >= send.y + send.height);
        // Still inside the strip the input yields to them.
        assert!(layout.composer_input.x + layout.composer_input.width <= send.x);
    }

    #[test]
    fn sidebar_drag_is_bounded_and_preserves_terminal_floor() {
        assert_eq!(
            sidebar_width_from_pointer(90.0, 1000.0),
            SIDEBAR_MIN_WIDTH_DIP
        );
        assert_eq!(sidebar_width_from_pointer(330.0, 1000.0), 330.0);
        assert_eq!(sidebar_width_from_pointer(900.0, 1000.0), 480.0);
        assert_eq!(
            sidebar_width_from_pointer(300.0, 450.0),
            SIDEBAR_MIN_WIDTH_DIP
        );
    }

    #[test]
    fn scrollbar_owns_terminal_right_edge() {
        let g = terminal_scrollbar_geometry(
            TerminalViewport {
                width: 1200,
                height: 800,
                left: 224,
                top: 0,
                bottom_inset: 96,
                scale: 1.0,
                rows: 24,
            },
            0,
            100,
        );
        assert_eq!(
            (g.track.left, g.track.right, g.track.bottom),
            (1188, 1200, 704)
        );
    }

    fn viewport(scale: f64, rows: usize, bottom_inset: u32) -> TerminalViewport {
        TerminalViewport {
            width: 1200,
            height: 800,
            left: 224,
            top: 0,
            bottom_inset,
            scale,
            rows,
        }
    }

    /// With nothing to scroll (`maximum == 0`) the thumb is the whole track and
    /// rests at the top, whatever offset is handed in.
    #[test]
    fn scrollbar_thumb_fills_the_track_when_there_is_nothing_to_scroll() {
        let g = terminal_scrollbar_geometry(viewport(1.0, 24, 96), 0, 0);
        assert_eq!(g.thumb.top, g.track.top);
        assert_eq!(g.thumb.height(), g.track.height(), "the thumb is the track");
        // A stale non-zero offset cannot move a thumb with no travel.
        let moved = terminal_scrollbar_geometry(viewport(1.0, 24, 96), 50, 0);
        assert_eq!(moved.thumb.top, g.thumb.top);
    }

    /// `offset > maximum` is clamped, so the thumb never travels past the
    /// bottom of the track and never reports a negative travel.
    #[test]
    fn scrollbar_thumb_clamps_an_out_of_range_offset_to_the_bottom() {
        let bottom = terminal_scrollbar_geometry(viewport(1.0, 24, 96), 100, 100);
        let over = terminal_scrollbar_geometry(viewport(1.0, 24, 96), 1_000_000, 100);
        assert_eq!(
            over.thumb.top, bottom.thumb.top,
            "past the end clamps to the end"
        );
        assert!(
            over.thumb.top >= over.track.top,
            "the thumb stays in the track"
        );
        assert!(
            over.thumb.bottom <= over.track.bottom,
            "the thumb never leaves the track bottom"
        );
    }

    /// An inset taller than the viewport, or zero rows, must not underflow or
    /// divide by zero; the geometry stays a valid, empty-or-flat shape.
    #[test]
    fn scrollbar_geometry_survives_a_degenerate_viewport() {
        // bottom_inset > height: the track collapses to zero height. The
        // saturating subtraction is what makes the bottom land at the top of
        // the viewport instead of wrapping to a near-`i32::MAX` value.
        let collapsed = terminal_scrollbar_geometry(viewport(1.0, 24, 5000), 0, 100);
        assert_eq!(
            collapsed.track.bottom, 0,
            "an inset past the height must collapse the track, not wrap it"
        );
        assert_eq!(collapsed.track.height(), 0);

        // Zero visible rows: the proportional share is guarded by `.max(1)`.
        let no_rows = terminal_scrollbar_geometry(viewport(1.0, 0, 96), 0, 100);
        assert!(no_rows.thumb.height() >= 0);
        assert!(no_rows.thumb.top >= no_rows.track.top);

        // An extreme scale keeps the width conversion inside `i32`.
        let huge = terminal_scrollbar_geometry(viewport(f64::INFINITY, 24, 96), 0, 100);
        assert!(
            huge.track.right >= huge.track.left,
            "the track stays ordered"
        );
    }

    #[test]
    fn extreme_geometry_inputs_saturate_without_panicking_or_collapsing_sidebar() {
        let layout = Layout::with_sidebar_width(u32::MAX, u32::MAX, f64::INFINITY, f64::NAN);
        assert_eq!(layout.sidebar.width, u32::MAX);
        assert_eq!(layout.composer.width, 0);
        assert_eq!(layout.composer_send.width, 0);

        let close = layout.tree_close_rect(usize::MAX, f64::INFINITY);
        assert_eq!(close.y, u32::MAX);

        assert_eq!(
            sidebar_width_from_pointer(f64::NAN, 1000.0),
            SIDEBAR_MIN_WIDTH_DIP
        );
        assert_eq!(
            sidebar_width_from_pointer(300.0, f64::NAN),
            SIDEBAR_MIN_WIDTH_DIP
        );
    }

    /// The sidebar width follows the pointer within two bounds: the fixed
    /// minimum/maximum, and a cap derived from the client width so the terminal
    /// keeps `TERMINAL_MIN_WIDTH_DIP`. A client too narrow to satisfy both
    /// floors the range at the minimum, and a negative pointer still clamps to
    /// the minimum rather than going negative.
    /// The scrollbar width scales with the display and floors at one pixel, so
    /// it is never invisible. A scale below one is treated as one — the layout
    /// rule everywhere else — and a non-finite scale is bounded by that same
    /// floor rather than propagating a NaN width. The function's own trailing
    /// `.max(1)` is defensive: `dip` already floors the scale, so it cannot fire
    /// for a scale the layout produces (pinned below).
    #[test]
    fn the_scrollbar_width_scales_and_never_vanishes() {
        assert_eq!(
            terminal_scrollbar_width(1.0),
            TERMINAL_SCROLLBAR_WIDTH_DIP as u32
        );
        assert!(terminal_scrollbar_width(2.0) > terminal_scrollbar_width(1.0));
        assert!(terminal_scrollbar_width(4.0) > terminal_scrollbar_width(2.0));
        // Below one, the scale is floored: the same width as unscaled.
        for scale in [0.0, 0.5] {
            assert_eq!(
                terminal_scrollbar_width(scale),
                terminal_scrollbar_width(1.0),
                "a sub-unit scale is floored to one, not shrunk"
            );
        }
        // NaN floors to one through `f64::max`, so no NaN width escapes.
        assert_eq!(
            terminal_scrollbar_width(f64::NAN),
            terminal_scrollbar_width(1.0)
        );
        // And it is never zero at any scale the layout can produce.
        for scale in [0.0, 0.5, 1.0, 1.25, 1.5, 2.0, 3.0, 4.0] {
            assert!(
                terminal_scrollbar_width(scale) >= 1,
                "a scrollbar of zero width is invisible at scale {scale}"
            );
        }
        // An unbounded scale saturates to `u32::MAX` rather than wrapping; the
        // geometry layer clamps it to `i32::MAX` before it reaches a rect.
        assert_eq!(terminal_scrollbar_width(f64::INFINITY), u32::MAX);
    }

    /// Dragging to either end reaches the ends at every display scale, not just
    /// the unscaled one: the mapping between a thumb top and a scrollback offset
    /// has to survive a change of scale, and the track must stay ordered.
    #[test]
    fn dragging_reaches_the_ends_at_every_display_scale() {
        use minicon_core::scrollbar::scrollback_for_thumb_top;
        for scale in [1.0, 1.25, 1.5, 2.0, 3.0] {
            let geometry = terminal_scrollbar_geometry(viewport(scale, 24, 96), 0, 100);
            assert!(
                geometry.track.right >= geometry.track.left,
                "the track must stay ordered at scale {scale}"
            );
            assert!(
                geometry.thumb.height() > 0,
                "the thumb must be visible at scale {scale}"
            );
            assert_eq!(
                scrollback_for_thumb_top(geometry, geometry.track.top, 100),
                100,
                "the top of the track is the oldest scrollback at scale {scale}"
            );
            assert_eq!(
                scrollback_for_thumb_top(geometry, geometry.track.bottom, 100),
                0,
                "dragging to the bottom returns to the live view at scale {scale}"
            );
            // The drawn thumb for the maximum offset reads back as the maximum.
            let drawn = terminal_scrollbar_geometry(viewport(scale, 24, 96), 100, 100);
            assert_eq!(
                scrollback_for_thumb_top(geometry, drawn.thumb.top, 100),
                100,
                "the drawn top must read back as the maximum at scale {scale}"
            );
        }
    }

    #[test]
    fn sidebar_width_follows_the_pointer_within_the_client_cap() {
        // A wide client: the pointer is limited only by the fixed maximum.
        let wide = 2000.0;
        assert_eq!(
            sidebar_width_from_pointer(100.0, wide),
            SIDEBAR_MIN_WIDTH_DIP,
            "below the minimum clamps up"
        );
        assert_eq!(sidebar_width_from_pointer(300.0, wide), 300.0, "in range");
        assert_eq!(
            sidebar_width_from_pointer(999.0, wide),
            SIDEBAR_MAX_WIDTH_DIP,
            "past the maximum clamps down to the fixed cap"
        );

        // A narrow client caps the sidebar below the fixed maximum so the
        // terminal keeps its minimum width: 320 + 400 = 720.
        let narrow = TERMINAL_MIN_WIDTH_DIP + 400.0;
        assert_eq!(
            sidebar_width_from_pointer(999.0, narrow),
            400.0,
            "the client-derived cap wins over the fixed maximum"
        );

        // A client too narrow to hold the terminal at all floors both bounds at
        // the minimum, so the sidebar cannot shrink below it.
        assert_eq!(
            sidebar_width_from_pointer(999.0, 100.0),
            SIDEBAR_MIN_WIDTH_DIP,
            "a cramped client still keeps the minimum sidebar"
        );
        assert_eq!(
            sidebar_width_from_pointer(-50.0, wide),
            SIDEBAR_MIN_WIDTH_DIP,
            "a negative pointer clamps to the minimum"
        );
    }

    /// The drag path draws a thumb at the offset's position and then reads a
    /// dragged thumb position back into an offset, so the two must be inverses.
    /// Round-trip every offset: place the thumb for it, feed that top back, and
    /// require the same offset within the rounding the geometry implies.
    #[test]
    fn dragging_a_thumb_round_trips_to_the_offset_it_was_drawn_from() {
        use minicon_core::scrollbar::{ScrollbarGeometry, scrollback_for_thumb_top};
        let view = viewport(1.0, 24, 96);
        for maximum in [0usize, 1, 7, 100, 5000] {
            let geometry: ScrollbarGeometry = terminal_scrollbar_geometry(view, 0, maximum);
            let travel = geometry.track.height() - geometry.thumb.height();
            for offset in 0..=maximum.min(200) {
                let drawn = terminal_scrollbar_geometry(view, offset, maximum);
                let back = scrollback_for_thumb_top(geometry, drawn.thumb.top, maximum);
                if maximum == 0 || travel <= 0 {
                    assert_eq!(back, 0, "no travel means no scrollback");
                    continue;
                }
                // One pixel of travel can only resolve to a band of offsets, so
                // the round trip must land within that band, never wildly off.
                let band = (maximum as i64 + travel as i64 - 1) / travel as i64;
                let delta = (back as i64 - offset as i64).abs();
                assert!(
                    delta <= band,
                    "offset {offset} of {maximum} round-tripped to {back} (band {band})"
                );
            }

            // The round trip is only meaningful if the drawing actually moves:
            // with travel the two extremes must differ, so an offset cannot be
            // "recovered" merely because every drag reports the same position.
            if maximum > 0 && travel > 0 {
                let top = terminal_scrollbar_geometry(view, maximum, maximum)
                    .thumb
                    .top;
                let bottom = terminal_scrollbar_geometry(view, 0, maximum).thumb.top;
                assert!(
                    top < bottom,
                    "maximum {maximum} must place the thumb above offset 0: {top} vs {bottom}"
                );
                assert_eq!(
                    scrollback_for_thumb_top(geometry, top, maximum),
                    maximum,
                    "the drawn top must read back as the maximum offset"
                );
            }
        }
    }

    /// Dragging to either end lands at the ends: the top of the track is offset
    /// `maximum` and the bottom is `0`, and a drag past either edge clamps there
    /// instead of producing an out-of-range scrollback.
    #[test]
    fn dragging_to_the_ends_reaches_the_extremes_and_clamps() {
        use minicon_core::scrollbar::scrollback_for_thumb_top;
        let view = viewport(1.0, 24, 96);
        let geometry = terminal_scrollbar_geometry(view, 0, 100);
        assert_eq!(
            scrollback_for_thumb_top(geometry, geometry.track.top, 100),
            100,
            "the top of the track is the oldest scrollback"
        );
        assert_eq!(
            scrollback_for_thumb_top(geometry, geometry.track.bottom, 100),
            0,
            "dragging past the bottom clamps to the live view"
        );
        assert_eq!(
            scrollback_for_thumb_top(geometry, geometry.track.top - 500, 100),
            100,
            "dragging above the track clamps to the oldest"
        );
    }
}
