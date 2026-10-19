//! Literal cell edits without interpreting or disturbing a parser's VT stream.
use super::Screen;
use unicode_width::UnicodeWidthChar as _;

impl Screen {
    /// Write literal scalars into consecutive drawing cells, preserving their
    /// styles, cursor, drawing attributes, history and parser grammar. Wide
    /// scalars pad the final column and continue on the next row. Invalid
    /// origins consume nothing; screen-end clipping returns consumed scalars,
    /// excluding padding. Overwritten wide neighbours are healed. Controls and
    /// standalone zero-width scalars use the literal-cell projection contract.
    #[allow(clippy::missing_panics_doc)] // Drawing indices fit the existing u16 grid.
    pub fn write_cell_text(&mut self, row: u16, column: u16, text: &str) -> usize {
        let (rows, columns) = self.size();
        if row >= rows || column >= columns {
            return 0;
        }
        let mut text = text.chars().peekable();
        let mut consumed = 0;
        let mut damage = crate::RowRange::empty();
        for (index, line) in self
            .grid_mut()
            .drawing_rows_mut()
            .enumerate()
            .skip(usize::from(row))
        {
            let mut at = if index == usize::from(row) { column } else { 0 };
            while at < columns {
                let Some(&character) = text.peek() else { break };
                let width = u16::from(character.width().unwrap_or(1) > 1) + 1;
                damage.mark_row(u16::try_from(index).unwrap());
                if width > columns - at {
                    line.fill_literal(at, columns, ' ');
                    break;
                }
                line.fill_literal(at, at + width, character);
                at += width;
                text.next();
                consumed += 1;
            }
            if text.peek().is_none() {
                break;
            }
        }
        self.mark_literal_damage(damage);
        consumed
    }

    /// Write consecutive cell styles while preserving text and wide metadata.
    /// Clip at the screen end and return the number of styles consumed.
    /// Cursor, drawing attributes, grammar and history are preserved.
    #[allow(clippy::missing_panics_doc)] // Drawing indices fit the existing u16 grid.
    pub fn write_cell_styles(
        &mut self,
        row: u16,
        column: u16,
        styles: impl IntoIterator<Item = crate::CellStyle>,
    ) -> usize {
        let (rows, columns) = self.size();
        if row >= rows || column >= columns {
            return 0;
        }
        let mut styles = styles.into_iter();
        let mut consumed = 0;
        let mut damage = crate::RowRange::empty();
        'rows: for (index, line) in self
            .grid_mut()
            .drawing_rows_mut()
            .enumerate()
            .skip(usize::from(row))
        {
            let begin = if index == usize::from(row) { column } else { 0 };
            for at in begin..columns {
                let Some(style) = styles.next() else {
                    break 'rows;
                };
                line.fill_style(at, at + 1, style);
                damage.mark_row(u16::try_from(index).unwrap());
                consumed += 1;
            }
        }
        self.mark_literal_damage(damage);
        consumed
    }

    fn mark_literal_damage(&mut self, damage: crate::RowRange) {
        if !damage.is_empty() {
            if self.scrollback() == 0 {
                self.damage.mark_rows(damage);
            } else {
                self.damage.mark_full();
            }
        }
    }

    /// Replace one scalar and its style without moving the cursor or drawing
    /// state. A wide scalar requires two cells in this row; invalid placement
    /// returns false without changing either row. Controls and standalone
    /// zero-width scalars retain the literal-cell projection contract.
    pub fn write_cell(
        &mut self,
        row: u16,
        column: u16,
        character: char,
        style: crate::CellStyle,
    ) -> bool {
        let (rows, columns) = self.size();
        let width = u16::from(character.width().unwrap_or(1) > 1) + 1;
        if row >= rows || column >= columns || width > columns - column {
            return false;
        }
        self.fill_cell_text(row, column, 1, character);
        self.fill_cell_style(row, column, usize::from(width), style);
        true
    }

    /// Fill consecutive cells with a style, preserving text, width metadata,
    /// cursor and current drawing attributes. Clip at the screen end.
    /// Returns the number of cells changed; invalid origins return zero.
    pub fn fill_cell_style(
        &mut self,
        row: u16,
        column: u16,
        count: usize,
        style: crate::CellStyle,
    ) -> usize {
        self.fill_region(row, column, count, None, style)
    }

    /// Fill repeated literal characters, preserving each cell's attributes,
    /// cursor and current drawing state. Heal overwritten wide neighbours.
    /// Wide characters occupy pairs; an unplaceable final column is padded
    /// without consuming a repetition. Returns consumed repetitions, excluding
    /// padding. Controls and zero-width characters remain literal one-cell
    /// data; formatted VT output projects them safely without executing them.
    pub fn fill_cell_text(
        &mut self,
        row: u16,
        column: u16,
        count: usize,
        character: char,
    ) -> usize {
        self.fill_region(
            row,
            column,
            count,
            Some(character),
            crate::CellStyle::default(),
        )
    }

    fn fill_region(
        &mut self,
        row: u16,
        column: u16,
        count: usize,
        character: Option<char>,
        style: crate::CellStyle,
    ) -> usize {
        let (rows, columns) = self.size();
        if row >= rows || column >= columns || count == 0 {
            return 0;
        }
        let width = character.map_or(1, |c| c.width().unwrap_or(1).max(1));
        let mut remaining = count;
        let mut damage = crate::RowRange::empty();
        for (index, line) in self
            .grid_mut()
            .drawing_rows_mut()
            .enumerate()
            .skip(usize::from(row))
        {
            let begin = if index == usize::from(row) { column } else { 0 };
            let copies = remaining.min(usize::from(columns - begin) / width);
            let end = begin + u16::try_from(copies * width).unwrap();
            if let Some(character) = character {
                line.fill_literal(begin, end, character);
            } else {
                line.fill_style(begin, end, style);
            }
            remaining -= copies;
            if width == 2 && remaining > 0 && end == columns - 1 {
                line.fill_literal(end, columns, ' ');
            }
            damage.mark_row(u16::try_from(index).unwrap());
            if remaining == 0 {
                break;
            }
        }
        self.damage.mark_rows(damage);
        count - remaining
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn literal_spans_preserve_attributes_history_and_partial_grammar() {
        let mut parser = crate::Parser::new(3, 5, 8);
        parser.process(b"old\r\nkeep\r\nABCDE\r\nvwxyz\x1b[31;44m\x1b[3");
        let before = parser.screen().clone();
        parser.screen_mut().set_scrollback(1);
        parser.take_damage();
        let styles = (2..6).map(|n| crate::CellStyle {
            foreground: crate::Color::Idx(n),
            ..crate::CellStyle::default()
        });
        assert_eq!(parser.screen_mut().write_cell_styles(1, 3, styles), 4);
        assert_eq!(parser.screen_mut().write_cell_text(1, 4, "中XR"), 3);
        let damage = parser.take_damage();
        assert!(damage.needs_full_raster());
        assert!(!damage.scrollback_changed());
        assert_eq!(parser.screen().scrollback_len(), before.scrollback_len());
        assert_eq!(parser.screen().cursor_position(), before.cursor_position());
        assert_eq!(
            parser.screen().attributes_formatted(),
            before.attributes_formatted()
        );
        parser.screen_mut().set_scrollback(0);
        assert_eq!(parser.screen().rows(0, 5).nth(1).unwrap(), "ABCD ");
        assert_eq!(parser.screen().rows(0, 5).nth(2).unwrap(), "中XRz");
        assert_eq!(
            parser.screen().cell(2, 0).unwrap().fgcolor(),
            crate::Color::Idx(4)
        );
        assert_eq!(
            parser.screen().cell(2, 1).unwrap().fgcolor(),
            crate::Color::Idx(5)
        );
        assert!(parser.screen().cell(2, 1).unwrap().is_wide_continuation());
        assert_eq!(parser.screen_mut().write_cell_text(2, 1, "Q"), 1);
        assert!(!parser.screen().cell(2, 0).unwrap().is_wide());
        assert_eq!(parser.screen().cell(2, 1).unwrap().contents(), "Q");
        assert_eq!(parser.screen_mut().write_cell_text(2, 4, "😀"), 0);
        assert_eq!(parser.screen().cell(2, 4).unwrap().contents(), " ");
        assert_eq!(parser.screen_mut().write_cell_text(3, 0, "X"), 0);
        parser.process(b"2m");
        assert_eq!(parser.screen().fgcolor(), crate::Color::Idx(2));
    }

    #[test]
    fn rectangular_cell_writes_preserve_state_and_refuse_row_spill() {
        let mut parser = crate::Parser::new(2, 4, 0);
        parser.process(b"\x1b[31mAB\x1b[2;2H");
        let before = parser.screen().clone();
        let style = crate::CellStyle {
            foreground: crate::Color::Idx(2),
            ..crate::CellStyle::default()
        };
        assert!(!parser.screen_mut().write_cell(0, 3, '中', style));
        assert_eq!(parser.screen().state_formatted(), before.state_formatted());
        assert!(parser.screen_mut().write_cell(0, 1, '中', style));
        assert!(parser.screen().cell(0, 2).unwrap().is_wide_continuation());
        assert_eq!(
            parser.screen().cell(0, 1).unwrap().fgcolor(),
            crate::Color::Idx(2)
        );
        assert!(parser.screen_mut().write_cell(0, 2, 'X', style));
        assert!(!parser.screen().cell(0, 1).unwrap().is_wide());
        assert_eq!(parser.screen().cell(0, 2).unwrap().contents(), "X");
        assert_eq!(parser.screen().cursor_position(), before.cursor_position());
        assert_eq!(
            parser.screen().attributes_formatted(),
            before.attributes_formatted()
        );
        assert!(parser.take_damage().rows().contains(0));
    }

    #[test]
    fn literal_cell_edits_preserve_state_and_project_safe_full_and_diff() {
        let mut parser = crate::Parser::new(3, 6, 0);
        parser.process(b"\x1b[31;44;4mabcdef\x1b[3;3H");
        let before = parser.screen().clone();
        parser.screen_mut().take_damage();
        assert_eq!(
            parser.screen_mut().fill_cell_style(
                0,
                2,
                3,
                crate::CellStyle {
                    foreground: crate::Color::Idx(2),
                    inverse: true,
                    ..crate::CellStyle::default()
                }
            ),
            3
        );
        assert_eq!(parser.screen().cell(0, 2).unwrap().contents(), "c");
        assert_eq!(parser.screen_mut().fill_cell_text(0, 3, 2, '\x1b'), 2);
        assert_eq!(parser.screen_mut().fill_cell_text(1, 0, 1, '\0'), 1);
        assert_eq!(parser.screen_mut().fill_cell_text(1, 1, 1, '\u{301}'), 1);
        assert_eq!(parser.screen_mut().fill_cell_text(1, 2, 1, '\u{fffd}'), 1);
        assert_eq!(parser.screen().cell(0, 3).unwrap().contents(), "\x1b");
        assert_eq!(
            parser.screen().cell(0, 3).unwrap().fgcolor(),
            crate::Color::Idx(2)
        );
        assert_eq!(parser.screen().cursor_position(), before.cursor_position());
        assert_eq!(
            parser.screen().attributes_formatted(),
            before.attributes_formatted()
        );
        let damage = parser.screen_mut().take_damage();
        assert!(damage.rows().contains(0) && damage.rows().contains(1));
        for projection in [
            parser.screen().state_formatted(),
            parser.screen().state_diff(&before),
        ] {
            let mut replay = crate::Parser::new(3, 6, 0);
            replay.process(&before.state_formatted());
            replay.process(&projection);
            assert_eq!(replay.screen().cursor_position(), before.cursor_position());
            assert_eq!(replay.screen().cell(0, 3).unwrap().contents(), " ");
            assert_eq!(replay.screen().cell(1, 1).unwrap().contents(), "\u{25cc}");
            assert_eq!(replay.screen().cell(1, 2).unwrap().contents(), "\u{fffd}");
            assert_eq!(replay.screen().cell(0, 2).unwrap().contents(), "c");
            assert_eq!(
                replay.screen().attributes_formatted(),
                before.attributes_formatted()
            );
        }
        parser.screen_mut().fill_cell_text(1, 5, 1, 'R');
        assert_eq!(parser.screen_mut().fill_cell_text(1, 5, 2, '中'), 2);
        assert_eq!(parser.screen().cell(1, 5).unwrap().contents(), " ");
        assert!(parser.screen().cell(2, 1).unwrap().is_wide_continuation());
        assert_eq!(parser.screen_mut().fill_cell_text(2, 1, 1, 'Q'), 1);
        assert!(!parser.screen().cell(2, 0).unwrap().is_wide());
        assert_eq!(parser.screen().cell(2, 0).unwrap().contents(), "");
        assert_eq!(parser.screen_mut().fill_cell_text(3, 0, 1, 'X'), 0);
        parser.process(b"\x1b[3;5H\xe4");
        parser
            .screen_mut()
            .fill_cell_style(0, 0, 1, crate::CellStyle::default());
        parser.process(b"\xb8\xad");
        assert_eq!(parser.screen().cell(2, 4).unwrap().contents(), "中");
    }
}
