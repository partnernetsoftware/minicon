//! Product clock adapter. Shared protocol state remains in minicon-console.
use crate::ConCallbacks;
use minicon_console::terminal::{Screen, ScreenDamage, TerminalPresentation};
use std::time::{Duration, Instant};

const HOLD_TIMEOUT: Duration = Duration::from_millis(250);
const HOLD_BYTES: usize = 1024 * 1024;

pub(crate) struct SessionParser {
    origin: Instant,
    terminal: TerminalPresentation<ConCallbacks>,
}
impl SessionParser {
    pub(crate) fn new(rows: u16, cols: u16, scrollback: usize) -> Self {
        Self {
            origin: Instant::now(),
            terminal: TerminalPresentation::with_callbacks(
                rows,
                cols,
                scrollback,
                HOLD_TIMEOUT,
                HOLD_BYTES,
                ConCallbacks::default(),
            ),
        }
    }
    pub(crate) fn process(&mut self, bytes: &[u8]) {
        self.terminal.process(bytes, self.origin.elapsed());
    }
    pub(crate) fn poll(&mut self, now: Instant) {
        self.terminal
            .poll(now.saturating_duration_since(self.origin));
    }
    pub(crate) fn deadline(&self) -> Option<Instant> {
        self.terminal
            .deadline()
            .map(|d| self.origin.checked_add(d).unwrap_or_else(Instant::now))
    }
    pub(crate) fn is_holding(&self) -> bool {
        self.terminal.is_holding()
    }
    pub(crate) fn finish(&mut self) {
        self.terminal.finish();
    }
    pub(crate) fn screen(&self) -> &Screen {
        self.terminal.presented_screen()
    }
    pub(crate) fn active_screen(&self) -> &Screen {
        self.terminal.active_screen()
    }
    pub(crate) fn screen_mut(&mut self) -> &mut Screen {
        self.terminal.screen_mut()
    }
    pub(crate) fn take_damage(&mut self) -> ScreenDamage {
        self.terminal.take_damage()
    }
    pub(crate) fn callbacks_mut(&mut self) -> &mut ConCallbacks {
        self.terminal.callbacks_mut()
    }
}
