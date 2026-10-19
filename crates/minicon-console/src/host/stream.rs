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
        match self.terminal.write_output(bytes, output) {
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
        self.terminal.write_projection(bytes, output)?;
        self.sync_output_wrap();
        Ok(())
    }
}
