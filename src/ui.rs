//! Pure geometry and hit-testing for the lightweight console host UI.
//!
//! This module deliberately contains no window-host, PTY, or product authority
//! state. Proven rules can therefore move into a shared frontend contract
//! without importing `minicon` lifecycle policy.

pub const SIDEBAR_WIDTH_DIP: f64 = 224.0;
pub const TREE_HEADER_HEIGHT_DIP: f64 = 40.0;
pub const TREE_ROW_HEIGHT_DIP: f64 = 40.0;
pub const COMPOSER_HEIGHT_DIP: f64 = 96.0;
pub const STATUS_HEIGHT_DIP: f64 = 24.0;
pub const SIDEBAR_MIN_WIDTH_DIP: f64 = 224.0;
pub const SIDEBAR_MAX_WIDTH_DIP: f64 = 480.0;
pub const TERMINAL_MIN_WIDTH_DIP: f64 = 320.0;
pub const SIDEBAR_RESIZE_GRIP_DIP: f64 = 6.0;
/// Width of the collapsed sidebar. Wide enough for one icon-sized control plus
/// its padding, and for an `@12` label at the smallest supported scale — the
/// rail's whole job is to keep every tab reachable and identifiable while
/// giving the terminal back the rest of the column.
pub const SIDEBAR_RAIL_WIDTH_DIP: f64 = 80.0;
/// Horizontal gap between the rail and a hover tooltip, and the tooltip's own
/// padding. The tooltip is drawn over the terminal, so it must not touch the
/// rail edge or it reads as part of the sidebar.
pub const TOOLTIP_GAP_DIP: f64 = 6.0;
pub const TOOLTIP_PADDING_DIP: f64 = 8.0;
pub const TOOLTIP_HEIGHT_DIP: f64 = 26.0;
pub const TERMINAL_SCROLLBAR_WIDTH_DIP: f64 = 18.0;
pub const COMPOSER_SCROLLBAR_WIDTH_DIP: f64 = 14.0;
/// Width of the right-side history/notes panel when it is open. Closing it
/// hands the terminal back the whole column, the way the sidebar rail does.
pub const HISTORY_PANEL_WIDTH_DIP: f64 = 260.0;
pub const HISTORY_PANEL_MIN_WIDTH_DIP: f64 = 180.0;
pub const HISTORY_PANEL_MAX_WIDTH_DIP: f64 = 340.0;
/// Height of one sent-history row in the panel.
pub const HISTORY_ROW_HEIGHT_DIP: f64 = 22.0;
/// Size of the panel's own close control, seated at its top-right corner.
pub const HISTORY_CLOSE_SIZE_DIP: f64 = 20.0;
/// Height reserved at the bottom of the panel for future loop/goal controls.
/// It is blank on purpose — the owner asked for the room to be kept free.
pub const HISTORY_RESERVED_HEIGHT_DIP: f64 = 96.0;
/// Size of the per-row icon buttons (edit, new-child, close) and the margin
/// from the sidebar's right edge to the outermost one. Shared by the
/// `tree_*_rect` helpers so the trio stays aligned.
const TREE_BUTTON_SIZE_DIP: f64 = 20.0;
const TREE_BUTTON_RIGHT_MARGIN_DIP: f64 = 8.0;
/// Gap between adjacent row buttons, so their hit boxes never overlap.
const TREE_BUTTON_GAP_DIP: f64 = 4.0;

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

/// One geometry owner for menu paint, input, keyboard reveal and snapshots.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MenuLayout {
    pub rect: Rect,
    pub count: usize,
    pub start: usize,
    pub visible_rows: usize,
    row_height: u32,
    pad: u32,
}

impl MenuLayout {
    fn fit(available: Rect, row_height: u32, pad: u32, count: usize, start: usize) -> Self {
        let visible_rows = ((available.height.saturating_sub(pad.saturating_mul(2))
            / row_height.max(1)) as usize)
            .min(count);
        let start = start.min(count.saturating_sub(visible_rows));
        Self {
            rect: Rect {
                height: if visible_rows == 0 {
                    0
                } else {
                    (visible_rows as u32)
                        .saturating_mul(row_height)
                        .saturating_add(pad.saturating_mul(2))
                },
                ..available
            },
            count,
            start,
            visible_rows,
            row_height,
            pad,
        }
    }

    pub fn row(self, index: usize) -> Rect {
        if index < self.start || index >= self.start.saturating_add(self.visible_rows) {
            return Rect {
                width: 0,
                height: 0,
                ..self.rect
            };
        }
        Rect {
            x: self.rect.x.saturating_add(self.pad),
            y: self
                .rect
                .y
                .saturating_add(self.pad)
                .saturating_add(((index - self.start) as u32).saturating_mul(self.row_height)),
            width: self.rect.width.saturating_sub(self.pad.saturating_mul(2)),
            height: self.row_height,
        }
    }

    pub fn reveal(self, selected: usize) -> usize {
        if selected < self.start {
            selected
        } else if selected >= self.start.saturating_add(self.visible_rows) {
            selected.saturating_add(1).saturating_sub(self.visible_rows)
        } else {
            self.start
        }
        .min(self.count.saturating_sub(self.visible_rows))
    }

    pub fn hit(self, x: u32, y: u32) -> Option<usize> {
        (self.start..self.start.saturating_add(self.visible_rows))
            .find(|&index| self.row(index).contains(x, y))
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct Layout {
    pub sidebar: Rect,
    /// Fixed @0 home destination; real tab rows start below it.
    pub home: Rect,
    pub tree_header_height: u32,
    pub tree_row_height: u32,
    pub composer: Rect,
    pub composer_input: Rect,
    pub composer_scrollbar: Rect,
    pub composer_send: Rect,
    /// Directly under [`Self::composer_send`], same column.
    ///
    /// Stacked rather than placed side by side: the pair shares one strip of
    /// width the input would otherwise have, and taking a second column from a
    /// single-line input is what makes a long command stop fitting.
    pub composer_newline: Rect,
    /// Copy / Paste / Cut convenience buttons, stacked in a column just left of
    /// the Send/New Line column so mouse-only users can edit the draft without
    /// knowing a shortcut. Zero-width (hidden, and skipped by `composer_hit`)
    /// when the composer is too narrow to spare the column for the input.
    pub composer_copy: Rect,
    pub composer_paste: Rect,
    pub composer_cut: Rect,
    /// Informational bar along the bottom of the terminal side. Carved from
    /// the terminal height, it never overlaps the sidebar or composer.
    pub status: Rect,
    /// Directly opens a new Terminal tab.
    pub new_root: Rect,
    /// Directly opens a new Browser tab.
    pub new_web: Rect,
    /// Opens the settings panel. Help, language, font size and theme all live
    /// inside that panel.
    pub settings: Rect,
    /// Reversible Harness destination, immediately left of Settings.
    pub harness: Rect,
    /// Collapses the sidebar to a rail and expands it again. Sits at the far
    /// right of the header, to the right of [`Self::settings`], because the
    /// rightmost edge is where a panel's own collapse affordance belongs — the
    /// control that takes the column away is the one nearest the column edge.
    ///
    /// While collapsed the header controls stack vertically instead, since the
    /// rail cannot seat them side by side; every one of them stays reachable,
    /// which is the point of a rail rather than a hidden sidebar.
    pub sidebar_toggle: Rect,
    /// Toggles the right-side history/interaction panel. It takes the header's
    /// true rightmost edge — the two chrome toggles then read left to right as
    /// the columns they control (the tab sidebar, then the panel), and the one
    /// that takes the right column away sits nearest that column's edge.
    pub panel_toggle: Rect,
    /// True when the sidebar is showing as a rail.
    pub sidebar_collapsed: bool,
    /// The right-side history/notes panel. Zero-width when closed, so the
    /// terminal column reclaims the space instead of leaving a gap where the
    /// panel used to be.
    pub panel: Rect,
    /// Whether the panel is open. Mirrors `panel.width != 0`; published so
    /// callers and `ui-snapshot` read one fact instead of re-deriving it.
    pub panel_open: bool,
}

impl Layout {
    /// Browser pages have navigation chrome and no terminal composer.
    pub fn browser_toolbar(self, scale: f64) -> Rect {
        let rows = if self.composer.width >= dip(640.0, scale) {
            1.0
        } else if self.composer.width >= dip(320.0, scale) {
            2.0
        } else {
            3.0
        };
        Rect {
            x: self.composer.x,
            y: 0,
            width: self.composer.width,
            height: dip(44.0 * rows, scale).min(self.status.y + self.status.height),
        }
    }

    pub fn browser_page(self, scale: f64) -> Option<Rect> {
        let column = self.content_column()?;
        let toolbar = self.browser_toolbar(scale);
        let bottom = self.status.y.saturating_add(self.status.height);
        let height = bottom.saturating_sub(toolbar.height);
        (toolbar.width > 0 && height > 0).then_some(Rect {
            x: column.x,
            y: toolbar.height,
            width: toolbar.width,
            height,
        })
    }

    pub fn browser_address(self, scale: f64) -> Rect {
        let toolbar = self.browser_toolbar(scale);
        let very_narrow = toolbar.width < dip(320.0, scale);
        let inset = dip(if very_narrow { 8.0 } else { 112.0 }, scale).min(toolbar.width);
        let x = toolbar.x + inset;
        let y = dip(if very_narrow { 50.0 } else { 6.0 }, scale).min(toolbar.height);
        let right = if toolbar.width >= dip(640.0, scale) {
            self.browser_proxy(scale).x.saturating_sub(dip(8.0, scale))
        } else {
            toolbar
                .x
                .saturating_add(toolbar.width)
                .saturating_sub(dip(8.0, scale))
        };
        Rect {
            x,
            y,
            width: right.saturating_sub(x),
            height: dip(32.0, scale).min(toolbar.height.saturating_sub(y)),
        }
    }

    /// An independent, optional proxy editor. Narrow content columns keep the
    /// full editable width by moving it onto a second or third toolbar row.
    pub fn browser_proxy(self, scale: f64) -> Rect {
        let toolbar = self.browser_toolbar(scale);
        let pad = dip(8.0, scale).min(toolbar.width / 2);
        let wide = toolbar.width >= dip(640.0, scale);
        let row = if wide {
            0.0
        } else if toolbar.width >= dip(320.0, scale) {
            44.0
        } else {
            88.0
        };
        let width = if wide {
            dip(240.0, scale)
        } else {
            toolbar.width.saturating_sub(pad.saturating_mul(2))
        };
        let x = if wide {
            toolbar
                .x
                .saturating_add(toolbar.width)
                .saturating_sub(pad)
                .saturating_sub(width)
        } else {
            toolbar.x.saturating_add(pad)
        };
        let y = dip(row + 6.0, scale).min(toolbar.height);
        Rect {
            x,
            y,
            width,
            height: dip(32.0, scale).min(toolbar.height.saturating_sub(y)),
        }
    }

    /// Creation stays in the sidebar when expanded; a narrow viewport shows
    /// whole 40-DIP actions with bounded scrolling rather than shrinking them.
    pub fn new_tab_choices(self, scale: f64, start: usize) -> MenuLayout {
        let frame_width = self
            .sidebar
            .width
            .saturating_add(self.composer.width)
            .saturating_add(self.panel.width);
        let pad = dip(8.0, scale).min(frame_width / 2);
        let x = pad;
        let y = self
            .new_root
            .y
            .saturating_add(self.new_root.height)
            .saturating_add(dip(4.0, scale))
            .min(self.sidebar.height);
        let desired = if self.sidebar_collapsed {
            dip(216.0, scale)
        } else {
            self.sidebar.width.saturating_sub(pad.saturating_mul(2))
        };
        MenuLayout::fit(
            Rect {
                x,
                y,
                width: desired.min(frame_width.saturating_sub(x).saturating_sub(pad)),
                height: self.sidebar.height.saturating_sub(y),
            },
            dip(40.0, scale),
            dip(8.0, scale),
            2,
            start,
        )
    }

    pub fn browser_recovery_card(self, scale: f64) -> Rect {
        let Some(page) = self.browser_page(scale) else {
            return Rect::default();
        };
        let pad = dip(16.0, scale).min(page.width / 4).min(page.height / 4);
        let width = dip(480.0, scale).min(page.width.saturating_sub(pad.saturating_mul(2)));
        let height = dip(212.0, scale).min(page.height.saturating_sub(pad.saturating_mul(2)));
        Rect {
            x: page.x.saturating_add(page.width.saturating_sub(width) / 2),
            y: page
                .y
                .saturating_add(dip(40.0, scale).min(page.height.saturating_sub(height) / 2)),
            width,
            height,
        }
    }

    pub fn browser_retry(self, scale: f64) -> Rect {
        self.browser_recovery_action(scale, false)
    }

    pub fn browser_edit_address(self, scale: f64) -> Rect {
        self.browser_recovery_action(scale, true)
    }

    fn browser_recovery_action(self, scale: f64, second: bool) -> Rect {
        let card = self.browser_recovery_card(scale);
        let pad = dip(16.0, scale).min(card.width / 4).min(card.height / 8);
        let gap = dip(8.0, scale).min(card.width / 8).min(card.height / 8);
        let height = dip(32.0, scale).min(card.height / 4);
        let stacked = card.width < dip(300.0, scale);
        let width = if stacked {
            card.width.saturating_sub(pad.saturating_mul(2))
        } else {
            card.width
                .saturating_sub(pad.saturating_mul(2))
                .saturating_sub(gap)
                / 2
        };
        Rect {
            x: card
                .x
                .saturating_add(pad)
                .saturating_add(if second && !stacked {
                    width.saturating_add(gap)
                } else {
                    0
                }),
            y: card
                .y
                .saturating_add(card.height)
                .saturating_sub(pad)
                .saturating_sub(height)
                .saturating_sub(if stacked && !second {
                    height.saturating_add(gap)
                } else {
                    0
                }),
            width,
            height,
        }
    }

    /// Terminal column: under the tab header, above the composer, inset by
    /// the sidebar and the history chrome. The browser page uses the same
    /// rectangle so it does not cover the tab strip.
    #[must_use]
    pub fn content_column(self) -> Option<Rect> {
        let height = self.composer.y.saturating_sub(self.tree_header_height);
        if self.composer.width == 0 || height == 0 {
            return None;
        }
        Some(Rect {
            x: self.composer.x,
            y: self.tree_header_height,
            width: self.composer.width,
            height,
        })
    }

    #[cfg(test)]
    pub fn new(width: u32, height: u32, scale: f64) -> Self {
        Self::with_sidebar(width, height, scale, SIDEBAR_WIDTH_DIP, false)
    }

    #[cfg(test)]
    pub fn collapsed(width: u32, height: u32, scale: f64) -> Self {
        Self::with_sidebar(width, height, scale, SIDEBAR_WIDTH_DIP, true)
    }

    /// The historical constructor: the panel is closed. Now test-only — the
    /// product builds its layout through [`Self::with_chrome`].
    #[cfg(test)]
    pub fn with_sidebar(
        width: u32,
        height: u32,
        scale: f64,
        sidebar_dip: f64,
        collapsed: bool,
    ) -> Self {
        Self::with_chrome(
            width,
            height,
            scale,
            sidebar_dip,
            collapsed,
            false,
            HISTORY_PANEL_WIDTH_DIP,
        )
    }

    /// Full constructor including the right-side history/notes panel.
    pub fn with_chrome(
        width: u32,
        height: u32,
        scale: f64,
        sidebar_dip: f64,
        collapsed: bool,
        panel_open: bool,
        panel_dip: f64,
    ) -> Self {
        let scale = scale.max(1.0);
        let maximum = clamp_f64(
            f64::from(width) / scale - TERMINAL_MIN_WIDTH_DIP,
            SIDEBAR_MIN_WIDTH_DIP,
            SIDEBAR_MAX_WIDTH_DIP,
        );
        // Collapsing bypasses the expanded clamp entirely: the rail is allowed
        // to be narrower than SIDEBAR_MIN_WIDTH_DIP, because that minimum
        // exists to keep tree labels readable and a rail has no labels.
        let sidebar_width = if collapsed {
            dip(SIDEBAR_RAIL_WIDTH_DIP, scale).min(width)
        } else {
            dip(
                clamp_f64(sidebar_dip, SIDEBAR_MIN_WIDTH_DIP, maximum),
                scale,
            )
            .min(width)
        };
        // The right-side panel. Closed is zero width, and the terminal column
        // then ends at the window edge exactly as before.
        let panel_width = if panel_open {
            dip(
                clamp_f64(
                    panel_dip,
                    HISTORY_PANEL_MIN_WIDTH_DIP,
                    HISTORY_PANEL_MAX_WIDTH_DIP,
                ),
                scale,
            )
            .min(width.saturating_sub(sidebar_width))
        } else {
            0
        };
        // Right edge of the terminal column: the window's, less the panel. The
        // composer and status bars stop here, so neither slides under the panel.
        let column_right = width.saturating_sub(panel_width);
        let status_height = dip(STATUS_HEIGHT_DIP, scale).min(height);
        let composer_height =
            dip(COMPOSER_HEIGHT_DIP, scale).min(height.saturating_sub(status_height));
        let composer = Rect {
            x: sidebar_width,
            y: height
                .saturating_sub(status_height)
                .saturating_sub(composer_height),
            width: column_right.saturating_sub(sidebar_width),
            height: composer_height,
        };
        let status = Rect {
            x: sidebar_width,
            y: height.saturating_sub(status_height),
            width: column_right.saturating_sub(sidebar_width),
            height: status_height,
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
        let button_x = column_right
            .saturating_sub(padding)
            .saturating_sub(send_width);
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
        // Copy / Paste / Cut column, stacked just left of the Send column. It is
        // only reserved when the input keeps a usable width afterwards, so a
        // narrow window keeps a typable input instead of three buttons.
        let clip_gap = dip(8.0, scale);
        let clip_width = dip(60.0, scale);
        let min_input = dip(160.0, scale);
        let clip_col_x = button_x.saturating_sub(clip_gap).saturating_sub(clip_width);
        let show_clip = clip_col_x.saturating_sub(clip_gap).saturating_sub(input_x) >= min_input;
        let clip_button_h = control_height.saturating_sub(button_gap.saturating_mul(2)) / 3;
        let clip_slot_stride = clip_button_h.saturating_add(button_gap);
        let clip_rect = |slot: u32, height: u32| {
            if show_clip {
                Rect {
                    x: clip_col_x,
                    y: control_y.saturating_add(clip_slot_stride.saturating_mul(slot)),
                    width: clip_width,
                    height,
                }
            } else {
                Rect {
                    x: clip_col_x,
                    y: control_y,
                    width: 0,
                    height: 0,
                }
            }
        };
        let copy = clip_rect(0, clip_button_h);
        let paste = clip_rect(1, clip_button_h);
        let cut = clip_rect(
            2,
            control_height.saturating_sub(clip_slot_stride.saturating_mul(2)),
        );
        let input_right = if show_clip {
            clip_col_x.saturating_sub(clip_gap)
        } else {
            send.x.saturating_sub(dip(8.0, scale))
        };
        let input = Rect {
            x: input_x,
            y: control_y,
            width: input_right.saturating_sub(input_x),
            height: control_height,
        };
        let scrollbar_width =
            dip(COMPOSER_SCROLLBAR_WIDTH_DIP, scale).min(input.width.saturating_sub(2));
        let composer_scrollbar = Rect {
            x: input
                .x
                .saturating_add(input.width.saturating_sub(scrollbar_width + 1)),
            y: input.y.saturating_add(1),
            width: scrollbar_width,
            height: input.height.saturating_sub(2),
        };
        let pad = dip(4.0, scale);
        let tool_y = pad;
        let tool_size = dip(32.0, scale);
        // Keep secondary tools at their existing 24-DIP size. The expanded
        // creation label and the 32-DIP robot/settings pair retain their space.
        let secondary_size = dip(24.0, scale);
        let label_width = dip(80.0, scale);
        let expanded_width = tool_size
            .checked_mul(2)
            .and_then(|width| width.checked_add(secondary_size.checked_mul(2)?))
            .and_then(|width| width.checked_add(pad.checked_mul(6)?))
            .and_then(|width| width.checked_add(label_width));
        let wrapped_tools = collapsed || expanded_width.is_none_or(|width| sidebar_width < width);
        // An impossible viewport has no partial hit target. Normal layouts keep
        // complete control sizes; degenerate coordinates never wrap or overflow.
        let button = |x: u32, y: u32, button_width: u32, button_height: u32| {
            if button_width == 0
                || button_height == 0
                || x > sidebar_width
                || y > height
                || button_width > sidebar_width.saturating_sub(x)
                || button_height > height.saturating_sub(y)
            {
                Rect {
                    x: x.min(sidebar_width),
                    y: y.min(height),
                    ..Rect::default()
                }
            } else {
                Rect {
                    x,
                    y,
                    width: button_width,
                    height: button_height,
                }
            }
        };
        let (new_area, mut harness, mut settings, sidebar_toggle, panel_toggle) = if wrapped_tools {
            let new_width = if collapsed {
                dip(32.0, scale)
            } else {
                label_width
            };
            let new_height = if collapsed { secondary_size } else { tool_size };
            let gap = if collapsed { dip(2.0, scale) } else { pad };
            let first_width = secondary_size
                .checked_mul(2)
                .and_then(|width| width.checked_add(gap.checked_mul(2)?))
                .and_then(|width| width.checked_add(new_width));
            let (new_root, sidebar_toggle, panel_toggle, pair_y) =
                if let Some(first_width) = first_width.filter(|width| *width <= sidebar_width) {
                    let x = sidebar_width.saturating_sub(first_width) / 2;
                    let sidebar_x = x.saturating_add(new_width).saturating_add(gap);
                    let panel_x = sidebar_x.saturating_add(secondary_size).saturating_add(gap);
                    (
                        button(x, tool_y, new_width, new_height),
                        button(sidebar_x, tool_y, secondary_size, secondary_size),
                        button(panel_x, tool_y, secondary_size, secondary_size),
                        tool_y
                            .saturating_add(new_height.max(secondary_size))
                            .saturating_add(pad),
                    )
                } else {
                    // Narrow expanded frames wrap whole groups, never the pair.
                    let x = sidebar_width
                        .saturating_sub(secondary_size.saturating_mul(2).saturating_add(pad))
                        / 2;
                    let new_y = tool_y.saturating_add(secondary_size).saturating_add(pad);
                    (
                        button(
                            sidebar_width.saturating_sub(new_width) / 2,
                            new_y,
                            new_width,
                            new_height,
                        ),
                        button(x, tool_y, secondary_size, secondary_size),
                        button(
                            x.saturating_add(secondary_size).saturating_add(pad),
                            tool_y,
                            secondary_size,
                            secondary_size,
                        ),
                        new_y.saturating_add(new_height).saturating_add(pad),
                    )
                };
            let pair_width = tool_size
                .checked_mul(2)
                .and_then(|width| width.checked_add(pad));
            let (harness, settings) =
                if let Some(pair_width) = pair_width.filter(|width| *width <= sidebar_width) {
                    let x = sidebar_width.saturating_sub(pair_width) / 2;
                    (
                        button(x, pair_y, tool_size, tool_size),
                        button(
                            x.saturating_add(tool_size).saturating_add(pad),
                            pair_y,
                            tool_size,
                            tool_size,
                        ),
                    )
                } else {
                    (Rect::default(), Rect::default())
                };
            (new_root, harness, settings, sidebar_toggle, panel_toggle)
        } else {
            let panel_x = sidebar_width
                .saturating_sub(pad)
                .saturating_sub(secondary_size);
            let sidebar_x = panel_x.saturating_sub(pad).saturating_sub(secondary_size);
            let settings_x = sidebar_x.saturating_sub(pad).saturating_sub(tool_size);
            let harness_x = settings_x.saturating_sub(pad).saturating_sub(tool_size);
            (
                button(
                    pad,
                    tool_y,
                    dip(100.0, scale).min(harness_x.saturating_sub(pad.saturating_mul(2))),
                    tool_size,
                ),
                button(harness_x, tool_y, tool_size, tool_size),
                button(settings_x, tool_y, tool_size, tool_size),
                button(sidebar_x, tool_y, secondary_size, secondary_size),
                button(panel_x, tool_y, secondary_size, secondary_size),
            )
        };
        let (new_root, new_web) = if collapsed {
            let gap = dip(2.0, scale);
            let web = button(
                new_area.x,
                new_area
                    .y
                    .saturating_add(new_area.height)
                    .saturating_add(gap),
                new_area.width,
                new_area.height,
            );
            let shift = new_area.height.saturating_add(gap);
            harness.y = harness.y.saturating_add(shift);
            settings.y = settings.y.saturating_add(shift);
            (new_area, web)
        } else {
            let gap = dip(4.0, scale);
            let first = new_area.width.saturating_sub(gap) / 2;
            (
                Rect {
                    width: first,
                    ..new_area
                },
                Rect {
                    x: new_area.x.saturating_add(first).saturating_add(gap),
                    width: new_area.width.saturating_sub(first.saturating_add(gap)),
                    ..new_area
                },
            )
        };
        let header_bottom = [
            new_root,
            new_web,
            harness,
            settings,
            sidebar_toggle,
            panel_toggle,
        ]
        .into_iter()
        .map(|rect| rect.y.saturating_add(rect.height))
        .max()
        .unwrap_or(0);
        let home_top = if wrapped_tools {
            header_bottom.saturating_add(pad).min(height)
        } else {
            dip(TREE_HEADER_HEIGHT_DIP, scale).min(height)
        };
        let home = Rect {
            x: 0,
            y: home_top,
            width: sidebar_width,
            height: dip(TREE_ROW_HEIGHT_DIP, scale).min(height.saturating_sub(home_top)),
        };
        Self {
            sidebar: Rect {
                x: 0,
                y: 0,
                width: sidebar_width,
                height,
            },
            // Compact controls occupy two full rows, leaving short Settings
            // pages room for complete proxy options and their pagination.
            home,
            tree_header_height: home.y.saturating_add(home.height),
            tree_row_height: dip(TREE_ROW_HEIGHT_DIP, scale).max(1),
            composer,
            status,
            composer_input: input,
            composer_scrollbar,
            composer_send: send,
            composer_newline: newline,
            composer_copy: copy,
            composer_paste: paste,
            composer_cut: cut,
            new_root,
            new_web,
            settings,
            harness,
            sidebar_toggle,
            panel_toggle,
            sidebar_collapsed: collapsed,
            panel: Rect {
                x: width.saturating_sub(panel_width),
                y: 0,
                width: panel_width,
                height,
            },
            panel_open: panel_width != 0,
        }
    }

    pub fn tree_capacity(self) -> usize {
        self.sidebar
            .height
            .saturating_sub(self.tree_header_height)
            .checked_div(self.tree_row_height)
            .unwrap_or(0) as usize
    }

    /// Action targets on @0, shared by paint and pointer routing. The Web
    /// action moves below Terminal when the content column is narrow.
    pub fn home_actions(self, width: u32, height: u32, scale: f64) -> [Rect; 2] {
        let content_width = width.saturating_sub(self.sidebar.width);
        let pad = dip(28.0, scale).min(content_width / 4);
        let gap = dip(12.0, scale);
        let button_width =
            dip(148.0, scale).min(content_width.saturating_sub(pad.saturating_mul(2)));
        let button_height = dip(40.0, scale);
        let y = (height / 7)
            .max(dip(24.0, scale))
            .saturating_add(dip(116.0, scale));
        let first = Rect {
            x: self.sidebar.width.saturating_add(pad),
            y,
            width: button_width,
            height: button_height.min(height.saturating_sub(y)),
        };
        let two_buttons = pad
            .saturating_mul(2)
            .saturating_add(button_width.saturating_mul(2))
            .saturating_add(gap);
        let second = if content_width >= two_buttons {
            Rect {
                x: first.x.saturating_add(button_width).saturating_add(gap),
                ..first
            }
        } else {
            let y = first.y.saturating_add(first.height).saturating_add(gap);
            Rect {
                x: first.x,
                y,
                width: button_width,
                height: button_height.min(height.saturating_sub(y)),
            }
        };
        [first, second]
    }

    /// The @0 fleet viewport is shared by painting, wheel routing and row
    /// hit-testing. Its rows are presentation indexes resolved to stable IDs
    /// at the instant of an action, never cached tab identities.
    pub fn home_fleet(self, width: u32, height: u32, scale: f64) -> Rect {
        let content_width = width.saturating_sub(self.sidebar.width);
        let pad = dip(28.0, scale).min(content_width / 4);
        let actions = self.home_actions(width, height, scale);
        let top = actions
            .iter()
            .map(|rect| rect.y.saturating_add(rect.height))
            .max()
            .unwrap_or(0)
            .saturating_add(pad);
        Rect {
            x: self.sidebar.width.saturating_add(pad),
            y: top,
            width: content_width.saturating_sub(pad.saturating_mul(2)),
            height: height.saturating_sub(top.saturating_add(pad)),
        }
    }

    pub fn home_fleet_capacity(self, width: u32, height: u32, scale: f64) -> usize {
        let fleet = self.home_fleet(width, height, scale);
        let inspector = self.home_inspector(width, height, scale);
        let reserved = if inspector.x > fleet.x {
            0
        } else {
            inspector.height.saturating_add(dip(8.0, scale))
        };
        fleet
            .height
            .saturating_sub(dip(48.0, scale))
            .saturating_sub(reserved)
            .checked_div(dip(48.0, scale).max(1))
            .unwrap_or(0) as usize
    }

    pub fn home_inspector(self, width: u32, height: u32, scale: f64) -> Rect {
        let fleet = self.home_fleet(width, height, scale);
        if fleet.width >= dip(520.0, scale) {
            let inspector_width = dip(210.0, scale).min(fleet.width / 3);
            let top = fleet.y.saturating_add(dip(48.0, scale));
            return Rect {
                x: fleet
                    .x
                    .saturating_add(fleet.width.saturating_sub(inspector_width)),
                y: top,
                width: inspector_width,
                height: fleet.y.saturating_add(fleet.height).saturating_sub(top),
            };
        }
        let available = fleet.height.saturating_sub(dip(56.0, scale));
        let inspector_height = dip(100.0, scale).min(available);
        Rect {
            x: fleet.x,
            y: fleet
                .y
                .saturating_add(fleet.height.saturating_sub(inspector_height)),
            width: fleet.width,
            height: inspector_height,
        }
    }

    pub fn home_focus_action(self, width: u32, height: u32, scale: f64) -> Rect {
        let inspector = self.home_inspector(width, height, scale);
        let pad = dip(12.0, scale);
        let button_height = dip(32.0, scale).min(inspector.height.saturating_sub(pad));
        Rect {
            x: inspector.x.saturating_add(pad),
            y: inspector.y.saturating_add(
                inspector
                    .height
                    .saturating_sub(button_height)
                    .saturating_sub(pad),
            ),
            width: dip(154.0, scale).min(inspector.width.saturating_sub(pad.saturating_mul(2))),
            height: button_height,
        }
    }

    pub fn home_fleet_row(self, width: u32, height: u32, scale: f64, visible: usize) -> Rect {
        let fleet = self.home_fleet(width, height, scale);
        let inspector = self.home_inspector(width, height, scale);
        let row_height = dip(48.0, scale);
        let top = fleet
            .y
            .saturating_add(row_height)
            .saturating_add(row_height.saturating_mul(visible as u32));
        Rect {
            x: fleet.x,
            y: top,
            width: if inspector.x > fleet.x {
                inspector
                    .x
                    .saturating_sub(fleet.x)
                    .saturating_sub(dip(8.0, scale))
            } else {
                fleet.width
            },
            height: row_height.min(fleet.y.saturating_add(fleet.height).saturating_sub(top)),
        }
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

    /// The greeting page's second button, stacked under `empty_new_terminal`.
    ///
    /// With no tabs open the window has no other way out: `CloseRequested` is
    /// swallowed by the empty-workspace path, so without this the only exit is
    /// killing the process. Same width as the New Terminal button so the two
    /// read as one column.
    pub fn empty_quit(self, width: u32, height: u32, scale: f64) -> Rect {
        let above = self.empty_new_terminal(width, height, scale);
        Rect {
            x: above.x,
            y: above
                .y
                .saturating_add(above.height)
                .saturating_add(dip(44.0, scale)),
            width: above.width,
            height: above.height,
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
        let size = dip(TREE_BUTTON_SIZE_DIP, scale).min(self.tree_row_height);
        Rect {
            x: self
                .sidebar
                .width
                .saturating_sub(dip(TREE_BUTTON_RIGHT_MARGIN_DIP, scale))
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

    /// The per-row new-child button, one gap to the left of the close button
    /// (so the row reads edit | new-child | close). Same action as
    /// Ctrl+Shift+N for that row's tab.
    ///
    /// Shares the close button's size and vertical centring; only `x` differs.
    pub fn tree_child_rect(self, visible_row: usize, scale: f64) -> Rect {
        let close = self.tree_close_rect(visible_row, scale);
        let gap = dip(TREE_BUTTON_GAP_DIP, scale);
        Rect {
            x: close.x.saturating_sub(gap).saturating_sub(close.width),
            y: close.y,
            width: close.width,
            height: close.height,
        }
    }

    /// The per-row edit button, one gap to the left of the new-child button.
    ///
    /// Shares the close button's size and vertical centring so the trio reads
    /// as one aligned group; only `x` differs.
    pub fn tree_edit_rect(self, visible_row: usize, scale: f64) -> Rect {
        let child = self.tree_child_rect(visible_row, scale);
        let gap = dip(TREE_BUTTON_GAP_DIP, scale);
        Rect {
            x: child.x.saturating_sub(gap).saturating_sub(child.width),
            y: child.y,
            width: child.width,
            height: child.height,
        }
    }

    /// The painted inline rename plate and its text origin. Pointer editing,
    /// painting and the IME anchor must all use this same geometry.
    pub fn tree_rename_geometry(
        self,
        visible_row: usize,
        depth: u32,
        cell_height: u32,
    ) -> TreeRenameGeometry {
        let text_x = 14u32.saturating_add(depth.min(8) * 18);
        let text_width = self.sidebar.width.saturating_sub(8).saturating_sub(text_x);
        let row_y = self.tree_header_height.saturating_add(
            u32::try_from(visible_row)
                .unwrap_or(u32::MAX)
                .saturating_mul(self.tree_row_height),
        );
        let plate_height = cell_height.saturating_add(6);
        let plate_y = row_y.saturating_add(self.tree_row_height.saturating_sub(plate_height) / 2);
        TreeRenameGeometry {
            plate: Rect {
                x: text_x.saturating_sub(3),
                y: plate_y,
                width: text_width.saturating_add(6),
                height: plate_height,
            },
            text_x,
            text_y: plate_y.saturating_add(3),
            text_width,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct TreeRenameGeometry {
    pub plate: Rect,
    pub text_x: u32,
    pub text_y: u32,
    pub text_width: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum TreeHit {
    Outside,
    Background,
    Home,
    NewRoot,
    NewWeb,
    Harness,
    Settings,
    SidebarToggle,
    PanelToggle,
    Select(usize),
    Close(usize),
    Edit(usize),
    /// The per-row new-child button: open a child tab beneath this row's tab,
    /// the pointer twin of Ctrl+Shift+N.
    Child(usize),
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ComposerHit {
    Outside,
    Input,
    Scrollbar,
    Send,
    Newline,
    Copy,
    Paste,
    Cut,
}

/// Where a pointer landed on the bottom status bar. The bar is informational
/// today; the variant exists so clicks on it are consumed rather than falling
/// through, and so the settings-era controls have a home to grow into.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum StatusHit {
    Outside,
    Bar,
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
    ChineseSimplified,
    ChineseTraditional,
}

impl UiLanguage {
    /// The stable identifier used by the control surface, so automation can
    /// read and assert the language without matching a display label.
    #[must_use]
    pub const fn tag(self) -> &'static str {
        match self {
            Self::English => "en",
            Self::ChineseSimplified => "zh-hans",
            Self::ChineseTraditional => "zh-hant",
        }
    }

    /// The language a stored [`Self::tag`] names, or `None` for an id this
    /// build does not know.
    ///
    /// The inverse of `tag`, and what lets the chosen language be a stored id
    /// rather than a display label: an id from a hand-edited file or a newer
    /// build resolves to nothing, so the caller keeps its default and the
    /// product still starts. A `match` rather than a table so adding a language
    /// without teaching it here is a compile error, not a silent regression.
    #[must_use]
    pub fn from_tag(tag: &str) -> Option<Self> {
        match tag {
            "en" => Some(Self::English),
            "zh-hans" => Some(Self::ChineseSimplified),
            "zh-hant" => Some(Self::ChineseTraditional),
            _ => None,
        }
    }

    /// The label painted on this language's own entry. Each is written in the
    /// language it selects: a switch labelled in the language you are leaving
    /// is unreadable to the person who needs it.
    #[must_use]
    pub const fn entry_label(self) -> &'static str {
        match self {
            Self::English => "En",
            Self::ChineseSimplified => "中",
            Self::ChineseTraditional => "繁",
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
                copy: "Copy",
                paste: "Paste",
                cut: "Cut",
                send_to: "SEND TO @",
                empty_title: "READY FOR A NEW TERMINAL",
                new_terminal: "NEW TERMINAL",
                new_terminal_hint: "Ctrl+Shift+T",
                quit: "QUIT MINICON",
                settings_title: "SETTINGS",
                settings_language: "Interface language",
                settings_font: "Font size",
                settings_theme: "Theme",
                settings_shortcuts: "Shortcuts",
                history_title: "HISTORY",
                history_empty: "(nothing sent yet)",
                notes_title: "NOTES",
                notes_empty: "(click to jot a note)",
                settings_updates: "Updates",
                update_check: "Check",
                update_install: "Update",
                update_restart: "Restart",
                update_checking: "checking…",
                update_installing: "downloading and verifying…",
                update_latest: "on the latest release",
                update_available: "available",
                update_ahead: "you are ahead of the latest release",
                update_ready: "ready — restart to finish",
                update_staged_open: "download page opened — install from there",
            },
            Self::ChineseSimplified => HostUiStrings {
                paste_failed: "粘贴失败",
                send: "送出",
                send_hint: "(ctrl-o)",
                newline: "换行",
                newline_hint: "(Enter)",
                copy: "复制",
                paste: "粘贴",
                cut: "剪切",
                send_to: "送往 @",
                empty_title: "准备开启新终端",
                new_terminal: "新建终端",
                new_terminal_hint: "Ctrl+Shift+T",
                quit: "退出 MiniCon",
                settings_title: "设置",
                settings_language: "界面语言",
                settings_font: "字号",
                settings_theme: "主题",
                settings_shortcuts: "快捷键",
                history_title: "历史发送",
                history_empty: "（还没有发送过内容）",
                notes_title: "笔记",
                notes_empty: "（点击记录一条笔记）",
                settings_updates: "更新",
                update_check: "检查",
                update_install: "更新",
                update_restart: "重启",
                update_checking: "检查中…",
                update_installing: "下载并校验中…",
                update_latest: "已是最新版本",
                update_available: "可更新",
                update_ahead: "比最新发布版本更新",
                update_ready: "已就绪——重启以完成",
                update_staged_open: "已打开下载页——请在那里安装",
            },
            Self::ChineseTraditional => HostUiStrings {
                paste_failed: "貼上失敗",
                send: "送出",
                send_hint: "(ctrl-o)",
                newline: "換行",
                newline_hint: "(Enter)",
                copy: "複製",
                paste: "貼上",
                cut: "剪下",
                send_to: "送往 @",
                empty_title: "準備開啟新終端",
                new_terminal: "新建終端",
                new_terminal_hint: "Ctrl+Shift+T",
                quit: "結束 MiniCon",
                settings_title: "設定",
                settings_language: "介面語言",
                settings_font: "字級",
                settings_theme: "佈景主題",
                settings_shortcuts: "快捷鍵",
                history_title: "歷史發送",
                history_empty: "（還沒有送出過內容）",
                notes_title: "筆記",
                notes_empty: "（點擊記錄一條筆記）",
                settings_updates: "更新",
                update_check: "檢查",
                update_install: "更新",
                update_restart: "重啟",
                update_checking: "檢查中…",
                update_installing: "下載並驗證中…",
                update_latest: "已是最新版本",
                update_available: "可更新",
                update_ahead: "比最新發布版本更新",
                update_ready: "已就緒——重啟以完成",
                update_staged_open: "已開啟下載頁——請在那裡安裝",
            },
        }
    }

    #[must_use]
    pub const fn help_lines(self) -> [&'static str; SETTINGS_SHORTCUT_LINES as usize] {
        // Workspace chords stay Ctrl+Shift. macOS also reserves Command arrows
        // for its owned window; its extra row shares the layout's line count.
        // The clipboard row uses the native modifier on each platform.
        match self {
            Self::English => [
                "KEYBOARD",
                "Ctrl+Shift+T   New root",
                "Ctrl+Shift+N   New child",
                "Ctrl+Shift+W   Close tab",
                "Ctrl+Shift+[/] Switch tab",
                "Ctrl+Shift+I   Focus input",
                "Ctrl+Shift+P   Cycle theme",
                "Ctrl+Shift+,   Settings",
                "Ctrl+Shift+G   Crosshair",
                "Ctrl+Shift+B   Collapse tabs",
                "Ctrl+Shift+H   History panel",
                #[cfg(target_os = "macos")]
                "Cmd+Left/Right Window halves",
                #[cfg(target_os = "macos")]
                "Cmd+C/V/X      Copy/Paste/Cut",
                #[cfg(not(target_os = "macos"))]
                "Ctrl+C/V/X     Copy/Paste/Cut",
                "Ctrl+O         Send",
                "Shift+Enter    Send",
                "Enter          Newline",
                "Up / Down      History",
            ],
            Self::ChineseSimplified => [
                "快捷键",
                "Ctrl+Shift+T   新建根",
                "Ctrl+Shift+N   新建子",
                "Ctrl+Shift+W   关闭标签",
                "Ctrl+Shift+[/] 切换标签",
                "Ctrl+Shift+I   聚焦输入",
                "Ctrl+Shift+P   切换主题",
                "Ctrl+Shift+,   设置",
                "Ctrl+Shift+G   十字线",
                "Ctrl+Shift+B   收起标签",
                "Ctrl+Shift+H   历史面板",
                #[cfg(target_os = "macos")]
                "Cmd+Left/Right 左右半屏",
                #[cfg(target_os = "macos")]
                "Cmd+C/V/X      复制/粘贴/剪切",
                #[cfg(not(target_os = "macos"))]
                "Ctrl+C/V/X     复制/粘贴/剪切",
                "Ctrl+O         发送",
                "Shift+Enter    发送",
                "Enter          软换行",
                "Up / Down      历史",
            ],
            Self::ChineseTraditional => [
                "快捷鍵",
                "Ctrl+Shift+T   新建根",
                "Ctrl+Shift+N   新建子",
                "Ctrl+Shift+W   關閉分頁",
                "Ctrl+Shift+[/] 切換分頁",
                "Ctrl+Shift+I   聚焦輸入",
                "Ctrl+Shift+P   切換主題",
                "Ctrl+Shift+,   設定",
                "Ctrl+Shift+G   十字線",
                "Ctrl+Shift+B   收起分頁",
                "Ctrl+Shift+H   歷史面板",
                #[cfg(target_os = "macos")]
                "Cmd+Left/Right 左右半螢幕",
                #[cfg(target_os = "macos")]
                "Cmd+C/V/X      複製/貼上/剪下",
                #[cfg(not(target_os = "macos"))]
                "Ctrl+C/V/X     複製/貼上/剪下",
                "Ctrl+O         送出",
                "Shift+Enter    送出",
                "Enter          軟換行",
                "Up / Down      歷史",
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
    pub copy: &'static str,
    pub paste: &'static str,
    pub cut: &'static str,
    pub send_to: &'static str,
    pub empty_title: &'static str,
    pub new_terminal: &'static str,
    pub new_terminal_hint: &'static str,
    pub quit: &'static str,
    pub settings_title: &'static str,
    pub settings_language: &'static str,
    pub settings_font: &'static str,
    pub settings_theme: &'static str,
    pub settings_shortcuts: &'static str,
    pub history_title: &'static str,
    pub history_empty: &'static str,
    pub notes_title: &'static str,
    pub notes_empty: &'static str,
    /// The Updates section: its label, its three controls and the phase line.
    ///
    /// The detailed cause a failure carries is diagnostic text from the worker
    /// and stays in one language; these are the labels the user acts on.
    pub settings_updates: &'static str,
    pub update_check: &'static str,
    pub update_install: &'static str,
    pub update_restart: &'static str,
    pub update_checking: &'static str,
    pub update_installing: &'static str,
    pub update_latest: &'static str,
    pub update_available: &'static str,
    pub update_ahead: &'static str,
    pub update_ready: &'static str,
    pub update_staged_open: &'static str,
}

/// The Updates section's one status line, and the failure sentence inside it.
///
/// Kept on the language rather than in the paint code so there is exactly one
/// owner of what the panel says: `host_ui` paints the result and `ui-snapshot`
/// publishes the same string, so a reader cannot catch them disagreeing.
/// Every arm is a `match` over the language *and* over the phase, so a new
/// phase or a missing translation is a compile error rather than a blank label
/// a user finds.
impl UiLanguage {
    /// What failed, in this language.
    #[must_use]
    const fn update_failure_text(self, kind: crate::update::FailureKind) -> &'static str {
        use crate::update::FailureKind as Kind;
        match self {
            Self::English => match kind {
                Kind::Platform => "no release is published for this platform",
                Kind::Release => "the release information could not be read",
                Kind::Transport => "the download could not be completed",
                Kind::Checksum => "the download did not match the release checksum",
                Kind::Archive => "the downloaded archive could not be unpacked",
                Kind::Staging => "the new version could not be prepared",
                Kind::Swap => "the new version could not be put in place",
                Kind::Launch => "the new version would not start",
                Kind::LaunchUnrestored => {
                    "the new version would not start, and the previous one could not be put back"
                }
            },
            Self::ChineseSimplified => match kind {
                Kind::Platform => "这个平台没有发布安装包",
                Kind::Release => "读取发布信息失败",
                Kind::Transport => "下载未能完成",
                Kind::Checksum => "下载内容与发布校验值不符",
                Kind::Archive => "下载的压缩包无法解包",
                Kind::Staging => "新版本未能准备好",
                Kind::Swap => "新版本未能就位",
                Kind::Launch => "新版本无法启动",
                Kind::LaunchUnrestored => "新版本无法启动，旧版本也未能放回",
            },
            Self::ChineseTraditional => match kind {
                Kind::Platform => "這個平台沒有發布安裝包",
                Kind::Release => "讀取發布資訊失敗",
                Kind::Transport => "下載未能完成",
                Kind::Checksum => "下載內容與發布校驗值不符",
                Kind::Archive => "下載的壓縮檔無法解包",
                Kind::Staging => "新版本未能準備好",
                Kind::Swap => "新版本未能就位",
                Kind::Launch => "新版本無法啟動",
                Kind::LaunchUnrestored => "新版本無法啟動，舊版本也未能放回",
            },
        }
    }

    /// Whether the install still runs. Never left out: a failure a user cannot
    /// judge is a failure they will retry into the same wall.
    #[must_use]
    const fn update_install_text(self, kind: crate::update::FailureKind) -> &'static str {
        use crate::update::InstallState as State;
        match (self, kind.install_state()) {
            (Self::English, State::Untouched) => "your current install was not changed",
            (Self::English, State::Restored) => {
                "your previous version was put back and is kept as {}"
            }
            (Self::English, State::Unknown) => "your install may need to be replaced",
            (Self::ChineseSimplified, State::Untouched) => "当前安装未受影响",
            (Self::ChineseSimplified, State::Restored) => "已放回更新前的版本，保存在 {}",
            (Self::ChineseSimplified, State::Unknown) => "当前安装可能需要重新安装",
            (Self::ChineseTraditional, State::Untouched) => "目前安裝未受影響",
            (Self::ChineseTraditional, State::Restored) => "已放回更新前的版本，保存在 {}",
            (Self::ChineseTraditional, State::Unknown) => "目前安裝可能需要重新安裝",
        }
    }

    /// The next thing the user can do, which is the third fact every failure
    /// sentence owes them.
    #[must_use]
    const fn update_next_text(self, kind: crate::update::FailureKind) -> &'static str {
        use crate::update::FailureKind as Kind;
        if let Kind::LaunchUnrestored = kind {
            return match self {
                Self::English => "reinstall from the releases page",
                Self::ChineseSimplified => "请从发布页重新安装",
                Self::ChineseTraditional => "請從發布頁重新安裝",
            };
        }
        match self {
            Self::English => "try again",
            Self::ChineseSimplified => "请重试",
            Self::ChineseTraditional => "請重試",
        }
    }

    /// The whole failure sentence: what failed, whether the install still runs,
    /// and what to do next.
    #[must_use]
    pub fn update_failure_sentence(self, failure: &crate::update::Failure) -> String {
        let install = self
            .update_install_text(failure.kind)
            .replace("{}", &crate::update::backup_file_name());
        format!(
            "{} — {}; {}.",
            self.update_failure_text(failure.kind),
            install,
            self.update_next_text(failure.kind)
        )
    }

    /// The panel's one status line for the current update phase.
    #[must_use]
    pub fn update_status(self, update: &crate::update::State) -> String {
        use crate::update::Phase;
        let strings = self.strings();
        match &update.phase {
            Phase::Idle => format!("v{}", update.current),
            Phase::Checking => strings.update_checking.to_owned(),
            Phase::Installing { remote } => format!("v{remote} — {}", strings.update_installing),
            Phase::UpToDate => format!("v{} — {}", update.current, strings.update_latest),
            Phase::Available { remote } => format!("v{remote} {}", strings.update_available),
            Phase::Ahead { remote } => format!("v{remote} — {}", strings.update_ahead),
            Phase::Ready { remote, opens_dmg } => format!(
                "v{remote} — {}",
                if *opens_dmg {
                    strings.update_staged_open
                } else {
                    strings.update_ready
                }
            ),
            Phase::Failed(failure) => self.update_failure_sentence(failure),
        }
    }

    /// What pressing Restart costs, in this language.
    ///
    /// The Restart control is the first moment a user learns that taking the
    /// new version costs them every open terminal, and the host already knows
    /// the count, so the sentence names it rather than warning in the abstract.
    /// One tab and N tabs read differently because a count of one is the case a
    /// person actually sees, and the empty case is its own arm so nobody is
    /// told "0 tabs are open" — a sentence that reads as a bug.
    #[must_use]
    pub fn restart_cost(self, tabs: usize) -> String {
        match (self, tabs) {
            (Self::English, 0) => "Restarting closes this window; no tabs are open.".to_owned(),
            (Self::English, 1) => {
                "Restarting closes 1 tab and stops its running command.".to_owned()
            }
            (Self::English, count) => {
                format!("Restarting closes {count} tabs and stops their running commands.")
            }
            (Self::ChineseSimplified, 0) => "重启会关闭这个窗口；当前没有打开的标签页。".to_owned(),
            (Self::ChineseSimplified, 1) => {
                "重启会关闭 1 个标签页，并终止其中正在运行的命令。".to_owned()
            }
            (Self::ChineseSimplified, count) => {
                format!("重启会关闭 {count} 个标签页，并终止其中正在运行的命令。")
            }
            (Self::ChineseTraditional, 0) => "重啟會關閉這個視窗；目前沒有開啟的分頁。".to_owned(),
            (Self::ChineseTraditional, 1) => {
                "重啟會關閉 1 個分頁，並終止其中正在執行的命令。".to_owned()
            }
            (Self::ChineseTraditional, count) => {
                format!("重啟會關閉 {count} 個分頁，並終止其中正在執行的命令。")
            }
        }
    }
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
    if layout.new_web.contains(x, y) {
        return TreeHit::NewWeb;
    }
    if layout.harness.contains(x, y) {
        return TreeHit::Harness;
    }
    if layout.settings.contains(x, y) {
        return TreeHit::Settings;
    }
    if layout.sidebar_toggle.contains(x, y) {
        return TreeHit::SidebarToggle;
    }
    if layout.panel_toggle.contains(x, y) {
        return TreeHit::PanelToggle;
    }
    if layout.home.contains(x, y) {
        return TreeHit::Home;
    }
    if y < layout.tree_header_height {
        return TreeHit::Background;
    }
    let visible_row = ((y - layout.tree_header_height) / layout.tree_row_height) as usize;
    let index = scroll_offset.saturating_add(visible_row);
    if index >= item_count {
        return TreeHit::Background;
    }
    // A collapsed rail is one icon wide, so the close box covers most of the
    // row and selecting a tab keeps closing it by accident (user report
    // against 0.1.22). While collapsed the whole row selects; the row buttons
    // need the rail expanded, where each box is a small target beside the
    // title. The boxes do not overlap (a gap separates them), and they are
    // tested left to right (edit, new-child, close) so a click on an inner
    // button never reads as the destructive close.
    if !layout.sidebar_collapsed && layout.tree_edit_rect(visible_row, scale).contains(x, y) {
        TreeHit::Edit(index)
    } else if !layout.sidebar_collapsed && layout.tree_child_rect(visible_row, scale).contains(x, y)
    {
        TreeHit::Child(index)
    } else if !layout.sidebar_collapsed && layout.tree_close_rect(visible_row, scale).contains(x, y)
    {
        TreeHit::Close(index)
    } else {
        TreeHit::Select(index)
    }
}

/// Where the hover tooltip for a collapsed rail row is drawn.
///
/// The rail shows only `@N`, so the full label has to appear somewhere; it is
/// drawn over the terminal just right of the rail, vertically centred on its
/// row. Returns `None` when the sidebar is expanded (labels are already
/// visible) or when the terminal column is too narrow to host the tooltip
/// without covering it entirely — a tooltip wider than the area it floats over
/// is worse than none.
pub fn rail_tooltip_rect(
    layout: Layout,
    visible_row: u32,
    text_width_px: u32,
    window_width: u32,
    scale: f64,
) -> Option<Rect> {
    if !layout.sidebar_collapsed {
        return None;
    }
    let gap = dip(TOOLTIP_GAP_DIP, scale);
    let padding = dip(TOOLTIP_PADDING_DIP, scale);
    let height = dip(TOOLTIP_HEIGHT_DIP, scale);
    let x = layout.sidebar.width.saturating_add(gap);
    let width = text_width_px.saturating_add(padding.saturating_mul(2));
    let available = window_width.saturating_sub(x);
    if width == 0 || available == 0 || width > available {
        return None;
    }
    let row_top = layout
        .tree_header_height
        .saturating_add(layout.tree_row_height.saturating_mul(visible_row));
    let centred = row_top.saturating_add(layout.tree_row_height / 2);
    let y = centred.saturating_sub(height / 2);
    // Never let it hang below the terminal area into the composer band.
    let floor = layout
        .composer
        .y
        .saturating_sub(height)
        .max(layout.tree_header_height);
    Some(Rect {
        x,
        y: y.min(floor),
        width,
        height,
    })
}

pub fn composer_hit(layout: Layout, x: u32, y: u32) -> ComposerHit {
    if layout.composer_send.contains(x, y) {
        ComposerHit::Send
    } else if layout.composer_newline.contains(x, y) {
        ComposerHit::Newline
    } else if layout.composer_copy.contains(x, y) {
        ComposerHit::Copy
    } else if layout.composer_paste.contains(x, y) {
        ComposerHit::Paste
    } else if layout.composer_cut.contains(x, y) {
        ComposerHit::Cut
    } else if layout.composer_scrollbar.contains(x, y) {
        ComposerHit::Scrollbar
    } else if layout.composer.contains(x, y) {
        ComposerHit::Input
    } else {
        ComposerHit::Outside
    }
}

pub fn composer_scrollbar_geometry(
    layout: Layout,
    visible: usize,
    first_line: usize,
    total: usize,
) -> minicon_ui_core::ScrollbarGeometry {
    let area = layout.composer_scrollbar;
    let maximum = total.saturating_sub(visible);
    minicon_ui_core::terminal_scrollbar_geometry(
        minicon_ui_core::Rect {
            left: area.x.min(i32::MAX as u32) as i32,
            top: area.y.min(i32::MAX as u32) as i32,
            right: area.x.saturating_add(area.width).min(i32::MAX as u32) as i32,
            bottom: area.y.saturating_add(area.height).min(i32::MAX as u32) as i32,
        },
        area.width.min(i32::MAX as u32) as i32,
        visible,
        maximum.saturating_sub(first_line.min(maximum)),
        maximum,
    )
}

/// Hit-test the bottom status bar.
#[must_use]
pub fn status_hit(layout: Layout, x: u32, y: u32) -> StatusHit {
    if layout.status.contains(x, y) {
        StatusHit::Bar
    } else {
        StatusHit::Outside
    }
}

/// Number of settings-panel shortcut lines.
///
/// The painter draws every entry of `UiLanguage::help_lines` — including its
/// leading all-caps section row — so the reserved height is the array's own
/// length. Reserving one less let the final line spill into whatever followed.
pub const SETTINGS_SHORTCUT_LINES: u32 = if cfg!(target_os = "macos") { 17 } else { 16 };

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum SettingsSection {
    #[default]
    Console,
    Web,
    Harness,
}

impl SettingsSection {
    pub const fn index(self) -> usize {
        match self {
            Self::Console => 0,
            Self::Web => 1,
            Self::Harness => 2,
        }
    }

    pub const fn from_index(index: usize) -> Self {
        match index {
            0 => Self::Console,
            1 => Self::Web,
            _ => Self::Harness,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct WebSettingsLayout {
    pub panel: Rect,
    pub routes: [Rect; 3],
    pub proxy_input: Rect,
    pub proxy_add: Rect,
    pub proxy_rows: [Rect; 8],
    pub proxy_remove: [Rect; 8],
    pub visible_rows: usize,
    pub list_start: usize,
    pub previous: Rect,
    pub next: Rect,
    pub notice: Rect,
}

/// Fixed-height scrolling Harness rows. Product state chooses row contents;
/// this geometry is shared by paint, hit testing and public observations.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HarnessSettingsLayout {
    pub panel: Rect,
    pub routes: [Rect; 3],
    pub close: Rect,
    pub navigation_rail: bool,
    pub section_selector: Rect,
    pub navigation: [Rect; 4],
    pub rows: MenuLayout,
    pub previous: Rect,
    pub next: Rect,
}

impl HarnessSettingsLayout {
    pub fn cell(self, row: usize, column: usize, columns: usize, scale: f64) -> Rect {
        let rect = self.rows.row(row);
        let columns = columns.max(1) as u32;
        let gap = dip(6.0, scale);
        let width = rect.width.saturating_sub(gap * (columns - 1)) / columns;
        Rect {
            x: rect
                .x
                .saturating_add((column as u32).saturating_mul(width + gap)),
            width,
            height: rect.height.saturating_sub(dip(4.0, scale)),
            ..rect
        }
    }
}

/// Geometry of the settings panel and its interactive controls. Computed on
/// demand from the `Layout` + scale so paint and hit-testing agree exactly.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SettingsLayout {
    pub panel: Rect,
    pub routes: [Rect; 3],
    /// Interface-language options: English, Simplified, Traditional (in order).
    pub language: [Rect; 3],
    pub font_down: Rect,
    pub font_reset: Rect,
    pub font_up: Rect,
    /// Theme swatches: Neutral, Docs, Paper, Dark (in order).
    pub theme: [Rect; 4],
    /// Where the shortcut lines begin (the reused help text).
    pub shortcuts_y: u32,
    pub shortcut_line_height: u32,
    /// How many shortcut lines actually fit above the Updates section.
    ///
    /// A short window truncates the help list rather than pushing the Updates
    /// controls past the bottom edge, because the list is readable from
    /// `--help` and the Update / Restart controls are not.
    pub shortcut_lines: u32,
    /// The Updates section: the row its label is anchored above, its three
    /// controls, its status line, and the line that states what a Restart
    /// costs.
    pub updates_row: u32,
    pub update: [Rect; 3],
    pub updates_status_y: u32,
    pub updates_cost_y: u32,
}

/// Where a pointer landed inside the settings panel. Theme is an index (0=Neutral
/// 1=Docs 2=Paper 3=Dark) so this module stays free of the host-UI theme type.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SettingsHit {
    Route(SettingsSection),
    Outside,
    Panel,
    Language(UiLanguage),
    FontDown,
    FontReset,
    FontUp,
    Theme(usize),
    /// Check for updates.
    UpdateCheck,
    /// Download and stage the available update.
    UpdateInstall,
    /// Replace the running executable and start the staged one.
    UpdateRestart,
}

impl Layout {
    pub fn harness_settings(self, scale: f64, count: usize, start: usize) -> HarnessSettingsLayout {
        let panel = Rect {
            x: 0,
            y: self.tree_header_height,
            width: dip(760.0, scale).min(self.status.x.saturating_add(self.status.width)),
            height: self.status.y.saturating_sub(self.tree_header_height),
        };
        let pad = dip(12.0, scale);
        let close_width = dip(72.0, scale).min(panel.width / 4);
        let close = Rect {
            x: panel.x + panel.width.saturating_sub(pad + close_width),
            y: panel.y + dip(8.0, scale),
            width: close_width,
            height: dip(28.0, scale).min(panel.height.saturating_sub(dip(8.0, scale))),
        };
        let routes = self.settings_routes(
            Rect {
                width: panel.width.saturating_sub(close_width + pad),
                ..panel
            },
            scale,
        );
        let top = panel.y.saturating_add(dip(48.0, scale));
        let navigation_rail = panel.width >= dip(640.0, scale) && panel.height >= dip(232.0, scale);
        let section_selector = if navigation_rail {
            Rect::default()
        } else {
            Rect {
                x: panel.x + pad,
                y: top,
                width: panel.width.saturating_sub(pad * 2),
                height: dip(32.0, scale).min(panel.height.saturating_sub(dip(48.0, scale))),
            }
        };
        let nav_stride = dip(36.0, scale).min(
            panel
                .y
                .saturating_add(panel.height)
                .saturating_sub(top + pad)
                / 4,
        );
        let navigation = std::array::from_fn(|i| Rect {
            x: panel.x + pad,
            y: top + i as u32 * nav_stride,
            width: if navigation_rail {
                dip(176.0, scale)
            } else {
                panel.width.saturating_sub(pad * 2)
            },
            height: nav_stride.saturating_sub(dip(4.0, scale)),
        });
        let content_x = panel.x
            + pad
            + if navigation_rail {
                dip(176.0, scale) + pad
            } else {
                0
            };
        let content_top = top + if navigation_rail { 0 } else { dip(40.0, scale) };
        let content_width = panel
            .x
            .saturating_add(panel.width)
            .saturating_sub(content_x + pad);
        let footer_y = panel
            .y
            .saturating_add(panel.height)
            .saturating_sub(dip(40.0, scale));
        let rows = MenuLayout::fit(
            Rect {
                x: content_x,
                y: content_top,
                width: content_width,
                height: footer_y.saturating_sub(content_top),
            },
            dip(36.0, scale),
            0,
            count,
            start,
        );
        let width = content_width.saturating_sub(pad) / 2;
        let previous = Rect {
            x: content_x,
            y: footer_y.max(content_top),
            width,
            height: dip(28.0, scale).min(
                panel
                    .y
                    .saturating_add(panel.height)
                    .saturating_sub(footer_y.max(content_top)),
            ),
        };
        HarnessSettingsLayout {
            panel,
            routes,
            close,
            navigation_rail,
            section_selector,
            navigation,
            rows,
            previous,
            next: Rect {
                x: previous.x + width + pad,
                ..previous
            },
        }
    }

    fn settings_routes(self, panel: Rect, scale: f64) -> [Rect; 3] {
        let route_width = panel.width.min(dip(300.0, scale));
        let pad = dip(12.0, scale).min(route_width / 4);
        let width = route_width.saturating_sub(pad * 4) / 3;
        std::array::from_fn(|i| Rect {
            x: panel.x + pad + i as u32 * (width + pad),
            y: panel.y + dip(8.0, scale),
            width,
            height: dip(28.0, scale).min(panel.height.saturating_sub(dip(8.0, scale))),
        })
    }

    pub fn settings_route(self, section: SettingsSection, scale: f64) -> Rect {
        self.settings_panel(scale).routes[section.index()]
    }

    pub fn web_settings(self, scale: f64, list_start: usize) -> WebSettingsLayout {
        let total_width = self.status.x + self.status.width;
        let bottom = self.status.y;
        let panel = Rect {
            x: 0,
            y: self.home.y,
            width: dip(480.0, scale).min(total_width),
            height: bottom.saturating_sub(self.home.y),
        };
        let pad = dip(12.0, scale).min(panel.width / 4);
        let inner = panel.width.saturating_sub(pad * 2);
        let row_h = dip(32.0, scale);
        let compact = panel.height < dip(300.0, scale);
        let list_stride = dip(if compact { 32.0 } else { 40.0 }, scale);
        let row = |y: u32| Rect {
            x: pad,
            y,
            width: inner,
            height: row_h.min(bottom.saturating_sub(y)),
        };
        let notice = Rect {
            height: dip(if compact { 24.0 } else { 48.0 }, scale)
                .min(panel.height.saturating_sub(dip(48.0, scale))),
            ..row(panel.y + dip(48.0, scale))
        };
        let input_y = notice.y + notice.height + dip(8.0, scale);
        let add_width = dip(64.0, scale).min(inner / 3);
        let proxy_input = Rect {
            width: inner.saturating_sub(add_width + dip(8.0, scale)),
            ..row(input_y)
        };
        let proxy_add = Rect {
            x: pad + inner - add_width,
            width: add_width,
            ..row(input_y)
        };
        let list_y = input_y + row_h + dip(if compact { 20.0 } else { 40.0 }, scale);
        let pager_y = bottom.saturating_sub(row_h + pad).max(list_y);
        let visible_rows = ((pager_y.saturating_sub(list_y) / list_stride.max(1)) as usize).min(8);
        let list_start = list_start.min(8usize.saturating_sub(visible_rows.max(1)));
        let proxy_rows = std::array::from_fn(|i| {
            if i < visible_rows {
                row(list_y + i as u32 * list_stride)
            } else {
                Rect {
                    x: pad,
                    y: bottom,
                    width: 0,
                    height: 0,
                }
            }
        });
        let proxy_remove = proxy_rows.map(|r| Rect {
            x: r.x + r.width.saturating_sub(row_h),
            width: row_h.min(r.width),
            ..r
        });
        let previous = Rect {
            width: inner / 2,
            ..row(pager_y)
        };
        let next = Rect {
            x: pad + inner / 2,
            width: inner - inner / 2,
            ..row(pager_y)
        };
        WebSettingsLayout {
            panel,
            routes: self.settings_routes(panel, scale),
            proxy_input,
            proxy_add,
            proxy_rows,
            proxy_remove,
            visible_rows,
            list_start,
            previous,
            next,
            notice,
        }
    }

    pub fn browser_proxy_dropdown(self, scale: f64) -> Rect {
        let r = self.browser_proxy(scale);
        let width = dip(32.0, scale).min(r.width);
        Rect {
            x: r.x + r.width - width,
            width,
            ..r
        }
    }

    pub fn browser_proxy_text(self, scale: f64) -> Rect {
        let r = self.browser_proxy(scale);
        Rect {
            width: r
                .width
                .saturating_sub(self.browser_proxy_dropdown(scale).width),
            ..r
        }
    }

    pub fn browser_proxy_menu(self, scale: f64, count: usize, start: usize) -> MenuLayout {
        let field = self.browser_proxy(scale);
        let frame_right = self.status.x.saturating_add(self.status.width);
        let width = dip(320.0, scale).max(field.width).min(frame_right);
        let y = field.y.saturating_add(field.height);
        MenuLayout::fit(
            Rect {
                x: field.x.saturating_add(field.width).saturating_sub(width),
                y,
                width,
                height: self.status.y.saturating_sub(y),
            },
            dip(32.0, scale),
            dip(4.0, scale),
            count.min(9),
            start,
        )
    }

    /// Compute the settings panel geometry. Anchored under the header on the
    /// sidebar side, never a centered modal.
    #[must_use]
    pub fn settings_panel(self, scale: f64) -> SettingsLayout {
        let scale = scale.max(1.0);
        let pad = dip(12.0, scale);
        let gap = dip(8.0, scale);
        let row_h = dip(28.0, scale);
        let label_h = dip(20.0, scale);
        let line_h = dip(20.0, scale);
        let panel_width = self
            .sidebar
            .width
            .max(dip(300.0, scale))
            .min(self.status.x + self.status.width);
        let inner_x = self.sidebar.x.saturating_add(pad);
        let inner_w = panel_width.saturating_sub(pad.saturating_mul(2));
        let top = self
            .home
            .y
            .saturating_add(pad)
            .saturating_add(dip(40.0, scale));

        // three equal cells across the inner width, for a labelled control row.
        let cell_gap = dip(6.0, scale);
        let cell_w = inner_w
            .saturating_sub(cell_gap.saturating_mul(2))
            .checked_div(3)
            .unwrap_or(0);
        let cell = |i: u32, y: u32| Rect {
            x: inner_x.saturating_add((cell_w.saturating_add(cell_gap)).saturating_mul(i)),
            y,
            width: cell_w,
            height: row_h,
        };

        let lang_row = top.saturating_add(label_h);
        let language = [cell(0, lang_row), cell(1, lang_row), cell(2, lang_row)];

        let font_row = lang_row
            .saturating_add(row_h)
            .saturating_add(gap)
            .saturating_add(label_h);
        let font_down = cell(0, font_row);
        let font_reset = cell(1, font_row);
        let font_up = cell(2, font_row);

        let theme_row = font_row
            .saturating_add(row_h)
            .saturating_add(gap)
            .saturating_add(label_h);
        let theme_w = inner_w.saturating_sub(cell_gap.saturating_mul(3)) / 4;
        let theme = std::array::from_fn(|index| Rect {
            x: inner_x.saturating_add((theme_w + cell_gap) * index as u32),
            y: theme_row,
            width: theme_w,
            height: row_h,
        });

        // The Updates section comes before the shortcut list: a short window
        // truncates the help text, which `--help` also carries, rather than
        // pushing the Update / Restart controls off the bottom edge.
        //
        // `section_label` draws a label a line-height above the row it is
        // anchored to, so the row leaves it that line.
        let updates_row = theme_row
            .saturating_add(row_h)
            .saturating_add(gap)
            .saturating_add(label_h);
        let update = [
            cell(0, updates_row),
            cell(1, updates_row),
            cell(2, updates_row),
        ];
        let updates_status_y = updates_row.saturating_add(row_h).saturating_add(gap);
        // The cost line sits under the status line and above the shortcut list:
        // it belongs to the Restart control, so it is read before the list that
        // truncates on a short window.
        let updates_cost_y = updates_status_y.saturating_add(line_h);

        let shortcuts_label_y = updates_cost_y.saturating_add(line_h);
        let shortcuts_y = shortcuts_label_y.saturating_add(label_h);
        // The panel is anchored under the header and may not spill past the
        // composer band, so the list takes what is left.
        let room = self
            .composer
            .y
            .saturating_sub(shortcuts_y)
            .saturating_sub(pad);
        let shortcut_lines = (room / line_h.max(1)).min(SETTINGS_SHORTCUT_LINES);
        let panel_bottom = shortcuts_y
            .saturating_add(line_h.saturating_mul(shortcut_lines))
            .saturating_add(pad)
            .min(self.composer.y);
        let panel = Rect {
            x: self.sidebar.x,
            y: self.home.y,
            width: panel_width,
            height: panel_bottom.saturating_sub(self.home.y),
        };
        SettingsLayout {
            panel,
            routes: self.settings_routes(panel, scale),
            language,
            font_down,
            font_reset,
            font_up,
            theme,
            shortcuts_y,
            shortcut_line_height: line_h,
            shortcut_lines,
            updates_row,
            update,
            updates_status_y,
            updates_cost_y,
        }
    }

    /// Geometry of the right-side history/notes panel. Call only while it is
    /// open (`panel_open`).
    ///
    /// The panel is a column of stacked sections: a title row, the
    /// sent-history list, and — at the bottom — a blank area deliberately
    /// reserved for future loop/goal controls. That reservation is what the
    /// owner asked to keep free, so it is computed here rather than left to the
    /// painter, and the history list fills only the space above it.
    pub fn history_panel(self, scale: f64) -> HistoryPanelLayout {
        let scale = scale.max(1.0);
        let pad = dip(12.0, scale);
        let title_h = dip(20.0, scale);
        let row_height = dip(HISTORY_ROW_HEIGHT_DIP, scale).max(1);
        let close_size = dip(HISTORY_CLOSE_SIZE_DIP, scale);
        let close = Rect {
            x: self
                .panel
                .x
                .saturating_add(self.panel.width)
                .saturating_sub(pad)
                .saturating_sub(close_size),
            y: self.panel.y.saturating_add(pad / 2),
            width: close_size,
            height: close_size,
        };
        let title_y = self.panel.y.saturating_add(pad);
        let list_top = title_y.saturating_add(title_h);
        let reserved_top = self
            .panel
            .y
            .saturating_add(self.panel.height)
            .saturating_sub(pad)
            .saturating_sub(dip(HISTORY_RESERVED_HEIGHT_DIP, scale))
            .max(list_top);
        let row_capacity = reserved_top.saturating_sub(list_top) / row_height;
        // The notes section occupies the former blank reservation: a title
        // line, then the editable area down to the panel's bottom pad.
        let notes_title_h = dip(20.0, scale);
        let notes_title_y = reserved_top.saturating_add(dip(4.0, scale));
        let notes_text_y = notes_title_y.saturating_add(notes_title_h);
        let panel_bottom = self
            .panel
            .y
            .saturating_add(self.panel.height)
            .saturating_sub(pad);
        let notes_text_h = panel_bottom.saturating_sub(notes_text_y);
        let notes_area = Rect {
            x: self.panel.x.saturating_add(pad / 2),
            y: reserved_top,
            width: self.panel.width.saturating_sub(pad),
            height: panel_bottom.saturating_sub(reserved_top),
        };
        let cell_h = dip(18.0, scale).max(1);
        HistoryPanelLayout {
            panel: self.panel,
            close,
            title_y,
            list_top,
            row_height,
            row_capacity: row_capacity as usize,
            reserved_top,
            notes: NotesAreaLayout {
                title_y: notes_title_y,
                text_x: self.panel.x.saturating_add(pad),
                text_y: notes_text_y,
                text_w: self.panel.width.saturating_sub(pad.saturating_mul(2)),
                text_h: notes_text_h,
                area: notes_area,
            },
            notes_visible_rows: (notes_text_h / cell_h) as usize,
        }
    }
}

/// Geometry of the open history/notes panel, in panel coordinates.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct HistoryPanelLayout {
    pub panel: Rect,
    pub close: Rect,
    pub title_y: u32,
    pub list_top: u32,
    pub row_height: u32,
    /// How many history rows fit above the reserved area at this size.
    pub row_capacity: usize,
    /// Top of the notes scratchpad section. The former blank reservation
    /// (96 DIP kept free for "future loop/goal controls") is the notes
    /// area now — the same room, finally spent on the section it was
    /// held for. The history list never paints below this line.
    pub reserved_top: u32,
    /// The notes section's own geometry, derived from `reserved_top`.
    pub notes: NotesAreaLayout,
    /// How many text rows the notes area can show at this size. The editor
    /// does not scroll its own; overflow is clipped, and the IME anchor
    /// clamps to this so a long note parks the candidate window inside
    /// the panel.
    pub notes_visible_rows: usize,
}

/// Geometry of the notes scratchpad area inside the open right panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct NotesAreaLayout {
    /// The section's title baseline y.
    pub title_y: u32,
    /// The editable text area's top-left, inset from the panel edges.
    pub text_x: u32,
    pub text_y: u32,
    /// Inner width available to note text, between the insets.
    pub text_w: u32,
    /// Inner height available to note text, down to the panel's bottom pad.
    pub text_h: u32,
    /// The whole section, for hit-testing focus clicks.
    pub area: Rect,
}

/// Where a pointer landed on the open history panel.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum HistoryHit {
    Outside,
    Panel,
    Close,
    /// Row index counting from the top of the list (`0` = newest shown). May
    /// exceed the entry count; the caller ignores an index with no entry.
    Row(usize),
    /// The notes scratchpad section — a click focuses the notes editor.
    Notes,
}

/// Hit-test the open history/notes panel. Call only while [`Layout::panel_open`].
#[must_use]
pub fn history_hit(layout: Layout, x: u32, y: u32, scale: f64) -> HistoryHit {
    if !layout.panel_open || !layout.panel.contains(x, y) {
        return HistoryHit::Outside;
    }
    let p = layout.history_panel(scale);
    if p.close.contains(x, y) {
        return HistoryHit::Close;
    }
    if y >= p.list_top && y < p.reserved_top {
        return HistoryHit::Row(((y - p.list_top) / p.row_height.max(1)) as usize);
    }
    if p.notes.area.contains(x, y) {
        return HistoryHit::Notes;
    }
    HistoryHit::Panel
}

/// Hit-test the open settings panel. Call only while the panel is open.
#[must_use]
pub fn settings_hit(layout: Layout, x: u32, y: u32, scale: f64) -> SettingsHit {
    let s = layout.settings_panel(scale);
    for (index, rect) in s.routes.iter().enumerate() {
        if rect.contains(x, y) {
            return SettingsHit::Route(SettingsSection::from_index(index));
        }
    }
    if !s.panel.contains(x, y) {
        return SettingsHit::Outside;
    }
    let langs = [
        UiLanguage::English,
        UiLanguage::ChineseSimplified,
        UiLanguage::ChineseTraditional,
    ];
    for (rect, lang) in s.language.iter().zip(langs) {
        if rect.contains(x, y) {
            return SettingsHit::Language(lang);
        }
    }
    if s.font_down.contains(x, y) {
        return SettingsHit::FontDown;
    }
    if s.font_reset.contains(x, y) {
        return SettingsHit::FontReset;
    }
    if s.font_up.contains(x, y) {
        return SettingsHit::FontUp;
    }
    for (i, rect) in s.theme.iter().enumerate() {
        if rect.contains(x, y) {
            return SettingsHit::Theme(i);
        }
    }
    if s.update[0].contains(x, y) {
        return SettingsHit::UpdateCheck;
    }
    if s.update[1].contains(x, y) {
        return SettingsHit::UpdateInstall;
    }
    if s.update[2].contains(x, y) {
        return SettingsHit::UpdateRestart;
    }
    SettingsHit::Panel
}

pub fn clamp_tree_scroll(offset: usize, item_count: usize, capacity: usize) -> usize {
    offset.min(item_count.saturating_sub(capacity.max(1)))
}

/// Clamp a history-panel scroll offset.
///
/// The offset counts rows **back from the newest entry**, so `0` shows the
/// newest rows and the maximum is whatever leaves the oldest entry on the last
/// visible row. A positive `delta` moves toward older entries, which is the
/// direction a wheel-up reports; a delta that would run off either end stops
/// there instead of wrapping, because a list that silently jumped would read as
/// entries that went missing.
#[must_use]
pub fn clamp_history_scroll(offset: usize, delta: isize, entries: usize, capacity: usize) -> usize {
    offset
        .saturating_add_signed(delta)
        .min(entries.saturating_sub(capacity))
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

/// Total physical height reserved at the bottom of the window for host UI: the
/// composer plus the status bar stacked beneath it (see `with_sidebar_width`,
/// where `composer.y = height - status_height - composer_height`). The terminal
/// viewport's bottom must land exactly on the composer's top, so this reserves
/// both bands — summed the same way the layout stacks them, so rounding matches
/// `composer.y` to the pixel.
///
/// Reserving only the composer left the terminal's bottom rows overlapping the
/// composer by `status_height`. That gap was masked while Windows ran
/// DPI-unaware (rendered at scale 1.0, then bitmap-stretched by the OS); once
/// per-monitor DPI awareness made rendering happen at the real scale, the
/// `status_height * scale` overlap became visible.
pub fn bottom_inset(scale: f64) -> u32 {
    dip(COMPOSER_HEIGHT_DIP, scale).saturating_add(dip(STATUS_HEIGHT_DIP, scale))
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
    /// Width reserved on the right by the history/notes panel, so the terminal
    /// column's scrollbar stops at the panel's edge instead of sliding under it.
    pub right_inset: u32,
    pub scale: f64,
    pub rows: usize,
}

pub fn terminal_scrollbar_geometry(
    viewport: TerminalViewport,
    offset: usize,
    maximum: usize,
) -> minicon_ui_core::ScrollbarGeometry {
    minicon_ui_core::terminal_scrollbar_geometry(
        minicon_ui_core::Rect {
            left: viewport.left.min(i32::MAX as u32) as i32,
            top: viewport.top.min(i32::MAX as u32) as i32,
            right: viewport
                .width
                .saturating_sub(viewport.right_inset)
                .min(i32::MAX as u32) as i32,
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

    /// A representative spread of window sizes and DPI scales the layout must
    /// survive: phone-narrow to 4K-wide, 100 % to 300 %, plus a couple of
    /// awkward fractional scales real monitors actually report. Kept in one
    /// place so every cross-cutting invariant below sweeps the same grid.
    const LAYOUT_SWEEP: &[(u32, u32, f64)] = &[
        (420, 600, 1.0),
        (800, 600, 1.0),
        (1200, 800, 1.0),
        (1280, 720, 1.25),
        (1600, 900, 1.5),
        (1920, 1080, 1.5),
        (2560, 1440, 2.0),
        (3840, 2160, 2.0),
        (1000, 700, 1.75),
        (900, 1600, 3.0),
    ];

    #[test]
    fn home_inspector_and_rows_have_disjoint_targets_at_supported_sizes() {
        for &(width, height, scale) in LAYOUT_SWEEP {
            let layout = Layout::new(width, height, scale);
            let inspector = layout.home_inspector(width, height, scale);
            let focus = layout.home_focus_action(width, height, scale);
            let capacity = layout.home_fleet_capacity(width, height, scale);
            if inspector.width == 0 || inspector.height == 0 {
                continue;
            }
            assert!(focus.width > 0 && focus.height > 0);
            assert!(focus.x >= inspector.x && focus.y >= inspector.y);
            assert!(focus.x + focus.width <= inspector.x + inspector.width);
            assert!(focus.y + focus.height <= inspector.y + inspector.height);
            for visible in 0..capacity {
                let row = layout.home_fleet_row(width, height, scale, visible);
                assert!(row.width > 0 && row.height > 0);
                let overlaps = row.x < inspector.x + inspector.width
                    && inspector.x < row.x + row.width
                    && row.y < inspector.y + inspector.height
                    && inspector.y < row.y + row.height;
                assert!(
                    !overlaps,
                    "Home row {visible} overlaps inspector at {width}x{height}@{scale}"
                );
            }
        }
        let layout = Layout::new(960, 600, 1.0);
        assert!(layout.home_fleet_capacity(960, 600, 1.0) >= 3);
    }

    #[test]
    fn web_proxy_settings_page_keeps_controls_and_pagination_inside_panel() {
        for scale in [1.0, 1.5, 2.0] {
            for (width, height) in [(420.0, 360.0), (800.0, 600.0), (1200.0, 900.0)] {
                let layout = Layout::new(dip(width, scale), dip(height, scale), scale);
                for offset in [0, 3, 7] {
                    let s = layout.web_settings(scale, offset);
                    assert!(s.visible_rows > 0);
                    assert!(s.list_start + s.visible_rows <= 8);
                    for rect in s
                        .routes
                        .into_iter()
                        .chain([s.proxy_input, s.proxy_add, s.previous, s.next])
                        .chain(s.proxy_rows.into_iter().take(s.visible_rows))
                    {
                        assert!(rect.width > 0 && rect.height > 0);
                        assert!(rect.x >= s.panel.x && rect.y >= s.panel.y);
                        assert!(rect.x + rect.width <= s.panel.x + s.panel.width);
                        assert!(rect.y + rect.height <= s.panel.y + s.panel.height);
                    }
                    assert!(s.proxy_input.x + s.proxy_input.width <= s.proxy_add.x);
                    for index in 0..s.visible_rows {
                        assert!(s.proxy_remove[index].x >= s.proxy_rows[index].x);
                        assert!(
                            s.proxy_remove[index].x + s.proxy_remove[index].width
                                <= s.proxy_rows[index].x + s.proxy_rows[index].width
                        );
                    }
                }
                let console = layout.settings_panel(scale);
                assert_eq!(
                    console.routes,
                    layout.web_settings(scale, 0).routes,
                    "route targets must stay in place when switching pages"
                );
                let rail = Layout::collapsed(dip(width, scale), dip(height, scale), scale);
                let rail_web = rail.web_settings(scale, 0);
                assert!(
                    rail_web.visible_rows > 0,
                    "collapsed short windows still expose a saved option"
                );
                assert!(rail.settings_panel(scale).panel.width >= dip(SIDEBAR_WIDTH_DIP, scale));
                assert!(console.routes[1].y + console.routes[1].height < console.language[0].y);
                assert_eq!(
                    settings_hit(
                        layout,
                        console.routes[1].x + 1,
                        console.routes[1].y + 1,
                        scale
                    ),
                    SettingsHit::Route(SettingsSection::Web)
                );
            }
        }
    }

    #[test]
    fn browser_proxy_menu_preserves_full_targets_for_actual_counts_and_scrolls() {
        for scale in [1.0, 1.5, 2.0] {
            for (width, height) in [
                (420.0, 180.0),
                (420.0, 360.0),
                (800.0, 600.0),
                (1200.0, 900.0),
            ] {
                let layout = Layout::new(dip(width, scale), dip(height, scale), scale);
                for count in [1, 2, 9] {
                    let menu = layout.browser_proxy_menu(scale, count, 0);
                    assert!(menu.visible_rows <= count);
                    for index in 0..menu.visible_rows {
                        let row = menu.row(index);
                        assert_eq!(row.height, dip(32.0, scale));
                        assert!(row.y + row.height <= menu.rect.y + menu.rect.height);
                    }
                    if menu.visible_rows > 0 {
                        let last = layout.browser_proxy_menu(scale, count, menu.reveal(count - 1));
                        assert_eq!(last.row(count - 1).height, dip(32.0, scale));
                        assert_eq!(
                            last.rect.height,
                            (last.visible_rows as u32) * dip(32.0, scale) + dip(8.0, scale)
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn browser_proxy_and_address_are_independent_and_inside_the_toolbar() {
        for &(width, height, scale) in LAYOUT_SWEEP {
            for collapsed in [false, true] {
                let layout =
                    Layout::with_sidebar(width, height, scale, SIDEBAR_WIDTH_DIP, collapsed);
                let toolbar = layout.browser_toolbar(scale);
                let address = layout.browser_address(scale);
                let proxy = layout.browser_proxy(scale);
                for rect in [address, proxy] {
                    assert!(rect.width > 0 && rect.height > 0);
                    assert!(rect.x >= toolbar.x && rect.y >= toolbar.y);
                    assert!(rect.x + rect.width <= toolbar.x + toolbar.width);
                    assert!(rect.y + rect.height <= toolbar.y + toolbar.height);
                }
                assert!(
                    address.x + address.width <= proxy.x || address.y + address.height <= proxy.y
                );
                if let Some(page) = layout.browser_page(scale) {
                    assert_eq!(page.y, toolbar.y + toolbar.height);
                }
            }
        }
    }

    #[test]
    fn browser_proxy_reflows_without_shrinking_the_editable_proxy_text() {
        for scale in [1.0, 1.5, 2.0] {
            let wide = Layout::new(dip(960.0, scale), dip(600.0, scale), scale);
            assert_eq!(wide.browser_toolbar(scale).height, dip(44.0, scale));
            assert_eq!(wide.browser_proxy(scale).width, dip(240.0, scale));
            assert_eq!(wide.browser_proxy(scale).y, wide.browser_address(scale).y);
            let narrow = Layout::new(dip(800.0, scale), dip(600.0, scale), scale);
            assert_eq!(narrow.browser_toolbar(scale).height, dip(88.0, scale));
            assert!(narrow.browser_proxy(scale).y > narrow.browser_address(scale).y);
            let smallest = Layout::new(dip(420.0, scale), dip(600.0, scale), scale);
            assert_eq!(smallest.browser_toolbar(scale).height, dip(132.0, scale));
            assert_eq!(
                smallest.browser_address(scale).x,
                smallest.browser_proxy(scale).x
            );
            assert_eq!(
                smallest.browser_address(scale).width,
                smallest.browser_proxy(scale).width
            );
            assert!(smallest.browser_address(scale).width >= dip(180.0, scale));
        }
    }

    #[test]
    fn creation_menu_actions_are_separate_and_stay_inside_the_window() {
        for &(width, height, scale) in LAYOUT_SWEEP {
            for collapsed in [false, true] {
                let layout =
                    Layout::with_sidebar(width, height, scale, SIDEBAR_WIDTH_DIP, collapsed);
                let choices = layout.new_tab_choices(scale, 0);
                let menu = choices.rect;
                let terminal = choices.row(0);
                let browser = choices.row(1);
                assert!(menu.x + menu.width <= width && menu.y + menu.height <= height);
                assert!(terminal.width > 0 && terminal.height > 0);
                assert!(browser.width > 0 && browser.height > 0);
                for rect in [terminal, browser] {
                    assert!(rect.x >= menu.x && rect.y >= menu.y);
                    assert!(rect.x + rect.width <= menu.x + menu.width);
                    assert!(rect.y + rect.height <= menu.y + menu.height);
                }
                assert!(terminal.y + terminal.height <= browser.y);
                if !collapsed {
                    assert!(menu.x + menu.width <= layout.sidebar.width);
                }
            }
        }
    }

    #[test]
    fn browser_recovery_actions_fit_the_page_and_stack_when_narrow() {
        for &(width, height, scale) in LAYOUT_SWEEP {
            for collapsed in [false, true] {
                let layout =
                    Layout::with_sidebar(width, height, scale, SIDEBAR_WIDTH_DIP, collapsed);
                let Some(page) = layout.browser_page(scale) else {
                    continue;
                };
                let card = layout.browser_recovery_card(scale);
                let retry = layout.browser_retry(scale);
                let edit = layout.browser_edit_address(scale);
                assert!(card.x >= page.x && card.y >= page.y);
                assert!(card.x + card.width <= page.x + page.width);
                assert!(card.y + card.height <= page.y + page.height);
                for rect in [retry, edit] {
                    assert!(rect.width > 0 && rect.height > 0);
                    assert!(rect.x >= card.x && rect.y >= card.y);
                    assert!(rect.x + rect.width <= card.x + card.width);
                    assert!(rect.y + rect.height <= card.y + card.height);
                }
                assert!(retry.y + retry.height <= edit.y || retry.x + retry.width <= edit.x);
                if card.width < dip(300.0, scale) {
                    assert_eq!(retry.x, edit.x);
                    assert!(retry.y + retry.height < edit.y);
                }
            }
        }
    }

    #[test]
    fn expanded_creation_entries_keep_distinct_hit_targets() {
        for &(width, height, scale) in LAYOUT_SWEEP {
            let layout = Layout::new(width, height, scale);
            assert!(layout.new_root.width >= dip(32.0, scale));
            assert!(layout.new_web.width >= dip(32.0, scale));
            assert!(layout.new_root.x + layout.new_root.width <= layout.new_web.x);
            assert!(layout.new_web.x + layout.new_web.width <= layout.settings.x);
        }
    }

    /// Every invariant the expanded sidebar has, the rail has too. Collapsing is
    /// a geometry change, and the sweep is what stops it from being a geometry
    /// change that only looks right at the size it was written on.
    #[test]
    fn the_rail_keeps_every_header_control_reachable_and_inside_it() {
        for &(w, h, scale) in LAYOUT_SWEEP {
            let rail = Layout::collapsed(w, h, scale);
            assert!(rail.sidebar_collapsed, "{w}x{h}@{scale}");
            let controls = [
                ("toggle", rail.sidebar_toggle),
                ("new", rail.new_root),
                ("settings", rail.settings),
                ("panel", rail.panel_toggle),
            ];
            for (name, control) in controls {
                assert!(
                    control.width > 0 && control.height > 0,
                    "{name} vanished at {w}x{h}@{scale}: {control:?}"
                );
                // Inside the rail horizontally, and inside the window vertically.
                assert!(
                    control.x.saturating_add(control.width) <= rail.sidebar.width,
                    "{name} overflows the rail at {w}x{h}@{scale}: {control:?}"
                );
                assert!(
                    control.y.saturating_add(control.height) <= h,
                    "{name} runs past the window bottom at {w}x{h}@{scale}: {control:?}"
                );
                // Each one must be hit-testable as itself, or it is decoration.
                let hit = tree_hit(
                    rail,
                    control.x + control.width / 2,
                    control.y + control.height / 2,
                    0,
                    0,
                    scale,
                );
                let expected = match name {
                    "toggle" => TreeHit::SidebarToggle,
                    "new" => TreeHit::NewRoot,
                    "panel" => TreeHit::PanelToggle,
                    _ => TreeHit::Settings,
                };
                assert_eq!(hit, expected, "{name} is not clickable at {w}x{h}@{scale}");
            }
            // Stacked, so no two may overlap.
            for (a_name, a) in controls {
                for (b_name, b) in controls {
                    if a_name == b_name {
                        continue;
                    }
                    let disjoint = a.y.saturating_add(a.height) <= b.y
                        || b.y.saturating_add(b.height) <= a.y
                        || a.x.saturating_add(a.width) <= b.x
                        || b.x.saturating_add(b.width) <= a.x;
                    assert!(disjoint, "{a_name} overlaps {b_name} at {w}x{h}@{scale}");
                }
            }
            // Rows begin below the stack, never under it.
            assert!(
                rail.tree_header_height
                    >= rail.panel_toggle.y.saturating_add(rail.panel_toggle.height),
                "rail rows would start inside the controls at {w}x{h}@{scale}"
            );
        }
    }

    #[test]
    fn collapsing_gives_the_column_back_to_the_terminal() {
        for &(w, h, scale) in LAYOUT_SWEEP {
            let open = Layout::new(w, h, scale);
            let rail = Layout::collapsed(w, h, scale);
            assert!(
                rail.sidebar.width < open.sidebar.width,
                "the rail is not narrower at {w}x{h}@{scale}: {} vs {}",
                rail.sidebar.width,
                open.sidebar.width
            );
            assert!(
                rail.sidebar.width > 0,
                "a collapsed sidebar is a rail, not a disappearance, at {w}x{h}@{scale}"
            );
        }
    }

    #[test]
    fn the_expanded_header_seats_the_tools_without_collision() {
        for &(w, h, scale) in LAYOUT_SWEEP {
            let layout = Layout::new(w, h, scale);
            assert!(!layout.sidebar_collapsed);
            // Left to right: New, Settings, the sidebar toggle, the panel
            // toggle. Each chrome toggle takes the column it controls, and the
            // panel toggle holds the rightmost edge nearest the panel.
            assert!(
                layout.panel_toggle.x > layout.sidebar_toggle.x,
                "the panel toggle must sit right of the sidebar toggle at {w}x{h}@{scale}"
            );
            assert!(
                layout.sidebar_toggle.x > layout.settings.x,
                "the sidebar toggle must sit right of settings at {w}x{h}@{scale}"
            );
            assert!(
                layout.settings.x.saturating_add(layout.settings.width) <= layout.sidebar_toggle.x,
                "settings and the sidebar toggle overlap at {w}x{h}@{scale}"
            );
            assert!(
                layout
                    .sidebar_toggle
                    .x
                    .saturating_add(layout.sidebar_toggle.width)
                    <= layout.panel_toggle.x,
                "the two toggles overlap at {w}x{h}@{scale}"
            );
            assert!(
                layout.new_root.x.saturating_add(layout.new_root.width) <= layout.settings.x,
                "New collides with settings at {w}x{h}@{scale}"
            );
            assert!(
                layout
                    .panel_toggle
                    .x
                    .saturating_add(layout.panel_toggle.width)
                    <= layout.sidebar.width,
                "the panel toggle overflows the sidebar at {w}x{h}@{scale}"
            );
        }
    }

    #[test]
    fn a_rail_tooltip_floats_over_the_terminal_and_never_over_the_composer() {
        for &(w, h, scale) in LAYOUT_SWEEP {
            let rail = Layout::collapsed(w, h, scale);
            // An expanded sidebar already shows labels, so it has no tooltip.
            assert!(
                rail_tooltip_rect(Layout::new(w, h, scale), 0, 80, w, scale).is_none(),
                "expanded sidebars must not draw tooltips at {w}x{h}@{scale}"
            );
            let Some(tip) = rail_tooltip_rect(rail, 0, 80, w, scale) else {
                continue; // too narrow to host one; that is a documented answer
            };
            assert!(
                tip.x >= rail.sidebar.width,
                "the tooltip covers the rail at {w}x{h}@{scale}"
            );
            assert!(
                tip.x.saturating_add(tip.width) <= w,
                "the tooltip runs off the window at {w}x{h}@{scale}: {tip:?}"
            );
            assert!(
                tip.y.saturating_add(tip.height) <= rail.composer.y,
                "the tooltip hangs into the composer at {w}x{h}@{scale}: {tip:?}"
            );
        }
    }

    #[test]
    fn a_tooltip_wider_than_the_terminal_column_is_refused_rather_than_clipped() {
        let rail = Layout::collapsed(1200, 800, 1.0);
        assert!(
            rail_tooltip_rect(rail, 0, 100, 1200, 1.0).is_some(),
            "a normal label should fit"
        );
        assert!(
            rail_tooltip_rect(rail, 0, 100_000, 1200, 1.0).is_none(),
            "a label wider than the column must be refused, not clipped over itself"
        );
    }

    /// The bottom of the window is three stacked bands on the terminal side —
    /// terminal viewport, composer, status — and they must tile it with no gap
    /// and no overlap at every size and scale. This is the invariant the Windows
    /// "last terminal row overlaps the composer" bug violated: the reserved
    /// bottom inset had omitted the status band, so the terminal viewport
    /// extended past the composer's top. See [`bottom_inset`].
    #[test]
    fn terminal_composer_and_status_tile_the_bottom_with_no_overlap() {
        for &(w, h, scale) in LAYOUT_SWEEP {
            let layout = Layout::new(w, h, scale);
            let viewport_bottom = h.saturating_sub(bottom_inset(scale));
            assert_eq!(
                viewport_bottom, layout.composer.y,
                "terminal viewport bottom must meet the composer top ({w}x{h}@{scale})"
            );
            assert_eq!(
                layout.composer.y.saturating_add(layout.composer.height),
                layout.status.y,
                "composer must sit flush on the status bar ({w}x{h}@{scale})"
            );
            assert_eq!(
                layout.status.y.saturating_add(layout.status.height),
                h,
                "status bar must reach the window bottom ({w}x{h}@{scale})"
            );
        }
    }

    /// No pixel a mouse-tracking child draws into may be stolen by host UI: the
    /// last terminal-viewport pixel row must classify as terminal (outside both
    /// the composer and the status bar), while the very next row down is the
    /// composer. This is the direct guard for "can't click the TUI's bottom
    /// input line" — with the old inset that row's pixels fell inside the
    /// composer rect and the click never reached the child.
    #[test]
    fn the_last_terminal_row_is_never_captured_by_host_ui() {
        for &(w, h, scale) in LAYOUT_SWEEP {
            let layout = Layout::new(w, h, scale);
            // A column safely on the terminal side of the sidebar.
            let x = layout
                .sidebar
                .width
                .saturating_add(4)
                .min(w.saturating_sub(1));
            let last_terminal_y = layout.composer.y.saturating_sub(1);
            assert_eq!(
                composer_hit(layout, x, last_terminal_y),
                ComposerHit::Outside,
                "last terminal row leaked into the composer ({w}x{h}@{scale})"
            );
            assert_eq!(
                status_hit(layout, x, last_terminal_y),
                StatusHit::Outside,
                "last terminal row leaked into the status bar ({w}x{h}@{scale})"
            );
            // And the seam is exact: one row lower is unambiguously the composer.
            assert_eq!(
                composer_hit(layout, x, layout.composer.y),
                ComposerHit::Input,
                "composer top row is not classified as the composer ({w}x{h}@{scale})"
            );
        }
    }

    /// Every interactive rect stays inside the window at any size or scale —
    /// nothing overflows the right or bottom edge, which would put a control
    /// half off-screen or make it unhittable. Degenerate inputs (a window
    /// smaller than the chrome, a non-finite scale) must clamp, not overflow.
    #[test]
    fn host_ui_rects_never_overflow_the_window() {
        let mut cases: Vec<(u32, u32, f64)> = LAYOUT_SWEEP.to_vec();
        cases.extend_from_slice(&[
            (1, 1, 1.0),
            (200, 100, 2.0),
            (0, 0, 1.0),
            (u32::MAX, u32::MAX, f64::INFINITY),
            (640, 480, f64::NAN),
        ]);
        for (w, h, scale) in cases {
            let layout = Layout::new(w, h, scale);
            for (name, r) in [
                ("sidebar", layout.sidebar),
                ("composer", layout.composer),
                ("status", layout.status),
                ("composer_input", layout.composer_input),
                ("composer_send", layout.composer_send),
                ("composer_newline", layout.composer_newline),
            ] {
                // A zero-area rect paints nothing and `contains` rejects every
                // pixel, so its origin is harmless; only a rect that is actually
                // drawn (positive on both axes) must stay inside the window.
                if r.width == 0 || r.height == 0 {
                    continue;
                }
                assert!(
                    r.x.saturating_add(r.width) <= w,
                    "{name} overflows width at {w}x{h}@{scale}: {r:?}"
                );
                assert!(
                    r.y.saturating_add(r.height) <= h,
                    "{name} overflows height at {w}x{h}@{scale}: {r:?}"
                );
            }
        }
    }

    /// Every interactive control inside the composer must stay within the
    /// composer band and the typing area must never overlap the Send/Newline
    /// buttons — otherwise a click meant for the input toggles a button, or a
    /// button paints over the text. Swept across sizes and scales so a scale-only
    /// rounding drift cannot slip a control half out of its band.
    #[test]
    fn composer_controls_stay_within_the_band_and_never_overlap_the_input() {
        fn overlaps(a: Rect, b: Rect) -> bool {
            a.width > 0
                && a.height > 0
                && b.width > 0
                && b.height > 0
                && a.x < b.x.saturating_add(b.width)
                && b.x < a.x.saturating_add(a.width)
                && a.y < b.y.saturating_add(b.height)
                && b.y < a.y.saturating_add(a.height)
        }
        fn within(inner: Rect, outer: Rect) -> bool {
            inner.width == 0
                || inner.height == 0
                || (inner.x >= outer.x
                    && inner.y >= outer.y
                    && inner.x.saturating_add(inner.width) <= outer.x.saturating_add(outer.width)
                    && inner.y.saturating_add(inner.height) <= outer.y.saturating_add(outer.height))
        }
        for &(w, h, scale) in LAYOUT_SWEEP {
            let l = Layout::new(w, h, scale);
            for (name, r) in [
                ("input", l.composer_input),
                ("send", l.composer_send),
                ("newline", l.composer_newline),
                ("copy", l.composer_copy),
                ("paste", l.composer_paste),
                ("cut", l.composer_cut),
            ] {
                assert!(
                    within(r, l.composer),
                    "composer {name} escapes the band at {w}x{h}@{scale}: {r:?} vs {:?}",
                    l.composer
                );
            }
            assert!(
                !overlaps(l.composer_input, l.composer_send),
                "input overlaps Send at {w}x{h}@{scale}"
            );
            assert!(
                !overlaps(l.composer_input, l.composer_newline),
                "input overlaps Newline at {w}x{h}@{scale}"
            );
        }
    }

    /// With no tabs open the greeting page is the **only** way out of the
    /// window, so both of its buttons must stay reachable: Quit sits below New
    /// Terminal with a gap, they never overlap, and neither leaves the screen.
    /// A Quit button pushed off-screen would trap the user with no exit at all.
    #[test]
    fn greeting_page_buttons_stay_reachable_and_disjoint() {
        for &(w, h, scale) in LAYOUT_SWEEP {
            let layout = Layout::new(w, h, scale);
            let new = layout.empty_new_terminal(w, h, scale);
            let quit = layout.empty_quit(w, h, scale);

            assert!(
                quit.y >= new.y.saturating_add(new.height),
                "Quit must sit below New Terminal at {w}x{h}@{scale}"
            );
            let vertical_gap = quit.y >= new.y.saturating_add(new.height);
            assert!(vertical_gap, "buttons overlap at {w}x{h}@{scale}");
            assert_eq!(quit.x, new.x, "buttons must share a column");
            assert_eq!(quit.width, new.width, "buttons must share a width");

            for (name, r) in [("new-terminal", new), ("quit", quit)] {
                assert!(
                    r.width > 0 && r.height > 0,
                    "{name} vanished at {w}x{h}@{scale}"
                );
                assert!(
                    r.x.saturating_add(r.width) <= w,
                    "{name} runs off the right edge at {w}x{h}@{scale}: {r:?}"
                );
                assert!(
                    r.y.saturating_add(r.height) <= h,
                    "{name} runs off the bottom at {w}x{h}@{scale}: {r:?}"
                );
            }
        }
    }

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
        // The status bar carves 24 DIP off the bottom, so the composer ends
        // above it rather than at the window edge.
        assert_eq!(layout.composer.y, 680);
        assert!(layout.composer_input.width > layout.composer_send.width);
        assert!(layout.composer_input.x >= layout.composer.x);
        assert!(layout.composer_input.x + layout.composer_input.width < layout.composer_send.x);
        // The status bar spans the terminal side, sits flush at the bottom, and
        // the composer stacks directly on top of it with no gap or overlap.
        assert_eq!(layout.status.x, layout.sidebar.width);
        assert_eq!(layout.status.width, 1200 - layout.sidebar.width);
        assert_eq!(layout.status.y, 776);
        assert_eq!(layout.status.y + layout.status.height, 800);
        assert_eq!(layout.composer.y + layout.composer.height, layout.status.y);
        let column = layout.content_column().expect("content column");
        assert_eq!(column.x, layout.sidebar.width);
        assert_eq!(column.y, layout.tree_header_height);
        assert_eq!(column.width, layout.composer.width);
        assert_eq!(column.y + column.height, layout.composer.y);
        assert!(column.y > 0);
        assert!(column.height < layout.sidebar.height);
    }

    /// The browser embed slot is the terminal rectangle: under the tab header,
    /// above the composer, clear of the sidebar and the history chrome.
    #[test]
    fn content_column_is_the_terminal_rectangle() {
        let layout = Layout::with_chrome(
            1200,
            800,
            1.0,
            SIDEBAR_WIDTH_DIP,
            false,
            true,
            HISTORY_PANEL_WIDTH_DIP,
        );
        let column = layout.content_column().expect("content column");
        assert!(layout.panel_open);
        assert_eq!(column.x, layout.composer.x);
        assert_eq!(column.x, layout.sidebar.width);
        assert_eq!(column.y, layout.tree_header_height);
        assert!(column.y > 0);
        assert_eq!(column.width, layout.composer.width);
        assert_eq!(column.y + column.height, layout.composer.y);
        assert_eq!(column.x + column.width, layout.panel.x);
        assert!(layout.panel.x >= layout.sidebar.width);
        let flat = Layout::with_chrome(1200, 1, 1.0, SIDEBAR_WIDTH_DIP, false, false, 0.0);
        assert_eq!(flat.content_column(), None);
    }

    /// The terminal viewport's reserved bottom inset must land exactly on the
    /// composer's top at every scale, so the bottom terminal rows never overlap
    /// the composer. Regression for the Windows overlap that surfaced once
    /// per-monitor DPI awareness made rendering happen at the real scale: the
    /// inset had reserved only the composer, omitting the status bar beneath it.
    #[test]
    fn bottom_inset_matches_the_composer_top_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0] {
            let height = 800;
            let layout = Layout::new(1200, height, scale);
            assert_eq!(
                height - bottom_inset(scale),
                layout.composer.y,
                "viewport bottom must equal composer top at scale {scale}"
            );
        }
    }

    /// The two header tools — New (left) and Settings (right) — never overlap
    /// and both stay inside the sidebar header row, at any width or scale.
    #[test]
    fn the_header_tools_are_ordered_and_disjoint() {
        for (width, scale) in [(1000, 1.0), (760, 1.0), (1600, 1.5), (2400, 2.0)] {
            let layout = Layout::new(width, 600, scale);
            assert!(
                layout.new_root.x + layout.new_root.width <= layout.settings.x,
                "New and Settings overlap at {width}x{scale}"
            );
            assert!(
                layout.settings.x + layout.settings.width <= layout.sidebar.width,
                "Settings leaves the sidebar at {width}x{scale}"
            );
            for tool in [layout.new_root, layout.settings] {
                assert!(
                    tool.y + tool.height <= layout.tree_header_height,
                    "a header tool spills into the tab rows at {width}x{scale}"
                );
            }
        }
    }

    /// With only two header tools the row no longer overflows a narrow sidebar
    /// (the old seven-tool overflow is designed out), and the frame stays
    /// partitioned at every width.
    #[test]
    fn the_header_row_fits_and_the_frame_stays_partitioned() {
        for (width, height) in [(200u32, 80u32), (176, 80), (150, 60), (100, 40)] {
            let layout = Layout::new(width, height, 1.0);
            assert!(
                layout.new_root.x + layout.new_root.width <= layout.harness.x
                    || layout.new_root.y + layout.new_root.height <= layout.harness.y,
                "New overlaps the wrapped robot/settings pair at {width}x{height}"
            );
            assert!(
                layout.settings.x + layout.settings.width <= layout.sidebar.width,
                "the header row holds both tools at {width}x{height}"
            );
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
    fn settings_header_button_has_its_own_hit_target() {
        let layout = Layout::new(1200, 800, 1.0);
        assert_eq!(
            tree_hit(
                layout,
                layout.settings.x + layout.settings.width / 2,
                layout.settings.y + layout.settings.height / 2,
                0,
                1,
                1.0,
            ),
            TreeHit::Settings
        );
    }

    /// Everything the panel paints must fit inside the panel it computed, and
    /// Everything the panel paints must fit inside the panel it computed, the
    /// Updates controls must sit above the shortcut list, and the panel must
    /// never spill past the composer band. This is the invariant a spilled
    /// line or a mis-anchored section breaks, and it is the only thing that
    /// catches it without a screenshot.
    #[test]
    fn the_settings_panel_contains_every_section() {
        for (width, height) in [(1200, 800), (960, 600), (640, 480)] {
            for scale in [1.0, 1.25, 1.5, 1.7675, 2.0] {
                let layout = Layout::new(width, height, scale);
                let s = layout.settings_panel(scale);
                let line = s.shortcut_line_height;
                let bottom = s.panel.y.saturating_add(s.panel.height);
                // The panel never spills past the band it is anchored above.
                assert!(bottom <= layout.composer.y);
                assert!(
                    s.shortcut_lines <= SETTINGS_SHORTCUT_LINES,
                    "the painter cannot draw more lines than the list has"
                );
                // The list begins below the Updates status line, never over it.
                assert!(
                    s.shortcuts_y >= s.updates_status_y.saturating_add(line),
                    "the shortcut list overlaps the Updates section at scale {scale}"
                );
                // The cost line is the next line down, so it reads as the
                // status line's continuation and never lands on the list.
                assert!(
                    s.updates_cost_y >= s.updates_status_y.saturating_add(line),
                    "the Restart-cost line overlaps the status line at scale {scale}"
                );
                assert!(
                    s.shortcuts_y >= s.updates_cost_y.saturating_add(line),
                    "the shortcut list overlaps the Restart-cost line at scale {scale}"
                );
                // The Updates row leaves a full line above for its label, so
                // the label cannot land on the theme swatches.
                let swatches = s.theme[0];
                assert!(
                    s.update[0].y
                        >= swatches
                            .y
                            .saturating_add(swatches.height)
                            .saturating_add(s.update[0].height),
                    "the Updates label has no room above its row at scale {scale}"
                );
                // A window too short for even the fixed sections truncates the
                // list to nothing and clamps the panel; that is honest, so the
                // containment claims below only apply when the sections fit.
                if s.updates_cost_y.saturating_add(line) > layout.composer.y {
                    assert_eq!(
                        s.shortcut_lines, 0,
                        "short settings must reserve fixed sections before shortcut help"
                    );
                    assert_eq!(
                        bottom, layout.composer.y,
                        "short settings must remain bounded"
                    );
                    continue;
                }
                for rect in s.update {
                    assert!(
                        rect.y.saturating_add(rect.height) <= bottom,
                        "an Updates control falls outside the panel at scale {scale}"
                    );
                }
                assert!(
                    s.updates_status_y.saturating_add(line) <= bottom,
                    "the Updates status line falls outside the panel at scale {scale}"
                );
                assert!(
                    s.updates_cost_y.saturating_add(line) <= bottom,
                    "the Restart-cost line falls outside the panel at scale {scale}"
                );
                for rect in s.language {
                    assert!(rect.y.saturating_add(rect.height) <= bottom);
                }
            }
        }
    }

    /// The history panel's offset counts back from the newest entry, so zero is
    /// the top of the list and the maximum leaves the oldest entry on the last
    /// visible row. A list shorter than its capacity has nowhere to scroll, and
    /// the ends are floors rather than loops: a list that jumped would read as
    /// entries that went missing.
    #[test]
    fn the_history_scroll_stops_at_the_ends_and_never_lists_past_the_start() {
        // Twelve entries in a five-row panel: seven rows of travel.
        assert_eq!(clamp_history_scroll(0, 3, 12, 5), 3);
        assert_eq!(clamp_history_scroll(3, -1, 12, 5), 2);
        assert_eq!(
            clamp_history_scroll(0, -1, 12, 5),
            0,
            "the newest is a floor"
        );
        assert_eq!(
            clamp_history_scroll(7, 4, 12, 5),
            7,
            "the oldest is a floor"
        );
        assert_eq!(
            clamp_history_scroll(99, 0, 12, 5),
            7,
            "a stale offset is clamped"
        );
        // A list that fits has no travel at all.
        assert_eq!(clamp_history_scroll(0, 5, 3, 5), 0);
        assert_eq!(
            clamp_history_scroll(0, 5, 0, 5),
            0,
            "an empty list cannot scroll"
        );
    }

    #[test]
    fn harness_panel_and_close_remain_bounded_and_do_not_share_route_hits() {
        for scale in [1.0, 1.25, 2.0] {
            for width in [300, 420, 640, 1000] {
                let layout = Layout::new(dip(f64::from(width), scale), dip(360.0, scale), scale);
                let form = layout.harness_settings(scale, 100, 0);
                assert_eq!(form.panel.width, dip(f64::from(width.min(760)), scale));
                for control in form
                    .routes
                    .into_iter()
                    .chain([form.close, form.previous, form.next])
                {
                    assert!(control.width > 0 && control.height > 0);
                    assert!(control.x >= form.panel.x && control.y >= form.panel.y);
                    assert!(control.x + control.width <= form.panel.x + form.panel.width);
                    assert!(control.y + control.height <= form.panel.y + form.panel.height);
                }
                for route in form.routes {
                    assert!(
                        route.x + route.width <= form.close.x,
                        "one header press cannot both change route and close the form"
                    );
                }
            }
        }
    }

    #[test]
    fn settings_panel_hit_maps_language_font_and_theme_controls() {
        let layout = Layout::new(1200, 800, 1.0);
        let s = layout.settings_panel(1.0);
        assert_eq!(
            settings_hit(layout, s.language[1].x + 1, s.language[1].y + 1, 1.0),
            SettingsHit::Language(UiLanguage::ChineseSimplified)
        );
        assert_eq!(
            settings_hit(layout, s.font_up.x + 1, s.font_up.y + 1, 1.0),
            SettingsHit::FontUp
        );
        assert_eq!(
            settings_hit(layout, s.theme[3].x + 1, s.theme[3].y + 1, 1.0),
            SettingsHit::Theme(3)
        );
        // A point in the header above the panel is Outside the panel.
        assert_eq!(
            settings_hit(layout, layout.settings.x + 1, layout.settings.y + 1, 1.0),
            SettingsHit::Outside
        );
    }

    /// Every entry is labelled in the language it selects, and every language
    /// has a complete set of host UI strings. A struct rather than a map is
    /// what makes the second half a compile error instead of a blank label.
    #[test]
    fn each_language_labels_itself_and_translates_every_string() {
        for language in [
            UiLanguage::English,
            UiLanguage::ChineseSimplified,
            UiLanguage::ChineseTraditional,
        ] {
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
        assert_eq!(UiLanguage::ChineseSimplified.entry_label(), "中");
        assert_eq!(UiLanguage::ChineseTraditional.entry_label(), "繁");
        assert_eq!(UiLanguage::English.entry_label(), "En");
        assert_eq!(UiLanguage::English.strings().send, "Send");
        assert_eq!(UiLanguage::English.strings().send_hint, "(ctrl-o)");
        assert_eq!(UiLanguage::English.strings().newline, "New Line");
        assert_eq!(UiLanguage::English.strings().newline_hint, "(Enter)");
        assert_ne!(
            UiLanguage::English.strings().send,
            UiLanguage::ChineseSimplified.strings().send,
            "a language that translates nothing is a switch with no effect"
        );
        // The tab identifier travels with the label, so a translated prefix
        // must still leave the `@` that names the tab.
        assert!(
            UiLanguage::ChineseSimplified
                .strings()
                .send_to
                .ends_with('@')
        );
        assert!(
            UiLanguage::ChineseTraditional
                .strings()
                .send_to
                .ends_with('@')
        );
        assert!(UiLanguage::English.strings().send_to.ends_with('@'));
    }

    #[test]
    fn english_is_the_default_language() {
        assert_eq!(UiLanguage::default(), UiLanguage::English);
        assert_eq!(UiLanguage::default().tag(), "en");
    }

    /// The collapsed rail is one icon wide: the row buttons then cover most of
    /// a row, so a click meant to select a tab acted on a button instead.
    /// Collapsed, every row selects; the buttons come back with the rail.
    #[test]
    fn a_collapsed_rail_never_closes_a_tab_by_a_row_click() {
        let layout = Layout::collapsed(400, 300, 1.0);
        assert!(layout.sidebar_collapsed);
        let close = layout.tree_close_rect(0, 1.0);
        assert_eq!(
            tree_hit(layout, close.x + 1, close.y + 1, 0, 4, 1.0),
            TreeHit::Select(0),
            "a collapsed row selects wherever it is clicked"
        );
        let edit = layout.tree_edit_rect(0, 1.0);
        assert_eq!(
            tree_hit(layout, edit.x + 1, edit.y + 1, 0, 4, 1.0),
            TreeHit::Select(0),
            "the edit box is inert on a collapsed rail too"
        );
        let child = layout.tree_child_rect(0, 1.0);
        assert_eq!(
            tree_hit(layout, child.x + 1, child.y + 1, 0, 4, 1.0),
            TreeHit::Select(0),
            "the new-child box is inert on a collapsed rail too"
        );

        let expanded = Layout::new(400, 300, 1.0);
        assert!(!expanded.sidebar_collapsed);
        let close = expanded.tree_close_rect(0, 1.0);
        assert_eq!(
            tree_hit(expanded, close.x + 1, close.y + 1, 0, 4, 1.0),
            TreeHit::Close(0),
            "an expanded rail keeps its close box"
        );
        let edit = expanded.tree_edit_rect(0, 1.0);
        assert_eq!(
            tree_hit(expanded, edit.x + 1, edit.y + 1, 0, 4, 1.0),
            TreeHit::Edit(0),
            "and gains an edit box"
        );
        let child = expanded.tree_child_rect(0, 1.0);
        assert_eq!(
            tree_hit(expanded, child.x + 1, child.y + 1, 0, 4, 1.0),
            TreeHit::Child(0),
            "and a new-child box between edit and close"
        );
        // The trio must stay edit | new-child | close from left to right,
        // with no overlap, or a click intended for one acts as another.
        let close = expanded.tree_close_rect(0, 1.0);
        assert!(edit.x + edit.width < child.x, "edit sits left of new-child");
        assert!(
            child.x + child.width < close.x,
            "new-child sits left of close"
        );
    }

    #[test]
    fn tree_hit_distinguishes_rows_close_buttons_and_background() {
        let layout = Layout::new(1000, 500, 1.0);
        assert_eq!(tree_hit(layout, 220, 10, 0, 4, 1.0), TreeHit::Background);
        assert_eq!(
            tree_hit(
                layout,
                layout.settings.x + 1,
                layout.settings.y + 1,
                0,
                4,
                1.0
            ),
            TreeHit::Settings
        );
        assert_eq!(
            tree_hit(layout, 20, layout.home.y, 1, 4, 1.0),
            TreeHit::Home
        );
        assert_eq!(
            tree_hit(layout, 20, layout.tree_header_height, 1, 4, 1.0),
            TreeHit::Select(1)
        );
        let close = layout.tree_close_rect(0, 1.0);
        assert_eq!(
            tree_hit(layout, close.x + 1, close.y + 1, 1, 4, 1.0),
            TreeHit::Close(1)
        );
        let edit = layout.tree_edit_rect(0, 1.0);
        assert_eq!(
            tree_hit(layout, edit.x + 1, edit.y + 1, 1, 4, 1.0),
            TreeHit::Edit(1)
        );
        assert_eq!(tree_hit(layout, 20, 490, 0, 1, 1.0), TreeHit::Background);
        assert_eq!(tree_hit(layout, 500, 40, 0, 4, 1.0), TreeHit::Outside);
    }

    /// The edit box sits entirely to the left of the close box, at the same
    /// vertical band, and the two never overlap — otherwise a click could mean
    /// two different things depending on which box is tested first.
    #[test]
    fn the_edit_box_is_left_of_the_close_box_and_does_not_overlap_it() {
        let layout = Layout::new(1000, 500, 1.0);
        for row in 0..4 {
            let edit = layout.tree_edit_rect(row, 1.0);
            let close = layout.tree_close_rect(row, 1.0);
            assert_eq!(edit.y, close.y, "row {row}: same vertical band");
            assert_eq!(edit.height, close.height, "row {row}: same height");
            assert!(
                edit.x.saturating_add(edit.width) <= close.x,
                "row {row}: the edit box must not reach into the close box"
            );
        }
    }

    #[test]
    fn h0_robot_settings_pair_is_horizontal_readable_and_hittable() {
        for scale in [1.0, 1.5, 2.0] {
            for collapsed in [false, true] {
                let layout = if collapsed {
                    Layout::collapsed((540.0 * scale) as u32, (700.0 * scale) as u32, scale)
                } else {
                    Layout::new((540.0 * scale) as u32, (700.0 * scale) as u32, scale)
                };
                assert_eq!(layout.harness.y, layout.settings.y);
                assert!(layout.harness.width >= (32.0 * scale) as u32);
                assert!(layout.settings.width >= (32.0 * scale) as u32);
                assert!(
                    layout.harness.x + layout.harness.width + (4.0 * scale) as u32
                        <= layout.settings.x
                );
                assert!(layout.settings.x + layout.settings.width <= layout.sidebar.width);
                assert_eq!(
                    tree_hit(layout, layout.harness.x, layout.harness.y, 0, 4, scale),
                    TreeHit::Harness
                );
                assert!(layout.tree_header_height >= layout.harness.y + layout.harness.height);
            }
        }
    }

    /// The header tools each own their hit before the tree rows are considered,
    /// including the ones the first test did not probe. A click on any of them
    /// must never fall through to a row underneath.
    #[test]
    fn every_header_tool_wins_over_the_rows_beneath_it() {
        let layout = Layout::new(1000, 500, 1.0);
        for (rect, expected) in [
            (layout.harness, TreeHit::Harness),
            (layout.new_root, TreeHit::NewRoot),
            (layout.settings, TreeHit::Settings),
            (layout.sidebar_toggle, TreeHit::SidebarToggle),
            (layout.panel_toggle, TreeHit::PanelToggle),
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
        // The fixed Home row occupies the space immediately above real tabs.
        assert_eq!(tree_hit(layout, 20, header - 1, 0, 8, 1.0), TreeHit::Home);
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
        let cramped = Layout::with_sidebar(1000, layout.tree_header_height, 1.0, 224.0, false);
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
    fn status_hit_covers_the_bar_and_nothing_above_it() {
        let layout = Layout::new(1200, 800, 1.0);
        let bar = layout.status;
        assert_eq!(
            status_hit(layout, bar.x + bar.width / 2, bar.y + bar.height / 2),
            StatusHit::Bar
        );
        // A point one row above the bar (in the composer) is not the bar.
        assert_eq!(
            status_hit(layout, bar.x + 1, bar.y.saturating_sub(1)),
            StatusHit::Outside
        );
        // A point in the sidebar column is left of the bar's terminal-side span.
        assert_eq!(
            status_hit(layout, 0, bar.y + bar.height / 2),
            StatusHit::Outside
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

    #[test]
    fn composer_scrollbar_has_its_own_hit_area_and_tracks_draft_lines() {
        let layout = Layout::new(1000, 500, 1.0);
        let area = layout.composer_scrollbar;
        assert_eq!(area.width, COMPOSER_SCROLLBAR_WIDTH_DIP as u32);
        assert_eq!(
            composer_hit(layout, area.x + 2, area.y + 2),
            ComposerHit::Scrollbar
        );
        let bottom = composer_scrollbar_geometry(layout, 4, 16, 20);
        let top = composer_scrollbar_geometry(layout, 4, 0, 20);
        assert_eq!(bottom.thumb.bottom, bottom.track.bottom);
        assert_eq!(top.thumb.top, top.track.top);
        assert!(bottom.thumb.height() < bottom.track.height());
        let short = composer_scrollbar_geometry(layout, 4, 0, 2);
        assert_eq!(short.thumb.top, short.track.top);
        assert_eq!(short.thumb.bottom, short.track.bottom);
    }

    #[test]
    fn composer_clipboard_buttons_hit_test_and_hide_when_narrow() {
        // A comfortably wide composer shows the Copy / Paste / Cut column, each
        // a distinct hit inside the composer strip.
        let layout = Layout::new(1200, 800, 1.0);
        assert!(layout.composer_copy.width > 0);
        for (rect, expected) in [
            (layout.composer_copy, ComposerHit::Copy),
            (layout.composer_paste, ComposerHit::Paste),
            (layout.composer_cut, ComposerHit::Cut),
        ] {
            assert_eq!(composer_hit(layout, rect.x + 1, rect.y + 1), expected);
        }
        // The buttons sit left of the Send column, not overlapping it.
        assert!(layout.composer_cut.x + layout.composer_cut.width <= layout.composer_send.x);

        // A narrow composer drops the column (zero-width) so the input keeps a
        // usable width; a click where the column would be is plain Input.
        let narrow = Layout::new(480, 500, 1.0);
        assert_eq!(narrow.composer_copy.width, 0);
        assert_eq!(
            composer_hit(
                narrow,
                narrow.composer_input.x + 1,
                narrow.composer_input.y + 1
            ),
            ComposerHit::Input
        );
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
                right_inset: 0,
                scale: 1.0,
                rows: 24,
            },
            0,
            100,
        );
        assert_eq!(
            (g.track.left, g.track.right, g.track.bottom),
            (1182, 1200, 704)
        );
    }

    #[test]
    fn a_closed_history_panel_leaves_the_terminal_its_column() {
        let closed = Layout::with_chrome(1200, 800, 1.0, SIDEBAR_WIDTH_DIP, false, false, 260.0);
        assert!(!closed.panel_open);
        assert_eq!(closed.panel.width, 0, "a closed panel takes no column");

        let open = Layout::with_chrome(1200, 800, 1.0, SIDEBAR_WIDTH_DIP, false, true, 260.0);
        assert!(open.panel_open);
        assert!(open.panel.width > 0);
        // The composer and status bands stop at the panel's left edge; neither
        // slides under it, and the panel reaches the window's right edge.
        assert_eq!(open.composer.x + open.composer.width, open.panel.x);
        assert_eq!(open.status.x + open.status.width, open.panel.x);
        assert_eq!(open.panel.x + open.panel.width, 1200);
        assert!(open.composer.x < open.panel.x);
    }

    #[test]
    fn the_open_history_panel_never_covers_the_bottom_bands() {
        for &(w, h, scale) in LAYOUT_SWEEP {
            let l = Layout::with_chrome(
                w,
                h,
                scale,
                SIDEBAR_WIDTH_DIP,
                false,
                true,
                HISTORY_PANEL_WIDTH_DIP,
            );
            assert!(l.panel_open, "{w}x{h}@{scale} keeps a usable panel");
            assert!(
                l.composer.x + l.composer.width <= l.panel.x,
                "composer under the panel at {w}x{h}@{scale}"
            );
            assert!(
                l.status.x + l.status.width <= l.panel.x,
                "status under the panel at {w}x{h}@{scale}"
            );
            // The list always leaves room for at least one row above the
            // reserved area, so the panel is never all padding.
            let p = l.history_panel(scale);
            assert!(p.row_capacity >= 1, "{w}x{h}@{scale}");
            assert!(p.reserved_top > p.list_top, "{w}x{h}@{scale}");
        }
    }

    #[test]
    fn history_hit_tells_rows_close_and_background_apart() {
        let l = Layout::with_chrome(1200, 800, 1.0, SIDEBAR_WIDTH_DIP, false, true, 260.0);
        let p = l.history_panel(1.0);
        assert_eq!(
            history_hit(l, p.close.x + 1, p.close.y + 1, 1.0),
            HistoryHit::Close
        );
        let x = p.panel.x + p.panel.width / 2;
        assert_eq!(history_hit(l, x, p.list_top + 1, 1.0), HistoryHit::Row(0));
        assert_eq!(
            history_hit(l, x, p.list_top + p.row_height + 1, 1.0),
            HistoryHit::Row(1)
        );
        // Below the list, the notes scratchpad band answers Notes (its
        // click focuses the editor), not Panel and not a row.
        assert_eq!(
            history_hit(l, x, p.reserved_top + 1, 1.0),
            HistoryHit::Notes
        );
        // With the panel closed there is nothing to hit.
        let closed = Layout::with_chrome(1200, 800, 1.0, SIDEBAR_WIDTH_DIP, false, false, 260.0);
        assert_eq!(
            history_hit(closed, x, p.list_top + 1, 1.0),
            HistoryHit::Outside
        );
    }

    fn viewport(scale: f64, rows: usize, bottom_inset: u32) -> TerminalViewport {
        TerminalViewport {
            width: 1200,
            height: 800,
            left: 224,
            top: 0,
            bottom_inset,
            right_inset: 0,
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
        let layout = Layout::with_sidebar(u32::MAX, u32::MAX, f64::INFINITY, f64::NAN, false);
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
        use minicon_ui_core::scrollback_for_thumb_top;
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
        use minicon_ui_core::{ScrollbarGeometry, scrollback_for_thumb_top};
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
        use minicon_ui_core::scrollback_for_thumb_top;
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

    /// `{RECOVER}`: every failure sentence has to answer three questions --
    /// what failed, whether the install still runs, and what to do next -- and
    /// has to be in the language the panel is set to. The old panel painted the
    /// worker's raw English cause, so a Chinese UI showed English and said
    /// nothing about whether the product still started.
    #[test]
    fn a_failure_sentence_answers_all_three_questions_in_the_panels_language() {
        use crate::update::{Failure, FailureKind};

        let untouched = UiLanguage::English
            .update_failure_sentence(&Failure::new(FailureKind::Checksum, "raw cause"));
        assert!(untouched.contains("checksum"), "{untouched}");
        assert!(
            untouched.contains("was not changed"),
            "the user must be told the install is intact: {untouched}"
        );
        assert!(untouched.contains("try again"), "{untouched}");
        assert!(
            !untouched.contains("raw cause"),
            "the diagnostic is not the panel sentence: {untouched}"
        );

        // A failed launch is the state where the backup is the recovery net, so
        // the sentence has to name the file it is kept as.
        let restored = UiLanguage::English
            .update_failure_sentence(&Failure::new(FailureKind::Launch, "raw cause"));
        assert!(restored.contains("would not start"), "{restored}");
        assert!(
            restored.contains(".backup"),
            "the recovery has to name the file: {restored}"
        );

        let simplified = UiLanguage::ChineseSimplified
            .update_failure_sentence(&Failure::new(FailureKind::Launch, "raw cause"));
        assert!(simplified.contains("无法启动"), "{simplified}");
        assert!(simplified.contains("已放回"), "{simplified}");
        assert!(
            !simplified.contains("would not start"),
            "a Chinese panel must not show the English sentence: {simplified}"
        );

        let traditional = UiLanguage::ChineseTraditional
            .update_failure_sentence(&Failure::new(FailureKind::LaunchUnrestored, "raw cause"));
        assert!(traditional.contains("無法啟動"), "{traditional}");
        assert!(
            traditional.contains("重新安裝"),
            "an unrestored install is the one that needs a reinstall: {traditional}"
        );
    }

    /// The status line is one owner for both readers: the panel paints it and
    /// `ui-snapshot` publishes it, so the two can never disagree.
    #[test]
    fn the_status_line_covers_every_phase_including_a_failure() {
        use crate::update::{Failure, FailureKind, Phase, State, Version};

        let current = Version::parse("0.2.8").unwrap();
        let mut state = State::new(current);
        assert_eq!(UiLanguage::English.update_status(&state), "v0.2.8");

        state.phase = Phase::Checking;
        assert_eq!(
            UiLanguage::English.update_status(&state),
            UiLanguage::English.strings().update_checking
        );

        state.phase = Phase::Failed(Failure::new(FailureKind::Transport, "raw cause"));
        let failed = UiLanguage::ChineseSimplified.update_status(&state);
        assert!(
            failed.contains("下载未能完成") && failed.contains("当前安装未受影响"),
            "the status is the failure sentence, in the panel's language: {failed}"
        );
    }

    /// A stored language id must name exactly the language it was written from,
    /// and an id this build does not ship must resolve to nothing rather than
    /// to some other language. This is the invariant that keeps the tag table
    /// and the `from_tag` table from drifting apart.
    #[test]
    fn from_tag_is_the_inverse_of_each_languages_tag() {
        for language in [
            UiLanguage::English,
            UiLanguage::ChineseSimplified,
            UiLanguage::ChineseTraditional,
        ] {
            assert_eq!(UiLanguage::from_tag(language.tag()), Some(language));
        }
        for unknown in ["", "EN", "en-US", "zh", "zh-hans ", "fr"] {
            assert_eq!(UiLanguage::from_tag(unknown), None, "accepted {unknown:?}");
        }
    }

    /// The Restart sentence names the real count and never a count that cannot
    /// happen. Zero tabs is the case a person would read as a bug ("closes 0
    /// tabs"), one tab is the case they actually meet, and both must differ
    /// from the general form. Every language covers all three:
    /// a language that dropped the zero arm would read as a machine.
    #[test]
    fn the_restart_sentence_states_the_real_count_and_never_zero_tabs() {
        for language in [
            UiLanguage::English,
            UiLanguage::ChineseSimplified,
            UiLanguage::ChineseTraditional,
        ] {
            let none = language.restart_cost(0);
            let one = language.restart_cost(1);
            let many = language.restart_cost(7);
            assert!(
                !none.contains('0'),
                "a closed window must not say \"0 tabs\": {none}"
            );
            assert!(one.contains('1'), "one tab names its count: {one}");
            assert!(many.contains('7'), "N tabs names its count: {many}");
            assert_ne!(none, one);
            assert_ne!(one, many);
            assert_ne!(none, many);
        }
        assert!(
            UiLanguage::English
                .restart_cost(3)
                .contains("running commands"),
            "the sentence says a running command is stopped"
        );
        assert!(
            UiLanguage::ChineseSimplified
                .restart_cost(3)
                .contains("正在运行"),
            "the Chinese sentence says the same thing"
        );
    }
}
