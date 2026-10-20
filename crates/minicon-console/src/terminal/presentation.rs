//! Bounded active/presented terminal state. No transport or event loop.
use super::{Callbacks, Screen, ScreenDamage};
use std::time::Duration;

struct PresentationCallbacks<CB> {
    inner: CB,
    phase: bool,
    deadline: Option<Duration>,
    now: Duration,
    timeout: Duration,
    byte_limit: usize,
    bytes: usize,
}

struct BeginPreview {
    phase: bool,
    may_begin: bool,
}
impl vte::Perform for BeginPreview {
    fn csi_dispatch(
        &mut self,
        params: &vte::Params,
        intermediates: &[u8],
        ignore: bool,
        action: char,
    ) {
        if ignore || intermediates != b"?" || !matches!(action, 'h' | 'l') {
            return;
        }
        for param in params {
            if param == [2026] {
                let enabled = action == 'h';
                self.may_begin |= enabled && !self.phase;
                self.phase = enabled;
            }
        }
    }
    fn esc_dispatch(&mut self, intermediates: &[u8], ignore: bool, byte: u8) {
        if !ignore && intermediates.is_empty() && byte == b'c' {
            self.phase = false;
        }
    }
}

impl<CB: Callbacks> Callbacks for PresentationCallbacks<CB> {
    fn reset(&mut self, screen: &mut Screen) {
        self.phase = false;
        self.deadline = None;
        self.inner.reset(screen);
    }
    fn synchronized_update(&mut self, screen: &mut Screen, enabled: bool) {
        if !enabled {
            self.phase = false;
            self.deadline = None;
        } else if !self.phase {
            self.phase = true;
            self.bytes = 0;
            if self.byte_limit != 0 && !self.timeout.is_zero() {
                self.deadline = Some(self.now.saturating_add(self.timeout));
            }
        }
        self.inner.synchronized_update(screen, enabled);
    }
    fn audible_bell(&mut self, screen: &mut Screen) {
        self.inner.audible_bell(screen);
    }
    fn visual_bell(&mut self, screen: &mut Screen) {
        self.inner.visual_bell(screen);
    }
    fn resize(&mut self, screen: &mut Screen, request: (u16, u16)) {
        self.inner.resize(screen, request);
    }
    fn set_window_icon_name(&mut self, screen: &mut Screen, name: &[u8]) {
        self.inner.set_window_icon_name(screen, name);
    }
    fn set_window_title(&mut self, screen: &mut Screen, title: &[u8]) {
        self.inner.set_window_title(screen, title);
    }
    fn copy_to_clipboard(&mut self, screen: &mut Screen, ty: &[u8], data: &[u8]) {
        self.inner.copy_to_clipboard(screen, ty, data);
    }
    fn paste_from_clipboard(&mut self, screen: &mut Screen, ty: &[u8]) {
        self.inner.paste_from_clipboard(screen, ty);
    }
    fn unhandled_char(&mut self, screen: &mut Screen, c: char) {
        self.inner.unhandled_char(screen, c);
    }
    fn unhandled_control(&mut self, screen: &mut Screen, b: u8) {
        self.inner.unhandled_control(screen, b);
    }
    fn unhandled_escape(&mut self, screen: &mut Screen, i1: Option<u8>, i2: Option<u8>, b: u8) {
        self.inner.unhandled_escape(screen, i1, i2, b);
    }
    fn unhandled_csi(
        &mut self,
        screen: &mut Screen,
        i1: Option<u8>,
        i2: Option<u8>,
        params: &[&[u16]],
        c: char,
    ) {
        self.inner.unhandled_csi(screen, i1, i2, params, c);
    }
    fn unhandled_osc(&mut self, screen: &mut Screen, params: &[&[u8]]) {
        self.inner.unhandled_osc(screen, params);
    }
}

/// One active parser plus an optional immutable held screen. Queries and other
/// callbacks always see active state. Rendering, copy and snapshot consumers
/// must use `presented_screen`; input-mode consumers use `active_screen`.
///
/// Caller supplies monotonic elapsed time and schedules `poll` at `deadline`,
/// including when no output arrives. Repeated begins do not extend a phase.
/// Timeout, byte-budget exhaustion or explicit edits release presentation;
/// further begins are ignored until an end marker or RIS resets the phase.
/// Zero timeout or zero byte limit disables holding. No output bytes are queued.
///
/// This opt-in mechanism does not change raw-parser capability replies. A GUI
/// must integrate its paint/snapshot/timer paths before advertising mode 2026.
pub struct TerminalPresentation<CB: Callbacks = ()> {
    parser: vt100::Parser<PresentationCallbacks<CB>>,
    presented: Option<Screen>,
}

impl TerminalPresentation {
    #[must_use]
    pub fn new(
        rows: u16,
        columns: u16,
        scrollback: usize,
        timeout: Duration,
        byte_limit: usize,
    ) -> Self {
        Self::with_callbacks(rows, columns, scrollback, timeout, byte_limit, ())
    }
}

impl<CB: Callbacks> TerminalPresentation<CB> {
    #[must_use]
    pub fn with_callbacks(
        rows: u16,
        columns: u16,
        scrollback: usize,
        timeout: Duration,
        byte_limit: usize,
        callbacks: CB,
    ) -> Self {
        let parser = vt100::Parser::new_with_callbacks(
            rows,
            columns,
            scrollback,
            PresentationCallbacks {
                inner: callbacks,
                phase: false,
                deadline: None,
                now: Duration::ZERO,
                timeout,
                byte_limit,
                bytes: 0,
            },
        );
        Self {
            parser,
            presented: None,
        }
    }

    /// Process ordered output without delaying any parser callbacks. Quota
    /// accounting conservatively charges entire parser spans (at most 4096
    /// bytes and never larger than the quota), including begin-marker bytes.
    /// This can release early; retained work never exceeds the configured cap.
    pub fn process(&mut self, bytes: &[u8], now: Duration) {
        self.poll(now);
        if !self.is_holding() && self.may_begin(bytes) {
            self.presented = Some(self.parser.screen().clone());
        }
        let mut remaining_input = bytes;
        while !remaining_input.is_empty() {
            let gate = self.parser.callbacks();
            let remaining_quota = if gate.deadline.is_some() {
                gate.byte_limit.saturating_sub(gate.bytes)
            } else {
                gate.byte_limit
            };
            let span_limit = if remaining_quota == 0 {
                4096
            } else {
                remaining_quota.min(4096)
            };
            let length = remaining_input.len().min(span_limit);
            let (span, rest) = remaining_input.split_at(length);
            remaining_input = rest;
            self.parser.process(span);
            let gate = self.parser.callbacks_mut();
            if gate.deadline.is_some() {
                gate.bytes = gate.bytes.saturating_add(span.len());
                if gate.bytes >= gate.byte_limit {
                    gate.deadline = None;
                }
            }
        }
        self.publish();
    }

    /// Service the absolute deadline even without new bytes. A backwards clock
    /// observation is clamped to the latest supplied time.
    pub fn poll(&mut self, now: Duration) {
        let gate = self.parser.callbacks_mut();
        gate.now = gate.now.max(now);
        if gate.deadline.is_some_and(|deadline| gate.now >= deadline) {
            gate.deadline = None;
        }
        self.publish();
    }

    #[must_use]
    pub fn deadline(&self) -> Option<Duration> {
        self.parser.callbacks().deadline
    }
    #[must_use]
    pub fn is_holding(&self) -> bool {
        self.deadline().is_some()
    }
    #[must_use]
    pub fn active_screen(&self) -> &Screen {
        self.parser.screen()
    }
    #[must_use]
    pub fn presented_screen(&self) -> &Screen {
        self.presented
            .as_ref()
            .unwrap_or_else(|| self.parser.screen())
    }
    pub fn callbacks(&self) -> &CB {
        &self.parser.callbacks().inner
    }
    pub fn callbacks_mut(&mut self) -> &mut CB {
        &mut self.parser.callbacks_mut().inner
    }

    /// Explicit user geometry/viewport edits recover a held frame and publish
    /// the resulting active state. Unfinished VT/UTF-8 input is preserved.
    pub fn edit_screen(&mut self, edit: impl FnOnce(&mut Screen)) {
        edit(self.screen_mut());
        self.parser.screen_mut().mark_full_damage();
    }

    /// Borrow active state for an explicit caller-owned edit, recovering any
    /// hold first. Ordinary edits retain the screen API's own damage tracking;
    /// releasing a held frame reports full damage. Input grammar is preserved.
    pub fn screen_mut(&mut self) -> &mut Screen {
        self.parser.callbacks_mut().deadline = None;
        self.publish();
        self.parser.screen_mut()
    }

    /// Publish the latest complete model on EOF/shutdown. Does not invent a
    /// missing UTF-8 scalar or execute unfinished VT input. The caller owns
    /// transport shutdown and must stop producers before calling this method.
    pub fn finish(&mut self) {
        self.parser.callbacks_mut().phase = false;
        self.parser.callbacks_mut().deadline = None;
        self.publish();
    }

    /// Damage refers only to presented state. Ordinary output preserves the
    /// parser's incremental damage; releasing a held snapshot reports full
    /// damage. Active damage is retained throughout a hold.
    pub fn take_damage(&mut self) -> ScreenDamage {
        if let Some(presented) = &mut self.presented {
            presented.take_damage()
        } else {
            self.parser.take_damage()
        }
    }

    fn publish(&mut self) {
        if !self.is_holding() && self.presented.take().is_some() {
            self.parser.screen_mut().mark_full_damage();
        }
    }

    fn may_begin(&self, bytes: &[u8]) -> bool {
        let gate = self.parser.callbacks();
        if gate.byte_limit == 0
            || gate.timeout.is_zero()
            || (self.parser.at_boundary() && !bytes.contains(&0x1b) && !bytes.contains(&0x9b))
        {
            return false;
        }
        let mut preview = BeginPreview {
            phase: gate.phase,
            may_begin: false,
        };
        self.parser.inspect_control_sequences(bytes, &mut preview);
        preview.may_begin
    }
}
