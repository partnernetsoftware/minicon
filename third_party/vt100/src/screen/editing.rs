//! Literal cell edits without interpreting or disturbing a parser's VT stream.
use super::Screen;
use unicode_width::UnicodeWidthChar as _;

impl Screen {
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
            assert_eq!(
                replay.screen().cursor_position(),
                before.cursor_position()
            );
            assert_eq!(replay.screen().cell(0, 3).unwrap().contents(), " ");
            assert_eq!(
                replay.screen().cell(1, 1).unwrap().contents(),
                "\u{25cc}"
            );
            assert_eq!(
                replay.screen().cell(1, 2).unwrap().contents(),
                "\u{25cc}"
            );
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
        parser.screen_mut().fill_cell_style(
            0,
            0,
            1,
            crate::CellStyle::default(),
        );
        parser.process(b"\xb8\xad");
        assert_eq!(parser.screen().cell(2, 4).unwrap().contents(), "中");
    }
}
