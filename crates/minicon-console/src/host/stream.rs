//! Order complete client frames and native output using the shared grammar.
use super::ConsoleHost;
use std::io::{self, Write};

impl ConsoleHost<'_> {
    /// Publish an accepted unfinished tail only after the final native
    /// producer exits; no later native projection may follow.
    pub fn finish_output(&mut self, output: &mut impl Write) -> io::Result<()> {
        self.output_framer.finish(output)
    }

    pub(super) fn emit_framed(&mut self, bytes: &[u8], output: &mut impl Write) -> io::Result<()> {
        match self.output_framer.write(&mut self.screen, bytes, output) {
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
        // Reuse the same parser implementation with a transient grammar for
        // complete host commands, preserving the authoritative client decoder.
        let mut native = vt100::Parser::new(1, 1, 0);
        *native.screen_mut() = self.screen.screen().clone();
        native.process(bytes);
        if !native.at_boundary() {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "unfinished native output command",
            ));
        }
        output.write_all(bytes)?;
        let (screen, ()) = native.into_parts();
        *self.screen.screen_mut() = screen;
        self.sync_output_wrap();
        Ok(())
    }
}
