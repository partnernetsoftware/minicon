//! Read-only drawing-grid projection; no second authoritative terminal model.
use super::{CellRect, Screen, MODE_ALTERNATE_SCREEN, MODE_HIDE_CURSOR};

impl Screen {
    /// Build a transient, single-grid render snapshot of a signed half-open
    /// region, padded/clipped to the chosen output size. Source cells, parser
    /// grammar and history are untouched. Cursor position is translated;
    /// an out-of-view cursor is hidden. Clipped wide neighbours are healed.
    /// The caller owns the allocation budget, as with Parser construction.
    /// Invalid/empty regions or zero output dimensions return None.
    #[must_use]
    #[allow(clippy::missing_panics_doc)] // Indices are bounded by allocated u16 grids.
    pub fn project_region(&self, area: CellRect, rows: u16, columns: u16) -> Option<Self> {
        let height = i64::from(area.bottom) - i64::from(area.top);
        let width = i64::from(area.right) - i64::from(area.left);
        if rows == 0 || columns == 0 || height <= 0 || width <= 0 {
            return None;
        }
        let mut result = Self::new(
            crate::grid::Size {
                rows,
                cols: columns,
            },
            0,
        );
        result.attrs = self.attrs;
        result.modes = self.modes & !MODE_ALTERNATE_SCREEN;
        result.cursor_shape = self.cursor_shape;
        result.cursor_blinking = self.cursor_blinking;
        result.mouse_protocol_mode = self.mouse_protocol_mode;
        result.mouse_protocol_encoding = self.mouse_protocol_encoding;
        let (source_rows, source_columns) = self.size();
        for row in 0..rows {
            let source_row = i64::from(area.top) + i64::from(row);
            if i64::from(row) >= height || source_row < 0 || source_row >= i64::from(source_rows) {
                continue;
            }
            let source = self
                .grid()
                .drawing_rows()
                .nth(usize::try_from(source_row).unwrap())
                .unwrap();
            let target = result.grid_mut().drawing_row_mut(row).unwrap();
            for column in 0..columns {
                let source_column = i64::from(area.left) + i64::from(column);
                if i64::from(column) < width
                    && source_column >= 0
                    && source_column < i64::from(source_columns)
                {
                    *target.get_mut(column).unwrap() = source
                        .get(u16::try_from(source_column).unwrap())
                        .unwrap()
                        .clone();
                }
            }
            target.repair_wide_pairs();
            target.wrap(
                area.left == 0
                    && area.right == i32::from(source_columns)
                    && columns == source_columns
                    && source.wrapped(),
            );
        }
        let (row, column) = self.cursor_position();
        let row = i64::from(row) - i64::from(area.top);
        let mut column = i64::from(column) - i64::from(area.left);
        if column == width && area.right == i32::from(source_columns) && width < i64::from(columns)
        {
            column = width - 1;
        }
        let delayed = column == i64::from(columns)
            && area.right == i32::from(source_columns)
            && width == i64::from(columns);
        let visible = row >= 0
            && row < i64::from(rows)
            && row < height
            && column >= 0
            && (column < i64::from(columns) && column < width || delayed);
        if visible {
            if !result
                .set_cursor_position(u16::try_from(row).unwrap(), u16::try_from(column).unwrap())
            {
                return None;
            }
        } else {
            result.modes |= MODE_HIDE_CURSOR;
        }
        Some(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn viewport_regions_clip_wide_cells_and_preserve_authoritative_state() {
        let mut parser = crate::Parser::new(3, 8, 4);
        parser.process("A中e\u{301}😀Z\x1b[6 q\x1b[3;5H".as_bytes());
        let before = parser.screen().state_formatted();
        let projected = parser
            .screen()
            .project_region(
                CellRect {
                    top: 0,
                    left: 2,
                    bottom: 2,
                    right: 7,
                },
                2,
                4,
            )
            .unwrap();
        assert_eq!(projected.rows(0, 4).next().unwrap(), " e\u{301}😀");
        assert!(!projected.cell(0, 0).unwrap().is_wide_continuation());
        assert!(projected.cell(0, 3).unwrap().is_wide_continuation());
        assert!(projected.hide_cursor());
        assert_eq!(projected.cursor_shape(), super::super::CursorShape::Bar);
        assert!(!projected.cursor_blinking());
        let clipped = parser
            .screen()
            .project_region(
                CellRect {
                    top: 0,
                    left: 3,
                    bottom: 1,
                    right: 5,
                },
                1,
                2,
            )
            .unwrap();
        assert!(!clipped.cell(0, 1).unwrap().is_wide());
        assert!(!clipped.cell(0, 1).unwrap().has_contents());
        assert_eq!(parser.screen().state_formatted(), before);
        assert!(parser
            .screen()
            .project_region(
                CellRect {
                    top: 0,
                    left: 0,
                    bottom: 1,
                    right: 1
                },
                0,
                2
            )
            .is_none());
        parser.process(b"\x1b[?1049hALT");
        let alt = parser
            .screen()
            .project_region(
                CellRect {
                    top: 0,
                    left: 0,
                    bottom: 3,
                    right: 8,
                },
                3,
                8,
            )
            .unwrap();
        assert_eq!(alt.rows(0, 8).next().unwrap(), "ALT");
        assert!(!alt.alternate_screen());
        let mut small = crate::Parser::new(1, 1, 0);
        small.process(b"Z");
        let padded = small
            .screen()
            .project_region(
                CellRect {
                    top: 0,
                    left: 0,
                    bottom: 1,
                    right: 1,
                },
                2,
                4,
            )
            .unwrap();
        assert!(!padded.hide_cursor());
        assert_eq!(padded.cursor_position(), (0, 0));
    }
}
