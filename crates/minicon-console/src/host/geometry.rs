//! Native buffer/window metadata over the common viewport renderer.
use super::{ConsoleHost, ConsoleInput, INVALID_PARAMETER, NOT_SUPPORTED};
use crate::terminal::{CellRect, TerminalViewport};
use std::io::{self, Write};

// A headless virtual display has no monitor/font-derived physical maximum.
// Keep its advertised coordinate ceiling separate from its allocation budget.
pub(super) const MAX_AXIS: u16 = i16::MAX as u16 - 1;
const MAX_CELLS: usize = 1024 * 1024;

impl ConsoleHost<'_> {
    pub(super) fn viewport(&self) -> TerminalViewport {
        self.viewport.unwrap_or_else(|| {
            let (rows, columns) = self.terminal.screen().size();
            TerminalViewport::full(rows, columns)
        })
    }

    fn resize_capacity(&self, columns: u16, rows: u16) -> Result<(), i32> {
        if self.current_buffer == self.active_buffer
            && self.input_mode & 8 != 0
            && self.terminal.screen().size() != (rows, columns)
            && !self.input.can_store(
                &[ConsoleInput::Resize {
                    columns: columns as i16,
                    rows: rows as i16,
                }],
                true,
            )
        {
            Err(NOT_SUPPORTED)
        } else {
            Ok(())
        }
    }

    fn size_valid(columns: u16, rows: u16) -> Result<(), i32> {
        if columns == 0 || rows == 0 || columns > MAX_AXIS || rows > MAX_AXIS {
            return Err(INVALID_PARAMETER);
        }
        if usize::from(columns) * usize::from(rows) > super::buffers::MAX_TOTAL_CELLS {
            return Err(NOT_SUPPORTED);
        }
        Ok(())
    }

    fn commit_geometry(
        &mut self,
        rows: u16,
        columns: u16,
        proposed: TerminalViewport,
        output: &mut impl Write,
    ) -> io::Result<Result<(), i32>> {
        if let Err(status) =
            Self::size_valid(columns, rows).and_then(|()| self.resize_capacity(columns, rows))
        {
            return Ok(Err(status));
        }
        if !self.buffer_size_fits(rows, columns) {
            return Ok(Err(NOT_SUPPORTED));
        }
        let changed = self.terminal.screen().size() != (rows, columns);
        let mut next = self.terminal.screen().clone();
        next.set_size(rows, columns);
        let mut view = self.viewport();
        let full = self.viewport.is_none()
            || view.rows != proposed.rows
            || view.columns != proposed.columns;
        if let Err(error) = self
            .terminal
            .commit_viewport_screen(next, &mut view, proposed, MAX_CELLS, full, output)
        {
            return if crate::terminal::ProjectionLimit::from_error(&error).is_some() {
                Ok(Err(NOT_SUPPORTED))
            } else {
                Err(error)
            };
        }
        self.viewport = Some(view);
        if changed && self.input_mode & 8 != 0 && self.current_buffer == self.active_buffer {
            self.enqueue_input(
                &[ConsoleInput::Resize {
                    columns: columns as i16,
                    rows: rows as i16,
                }],
                output,
            )?;
        }
        Ok(Ok(()))
    }

    pub(super) fn set_buffer_size(
        &mut self,
        columns: u16,
        rows: u16,
        output: &mut impl Write,
    ) -> io::Result<Result<(), i32>> {
        let mut view = self.viewport();
        let width = view.area.right - view.area.left;
        let height = view.area.bottom - view.area.top;
        if i32::from(columns) < width || i32::from(rows) < height {
            return Ok(Err(INVALID_PARAMETER));
        }
        view.area.left = view.area.left.clamp(0, i32::from(columns) - width);
        view.area.top = view.area.top.clamp(0, i32::from(rows) - height);
        view.area.right = view.area.left + width;
        view.area.bottom = view.area.top + height;
        self.commit_geometry(rows, columns, view, output)
    }

    pub(super) fn set_window_info(
        &mut self,
        absolute: bool,
        rect: [i16; 4],
        output: &mut impl Write,
    ) -> io::Result<Result<(), i32>> {
        let old = self.viewport();
        let mut values = rect.map(i32::from);
        if !absolute {
            for (value, base) in values.iter_mut().zip([
                old.area.left,
                old.area.top,
                old.area.right - 1,
                old.area.bottom - 1,
            ]) {
                *value += base;
            }
        }
        let width = values[2] - values[0] + 1;
        let height = values[3] - values[1] + 1;
        if width <= 0 || height <= 0 || width > i32::from(MAX_AXIS) || height > i32::from(MAX_AXIS)
        {
            return Ok(Err(INVALID_PARAMETER));
        }
        // Headless window-size requests resize the drawing buffer. Normalize
        // the viewport to valid buffer bounds instead of retaining an
        // out-of-range legacy rectangle.
        let view = TerminalViewport {
            area: CellRect {
                top: 0,
                left: 0,
                bottom: height,
                right: width,
            },
            ..old
        };
        self.commit_geometry(height as u16, width as u16, view, output)
    }

    pub(super) fn resize_projected_parent(
        &mut self,
        columns: u16,
        rows: u16,
        output: &mut impl Write,
    ) -> io::Result<()> {
        match self.commit_geometry(rows, columns, TerminalViewport::full(rows, columns), output)? {
            Ok(()) => Ok(()),
            Err(_) => Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "console dimensions exceed viewport budget",
            )),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_console_geometry_bounds_events_and_transactional_viewport() {
        let mut host = ConsoleHost::new();
        host.resize(16, 8, &mut Vec::new()).unwrap();
        host.input_mode |= 8;
        host.set_output_mode(7, &mut Vec::new()).unwrap();
        host.write_ansi(b"seed\x1b[3", &mut Vec::new()).unwrap();
        struct Reject;
        impl Write for Reject {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(host.set_buffer_size(32, 16, &mut Reject).is_err());
        assert_eq!(host.terminal.screen().size(), (8, 16));
        assert!(host.viewport.is_none());
        assert!(host.input.events.is_empty());
        assert_eq!(
            host.set_buffer_size(32, 16, &mut Vec::new()).unwrap(),
            Ok(())
        );
        assert_eq!(host.terminal.screen().size(), (16, 32));
        assert_eq!(host.viewport().area.right, 16);
        assert_eq!(host.viewport().columns, 16);
        assert!(matches!(
            host.input.events.back(),
            Some(ConsoleInput::Resize {
                columns: 32,
                rows: 16
            })
        ));
        assert_eq!(host.terminal.pending_bytes(), b"\x1b[3");
        host.write_ansi(b"2mZ", &mut Vec::new()).unwrap();
        assert_eq!(host.terminal.screen().contents(), "seedZ");
        assert_eq!(
            host.set_buffer_size(1, 1, &mut Vec::new()).unwrap(),
            Err(INVALID_PARAMETER)
        );
        assert_eq!(
            host.set_buffer_size(MAX_AXIS, MAX_AXIS, &mut Vec::new())
                .unwrap(),
            Err(NOT_SUPPORTED)
        );
        assert_eq!(
            host.set_window_info(true, [0, 0, 19, 9], &mut Vec::new())
                .unwrap(),
            Ok(())
        );
        assert_eq!(host.terminal.screen().size(), (10, 20));
        assert_eq!(host.viewport().columns, 16);
        assert_eq!(host.terminal.screen().rows(0, 20).next().unwrap(), "seedZ");
        assert_eq!(
            host.set_window_info(true, [2, 2, 1, 3], &mut Vec::new())
                .unwrap(),
            Err(INVALID_PARAMETER)
        );
        host.input.events =
            std::iter::repeat_n(ConsoleInput::Focus(false), super::super::input::MAX_EVENTS)
                .collect();
        assert_eq!(
            host.set_buffer_size(30, 15, &mut Vec::new()).unwrap(),
            Err(NOT_SUPPORTED)
        );
        assert_eq!(host.terminal.screen().size(), (10, 20));
        host.input.events.clear();
        assert!(host.resize(i16::MAX as u16, 1, &mut Vec::new()).is_err());
        host.resize(24, 12, &mut Vec::new()).unwrap();
        assert_eq!(host.terminal.screen().size(), (12, 24));
        assert_eq!(host.viewport(), TerminalViewport::full(12, 24));
        struct Count(usize);
        impl Write for Count {
            fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
                self.0 += 1;
                Ok(bytes.len())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        for mode in [3, 7] {
            host.set_output_mode(mode, &mut Vec::new()).unwrap();
            let mut sink = Count(0);
            host.write_utf16(std::iter::repeat_n(u16::from(b'A'), 1000), &mut sink)
                .unwrap();
            assert!(
                sink.0 <= 2,
                "projected ASCII must publish batches, got {} writes",
                sink.0
            );
        }
    }
}
