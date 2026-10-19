//! Order complete client frames and native output using the shared grammar.
use super::ConsoleHost;
use std::io::{self, Write};

impl ConsoleHost<'_> {
    /// Publish an accepted unfinished tail only after the final native
    /// producer exits; no later native projection may follow.
    pub fn finish_output(&mut self, output: &mut impl Write) -> io::Result<()> {
        self.terminal.flush_final_tail(output)
    }

    pub(super) fn emit_framed(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        let result = if self.current_buffer != self.active_buffer {
            let mut view = self.viewport.unwrap_or_else(|| {
                let (rows, columns) = self.terminal.screen().size();
                crate::terminal::TerminalViewport::full(rows, columns)
            });
            let result = self.terminal.write_output_viewport_hidden(
                bytes,
                &mut view,
                crate::terminal::ViewportFollow::Vertical,
                1024 * 1024,
                output,
            );
            self.viewport = Some(view);
            result
        } else if let Some(mut view) = self.viewport {
            let result = self.terminal.write_output_viewport(
                bytes,
                &mut view,
                crate::terminal::ViewportFollow::Vertical,
                1024 * 1024,
                output,
            );
            self.viewport = Some(view);
            result
        } else {
            self.terminal.write_output(bytes, output)
        };
        // Earlier complete frames can commit before a later frame fails.
        // Drain only committed metadata, including inactive-buffer producers.
        if let Some(title) = self.terminal.take_window_title_update() {
            self.title = String::from_utf8_lossy(&title).encode_utf16().collect();
        }
        match result {
            Ok(()) => {
                self.sync_output_wrap();
                Ok(())
            }
            Err(vt100::FrameError::PendingLimit) => {
                self.previous_output_cr = false;
                Err(io::Error::from_raw_os_error(1450))
            }
            Err(vt100::FrameError::Io(error)) => Err(error),
        }
    }

    pub(super) fn emit_native(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        if let Some(mut view) = self.viewport {
            self.terminal
                .write_viewport_projection(bytes, &mut view, true, 1024 * 1024, output)?;
            self.viewport = Some(view);
        } else {
            self.terminal.write_projection(bytes, output)?;
        }
        self.sync_output_wrap();
        Ok(())
    }
}
