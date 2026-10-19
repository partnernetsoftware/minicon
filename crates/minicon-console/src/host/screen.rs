//! Native screen mutations as transactional edits of the shared cell model.
use super::{ConsoleHost, INVALID_PARAMETER, NOT_SUPPORTED};
use std::io::{self, Write};
use windows_sys::Win32::Globalization::MultiByteToWideChar;

impl ConsoleHost<'_> {
    pub(super) fn commit_screen_update(
        &mut self,
        next: crate::terminal::Screen,
        output: &mut impl Write,
    ) -> io::Result<Result<(), i32>> {
        if let Some(mut view) = self.viewport {
            let proposed = view;
            match self.terminal.commit_viewport_screen(
                next,
                &mut view,
                proposed,
                1024 * 1024,
                false,
                output,
            ) {
                Ok(()) => {
                    self.viewport = Some(view);
                    return Ok(Ok(()));
                }
                Err(error) if error.raw_os_error() == Some(1450) => return Ok(Err(NOT_SUPPORTED)),
                Err(error) => return Err(error),
            }
        }
        let projection = next.contents_diff(self.terminal.screen());
        if projection.len() > 1024 * 1024 {
            return Ok(Err(NOT_SUPPORTED));
        }
        output.write_all(&projection)?;
        *self.terminal.screen_mut() = next;
        Ok(Ok(()))
    }

    pub(super) fn fill_output(
        &mut self,
        kind: u32,
        element: u16,
        row: u16,
        column: u16,
        count: usize,
        output: &mut impl Write,
    ) -> io::Result<Result<usize, i32>> {
        if !(1..=4).contains(&kind) {
            return Ok(Err(INVALID_PARAMETER));
        }
        if kind == 3 && element & !0xc0ff != 0 {
            return Ok(Err(NOT_SUPPORTED));
        }
        let mut next = self.terminal.screen().clone();
        let written = if kind == 3 {
            next.fill_cell_style(row, column, count, super::output::legacy_style(element))
        } else {
            let mut character = element;
            if kind == 1 {
                let byte = element as u8;
                // SAFETY: one initialized input byte and one output unit;
                // native ANSI fills convert through the current output CP.
                if unsafe {
                    MultiByteToWideChar(self.output_code_page, 0, &byte, 1, &mut character, 1)
                } != 1
                {
                    return Ok(Err(INVALID_PARAMETER));
                }
            }
            let Some(character) = char::from_u32(u32::from(character)) else {
                return Ok(Err(NOT_SUPPORTED));
            };
            next.fill_cell_text(row, column, count, character)
        };
        if count == 0 || row >= next.size().0 || column >= next.size().1 {
            return Ok(Ok(0));
        }
        // A transient clone supports commit-after-output without a persistent
        // shadow screen. Do not feed the projection back into the client VT
        // grammar: that would lose literal cells and disturb partial commands.
        if let Err(status) = self.commit_screen_update(next, output)? {
            return Ok(Err(status));
        }
        Ok(Ok(written))
    }
}
