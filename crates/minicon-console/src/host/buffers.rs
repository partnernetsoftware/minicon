//! Native identities around independent instances of the common terminal state.
use super::{ConsoleHost, HandleKind, HandleLifetime, INVALID_HANDLE, NOT_SUPPORTED, SUCCESS};
use crate::{
    RequestDescriptor,
    terminal::{TerminalStream, TerminalViewport, Utf16Decoder},
};
use std::io::{self, Write};
pub(super) const MAX_TOTAL_CELLS: usize = 1024 * 1024;
const MAX_BUFFERS: usize = 16;

pub(super) struct BufferState {
    terminal: TerminalStream,
    viewport: Option<TerminalViewport>,
    output_mode: u32,
    cursor_size: u32,
    output_decoder: Utf16Decoder,
    previous_output_cr: bool,
}

impl BufferState {
    pub(super) fn swap(&mut self, host: &mut ConsoleHost<'_>) {
        std::mem::swap(&mut self.terminal, &mut host.terminal);
        std::mem::swap(&mut self.viewport, &mut host.viewport);
        std::mem::swap(&mut self.output_mode, &mut host.output_mode);
        std::mem::swap(&mut self.cursor_size, &mut host.cursor_size);
        std::mem::swap(&mut self.output_decoder, &mut host.output_decoder);
        std::mem::swap(&mut self.previous_output_cr, &mut host.previous_output_cr);
    }

    fn cells(&self) -> usize {
        let (rows, columns) = self.terminal.screen().size();
        usize::from(rows) * usize::from(columns)
    }
}

impl ConsoleHost<'_> {
    pub(super) fn with_buffer<T>(
        &mut self,
        id: usize,
        action: impl FnOnce(&mut Self) -> io::Result<T>,
    ) -> io::Result<T> {
        let Some(mut state) = self.buffers.remove(&id) else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "missing native buffer state",
            ));
        };
        self.parked_cells = self.drawing_cells();
        state.swap(self);
        self.current_buffer = id;
        let result = action(self);
        state.swap(self);
        self.current_buffer = self.active_buffer;
        self.parked_cells = 0;
        self.buffers.insert(id, state);
        result
    }

    pub(super) fn drawing_cells(&self) -> usize {
        let (rows, columns) = self.terminal.screen().size();
        usize::from(rows) * usize::from(columns)
    }

    pub(super) fn buffer_size_fits(&self, rows: u16, columns: u16) -> bool {
        self.parked_cells
            + self.buffers.values().map(BufferState::cells).sum::<usize>()
            + usize::from(rows) * usize::from(columns)
            <= MAX_TOTAL_CELLS
    }

    pub(super) fn new_buffer(&self) -> io::Result<Option<BufferState>> {
        let view = self.viewport();
        let rows = (view.area.bottom - view.area.top) as u16;
        let columns = (view.area.right - view.area.left) as u16;
        if self.buffers.len() >= MAX_BUFFERS - 1
            || self.drawing_cells()
                + self.buffers.values().map(BufferState::cells).sum::<usize>()
                + usize::from(rows) * usize::from(columns)
                > MAX_TOTAL_CELLS
        {
            return Ok(None);
        }
        let mut terminal = TerminalStream::new(rows, columns, 0, 65536);
        let mut initial = self.terminal.screen().attributes_formatted();
        initial.extend_from_slice(b"\x1b[2J");
        terminal.write_projection(&initial, &mut io::sink())?;
        Ok(Some(BufferState {
            terminal,
            viewport: Some(TerminalViewport {
                area: crate::terminal::CellRect {
                    top: 0,
                    left: 0,
                    bottom: i32::from(rows),
                    right: i32::from(columns),
                },
                ..view
            }),
            output_mode: 7,
            cursor_size: 25,
            output_decoder: Utf16Decoder::default(),
            previous_output_cr: false,
        }))
    }

    pub(super) fn activate_buffer(
        &mut self,
        id: usize,
        output: &mut impl Write,
    ) -> io::Result<i32> {
        if id == self.active_buffer {
            return Ok(SUCCESS);
        }
        let Some(target) = self.buffers.get(&id) else {
            return Ok(INVALID_HANDLE);
        };
        let physical = self.viewport();
        let mut view = target.viewport.unwrap_or_else(|| {
            let (rows, columns) = target.terminal.screen().size();
            TerminalViewport::full(rows, columns)
        });
        view.rows = physical.rows;
        view.columns = physical.columns;
        // Layer selection and projection are common terminal mechanisms;
        // native identity ownership commits only after complete publication.
        if let Err(error) = target
            .terminal
            .write_viewport_activation(view, 1024 * 1024, output)
        {
            return if crate::terminal::ProjectionLimit::from_error(&error).is_some() {
                Ok(NOT_SUPPORTED)
            } else {
                Err(error)
            };
        }
        let mut next = self.buffers.remove(&id).unwrap();
        next.viewport = Some(view);
        next.swap(self);
        self.buffers.insert(self.active_buffer, next);
        self.active_buffer = id;
        self.current_buffer = id;
        Ok(SUCCESS)
    }

    pub(super) fn collect_buffers(&mut self, output: &mut impl Write) -> io::Result<()> {
        let live = |id: usize| {
            self.handles
                .values()
                .any(|h| h.kind == HandleKind::Output && h.buffer == id)
        };
        let unused: Vec<_> = self
            .buffers
            .keys()
            .copied()
            .filter(|id| !live(*id))
            .collect();
        let fallback = (!live(self.active_buffer))
            .then(|| self.buffers.keys().rev().copied().find(|id| live(*id)))
            .flatten();
        for id in unused {
            self.buffers.remove(&id);
        }
        if let Some(id) = fallback {
            let previous = self.active_buffer;
            let status = self.activate_buffer(id, output)?;
            if status != SUCCESS {
                return Err(io::Error::other(
                    "cannot publish active-buffer close fallback",
                ));
            }
            self.buffers.remove(&previous);
        }
        Ok(())
    }

    pub(super) fn close_may_publish(&self, descriptor: RequestDescriptor) -> bool {
        !self.buffers.is_empty()
            && self.handles.iter().any(|(id, handle)| {
                handle.kind == HandleKind::Output
                    && handle.buffer == self.active_buffer
                    && (descriptor.function == 4 && *id == descriptor.object
                        || descriptor.function == 2
                            && handle.lifetime == HandleLifetime::Connection(descriptor.process))
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_console_buffers_commit_isolation_grammar_and_aggregate_budget() {
        let mut host = ConsoleHost::new();
        host.resize(8, 4, &mut Vec::new()).unwrap();
        host.set_output_mode(7, &mut Vec::new()).unwrap();
        host.write_ansi(b"BASE\x1b[3", &mut Vec::new()).unwrap();
        host.buffers.insert(1, host.new_buffer().unwrap().unwrap());
        let before = host.terminal.screen().state_formatted();
        struct Reject;
        impl Write for Reject {
            fn write(&mut self, _: &[u8]) -> io::Result<usize> {
                Err(io::ErrorKind::BrokenPipe.into())
            }
            fn flush(&mut self) -> io::Result<()> {
                Ok(())
            }
        }
        assert!(host.activate_buffer(1, &mut Reject).is_err());
        assert_eq!(host.active_buffer, 0);
        assert_eq!(host.terminal.screen().state_formatted(), before);
        assert_eq!(host.terminal.pending_bytes(), b"\x1b[3");
        assert!(host.buffers[&1].terminal.screen().contents().is_empty());
        let result: io::Result<()> = host.with_buffer(1, |host| {
            assert_eq!(
                host.set_buffer_size(1024, 1024, &mut io::sink()).unwrap(),
                Err(NOT_SUPPORTED)
            );
            host.write_ansi(b"ALT\x1b[3", &mut io::sink())?;
            host.cursor_size = 61;
            Err(io::ErrorKind::Interrupted.into())
        });
        assert_eq!(result.unwrap_err().kind(), io::ErrorKind::Interrupted);
        assert_eq!(host.current_buffer, 0);
        assert_eq!(host.cursor_size, 25);
        assert_eq!(host.terminal.screen().state_formatted(), before);
        assert_eq!(host.buffers[&1].terminal.pending_bytes(), b"\x1b[3");
        assert_eq!(
            host.set_buffer_size(1024, 1024, &mut Vec::new()).unwrap(),
            Err(NOT_SUPPORTED)
        );
        let mut wire = Vec::new();
        assert_eq!(host.activate_buffer(1, &mut wire).unwrap(), SUCCESS);
        assert_eq!(host.cursor_size, 61);
        host.write_ansi(b"1mY", &mut wire).unwrap();
        assert_eq!(host.terminal.screen().contents(), "ALTY");
        assert_eq!(host.terminal.screen().fgcolor(), vt100::Color::Idx(1));
        host.write_utf16([0xd83d], &mut wire).unwrap();
        assert_eq!(host.activate_buffer(0, &mut wire).unwrap(), SUCCESS);
        host.write_ansi(b"2mZ", &mut wire).unwrap();
        assert_eq!(host.terminal.screen().contents(), "BASEZ");
        assert_eq!(host.terminal.screen().fgcolor(), vt100::Color::Idx(2));
        assert_eq!(host.activate_buffer(1, &mut wire).unwrap(), SUCCESS);
        host.write_utf16([0xde00], &mut wire).unwrap();
        assert_eq!(host.terminal.screen().contents(), "ALTY😀");
        let mut peer = vt100::Parser::new(4, 8, 0);
        peer.process(&wire);
        assert_eq!(peer.screen().contents(), "ALTY😀");
        assert_eq!(host.activate_buffer(99, &mut wire).unwrap(), INVALID_HANDLE);
        host.activate_buffer(0, &mut wire).unwrap();
        for id in 2..=15 {
            host.buffers.insert(id, host.new_buffer().unwrap().unwrap());
        }
        assert!(host.new_buffer().unwrap().is_none());
        host.buffers.remove(&15);
        assert!(host.new_buffer().unwrap().is_some());
        host.handles.insert(
            100,
            super::super::ClientHandle {
                lifetime: HandleLifetime::FileObject,
                kind: HandleKind::Output,
                access: super::super::objects::READ | super::super::objects::WRITE,
                share: 3,
                buffer: 0,
            },
        );
        host.collect_buffers(&mut wire).unwrap();
        assert!(host.buffers.is_empty());
    }
}
