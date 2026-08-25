//! Pure geometry and hit-testing for the lightweight console chrome.
//!
//! This module deliberately contains no window-host, PTY, or product authority
//! state. Proven rules can therefore move into a shared frontend contract
//! without importing `minicon` lifecycle policy.

pub const SIDEBAR_WIDTH_DIP: f64 = 224.0;
pub const TREE_HEADER_HEIGHT_DIP: f64 = 32.0;
pub const TREE_ROW_HEIGHT_DIP: f64 = 30.0;
pub const COMPOSER_HEIGHT_DIP: f64 = 96.0;
pub const SIDEBAR_MIN_WIDTH_DIP: f64 = 180.0;
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
    pub zoom_in: Rect,
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
        let zoom_in = Rect {
            x: sidebar_width
                .saturating_sub(dip(8.0, scale))
                .saturating_sub(tool_size),
            y: tool_y,
            width: tool_size,
            height: tool_size,
        };
        let zoom_out = Rect {
            x: zoom_in.x.saturating_sub(tool_size),
            y: tool_y,
            width: tool_size,
            height: tool_size,
        };
        // Language before size, reading right to left: `En` `中` `z` `Z`. The
        // sidebar is at least 180 dip and a tool is 24, so four of them and the
        // product name coexist even at the narrowest layout.
        let language_english = Rect {
            x: zoom_out.x.saturating_sub(tool_size),
            y: tool_y,
            width: tool_size,
            height: tool_size,
        };
        let language_chinese = Rect {
            x: language_english.x.saturating_sub(tool_size),
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
            zoom_in,
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
    ZoomOut,
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
/// Only the chrome is translated — the tab column, the send strip and the
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
    pub const fn strings(self) -> ChromeStrings {
        match self {
            Self::English => ChromeStrings {
                paste_failed: "PASTE FAILED",
                send: "SEND",
                newline: "NEWLINE",
                send_to: "SEND TO @",
            },
            Self::Chinese => ChromeStrings {
                paste_failed: "貼上失敗",
                send: "送出",
                newline: "換行",
                send_to: "送往 @",
            },
        }
    }
}

/// The chrome MiniCon writes itself.
///
/// A struct rather than a lookup by key: a missing translation is then a
/// compile error instead of a blank label discovered by a user.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChromeStrings {
    pub paste_failed: &'static str,
    pub send: &'static str,
    pub newline: &'static str,
    pub send_to: &'static str,
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
    if layout.language_chinese.contains(x, y) {
        return TreeHit::Language(UiLanguage::Chinese);
    }
    if layout.language_english.contains(x, y) {
        return TreeHit::Language(UiLanguage::English);
    }
    if layout.zoom_out.contains(x, y) {
        return TreeHit::ZoomOut;
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
    agenterm_platform::numeric::round_f64(value * scale.max(1.0)).max(0.0) as u32
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
) -> agenterm_ui_core::ScrollbarGeometry {
    agenterm_ui_core::terminal_scrollbar_geometry(
        agenterm_ui_core::Rect {
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

    #[test]
    fn layout_separates_sidebar_terminal_and_composer_controls() {
        let layout = Layout::new(1200, 800, 1.0);
        assert_eq!(layout.sidebar.width, 224);
        assert_eq!(layout.composer.y, 704);
        assert!(layout.composer_input.width > layout.composer_send.width);
        assert!(layout.composer_input.x >= layout.composer.x);
        assert!(layout.composer_input.x + layout.composer_input.width < layout.composer_send.x);
    }

    /// The two language entries sit left of the size controls and never
    /// overlap them or each other — four tool rects share one header row, and
    /// an overlap would make one of them unreachable.
    #[test]
    fn the_header_tools_are_ordered_and_disjoint() {
        for (width, scale) in [(1000, 1.0), (760, 1.0), (1600, 1.5), (2400, 2.0)] {
            let layout = Layout::new(width, 600, scale);
            let tools = [
                layout.language_chinese,
                layout.language_english,
                layout.zoom_out,
                layout.zoom_in,
            ];
            for pair in tools.windows(2) {
                assert!(
                    pair[0].x + pair[0].width <= pair[1].x,
                    "tools overlap at {width}x{scale}: {pair:?}"
                );
            }
            assert!(
                tools[3].x + tools[3].width <= layout.sidebar.width,
                "the rightmost tool leaves the sidebar at {width}x{scale}"
            );
            assert!(
                tools[0].y + tools[0].height <= layout.tree_header_height,
                "tools spill into the tab rows at {width}x{scale}"
            );
        }
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
    /// has a complete set of chrome strings. A struct rather than a map is
    /// what makes the second half a compile error instead of a blank label.
    #[test]
    fn each_language_labels_itself_and_translates_every_string() {
        for language in [UiLanguage::English, UiLanguage::Chinese] {
            assert!(!language.entry_label().is_empty());
            assert!(!language.tag().is_empty());
            let strings = language.strings();
            assert!(!strings.paste_failed.is_empty());
            assert!(!strings.send.is_empty());
            assert!(!strings.send_to.is_empty());
        }
        assert_eq!(UiLanguage::Chinese.entry_label(), "中");
        assert_eq!(UiLanguage::English.entry_label(), "En");
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
        assert_eq!(tree_hit(layout, 10, 10, 0, 4, 1.0), TreeHit::Background);
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

    #[test]
    fn scrolling_is_bounded_and_reveals_active_rows() {
        assert_eq!(scroll_tree(0, 3, 10, 4), 3);
        assert_eq!(scroll_tree(3, -9, 10, 4), 0);
        assert_eq!(scroll_tree(5, 9, 10, 4), 6);
        assert_eq!(reveal_tree_index(0, 8, 10, 4), 5);
        assert_eq!(reveal_tree_index(5, 2, 10, 4), 2);
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
        assert_eq!(sidebar_width_from_pointer(90.0, 1000.0), 180.0);
        assert_eq!(sidebar_width_from_pointer(330.0, 1000.0), 330.0);
        assert_eq!(sidebar_width_from_pointer(900.0, 1000.0), 480.0);
        assert_eq!(sidebar_width_from_pointer(300.0, 450.0), 180.0);
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

    #[test]
    fn extreme_geometry_inputs_saturate_without_panicking_or_collapsing_sidebar() {
        let layout = Layout::with_sidebar_width(u32::MAX, u32::MAX, f64::INFINITY, f64::NAN);
        assert_eq!(layout.sidebar.width, u32::MAX);
        assert_eq!(layout.composer.width, 0);
        assert_eq!(layout.composer_send.width, 0);

        let close = layout.tree_close_rect(usize::MAX, f64::INFINITY);
        assert_eq!(close.y, u32::MAX);

        assert_eq!(sidebar_width_from_pointer(f64::NAN, 1000.0), 180.0);
        assert_eq!(sidebar_width_from_pointer(300.0, f64::NAN), 180.0);
    }
}
