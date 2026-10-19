//! Snapshot-based block moves in the active drawing grid.
use super::Screen;
use unicode_width::UnicodeWidthChar as _;

/// A signed, half-open rectangle of terminal cells.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct CellRect {
    /// Inclusive first row.
    pub top: i32,
    /// Inclusive first column.
    pub left: i32,
    /// Exclusive final row.
    pub bottom: i32,
    /// Exclusive final column.
    pub right: i32,
}

impl CellRect {
    fn intersect(self, other: Self) -> Self {
        Self {
            top: self.top.max(other.top),
            left: self.left.max(other.left),
            bottom: self.bottom.min(other.bottom),
            right: self.right.min(other.right),
        }
    }
    fn empty(self) -> bool {
        self.top >= self.bottom || self.left >= self.right
    }
    fn contains(self, row: i64, column: i64) -> bool {
        i64::from(self.top) <= row
            && row < i64::from(self.bottom)
            && i64::from(self.left) <= column
            && column < i64::from(self.right)
    }
}

/// A block move cannot represent its requested fill.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CellMoveError {
    /// The repeated fill scalar requires more than one cell.
    WideFill,
}

impl Screen {
    /// Move cells from a signed half-open rectangle to a new row/column origin.
    /// The destination maps from the original source origin even when source
    /// cells are outside the drawing grid. Clip limits writes, not source reads.
    /// Exposed source cells are filled; overlapping copies read a snapshot.
    /// Cursor, drawing attributes, decoder and scrollback are preserved.
    /// Broken wide neighbours are cleared to maintain cell-pair invariants.
    ///
    /// Returns whether any cells were assigned. Empty regions and zero motion
    /// are no-ops. This edits the active drawing grid, not scrollback history.
    ///
    /// # Errors
    /// Returns `CellMoveError::WideFill` without changing cells for a multi-cell fill scalar.
    // All indices below are bounded by the existing u16 drawing grid.
    #[allow(clippy::missing_panics_doc)]
    pub fn move_region(
        &mut self,
        source: CellRect,
        destination_row: i32,
        destination_column: i32,
        clip: Option<CellRect>,
        fill: char,
        style: crate::CellStyle,
    ) -> Result<bool, CellMoveError> {
        if fill.width().unwrap_or(1) > 1 {
            return Err(CellMoveError::WideFill);
        }
        let (rows, columns) = self.size();
        let bounds = CellRect {
            top: 0,
            left: 0,
            bottom: i32::from(rows),
            right: i32::from(columns),
        };
        let actual_source = source.intersect(bounds);
        let clip = clip.unwrap_or(bounds).intersect(bounds);
        let delta_row = i64::from(destination_row) - i64::from(source.top);
        let delta_column = i64::from(destination_column) - i64::from(source.left);
        if source.empty()
            || actual_source.empty()
            || clip.empty()
            || (delta_row == 0 && delta_column == 0)
        {
            return Ok(false);
        }
        // Allocate only visible source cells, never the offscreen area.
        let snapshot: Vec<Vec<crate::Cell>> = self
            .grid()
            .drawing_rows()
            .skip(usize::try_from(actual_source.top).unwrap())
            .take(usize::try_from(actual_source.bottom - actual_source.top).unwrap())
            .map(|row| {
                (actual_source.left..actual_source.right)
                    .filter_map(|column| row.get(u16::try_from(column).unwrap()).cloned())
                    .collect()
            })
            .collect();
        let mut filler = crate::Cell::new();
        filler.set(fill, style.into());
        let mut changed = crate::RowRange::empty();
        for row in clip.top..clip.bottom {
            let mut row_changed = false;
            let line = self
                .grid_mut()
                .drawing_row_mut(u16::try_from(row).unwrap())
                .unwrap();
            for column in clip.left..clip.right {
                let from_row = i64::from(row) - delta_row;
                let from_column = i64::from(column) - delta_column;
                let next = if actual_source.contains(from_row, from_column) {
                    Some(
                        snapshot[usize::try_from(from_row - i64::from(actual_source.top)).unwrap()]
                            [usize::try_from(from_column - i64::from(actual_source.left)).unwrap()]
                        .clone(),
                    )
                } else if actual_source.contains(i64::from(row), i64::from(column)) {
                    Some(filler.clone())
                } else {
                    None
                };
                if let Some(next) = next {
                    *line.get_mut(u16::try_from(column).unwrap()).unwrap() = next;
                    row_changed = true;
                }
            }
            if row_changed {
                line.repair_wide_pairs();
                line.wrap(false);
                changed.mark_row(u16::try_from(row).unwrap());
            }
        }
        let any = !changed.is_empty();
        if any {
            if self.scrollback() == 0 {
                self.damage.mark_rows(changed);
            } else {
                self.damage.mark_full();
            }
        }
        Ok(any)
    }
}

#[cfg(test)]
mod tests {
    use super::CellRect;

    #[test]
    fn region_moves_snapshot_overlap_preserve_graphemes_and_clip_writes() {
        let mut parser = crate::Parser::new(4, 8, 16);
        parser.process("A中e\u{301}😀Z\x1b[2;1H12345678\x1b[4;7H\x1b[31;44m".as_bytes());
        let before = parser.screen().clone();
        let source = CellRect {
            top: 0,
            left: 0,
            bottom: 1,
            right: 7,
        };
        assert_eq!(
            parser
                .screen_mut()
                .move_region(source, 2, 1, None, '.', crate::CellStyle::default()),
            Ok(true)
        );
        assert_eq!(parser.screen().cell(2, 2).unwrap().contents(), "中");
        assert!(parser.screen().cell(2, 3).unwrap().is_wide_continuation());
        assert_eq!(parser.screen().cell(2, 4).unwrap().contents(), "e\u{301}");
        assert_eq!(parser.screen().cell(2, 5).unwrap().contents(), "😀");
        assert_eq!(parser.screen().cursor_position(), before.cursor_position());
        assert_eq!(
            parser.screen().attributes_formatted(),
            before.attributes_formatted()
        );
        assert_eq!(parser.screen().rows(0, 8).next().unwrap(), ".......");
        assert_eq!(
            parser.screen_mut().move_region(
                CellRect {
                    top: 1,
                    left: 0,
                    bottom: 2,
                    right: 6
                },
                1,
                2,
                None,
                '.',
                crate::CellStyle::default()
            ),
            Ok(true)
        );
        assert_eq!(parser.screen().rows(0, 8).nth(1).unwrap(), "..123456");
        let clip = CellRect {
            top: 3,
            left: 2,
            bottom: 4,
            right: 4,
        };
        assert_eq!(
            parser.screen_mut().move_region(
                CellRect {
                    top: 1,
                    left: 2,
                    bottom: 2,
                    right: 8
                },
                3,
                2,
                Some(clip),
                '_',
                crate::CellStyle::default()
            ),
            Ok(true)
        );
        assert_eq!(parser.screen().cell(3, 2).unwrap().contents(), "1");
        assert_eq!(parser.screen().cell(3, 3).unwrap().contents(), "2");
        assert_eq!(parser.screen().cell(3, 4).unwrap().contents(), "");
        assert!(!parser.take_damage().scrollback_changed());
        parser.process(b"\x1b[3");
        assert_eq!(
            parser
                .screen_mut()
                .move_region(source, 0, 0, None, ' ', crate::CellStyle::default()),
            Ok(false)
        );
        parser.process(b"2mX");
        assert_eq!(parser.screen().fgcolor(), crate::Color::Idx(2));
    }
}
