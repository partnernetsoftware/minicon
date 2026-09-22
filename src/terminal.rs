//! One terminal session: its PTY, its VT screen, and everything a user does to
//! it -- keys, IME, pointer, selection, scrollback -- plus how it paints.
//!
//! Moved out of `main.rs` unchanged. Its fields and methods are `pub(super)`
//! because `ConApp` in the crate root reads and drives them directly, exactly as
//! it did when both lived in one file; narrowing that surface is the next step,
//! not this one.

use super::*;

pub(super) struct ConTerminal {
    pub(super) working_dir: Option<String>,

    /// Program to host, from `-e`. `None` runs the user's default shell.
    pub(super) command: Option<Vec<String>>,

    /// Mirrors whatever the window title was last set to (default or an OSC
    /// title change), so `--emit-snapshot` can report it without needing to
    /// steal the one-shot `.take()` the render loop uses to notify the OS
    /// window.
    pub(super) current_title: String,
    /// The program this session runs, and its short name. Kept so a title the
    /// child sets can be distinguished from the child naming itself.
    pub(super) program_path: String,
    pub(super) program_label: String,

    /// `--emit-snapshot`: written after each render when set. See
    /// `agent_interface` module docs.
    pub(super) snapshot_path: Option<PathBuf>,

    /// VT model. Resized in lock-step with the PTY (see `apply_resize`).
    pub(super) parser: vt100::Parser<ConCallbacks>,

    /// PTY master (input writes + resize). `None` until `opened` spawns it.
    pub(super) master: Option<PtyMaster>,

    /// PTY child handle. MUST stay alive for the session lifetime: dropping it
    /// closes the platform-owned Job Object
    /// (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), which kills the shell tree.
    pub(super) child: Option<PtyChild>,

    /// Preallocated bounded handoff from the PTY reader thread.
    pub(super) pty_output: Arc<BoundedOutputPipe>,
    /// Coalesces reader notifications so a burst produces one GUI wake until
    /// the event thread has consumed its bounded share.
    pub(super) pty_wake_pending: Arc<AtomicBool>,

    /// Set once by the waiter thread when the child process actually exits
    /// (via Windows' process-exit notification, not PTY EOF — see `spawn_pty`).
    /// The existing window wake transports notification; this atomic owns only
    /// the completion state, so no general-purpose channel is required.
    pub(super) child_exit_pending: Arc<AtomicBool>,
    /// Encoded optional `ExitStatus::code`, published before
    /// `child_exit_pending` with release ordering.
    pub(super) child_exit_code_encoded: Arc<AtomicU64>,
    pub(super) child_exit_code: Option<i32>,

    /// Logical font size in DIPs. Adjusted by the tab column's zoom buttons.
    pub(super) font_size_logical: f64,
    /// Startup/configured font size restored by the `0` control.
    pub(super) font_size_baseline: f64,

    /// Physical cell metrics, recomputed whenever the font size or scale changes.
    pub(super) cell_w: u32,
    pub(super) cell_h: u32,
    pub(super) font_size_px: u16,

    pub(super) cols: u16,
    pub(super) rows: u16,

    /// Latest un-applied geometry (coalesced). Applied once the stream settles.
    pub(super) pending_geometry: Option<(u32, u32, f64)>,
    pub(super) last_geometry_at: Instant,
    /// A composer submission's Enter, held back so the child does not read it
    /// as trailing bytes of the paste. See [`COMPOSER_ENTER_DELAY`].
    pub(super) pending_submit_enter: Option<(Instant, Vec<u8>)>,

    pub(super) default_fg: Rgb,
    pub(super) default_bg: Rgb,
    /// Terminal cursor color and the themable 16 ANSI colors, kept in sync with
    /// `ui_theme` (see `ConTerminal::apply_theme`). The 6x6x6 cube and grayscale ramp
    /// stay standard.
    pub(super) term_cursor: Rgb,
    pub(super) term_ansi: [Rgb; 16],

    /// Set when the reader thread exits (PTY EOF or error).
    pub(super) child_gone: bool,
    pub(super) exit: bool,

    /// Scrollback scroll offset (0 = bottom/live). Positive = scrolled up.
    pub(super) scroll_offset: usize,
    /// Accumulated wheel delta (fractional lines pending application).
    pub(super) wheel_accumulator: f32,
    pub(super) scrollbar_drag: Option<ScrollbarThumbDrag>,

    /// Text selection: anchor + focus in terminal cell coordinates.
    /// None = no selection; Some = active or completed selection.
    pub(super) selection: Option<(TerminalPoint, TerminalPoint)>,
    /// True while left mouse button is held during a drag.
    pub(super) selecting: bool,
    /// True while the application (not local selection) owns a button gesture.
    /// Keeps press/release paired so TUI buttons do not get a stuck-down state.
    pub(super) mouse_dragging: bool,
    /// Last cell reported to the application, used to collapse motion spam.
    pub(super) last_reported_cell: Option<TerminalPoint>,
    /// Button code of the in-flight application gesture, so the release
    /// reports the same button that was pressed.
    pub(super) active_button: Option<u8>,
    /// Where the in-flight application gesture was pressed, so its span can
    /// be copied on release (`application_drag_copy_span`).
    pub(super) application_drag_anchor: Option<TerminalPoint>,
    pub(super) clipboard_paste_requested: bool,

    /// Whether the cursor is in its "on" phase of the blink cycle. Ignored
    /// entirely when `screen.cursor_blinking()` is false (a steady cursor).
    pub(super) blink_visible: bool,
    /// When `blink_visible` last flipped, for pacing the next flip.
    pub(super) last_blink_at: Instant,

    /// In-progress IME composition, drawn inline at the cursor. While this is
    /// non-empty the keystrokes feeding the composition must not also be sent
    /// to the PTY — the IME delivers the result once, as a commit.
    pub(super) ime_preedit: String,

    /// Whether an input method is attached (between Enabled and Disabled).
    /// Gates the logical-key fallback, which would otherwise double-type keys
    /// the IME consumed. See `TerminalKeyMode::ime_active`.
    pub(super) ime_attached: bool,

    /// Time and place of the last left press, plus how many clicks it
    /// continued, for double/triple-click selection.
    pub(super) clicks: minicon_core::click::ClickCounter<TerminalPoint>,
    /// Current scale factor (for pointer hit-test DIP→pixel conversion).
    pub(super) scale: f64,
    /// Physical space owned by the outer tab tree and composer.
    pub(super) content_left_px: u32,
    pub(super) content_top_px: u32,
    pub(super) content_bottom_px: u32,

    /// Conservative raster-candidate evidence for retained pixels and native
    /// redraw requests. Unknown damage remains full rather than guessed.
    pub(super) dirty: DirtyRegion,
    pub(super) last_cursor: Option<TerminalPoint>,
    /// Grid crosshair: the terminal cell the pointer last hovered (persists so
    /// the status readout does not blank when the pointer leaves), whether the
    /// pointer is currently over the grid (gates drawing the lines), and whether
    /// the crosshair is enabled at all (Ctrl+Shift+G).
    pub(super) crosshair_cell: Option<TerminalPoint>,
    pub(super) crosshair_active: bool,
    pub(super) crosshair_on: bool,
    pub(super) frame_width: u32,
    pub(super) frame_height: u32,
}

impl Drop for ConTerminal {
    fn drop(&mut self) {
        self.shutdown_pty();
    }
}

impl ConTerminal {
    /// Adopt a host-UI theme's terminal-body colors: default background/text,
    /// cursor, and the 16 ANSI colors. The 6x6x6 cube and grayscale ramp stay
    /// standard, and programs that request explicit colors are unaffected.
    pub(super) fn apply_theme(&mut self, choice: theme::ThemeChoice) {
        let t = theme::Theme::for_choice(choice);
        self.default_fg = t.term_fg;
        self.default_bg = t.term_bg;
        self.term_cursor = t.term_cursor;
        self.term_ansi = t.term_ansi;
    }

    /// The one place a window title is built, so the OSC path and the
    /// activation path cannot format it differently. They did, which is how
    /// the same window showed two different titles depending on which one had
    /// last written it.
    pub(super) fn window_title(&self) -> String {
        // Product last, context first: a title is read left to right and the
        // part that changes belongs in front. No tab id — that is a machine
        // identifier, and it is already in the tab column and in `list-tabs`;
        // a taskbar entry is read by a person. Version sits with the product
        // name so a taskbar entry answers "which MiniCon" without `--status`.
        format!("{} — {}", self.current_title, product_window_title())
    }

    pub(super) fn shutdown_pty(&mut self) {
        // First release product backpressure, then transfer both ownership
        // halves. ClosePseudoConsole may block while a flooded client drains,
        // so native teardown must never run on the GUI event thread.
        self.pty_output.close();
        let master = self.master.take();
        let child = self.child.take();
        let _ = agenterm_platform::pty::shutdown_session_detached(master, child);
    }

    pub(super) fn new(working_dir: Option<String>) -> Self {
        let pty_output = Arc::new(BoundedOutputPipe::new(PTY_QUEUE_BYTES));
        pty_output.close();
        Self {
            working_dir,
            command: None,
            current_title: String::from("terminal"),
            program_path: String::new(),
            program_label: String::from("terminal"),
            snapshot_path: None,
            parser: vt100::Parser::new_with_callbacks(24, 80, SCROLLBACK, ConCallbacks::default()),
            master: None,
            child: None,
            pty_output,
            pty_wake_pending: Arc::new(AtomicBool::new(false)),
            child_exit_pending: Arc::new(AtomicBool::new(false)),
            child_exit_code_encoded: Arc::new(AtomicU64::new(0)),
            child_exit_code: None,
            font_size_logical: DEFAULT_FONT_PX,
            font_size_baseline: DEFAULT_FONT_PX,
            cell_w: 8,
            cell_h: 16,
            font_size_px: 10,
            cols: 80,
            rows: 24,
            pending_geometry: None,
            last_geometry_at: Instant::now(),
            pending_submit_enter: None,
            default_fg: Rgb(0xF0, 0xF0, 0xF0),
            default_bg: Rgb(0x00, 0x00, 0x00),
            term_cursor: Rgb(0xFF, 0xFF, 0xFF),
            term_ansi: palette::STANDARD_ANSI,
            child_gone: false,
            exit: false,
            scroll_offset: 0,
            wheel_accumulator: 0.0,
            scrollbar_drag: None,
            selection: None,
            selecting: false,
            mouse_dragging: false,
            last_reported_cell: None,
            active_button: None,
            application_drag_anchor: None,
            clipboard_paste_requested: false,
            blink_visible: true,
            last_blink_at: Instant::now(),
            ime_preedit: String::new(),
            ime_attached: false,
            clicks: Default::default(),
            scale: 1.0,
            content_left_px: 0,
            content_top_px: 0,
            content_bottom_px: 0,
            dirty: DirtyRegion::full(),
            last_cursor: None,
            crosshair_cell: None,
            crosshair_active: false,
            crosshair_on: true,
            frame_width: 0,
            frame_height: 0,
        }
    }

    pub(super) fn set_content_insets(&mut self, left: u32, top: u32, bottom: u32) {
        if self.content_left_px != left
            || self.content_top_px != top
            || self.content_bottom_px != bottom
        {
            self.dirty.mark_full();
        }
        self.content_left_px = left;
        self.content_top_px = top;
        self.content_bottom_px = bottom;
    }

    pub(super) fn take_dirty(&mut self) -> DirtyRegion {
        std::mem::take(&mut self.dirty)
    }

    pub(super) fn request_dirty_redraw(&self, window: &PixelWindow) {
        request_candidate_redraw(window, self.dirty, self.frame_width, self.frame_height);
    }

    pub(super) fn note_frame_dimensions(&mut self, width: u32, height: u32) {
        if self.frame_width != width || self.frame_height != height {
            self.dirty.mark_full();
        }
        self.frame_width = width;
        self.frame_height = height;
    }

    pub(super) fn mark_cell(&mut self, point: TerminalPoint) {
        if !self.mark_cursor_position((point.row, point.col)) {
            self.dirty.mark_full();
        }
    }

    pub(super) fn mark_cursor_position(&mut self, position: (u16, u16)) -> bool {
        if self.frame_width == 0
            || self.frame_height == 0
            || self.cols == 0
            || self.rows == 0
            || self.cell_w == 0
            || self.cell_h == 0
        {
            return false;
        }
        let viewport_right = self
            .frame_width
            .saturating_sub(ui::terminal_scrollbar_width(self.scale));
        let viewport_bottom = self.frame_height.saturating_sub(self.content_bottom_px);
        let row = position.0.min(self.rows.saturating_sub(1));
        let col = position.1.min(self.cols.saturating_sub(1));
        let x = self
            .content_left_px
            .saturating_add(u32::from(col).saturating_mul(self.cell_w));
        let y = self
            .content_top_px
            .saturating_add(u32::from(row).saturating_mul(self.cell_h));
        let left = x.min(viewport_right);
        let top = y.min(viewport_bottom);
        let right = x
            .saturating_add(self.cell_w.saturating_mul(2))
            .min(viewport_right);
        let bottom = y.saturating_add(self.cell_h).min(viewport_bottom);
        let rect = PixelRect {
            left,
            top,
            right,
            bottom,
        };
        if rect.is_empty() {
            false
        } else {
            self.dirty.mark_rect(rect);
            true
        }
    }

    pub(super) fn mark_terminal_rows(&mut self, rows: vt100::RowRange) -> bool {
        let rows = rows.clip(self.rows);
        if rows.is_empty() {
            return false;
        }
        let viewport_right = self
            .frame_width
            .saturating_sub(ui::terminal_scrollbar_width(self.scale));
        let viewport_bottom = self.frame_height.saturating_sub(self.content_bottom_px);
        let terminal_right = self
            .content_left_px
            .saturating_add(u32::from(self.cols).saturating_mul(self.cell_w))
            .min(viewport_right);
        let mut dirty_rows = DirtyRows::empty();
        dirty_rows.mark_range(rows.first(), u64::from(rows.end()));
        let Some(rect) = dirty_rows.to_pixel_bounds(
            self.content_left_px,
            self.content_top_px,
            self.cell_w,
            self.cell_h,
            terminal_right,
            viewport_bottom,
        ) else {
            return false;
        };
        self.dirty.mark_rect(rect);
        true
    }

    pub(super) fn mark_vt_damage(&mut self, damage: vt100::ScreenDamage) {
        if damage.needs_full_raster() {
            self.dirty.mark_full();
            return;
        }

        let mut needs_full = false;
        if !damage.rows().is_empty() && !self.mark_terminal_rows(damage.rows()) {
            needs_full = true;
        }
        if damage.cursor_changed() {
            match (damage.cursor_before(), damage.cursor_after()) {
                (Some(before), Some(after)) => {
                    if !self.mark_cursor_position(before) || !self.mark_cursor_position(after) {
                        needs_full = true;
                    }
                }
                _ => needs_full = true,
            }
        }
        if needs_full {
            self.dirty.mark_full();
        }
    }

    pub(super) fn mark_cursor_change(&mut self) {
        if let Some(previous) = self.last_cursor {
            self.mark_cell(previous);
        }
        let cursor = self.parser.screen().cursor_position();
        self.mark_cell(TerminalPoint {
            row: cursor.0,
            col: cursor.1,
        });
    }

    /// True while a blinking caret would actually be painted on the live
    /// viewport. Hidden and scrolled-away carets must not arm the 530ms
    /// present timer.
    pub(super) fn cursor_blink_is_live(&self) -> bool {
        self.parser.screen().cursor_blinking()
            && cursor_visible(self.parser.screen(), self.scroll_offset, true)
    }

    pub(super) fn mark_ime_bounds(&mut self) {
        let cursor = self.parser.screen().cursor_position();
        let x = self
            .content_left_px
            .saturating_add(u32::from(cursor.1).saturating_mul(self.cell_w));
        let y = self
            .content_top_px
            .saturating_add(u32::from(cursor.0).saturating_mul(self.cell_h));
        let right = self
            .content_left_px
            .saturating_add(u32::from(self.cols).saturating_mul(self.cell_w));
        if right > x && self.cell_h > 0 {
            self.dirty.mark_rect(PixelRect::from_xywh(
                x,
                y,
                right.saturating_sub(x),
                self.cell_h,
            ));
        } else {
            self.dirty.mark_full();
        }
    }

    /// Replaces the terminal IME preedit, marking the old and new bounds. The
    /// pair of marks is the point: a preedit change repaints the cells it left
    /// and the cells it now occupies, and every caller must do both.
    pub(super) fn set_ime_preedit(&mut self, text: String) {
        self.mark_ime_bounds();
        self.ime_preedit = text;
        self.mark_ime_bounds();
    }

    pub(super) fn mark_selection(&mut self, selection: Option<(TerminalPoint, TerminalPoint)>) {
        let Some((start, end)) = selection.map(|(a, b)| normalize_endpoints(a, b)) else {
            return;
        };
        let mut rows = DirtyRows::empty();
        rows.mark_range(u32::from(start.row), u64::from(end.row).saturating_add(1));
        if let Some(bounds) = rows.to_pixel_bounds(
            self.content_left_px,
            self.content_top_px,
            self.cell_w,
            self.cell_h,
            self.frame_width,
            self.frame_height,
        ) {
            self.dirty.mark_rect(bounds);
        } else {
            self.dirty.mark_full();
        }
    }

    pub(super) fn mark_selection_change(
        &mut self,
        previous: Option<(TerminalPoint, TerminalPoint)>,
        current: Option<(TerminalPoint, TerminalPoint)>,
    ) {
        self.mark_selection(previous);
        self.mark_selection(current);
    }

    pub(super) fn mark_scrollbar_bounds(&mut self) {
        if self.frame_width == 0 || self.frame_height == 0 {
            self.dirty.mark_full();
            return;
        }
        let (geometry, _, _) = self.scrollbar_geometry(self.frame_width, self.frame_height);
        for rect in [geometry.track, geometry.thumb] {
            let left = rect.left.max(0) as u32;
            let top = rect.top.max(0) as u32;
            let right = rect.right.max(0) as u32;
            let bottom = rect.bottom.max(0) as u32;
            if right > left && bottom > top {
                self.dirty.mark_rect(PixelRect {
                    left,
                    top,
                    right,
                    bottom,
                });
            }
        }
    }

    /// Computes grid dimensions from physical pixels and current cell metrics.
    pub(super) fn compute_grid(phys_w: u32, phys_h: u32, cell_w: u32, cell_h: u32) -> (u16, u16) {
        let cols = (phys_w / cell_w.max(1)).clamp(2, 512) as u16;
        let rows = (phys_h / cell_h.max(1)).clamp(2, 512) as u16;
        (cols, rows)
    }

    /// (Re)computes physical cell metrics from the logical font size and scale.
    pub(super) fn recompute_metrics(&mut self, scale: f64) {
        self.font_size_px =
            minicon_core::numeric::round_f64(self.font_size_logical * scale).max(8.0) as u16;
        let m = font::cell_metrics(self.font_size_px);
        self.cell_w = m.width.max(1);
        self.cell_h = m.height.max(1);
    }

    /// Spawns the shell PTY and the reader thread. Called once from `opened`.
    pub(super) fn spawn_pty(&mut self, waker: &WindowWaker) -> Result<(), PixelWindowError> {
        agenterm_platform::pty::initialize_shutdown_reaper().map_err(|error| {
            PixelWindowError::failed("pty_reaper_init_failed", format!("{error}"))
        })?;
        // `-e` hosts a chosen program; otherwise fall back to the user's shell.
        let (program, extra_args) = match self.command.as_ref().and_then(|argv| argv.split_first())
        {
            Some((program, args)) => (program.clone(), args.to_vec()),
            None => (
                agenterm_platform::runtime::default_terminal_shell(),
                Vec::new(),
            ),
        };

        let mut command = ChildCommand::new(program.clone())
            .size(TerminalSize {
                rows: self.rows,
                cols: self.cols,
            })
            .env("TERM", "xterm-256color")
            .env("COLORTERM", "truecolor");

        if self.command.is_some() {
            for argument in extra_args {
                command = command.arg(argument);
            }
        } else if let Some(login_arg) =
            // Platform-neutral: returns Some("-l") on Unix for bare shells,
            // None on Windows or when the shell already has explicit args.
            // Only meaningful for the default-shell path — a program given
            // via -e must receive exactly the arguments the user wrote.
            agenterm_platform::pty::login_shell_argument(
                std::path::Path::new(&program),
                0,
            )
        {
            command = command.arg(login_arg);
        }
        if let Some(dir) = &self.working_dir {
            command = command.current_dir(dir.clone());
        }

        // Remembered so a title the child sets can be told apart from the
        // child merely naming itself — see `session_label`.
        self.program_path = program.clone();
        self.program_label = program_stem(&program);
        self.current_title = self.program_label.clone();

        let spawned = command.spawn().map_err(|error| {
            // Name the program: "failed to spawn" with no subject is the kind
            // of error message that costs a user ten minutes.
            PixelWindowError::failed("cmd_spawn_failed", format!("{program}: {error}"))
        })?;
        let (mut master, child) = spawned.into_parts();

        // Reader thread: blocking read loop (the platform read polls internally),
        // forwarding chunks over the channel and waking the window loop.
        let reader = master.try_clone_for_startup_reader().map_err(|error| {
            PixelWindowError::failed("cmd_reader_clone_failed", format!("{error}"))
        })?;
        let output = Arc::new(BoundedOutputPipe::new(PTY_QUEUE_BYTES));
        let reader_output = Arc::clone(&output);
        let reader_waker = waker.clone();
        let wake_pending = Arc::new(AtomicBool::new(false));
        let reader_wake_pending = Arc::clone(&wake_pending);
        agenterm_platform::threading::spawn_named_detached(
            "minicon-reader",
            Box::new(move || {
                let mut buf = [0u8; READ_BUF];
                loop {
                    match reader.io().read(&mut buf) {
                        Ok(0) => break,
                        Ok(n) => {
                            if reader_output.push_blocking(&buf[..n]).is_err() {
                                break;
                            }
                            if !reader_wake_pending.swap(true, Ordering::AcqRel) {
                                let _ = reader_waker.wake();
                            }
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
                        Err(_) => break,
                    }
                }
                reader_output.close();
                if !reader_wake_pending.swap(true, Ordering::AcqRel) {
                    let _ = reader_waker.wake();
                }
            }),
        )
        .map_err(|error| PixelWindowError::failed("cmd_reader_spawn_failed", format!("{error}")))?;

        // Waiter thread: on Windows, ConPTY's output pipe does not reliably
        // EOF just because the immediate child process exited — the pipe
        // stays open as long as the pseudoconsole handle does, which the
        // master side deliberately holds for the session's lifetime (see the
        // comment on `child` below). Without this, `-e cmd.exe /c <command>`
        // — or simply the user's shell exiting normally — left the window
        // open forever with nothing left to read and nothing to show for it;
        // caught by a black-box test that waited on a spawned `/c` command's
        // window to close and it never did. `try_wait`/`wait` go through
        // Windows' actual process-exit signal (WaitForSingleObject on the
        // process handle) rather than through PTY I/O, so this is the
        // correct detection path, not a workaround for the pipe's behavior.
        let mut waiter = child.try_clone_for_wait().map_err(|error| {
            PixelWindowError::failed("cmd_wait_clone_failed", format!("{error}"))
        })?;
        let child_exit_pending = Arc::new(AtomicBool::new(false));
        let waiter_exit_pending = Arc::clone(&child_exit_pending);
        let child_exit_code_encoded = Arc::new(AtomicU64::new(0));
        let waiter_exit_code = Arc::clone(&child_exit_code_encoded);
        let exit_waker = waker.clone();
        agenterm_platform::threading::spawn_named_detached(
            "minicon-waiter",
            Box::new(move || {
                let wait_result = waiter.wait();
                waiter_exit_code.store(
                    encode_child_exit_code(
                        wait_result
                            .as_ref()
                            .ok()
                            .and_then(std::process::ExitStatus::code),
                    ),
                    Ordering::Release,
                );
                waiter_exit_pending.store(true, Ordering::Release);
                let _ = exit_waker.wake();
            }),
        )
        .map_err(|error| PixelWindowError::failed("cmd_waiter_spawn_failed", format!("{error}")))?;

        self.master = Some(master);
        self.child = Some(child);
        self.pty_output = output;
        self.pty_wake_pending = wake_pending;
        self.child_exit_pending = child_exit_pending;
        self.child_exit_code_encoded = child_exit_code_encoded;

        Ok(())
    }

    pub(super) fn drain_pty(&mut self) -> DrainOutcome {
        self.drain_pty_with_budget(PTY_DRAIN_BUDGET_BYTES)
    }

    pub(super) fn drain_pty_with_budget(&mut self, budget: usize) -> DrainOutcome {
        self.pty_wake_pending.store(false, Ordering::Release);
        let mut outcome = DrainOutcome::default();
        if self.child_exit_pending.swap(false, Ordering::AcqRel) {
            self.child_gone = true;
            self.child_exit_code =
                decode_child_exit_code(self.child_exit_code_encoded.load(Ordering::Acquire));
            outcome.redraw = true;
            outcome.child_exited = true;
        }

        let output = Arc::clone(&self.pty_output);
        let report = output.drain(budget, |bytes| {
            self.parser.process(bytes);
            outcome.changed = true;
            outcome.redraw = true;
            // Flush terminal-query replies immediately after the contiguous
            // input span that completed them.
            let replies = std::mem::take(&mut self.parser.callbacks_mut().pending_replies);
            if !replies.is_empty() {
                let _ = self.write_pty(&replies);
            }
        });
        outcome.bytes = report.bytes;
        outcome.backlog = report.backlog;
        if outcome.backlog {
            self.pty_wake_pending.store(true, Ordering::Release);
        }
        // New output snaps scrollback to bottom.
        if outcome.changed && self.scroll_offset > 0 {
            self.scroll_offset = 0;
            self.parser.screen_mut().set_scrollback(0);
        }
        // New output clears stale selection.
        if outcome.changed && !self.selecting {
            self.mark_selection(self.selection);
            self.selection = None;
        }
        let damage = self.parser.take_damage();
        if !damage.is_empty() {
            outcome.redraw = true;
        }
        self.mark_vt_damage(damage);
        outcome
    }

    /// Applies a settled geometry: resize PTY first, then the VT model. The PTY
    /// resize is allowed to fail (some backends reject transient bad sizes);
    /// the model still converges so the next event is consistent.
    pub(super) fn apply_resize(&mut self, phys_w: u32, phys_h: u32, scale: f64) {
        // Resize, DPI, font metrics, and grid changes all invalidate the
        // complete terminal viewport, even when clamping preserves rows/cols.
        self.dirty.mark_full();
        self.scale = scale;
        self.recompute_metrics(scale);
        let usable_w = phys_w
            .saturating_sub(self.content_left_px)
            .saturating_sub(ui::terminal_scrollbar_width(scale));
        let usable_h = phys_h
            .saturating_sub(self.content_top_px)
            .saturating_sub(self.content_bottom_px);
        let (cols, rows) = Self::compute_grid(usable_w, usable_h, self.cell_w, self.cell_h);
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        if let Some(master) = &self.master {
            let _ = master.resize(TerminalSize { rows, cols });
        }
        self.parser.screen_mut().set_size(rows, cols);
    }

    pub(super) fn queue_resize(&mut self, phys_w: u32, phys_h: u32, scale: f64) {
        self.pending_geometry = Some((phys_w, phys_h, scale));
        self.last_geometry_at = Instant::now();
    }

    /// Handles one IME composition event.
    ///
    /// Without this, a Chinese/Japanese/Korean user can compose in the OS
    /// candidate window but the result never reaches the shell — which is what
    /// made "IME enabled" look like "keyboard broken" and led to IME being
    /// switched off entirely.
    pub(super) fn handle_ime(
        &mut self,
        window: &PixelWindow,
        event: agenterm_platform::ime::ImeEvent,
    ) {
        let _ = self.handle_ime_checked(window, event);
    }

    pub(super) fn handle_ime_checked(
        &mut self,
        window: &PixelWindow,
        event: agenterm_platform::ime::ImeEvent,
    ) -> Result<(), String> {
        use agenterm_platform::ime::{ImeAction, classify_event};

        // The terminal grid is always a valid composition anchor: we place the
        // candidate window at the cursor cell below.
        match &event {
            agenterm_platform::ime::ImeEvent::Enabled => self.ime_attached = true,
            agenterm_platform::ime::ImeEvent::Disabled => self.ime_attached = false,
            _ => {}
        }

        match classify_event(event, true) {
            ImeAction::UpdatePreedit { text, .. } => {
                self.set_ime_preedit(text);
                self.update_ime_anchor(window);
            }
            ImeAction::ClearPreedit => {
                self.set_ime_preedit(String::new());
            }
            ImeAction::CommitText(text) => {
                self.ensure_pty_input_open()?;
                self.write_pty(text.as_bytes())
                    .map_err(|error| format!("terminal input failed: {error}"))?;
                // Commit local presentation only after the complete PTY write
                // succeeds. A control-driven acceptance test must not receive
                // success while an exited child silently loses CJK input.
                self.dirty.mark_full();
                self.ime_preedit.clear();
                self.scroll_to_bottom();
            }
            // `ImeAction` is non-exhaustive; an unknown future action must not
            // silently drop a composition, so clear rather than guess.
            ImeAction::None => {}
            _ => {
                self.ime_preedit.clear();
                self.dirty.mark_full();
            }
        }
        self.request_dirty_redraw(window);
        Ok(())
    }

    /// Records a left press and returns the click count (1, 2, or 3).
    ///
    /// A repeat only counts when it lands on the same cell inside the
    /// multi-click window; moving to a different cell starts a fresh count, so
    /// a fast click in two places does not select a word by accident.
    pub(super) fn register_click(&mut self, point: TerminalPoint) -> u8 {
        self.clicks.register(point, Instant::now())
    }

    /// Expands to the word around `point`, or `None` if that cell is blank.
    pub(super) fn word_at(&self, point: TerminalPoint) -> Option<(TerminalPoint, TerminalPoint)> {
        word_selection(self.parser.screen(), point)
    }

    /// Triple-click owns one visible terminal row; soft-wrapped neighbors are
    /// separate selectable rows, matching the professional-selection contract.
    pub(super) fn line_at(&self, point: TerminalPoint) -> Option<(TerminalPoint, TerminalPoint)> {
        visible_row_selection(self.parser.screen(), point.row)
    }

    /// Draws the in-progress composition starting at the cursor cell and
    /// returns how many cells it occupied, so the caller can push the cursor
    /// past it. Wide (CJK) characters take two cells, matching the grid.
    pub(super) fn draw_preedit(&self, surface: &mut Surface<'_>, cursor: (u16, u16)) -> u32 {
        let y0 = self.content_top_px + u32::from(cursor.0) * self.cell_h;
        let mut advance = 0u32;
        // Inverted so the provisional text is unmistakable against committed
        // output, plus an underline in the conventional IME style.
        let fg = self.default_bg;
        let bg = self.default_fg;

        for character in self.ime_preedit.chars() {
            let cells = composer::character_cells(character) as u32;
            let x0 = self.content_left_px + (u32::from(cursor.1) + advance) * self.cell_w;
            if x0 >= surface.width || y0 >= surface.height {
                break;
            }
            let span = self.cell_w * cells;
            if !surface.intersects_rect(x0, y0, span, self.cell_h) {
                advance += cells;
                continue;
            }
            surface.fill_rect(x0, y0, span, self.cell_h, bg.to_xrgb());
            if let Some(glyph) = font::raster(character, self.font_size_px) {
                surface.blit_glyph(
                    &glyph,
                    CellRect {
                        x: x0,
                        y: y0,
                        w: span,
                        h: self.cell_h,
                    },
                    fg,
                    0.0,
                );
            }
            // Underline: the standard "this is not committed yet" affordance.
            let underline_y = y0 + self.cell_h.saturating_sub(1);
            surface.fill_rect(x0, underline_y, span, 1, fg.to_xrgb());
            advance += cells;
        }
        advance
    }

    /// Anchors the OS candidate window to the cursor cell, so it does not
    /// appear at an arbitrary corner of the screen.
    pub(super) fn update_ime_anchor(&self, window: &PixelWindow) {
        let (row, col) = self.parser.screen().cursor_position();
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        let x = f64::from(self.content_left_px + u32::from(col) * self.cell_w) / scale;
        let y = f64::from(self.content_top_px + u32::from(row) * self.cell_h) / scale;
        let _ = window.set_ime_cursor_area(agenterm_platform::window_host::LogicalRect::new(
            x,
            y,
            f64::from(self.cell_w) / scale,
            f64::from(self.cell_h) / scale,
        ));
    }

    pub(super) fn forward_key_checked(
        &mut self,
        event: &NormalizedKeyEvent,
    ) -> std::io::Result<()> {
        if self.exit || self.child_gone {
            return Ok(());
        }

        // Typing always shows the cursor and restarts the blink cycle —
        // every terminal does this so the cursor is never invisible right
        // when you start typing, which reads as "did that keystroke land?"
        self.blink_visible = true;
        self.last_blink_at = Instant::now();

        // If IME composition is in progress, suppress keys without committed
        // text because they are still editing the preedit candidate. Keys that
        // already carry committed text (including some winit IME commit
        // representations) must still be forwarded.
        if !self.ime_preedit.is_empty() && event.text.as_deref().is_none_or(str::is_empty) {
            return Ok(());
        }

        // Host shortcuts are resolved before the application sees the key.
        if let LogicalKey::Character(text) = &event.logical {
            let control = event.modifiers.control;
            // macOS: Command is the clipboard modifier (a Mac keyboard has no
            // Insert key and Ctrl+C stays SIGINT). Command never reaches the
            // shell, so it cannot shadow a terminal control key — Cmd+C copies
            // any selection, Cmd+V pastes, matching every native macOS terminal.
            #[cfg(target_os = "macos")]
            if event.modifiers.meta && !control && !event.modifiers.alt {
                if text.eq_ignore_ascii_case("c") {
                    self.copy_selection();
                    return Ok(());
                }
                if text.eq_ignore_ascii_case("v") {
                    self.request_clipboard_paste();
                    return Ok(());
                }
            }
            if control && event.modifiers.shift {
                if text.eq_ignore_ascii_case("c") {
                    self.copy_selection();
                    return Ok(());
                }
                if text.eq_ignore_ascii_case("v") {
                    self.request_clipboard_paste();
                    return Ok(());
                }
            }
            // Bare Ctrl+C copies when there is a selection, matching conhost;
            // with no selection it falls through to SIGINT (0x03).
            if control
                && !event.modifiers.alt
                && !event.modifiers.shift
                && text.eq_ignore_ascii_case("c")
                && self.active_selection().is_some()
            {
                self.copy_selection();
                self.selection = None;
                return Ok(());
            }
        }

        // Shift+PageUp/PageDown scroll the local viewport, matching conhost —
        // but not on the alternate screen, where those keys are the app's.
        let scrollable = event.modifiers.shift
            && event.state == KeyPressState::Pressed
            && !self.parser.screen().alternate_screen();
        if let LogicalKey::Named(named) = &event.logical
            && scrollable
        {
            {
                let page = usize::from(self.rows).saturating_sub(1).max(1) as isize;
                match named {
                    NamedKey::PageUp => {
                        self.scroll_by(page);
                        return Ok(());
                    }
                    NamedKey::PageDown => {
                        self.scroll_by(-page);
                        return Ok(());
                    }
                    _ => {}
                }
            }
        }

        let mode = TerminalKeyMode {
            application_cursor: self.parser.screen().application_cursor(),
            ime_active: self.ime_attached,
        };
        if let Some(bytes) = terminal_input::key_event_to_bytes(event, mode) {
            // Typing returns to the live view, as every terminal does.
            self.write_pty(&bytes)?;
            self.scroll_to_bottom();
        }
        Ok(())
    }

    /// The bytes a real Enter key press produces in this terminal's current
    /// keyboard mode — the same encoding a physical press and `send-keys Enter`
    /// go through. A composer commit is a key press, so it must not diverge
    /// from that path by hardcoding a carriage return.
    pub(super) fn encoded_enter(&self) -> Vec<u8> {
        let mode = TerminalKeyMode {
            application_cursor: self.parser.screen().application_cursor(),
            ime_active: self.ime_attached,
        };
        let event = injected_key_event(InjectedKey::Named(NamedKey::Enter), false, false, false);
        terminal_input::key_event_to_bytes(&event, mode).unwrap_or_else(|| vec![b'\r'])
    }

    pub(super) fn forward_key(&mut self, event: &NormalizedKeyEvent) {
        let _ = self.forward_key_checked(event);
    }

    pub(super) fn ensure_pty_input_open(&self) -> Result<(), String> {
        if self.child_gone
            || self.child_exit_pending.load(Ordering::Acquire)
            || self.master.is_none()
            || self.child.is_none()
        {
            return Err("terminal process has exited".to_owned());
        }
        Ok(())
    }

    /// Writes bytes to an owned PTY. Physical input may ignore a concurrent
    /// child-exit error; public control callers propagate it to the client.
    pub(super) fn write_pty(&self, bytes: &[u8]) -> std::io::Result<()> {
        self.master
            .as_ref()
            .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::BrokenPipe, "PTY is closed"))?
            .write_all(bytes)
    }

    /// Scrolls the viewport by `lines` (positive = toward older output).
    ///
    /// Uses vt100's read-only scrollback length instead of temporarily moving
    /// the viewport to `usize::MAX` and restoring it. Bounds queries must not
    /// create their own viewport damage or perturb parser state.
    pub(super) fn scroll_by(&mut self, lines: isize) {
        if self.parser.screen().alternate_screen() {
            return;
        }
        let requested = (self.scroll_offset as isize + lines).max(0) as usize;
        self.parser.screen_mut().set_scrollback(requested);
        self.scroll_offset = self.parser.screen().scrollback();
    }

    pub(super) fn scroll_to_bottom(&mut self) {
        if self.scroll_offset != 0 {
            self.scroll_offset = 0;
            self.parser.screen_mut().set_scrollback(0);
        }
    }

    pub(super) fn scrollback_bounds(&mut self) -> (usize, usize) {
        if self.parser.screen().alternate_screen() {
            return (0, 0);
        }
        let offset = self.parser.screen().scrollback();
        let maximum = self.parser.screen().scrollback_len();
        self.scroll_offset = offset;
        (offset, maximum)
    }

    pub(super) fn set_scrollback(&mut self, requested: usize) {
        if !self.parser.screen().alternate_screen() {
            let previous = self.scroll_offset;
            if previous != requested {
                self.mark_scrollbar_bounds();
            }
            self.parser.screen_mut().set_scrollback(requested);
            self.scroll_offset = self.parser.screen().scrollback();
            if self.scroll_offset != previous {
                // Scrolling changes the entire visible terminal viewport. The
                // scrollbar itself is also included so its old/new thumb
                // bounds remain observable in the candidate evidence.
                self.dirty.mark_full();
                self.mark_scrollbar_bounds();
            }
        }
    }

    pub(super) fn scrollbar_geometry(
        &mut self,
        width: u32,
        height: u32,
    ) -> (minicon_core::scrollbar::ScrollbarGeometry, usize, usize) {
        let (offset, maximum) = self.scrollback_bounds();
        (
            ui::terminal_scrollbar_geometry(
                ui::TerminalViewport {
                    width,
                    height,
                    left: self.content_left_px,
                    top: self.content_top_px,
                    bottom_inset: self.content_bottom_px,
                    scale: self.scale,
                    rows: usize::from(self.rows),
                },
                offset,
                maximum,
            ),
            offset,
            maximum,
        )
    }

    pub(super) fn handle_scrollbar_event(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        let metrics = window.metrics()?;
        let scale = self.scale;
        let physical = |position: &LogicalPoint| {
            (
                minicon_core::numeric::round_f64(position.x * scale) as i32,
                minicon_core::numeric::round_f64(position.y * scale) as i32,
            )
        };
        match event {
            PixelWindowEvent::PointerButton {
                button: PointerButton::Left,
                state: PointerButtonState::Pressed,
                position: Some(position),
                ..
            } => {
                let (geometry, current, _) =
                    self.scrollbar_geometry(metrics.physical_width, metrics.physical_height);
                let (x, y) = physical(position);
                let Some(hit) = scrollbar_hit_test(&geometry, x, y) else {
                    return Ok(false);
                };
                match hit {
                    ScrollbarHit::Thumb => {
                        self.mark_scrollbar_bounds();
                        self.scrollbar_drag =
                            Some(ScrollbarThumbDrag::begin(y, geometry.thumb.top));
                        let _ = window.set_pointer_capture(true);
                    }
                    ScrollbarHit::TrackAbove => {
                        self.set_scrollback(current.saturating_add(usize::from(self.rows).max(1)))
                    }
                    ScrollbarHit::TrackBelow => {
                        self.set_scrollback(current.saturating_sub(usize::from(self.rows).max(1)))
                    }
                }
                self.mark_scrollbar_bounds();
                self.request_dirty_redraw(window);
                Ok(true)
            }
            PixelWindowEvent::PointerMoved { position, .. } => {
                let Some(drag) = self.scrollbar_drag else {
                    return Ok(false);
                };
                let (geometry, _, maximum) =
                    self.scrollbar_geometry(metrics.physical_width, metrics.physical_height);
                let (_, y) = physical(position);
                self.set_scrollback(scrollback_for_thumb_top(
                    geometry,
                    drag.thumb_top(y),
                    maximum,
                ));
                self.request_dirty_redraw(window);
                Ok(true)
            }
            PixelWindowEvent::PointerButton {
                button: PointerButton::Left,
                state: PointerButtonState::Released,
                ..
            } if self.scrollbar_drag.take().is_some() => {
                self.mark_scrollbar_bounds();
                let _ = window.set_pointer_capture(false);
                self.request_dirty_redraw(window);
                Ok(true)
            }
            PixelWindowEvent::PointerCaptureLost if self.scrollbar_drag.take().is_some() => {
                self.mark_scrollbar_bounds();
                self.request_dirty_redraw(window);
                Ok(true)
            }
            _ => Ok(false),
        }
    }

    /// Converts a logical (DIP) pointer position to terminal cell coordinates.
    pub(super) fn hit_test(&self, pos: &LogicalPoint) -> TerminalPoint {
        let phys_x = (pos.x * self.scale - f64::from(self.content_left_px)).max(0.0);
        let phys_y = (pos.y * self.scale - f64::from(self.content_top_px)).max(0.0);
        TerminalPoint {
            // Both axes clamp to the last cell: a pointer or control coordinate
            // past the right/bottom edge lands on the edge cell rather than an
            // off-grid column, which would otherwise seed a phantom-width
            // selection or an out-of-grid reported cell.
            row: ((phys_y / self.cell_h as f64) as u16).min(self.rows.saturating_sub(1)),
            col: ((phys_x / self.cell_w as f64) as u16).min(self.cols.saturating_sub(1)),
        }
    }

    /// Update the grid crosshair for a pointer position. Returns whether the
    /// hovered cell or over-grid state changed (so the caller only repaints on a
    /// real change, not every pixel of motion). The cell persists when the
    /// pointer leaves the grid so the status readout does not blank.
    pub(super) fn update_crosshair(&mut self, pos: &LogicalPoint, fw: u32, fh: u32) -> bool {
        let px = pos.x * self.scale;
        let py = pos.y * self.scale;
        let bottom = fh.saturating_sub(self.content_bottom_px);
        let over = px >= f64::from(self.content_left_px)
            && py >= f64::from(self.content_top_px)
            && py < f64::from(bottom)
            && px < f64::from(fw);
        let cell = if over {
            Some(self.hit_test(pos))
        } else {
            self.crosshair_cell
        };
        let changed = over != self.crosshair_active || cell != self.crosshair_cell;
        self.crosshair_active = over;
        self.crosshair_cell = cell;
        changed
    }

    /// The inverse of [`Self::hit_test`]: a logical position that lands back
    /// on `point` when hit-tested. Targets the cell's center, not its
    /// top-left corner, so the result is robust to `hit_test`'s truncating
    /// division rather than sitting exactly on a rounding boundary. This is
    /// what lets control commands take cell coordinates (what a CLI caller
    /// actually thinks in) while still driving the same
    /// pixel-position-based handlers real pointer events go through.
    pub(super) fn terminal_point_to_logical(&self, point: TerminalPoint) -> LogicalPoint {
        let phys_x = f64::from(self.content_left_px)
            + f64::from(point.col) * self.cell_w as f64
            + self.cell_w as f64 / 2.0;
        let phys_y = f64::from(self.content_top_px)
            + f64::from(point.row) * self.cell_h as f64
            + self.cell_h as f64 / 2.0;
        let scale = if self.scale > 0.0 { self.scale } else { 1.0 };
        LogicalPoint {
            x: phys_x / scale,
            y: phys_y / scale,
        }
    }

    /// The selection as far as anything outside the drag gesture is concerned.
    ///
    /// A left press seeds `selection` with `(point, point)` so a drag has an
    /// anchor to extend from, but a degenerate range covers no text. The
    /// product already encoded that in `selection_should_auto_copy`, which
    /// refuses to copy it -- every other consumer treated it as a real
    /// one-cell selection. That single omission produced three separate
    /// symptoms from one plain click: the clicked cell stayed inverted for the
    /// rest of the session, a following right-click copied nothing instead of
    /// pasting, and bare Ctrl+C took the copy branch and returned, so the
    /// child process never received SIGINT.
    pub(super) fn active_selection(&self) -> Option<(TerminalPoint, TerminalPoint)> {
        self.selection.filter(|(anchor, focus)| anchor != focus)
    }

    pub(super) fn copy_selection(&mut self) {
        let Some((start, end)) = self.active_selection() else {
            return;
        };
        let text = selection_text(self.parser.screen(), start, end);
        self.copy_text(&text);
    }

    /// Every copy MiniCon makes goes through here, so the status bar's
    /// clipboard length follows it without waiting to re-read the clipboard.
    pub(super) fn copy_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let _ = clipboard_status::set_text(text);
    }

    pub(super) fn request_clipboard_paste(&mut self) {
        self.clipboard_paste_requested = true;
    }

    pub(super) fn take_clipboard_paste_request(&mut self) -> bool {
        std::mem::take(&mut self.clipboard_paste_requested)
    }

    /// The paste path proper, independent of where the text came from — the
    /// OS clipboard or a control `send-paste` command. Both
    /// must go through the same normalization and bracketing, which is the
    /// point of factoring this out: a scripted test exercises the exact
    /// logic a real Ctrl+V does, not a lookalike.
    pub(super) fn paste_text(&mut self, text: &str) -> std::io::Result<()> {
        // Normalization drops ESC, so a payload cannot close the bracketed
        // guard early and have its tail executed as keystrokes.
        let normalized = terminal_input::normalize_terminal_paste(text);
        if normalized.is_empty() {
            return Ok(());
        }
        let bracketed = self.parser.screen().bracketed_paste();
        self.write_pty(&terminal_input::terminal_paste_bytes(
            &normalized,
            bracketed,
        ))?;
        self.scroll_to_bottom();
        Ok(())
    }

    /// Whether `needle` appears in any rendered row right now. Used by the
    /// control `wait-text` command to sequence on real output instead of a
    /// guessed duration.
    pub(super) fn screen_contains(&self, needle: &str) -> bool {
        let screen = self.parser.screen();
        let cols = screen.size().1;
        screen
            .rows(0, cols)
            .any(|row| control::contains_utf8(&row, needle))
    }

    /// Synthesizes a [`NormalizedKeyEvent`] for a control key command and
    /// forwards it through [`ConTerminal::forward_key`] — the exact path a
    /// real keystroke takes, including host shortcuts and the live
    /// DECCKM/modifier-aware encoder.
    pub(super) fn inject_key(
        &mut self,
        key: InjectedKey,
        ctrl: bool,
        alt: bool,
        shift: bool,
    ) -> std::io::Result<()> {
        self.forward_key_checked(&injected_key_event(key, ctrl, alt, shift))
    }

    /// Presses then releases a mouse button at a control cell coordinate
    /// through the same path used by a physical click.
    pub(super) fn inject_click(
        &mut self,
        window: &PixelWindow,
        row: u16,
        col: u16,
        button: InjectedMouseButton,
    ) -> std::io::Result<MouseOutcome> {
        let press =
            self.inject_pointer_button(window, row, col, button, PointerButtonState::Pressed)?;
        let release =
            self.inject_pointer_button(window, row, col, button, PointerButtonState::Released)?;
        Ok(MouseOutcome {
            route: if press.route != "noop" {
                press.route
            } else {
                release.route
            },
            changed: press.changed || release.changed,
        })
    }

    /// One half of a control press-drag-release gesture, shared by
    /// [`Self::inject_click`] for the atomic press+release case.
    pub(super) fn inject_pointer_button(
        &mut self,
        window: &PixelWindow,
        row: u16,
        col: u16,
        button: InjectedMouseButton,
        state: PointerButtonState,
    ) -> std::io::Result<MouseOutcome> {
        let position = self.terminal_point_to_logical(TerminalPoint { row, col });
        let platform_button = match button {
            InjectedMouseButton::Left => PointerButton::Left,
            InjectedMouseButton::Middle => PointerButton::Middle,
            InjectedMouseButton::Right => PointerButton::Right,
        };
        self.handle_pointer_button_checked(
            window,
            platform_button,
            state,
            Some(position),
            &ModifierState::default(),
        )
    }

    /// Moves the pointer to a control cell coordinate through the physical
    /// pointer-motion path.
    pub(super) fn inject_mouse_move(
        &mut self,
        window: &PixelWindow,
        row: u16,
        col: u16,
    ) -> std::io::Result<MouseOutcome> {
        let position = self.terminal_point_to_logical(TerminalPoint { row, col });
        self.handle_pointer_moved_checked(window, position, &ModifierState::default())
    }

    /// Sends wheel input at a control cell coordinate through `handle_wheel`.
    /// `handle_wheel` itself never requests a
    /// redraw (real wheel events get that from the `MouseWheel` dispatch
    /// arm that calls it), so this mirrors that call site rather than
    /// leaving a scripted scroll invisible until the next unrelated redraw.
    pub(super) fn inject_wheel(
        &mut self,
        window: &PixelWindow,
        row: u16,
        col: u16,
        notches: f32,
        ctrl: bool,
    ) -> std::io::Result<WheelOutcome> {
        if ctrl {
            // Mirrors the real event: one `zoom_font` call per whole notch,
            // not one call scaled by magnitude — a real Ctrl+wheel session
            // is a *stream* of individual notch events, and reproducing a
            // crash tied to repeated cumulative resizes means replaying
            // that shape, not collapsing it into a single jump.
            let before = self.font_size_logical;
            let count = minicon_core::numeric::round_f32(notches.abs()).max(1.0) as usize;
            for _ in 0..count.min(64) {
                self.zoom_font(window, notches > 0.0);
            }
            let applied = (self.font_size_logical - before).abs() as i16;
            return Ok(WheelOutcome {
                route: "zoom",
                delivered_notches: if notches > 0.0 { applied } else { -applied },
                changed: applied != 0,
            });
        }
        let position = self.terminal_point_to_logical(TerminalPoint { row, col });
        let outcome = self.handle_wheel(notches, &ModifierState::default(), Some(position))?;
        window.request_redraw();
        Ok(outcome)
    }

    /// Builds the current [`ScreenSnapshot`] for `--emit-snapshot`.
    pub(super) fn build_snapshot(&mut self) -> ScreenSnapshot {
        let (_, max_scrollback) = self.scrollback_bounds();
        let screen = self.parser.screen();
        let (rows, cols) = screen.size();
        let cursor = screen.cursor_position();
        let shape = match screen.cursor_shape() {
            vt100::CursorShape::Block => "block",
            vt100::CursorShape::Underline => "underline",
            vt100::CursorShape::Bar => "bar",
        };
        let visible_now = cursor_visible(screen, self.scroll_offset, self.blink_visible);
        ScreenSnapshot {
            cols,
            rows,
            title: self.current_title.clone(),
            rows_text: screen.rows(0, cols).collect(),
            cursor: agent_interface::CursorSnapshot {
                row: cursor.0,
                col: cursor.1,
                shape,
                blinking: screen.cursor_blinking(),
                visible_now,
            },
            scroll_offset: self.scroll_offset,
            max_scrollback,
            selection: self.active_selection().map(|(a, b)| {
                (
                    agent_interface::PointSnapshot {
                        row: a.row,
                        col: a.col,
                    },
                    agent_interface::PointSnapshot {
                        row: b.row,
                        col: b.col,
                    },
                )
            }),
            ime_preedit: self.ime_preedit.clone(),
            child_alive: !self.child_gone,
            child_exit_code: self.child_exit_code,
            font_size_px: self.font_size_px,
        }
    }

    /// Writes the current snapshot to `--emit-snapshot`'s path, if set.
    /// Errors are deliberately swallowed: a full disk or a test harness that
    /// deleted the target directory mid-run must not crash the session it is
    /// trying to observe.
    pub(super) fn write_snapshot_if_requested(&mut self) {
        if let Some(path) = self.snapshot_path.clone() {
            let _ = agent_interface::write_snapshot_atomic(&path, &self.build_snapshot());
        }
    }

    /// Current mouse reporting contract negotiated by the running application.
    pub(super) fn mouse_mode(
        &self,
    ) -> (
        terminal_input::ApplicationMouseMode,
        terminal_input::MouseReportEncoding,
    ) {
        let screen = self.parser.screen();
        let mode = match screen.mouse_protocol_mode() {
            vt100::MouseProtocolMode::None => terminal_input::ApplicationMouseMode::None,
            vt100::MouseProtocolMode::Press => terminal_input::ApplicationMouseMode::Press,
            vt100::MouseProtocolMode::PressRelease => {
                terminal_input::ApplicationMouseMode::PressRelease
            }
            vt100::MouseProtocolMode::ButtonMotion => {
                terminal_input::ApplicationMouseMode::ButtonMotion
            }
            vt100::MouseProtocolMode::AnyMotion => terminal_input::ApplicationMouseMode::AnyMotion,
        };
        let encoding = match screen.mouse_protocol_encoding() {
            vt100::MouseProtocolEncoding::Default => terminal_input::MouseReportEncoding::Default,
            vt100::MouseProtocolEncoding::Utf8 => terminal_input::MouseReportEncoding::Utf8,
            vt100::MouseProtocolEncoding::Sgr => terminal_input::MouseReportEncoding::Sgr,
        };
        (mode, encoding)
    }

    /// Attempts to deliver a pointer event to the application. Returns true
    /// when the application consumed it, so the caller skips local selection.
    pub(super) fn report_mouse_checked(
        &mut self,
        button: u8,
        point: TerminalPoint,
        pressed: bool,
        motion: bool,
        modifiers: &agenterm_platform::input::ModifierState,
    ) -> std::io::Result<MouseReportOutcome> {
        let (mode, encoding) = self.mouse_mode();
        let delivery = terminal_input::mouse_delivery(
            mode,
            modifiers.shift,
            self.scroll_offset > 0,
            motion,
            self.mouse_dragging,
            pressed,
        );
        if delivery != terminal_input::MouseDelivery::Application {
            return Ok(MouseReportOutcome {
                consumed: false,
                wrote: false,
            });
        }
        // Motion reports repeat per pixel; collapse them to one per cell.
        if motion && self.last_reported_cell == Some(point) {
            return Ok(MouseReportOutcome {
                consumed: true,
                wrote: false,
            });
        }
        let code = terminal_input::mouse_code_with_modifiers(button, motion, *modifiers);
        let Some(bytes) =
            terminal_input::mouse_report_bytes(encoding, code, point.col, point.row, pressed)
        else {
            return Ok(MouseReportOutcome {
                consumed: false,
                wrote: false,
            });
        };
        self.write_pty(&bytes)?;
        self.last_reported_cell = Some(point);
        Ok(MouseReportOutcome {
            consumed: true,
            wrote: true,
        })
    }

    pub(super) fn report_mouse(
        &mut self,
        button: u8,
        point: TerminalPoint,
        pressed: bool,
        motion: bool,
        modifiers: &agenterm_platform::input::ModifierState,
    ) -> bool {
        // Physical input is best-effort across a concurrent child exit, but
        // application ownership must remain stable so a failed write does not
        // accidentally begin a local selection gesture.
        self.report_mouse_checked(button, point, pressed, motion, modifiers)
            .map_or(true, |outcome| outcome.consumed)
    }

    /// One Ctrl+wheel notch's worth of font-size zoom: `grow = true` is one
    /// step larger, `false` one step smaller, clamped to `[8.0, 36.0]`
    /// logical px. Factored out of the `MouseWheel` event arm so a
    /// control `send-wheel --ctrl` command drives the identical
    /// path a real Ctrl+wheel notch does — this is the exact repeated,
    /// cumulative resize path a reported "zoom past a certain size and the
    /// process exits" crash needs a live session (not an isolated
    /// `apply_resize` call) to actually exercise.
    ///
    /// Debounced, not applied synchronously — see the note below on why
    /// that changed. A fast wheel spin can queue many notches within a few
    /// hundred milliseconds; this now coalesces them into one grid/PTY
    /// resize per `RESIZE_DEBOUNCE` window (60ms) via the exact same
    /// `pending_geometry`/`about_to_wait` mechanism a window drag-resize
    /// already goes through, instead of duplicating that logic with a
    /// second, undebounced path.
    pub(super) fn zoom_font(&mut self, window: &PixelWindow, grow: bool) {
        let delta_size = if grow { 1.0 } else { -1.0 };
        self.font_size_logical = clamp_font_size(self.font_size_logical + delta_size);
        self.dirty.mark_full();
        // Cell metrics (and therefore what glyphs look like) update right
        // away, independent of the debounce below, so the zoom still reads
        // as instant — only the expensive part (recomputing cols/rows,
        // resizing the real ConPTY, resizing the vt100 model) is deferred.
        // Before this split, *every single notch* fired a full,
        // synchronous grid+PTY resize with zero throttling — unlike a
        // window drag-resize, which was already debounced. A hosted
        // program that repaints on every resize notification (a real TUI,
        // not just an idle prompt) receiving a burst of a dozen-plus
        // resizes within milliseconds is a real, previously-untested
        // stress shape; this brings Ctrl+wheel zoom in line with the
        // pacing window-resize already gets, on general principle, even
        // where a specific reported crash from it couldn't be reproduced
        // (see the black-box tests around `repeated_ctrl_wheel_zoom_...`).
        self.recompute_metrics(self.scale);
        if let Ok(m) = window.metrics() {
            self.pending_geometry = Some((m.physical_width, m.physical_height, m.scale_factor));
            self.last_geometry_at = Instant::now();
        }
        window.request_redraw();
    }

    pub(super) fn reset_font(&mut self, window: &PixelWindow) {
        self.font_size_logical = self.font_size_baseline;
        self.dirty.mark_full();
        self.recompute_metrics(self.scale);
        if let Ok(metrics) = window.metrics() {
            self.pending_geometry = Some((
                metrics.physical_width,
                metrics.physical_height,
                metrics.scale_factor,
            ));
            self.last_geometry_at = Instant::now();
        }
        window.request_redraw();
    }

    /// Routes a wheel notch: application report → alternate-screen cursor keys
    /// → local scrollback, in that order of precedence.
    pub(super) fn handle_wheel(
        &mut self,
        notches: f32,
        modifiers: &agenterm_platform::input::ModifierState,
        position: Option<LogicalPoint>,
    ) -> std::io::Result<WheelOutcome> {
        let up = notches > 0.0;
        let count = (minicon_core::numeric::round_f32(notches.abs()) as usize).clamp(1, 32);
        let signed_count = if up { count as i16 } else { -(count as i16) };

        // An application that grabbed the mouse gets buttons 64/65.
        let (mode, _) = self.mouse_mode();
        if mode != terminal_input::ApplicationMouseMode::None && !modifiers.shift {
            let point = position
                .map(|p| self.hit_test(&p))
                .unwrap_or(TerminalPoint { row: 0, col: 0 });
            let button = if up {
                terminal_input::MOUSE_WHEEL_UP
            } else {
                terminal_input::MOUSE_WHEEL_DOWN
            };
            let mut delivered = 0_i16;
            for _ in 0..count {
                // Wheel is press-only; never emit a matching release.
                if self
                    .report_mouse_checked(button, point, true, false, modifiers)?
                    .wrote
                {
                    delivered += 1;
                }
            }
            let delivered = if up { delivered } else { -delivered };
            return Ok(WheelOutcome {
                route: "application",
                delivered_notches: delivered,
                changed: delivered != 0,
            });
        }

        // Alternate screen has no local scrollback to move, so translate the
        // gesture into cursor keys the way xterm does — this is what makes the
        // wheel scroll inside less/man/vim.
        if self.parser.screen().alternate_screen() {
            let application_cursor = self.parser.screen().application_cursor();
            let sequence: &[u8] = match (up, application_cursor) {
                (true, true) => b"\x1bOA",
                (false, true) => b"\x1bOB",
                (true, false) => b"\x1b[A",
                (false, false) => b"\x1b[B",
            };
            self.write_pty(&sequence.repeat(count.min(120)))?;
            return Ok(WheelOutcome {
                route: "alternate-screen",
                delivered_notches: signed_count,
                changed: true,
            });
        }

        let before = self.scroll_offset;
        self.scroll_by(if up {
            count as isize
        } else {
            -(count as isize)
        });
        let delta = self.scroll_offset as isize - before as isize;
        Ok(WheelOutcome {
            route: "scrollback",
            delivered_notches: delta as i16,
            changed: delta != 0,
        })
    }

    /// Routes a pointer button press/release, preferring the application.
    pub(super) fn handle_pointer_button_checked(
        &mut self,
        window: &PixelWindow,
        button: PointerButton,
        state: PointerButtonState,
        position: Option<LogicalPoint>,
        modifiers: &agenterm_platform::input::ModifierState,
    ) -> std::io::Result<MouseOutcome> {
        let old_selection = self.selection;
        let pressed = state == PointerButtonState::Pressed;
        let point = match position {
            Some(pos) => self.hit_test(&pos),
            // A release with no position still has to close an open gesture.
            None => self
                .last_reported_cell
                .unwrap_or(TerminalPoint { row: 0, col: 0 }),
        };
        let code = match button {
            PointerButton::Left => 0,
            PointerButton::Middle => 1,
            PointerButton::Right => 2,
            _ => {
                return Ok(MouseOutcome {
                    route: "noop",
                    changed: false,
                });
            }
        };

        let _ = window.set_pointer_capture(pressed);

        if pressed {
            let report = match self.report_mouse_checked(code, point, true, false, modifiers) {
                Ok(report) => report,
                Err(error) => {
                    let _ = window.set_pointer_capture(false);
                    return Err(error);
                }
            };
            if report.consumed {
                self.mouse_dragging = true;
                self.active_button = Some(code);
                self.application_drag_anchor = Some(point);
                // The application owns this gesture; drop any stale selection
                // so the highlight does not linger over its UI.
                self.selection = None;
                self.mark_selection_change(old_selection, self.selection);
                self.request_dirty_redraw(window);
                return Ok(MouseOutcome {
                    route: "application",
                    changed: report.wrote,
                });
            }
        } else if self.mouse_dragging {
            let held = self.active_button.unwrap_or(code);
            // Copy before the program sees the release: it may redraw or clear
            // its own highlight in response, and the span is read from the
            // screen as the user saw it at the end of the drag.
            if let Some((start, end)) =
                application_drag_copy_span(held, self.application_drag_anchor.take(), point)
            {
                self.copy_text(&selection_text(self.parser.screen(), start, end));
            }
            let reported = self.report_mouse_checked(held, point, false, false, modifiers);
            self.mouse_dragging = false;
            self.active_button = None;
            let reported = reported?;
            return Ok(MouseOutcome {
                route: "application",
                changed: reported.wrote,
            });
        }

        // Local handling.
        let was_selecting = self.selecting;
        let route = match (button, pressed) {
            (PointerButton::Left, _) => "selection",
            (PointerButton::Right, true) => "clipboard",
            _ => "noop",
        };
        match (button, pressed) {
            (PointerButton::Left, true) => {
                match self.register_click(point) {
                    1 => {
                        self.selection = Some((point, point));
                        self.selecting = true;
                    }
                    2 => {
                        self.selection = self.word_at(point);
                        self.selecting = false;
                    }
                    // Third click and beyond select the whole logical line.
                    _ => {
                        self.selection = self.line_at(point);
                        self.selecting = false;
                    }
                }
            }
            (PointerButton::Left, false) => {
                self.selecting = false;
                if selection_should_auto_copy(self.selection) {
                    self.copy_selection();
                } else {
                    // The drag never left its anchor cell, so no selection was
                    // ever made. Dropping the seed here keeps the stored state
                    // equal to what every consumer already sees through
                    // `active_selection`.
                    self.selection = None;
                }
            }
            (PointerButton::Right, true) => {
                // Right-click: copy if a selection exists, else paste.
                if self.active_selection().is_some() {
                    self.copy_selection();
                    self.selection = None;
                } else {
                    self.request_clipboard_paste();
                }
            }
            _ => {}
        }
        let changed = old_selection != self.selection || was_selecting != self.selecting;
        self.mark_selection_change(old_selection, self.selection);
        self.request_dirty_redraw(window);
        Ok(MouseOutcome { route, changed })
    }

    pub(super) fn handle_pointer_button(
        &mut self,
        window: &PixelWindow,
        button: PointerButton,
        state: PointerButtonState,
        position: Option<LogicalPoint>,
        modifiers: &agenterm_platform::input::ModifierState,
    ) {
        let _ = self.handle_pointer_button_checked(window, button, state, position, modifiers);
    }

    /// Routes a pointer move: an application gesture in flight keeps
    /// ownership so its press/release stay paired; otherwise extends local
    /// selection, or reports hover motion under `ANY_MOTION` (1003).
    /// Factored out of the `PointerMoved` event arm so a control command
    /// `mouse_move` command drives the identical logic a real OS pointer
    /// move does, not a lookalike.
    pub(super) fn handle_pointer_moved_checked(
        &mut self,
        window: &PixelWindow,
        position: LogicalPoint,
        modifiers: &agenterm_platform::input::ModifierState,
    ) -> std::io::Result<MouseOutcome> {
        let (outcome, needs_redraw) = self.pointer_moved_outcome(position, modifiers)?;
        if needs_redraw {
            self.request_dirty_redraw(window);
        }
        Ok(outcome)
    }

    /// Updates selection / application mouse state for one move.
    ///
    /// A native present is requested only when local pixels actually change.
    /// Unix transient backing full-rasters every present, so an unchanged
    /// hover (including over an existing selection, or same-cell 1003 motion)
    /// must not schedule a frame. Application reports that write the PTY are
    /// painted when the child echoes, through `Wake`.
    pub(super) fn pointer_moved_outcome(
        &mut self,
        position: LogicalPoint,
        modifiers: &agenterm_platform::input::ModifierState,
    ) -> std::io::Result<(MouseOutcome, bool)> {
        let old_selection = self.selection;
        let pt = self.hit_test(&position);
        let (route, wrote) = if self.mouse_dragging {
            let button = self.active_button.unwrap_or(0);
            let report = self.report_mouse_checked(button, pt, true, true, modifiers)?;
            ("application", report.wrote)
        } else if self.selecting {
            if let Some((anchor, _)) = self.selection {
                self.selection = Some((anchor, pt));
            }
            ("selection", false)
        } else if self.mouse_mode().0 == terminal_input::ApplicationMouseMode::AnyMotion {
            let report = self.report_mouse_checked(3, pt, true, true, modifiers)?;
            ("application", report.wrote)
        } else {
            ("noop", false)
        };
        let selection_changed = old_selection != self.selection;
        if selection_changed {
            self.mark_selection_change(old_selection, self.selection);
        }
        let changed = if route == "application" {
            wrote
        } else {
            selection_changed
        };
        Ok((MouseOutcome { route, changed }, selection_changed))
    }

    pub(super) fn handle_pointer_moved(
        &mut self,
        window: &PixelWindow,
        position: LogicalPoint,
        modifiers: &agenterm_platform::input::ModifierState,
    ) {
        let _ = self.handle_pointer_moved_checked(window, position, modifiers);
    }

    pub(super) fn cancel_pointer_gesture(&mut self, window: &PixelWindow) {
        let _ = window.set_pointer_capture(false);
        if let Some((button, point)) = self.take_cancelled_pointer_release() {
            let modifiers = agenterm_platform::input::ModifierState {
                control: false,
                shift: false,
                alt: false,
                meta: false,
            };
            self.report_mouse(button, point, false, false, &modifiers);
        }
        window.request_redraw();
    }

    pub(super) fn take_cancelled_pointer_release(&mut self) -> Option<(u8, TerminalPoint)> {
        let release = self.mouse_dragging.then(|| {
            (
                self.active_button.unwrap_or(0),
                self.last_reported_cell
                    .unwrap_or(TerminalPoint { row: 0, col: 0 }),
            )
        });
        self.mouse_dragging = false;
        self.active_button = None;
        self.selecting = false;
        release
    }
}

impl ConTerminal {
    pub(super) fn opened(
        &mut self,
        window: &PixelWindow,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        let metrics = window.metrics()?;
        let scale = if metrics.scale_factor.is_finite() && metrics.scale_factor > 0.0 {
            metrics.scale_factor
        } else {
            1.0
        };
        self.recompute_metrics(scale);
        self.scale = scale;
        // Not the font. That was a development diagnostic living in the one
        // piece of host UI a user always sees; `--status` reports the resolved
        // face now, which is where someone diagnosing a font actually looks.
        window.set_title(&self.window_title());
        let (cols, rows) = Self::compute_grid(
            metrics
                .physical_width
                .saturating_sub(self.content_left_px)
                .saturating_sub(ui::terminal_scrollbar_width(scale)),
            metrics
                .physical_height
                .saturating_sub(self.content_top_px)
                .saturating_sub(self.content_bottom_px),
            self.cell_w,
            self.cell_h,
        );
        self.cols = cols;
        self.rows = rows;
        self.parser.screen_mut().set_size(rows, cols);

        self.spawn_pty(&window.waker())?;
        Ok(PixelWindowDirective::Continue)
    }

    /// Opens this session with no window to measure.
    ///
    /// The grid comes from `--cols`/`--rows` instead of a surface, and the
    /// scale is 1.0: a headless session has no display to be scaled for. An
    /// attach later re-measures and resizes, which is the same path a window
    /// resize already takes.
    pub(super) fn open_detached(&mut self, waker: &WindowWaker) -> Result<(), PixelWindowError> {
        self.recompute_metrics(1.0);
        self.scale = 1.0;
        self.spawn_pty(waker)
    }

    pub(super) fn event(
        &mut self,
        window: &PixelWindow,
        event: PixelWindowEvent,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        if self.handle_scrollbar_event(window, &event)? {
            return Ok(PixelWindowDirective::Continue);
        }
        match event {
            PixelWindowEvent::CloseRequested => {
                self.exit = true;
                Ok(PixelWindowDirective::Exit)
            }
            PixelWindowEvent::GeometryChanged { change, metrics } => {
                self.dirty.mark_full();
                if matches!(
                    change,
                    GeometryChange::Resized | GeometryChange::ScaleFactorChanged
                ) && metrics.is_drawable()
                {
                    // Coalesce: keep only the freshest metrics; the resize fires
                    // once the stream has been quiet for RESIZE_DEBOUNCE.
                    self.pending_geometry = Some((
                        metrics.physical_width,
                        metrics.physical_height,
                        metrics.scale_factor,
                    ));
                    self.last_geometry_at = Instant::now();
                }
                Ok(PixelWindowDirective::Continue)
            }
            PixelWindowEvent::Wake => {
                // Fired by the PTY reader thread's `waker.wake()` whenever
                // new output actually arrived (see `spawn_pty`) — this is
                // the *only* signal that a shell just echoed a keystroke or
                // printed something new. Before this arm existed, `Wake`
                // fell through to the wildcard `_ => Continue` below and
                // requested no redraw at all, so a keystroke's echo did not
                // actually appear on screen until the next unrelated redraw
                // happened to fire — in practice that was the cursor-blink
                // timer's ~530ms period (`BLINK_INTERVAL`), which is
                // measured, not guessed: it matches exactly the "often half
                // a second before it responds" symptom this fixes. Typing
                // was never actually slow — the PTY round-trip is fast —
                // painting the result just wasn't wired to happen promptly.
                if self.dirty.is_empty() {
                    window.request_redraw();
                } else {
                    self.request_dirty_redraw(window);
                }
                Ok(PixelWindowDirective::Continue)
            }
            PixelWindowEvent::Keyboard(key) => {
                let blink_was_visible = self.blink_visible;
                let old_selection = self.selection;
                let old_scroll = self.scroll_offset;
                let old_preedit_len = self.ime_preedit.len();
                self.forward_key(&key);
                // PTY echo is painted by `Wake`. A local present is only for
                // host-owned pixels that must not wait on the child: restoring
                // a blinked-off caret, selection/scroll changes, or IME.
                let local_visual = !blink_was_visible
                    || old_selection != self.selection
                    || old_scroll != self.scroll_offset
                    || old_preedit_len != self.ime_preedit.len();
                if local_visual {
                    if old_selection != self.selection {
                        self.mark_selection_change(old_selection, self.selection);
                    }
                    if old_scroll != self.scroll_offset {
                        self.dirty.mark_full();
                    }
                    self.mark_cursor_change();
                    self.request_dirty_redraw(window);
                }
                Ok(PixelWindowDirective::Continue)
            }
            PixelWindowEvent::Ime(ime) => {
                self.handle_ime(window, ime);
                Ok(PixelWindowDirective::Continue)
            }
            PixelWindowEvent::MouseWheel {
                delta,
                modifiers,
                position,
                ..
            } => {
                self.dirty.mark_full();
                // Interactive Ctrl+wheel font zoom is retired: the tab column's
                // z/Z buttons own font size now, and a modifier-sensitive wheel
                // made every scroll a chance to resize the grid by accident.
                // Every wheel notch scrolls, Ctrl held or not. `zoom_font` stays
                // reachable through the z/Z hit targets and through
                // `send-wheel --ctrl`, which the zoom soak tests drive.
                //
                // if modifiers.control {
                //     let dir = match delta {
                //         WheelDelta::Lines { y, .. } => y,
                //         _ => 0.0,
                //     };
                //     if dir.abs() > 0.0 {
                //         self.zoom_font(window, dir > 0.0);
                //     }
                // } else { ... }
                {
                    let lines = match delta {
                        WheelDelta::Lines { y, .. } => y,
                        WheelDelta::LogicalPixels { y, .. } => {
                            y as f32 / (self.cell_h as f32).max(1.0)
                        }
                        _ => 0.0,
                    };
                    self.wheel_accumulator += lines;
                    let whole = minicon_core::numeric::trunc_f32(self.wheel_accumulator);
                    self.wheel_accumulator -= whole;
                    if whole != 0.0 {
                        let _ = self.handle_wheel(whole, &modifiers, position);
                        window.request_redraw();
                    }
                }
                Ok(PixelWindowDirective::Continue)
            }
            PixelWindowEvent::PointerButton {
                button,
                state,
                position,
                modifiers,
            } => {
                self.handle_pointer_button(window, button, state, position, &modifiers);
                Ok(PixelWindowDirective::Continue)
            }
            PixelWindowEvent::PointerMoved {
                position,
                modifiers,
                ..
            } => {
                self.handle_pointer_moved(window, position, &modifiers);
                Ok(PixelWindowDirective::Continue)
            }
            PixelWindowEvent::PointerCaptureLost => {
                self.cancel_pointer_gesture(window);
                Ok(PixelWindowDirective::Continue)
            }
            _ => {
                // Unknown future host events are not safe to classify as a
                // smaller region.
                self.dirty.mark_full();
                Ok(PixelWindowDirective::Continue)
            }
        }
    }

    pub(super) fn render(
        &mut self,
        window: &PixelWindow,
        pixels: &mut [u32],
        width: u32,
        height: u32,
        candidate: DirtyRegion,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        // Apply OSC title changes (shell emits \e]0;title\a).
        if let Some(title) = self.parser.callbacks_mut().title.take() {
            self.current_title = session_label(&title, &self.program_path, &self.program_label);
            window.set_title(&self.window_title());
        }

        let fw = width;
        let fh = height;
        if candidate.is_empty() {
            self.write_snapshot_if_requested();
            return Ok(PixelWindowDirective::Continue);
        }
        let bg_word = self.default_bg.to_xrgb();
        let clip = candidate_bounds(candidate, fw, fh);
        let mut surface = Surface::with_clip(pixels, fw, fh, clip);
        if candidate.is_full() {
            surface.fill_rect(0, 0, fw, fh, bg_word);
        } else {
            let terminal_height = fh
                .saturating_sub(self.content_top_px)
                .saturating_sub(self.content_bottom_px);
            surface.fill_rect(
                self.content_left_px,
                self.content_top_px,
                fw.saturating_sub(self.content_left_px),
                terminal_height,
                bg_word,
            );
        }

        let (scrollbar, _, _) = self.scrollbar_geometry(fw, fh);
        let scrollbar_active = self.scrollbar_drag.is_some();
        let screen = self.parser.screen();
        let cursor = screen.cursor_position();
        self.last_cursor = Some(TerminalPoint {
            row: cursor.0,
            col: cursor.1,
        });
        // A steady request always shows the cursor; a blinking one is gated
        // by the timer in about_to_wait. conhost draws the caret the same
        // way — this is parity, not an enhancement — but getting it right
        // matters for vim/nvim, which switch shape *and* blink per mode.
        paint_cells_at(
            &mut surface,
            screen,
            self.active_selection(),
            self.cell_w,
            self.cell_h,
            self.default_fg,
            self.default_bg,
            self.term_ansi,
            self.font_size_px,
            self.content_left_px,
            self.content_top_px,
        );

        // IME composition, drawn over the cells to the right of the cursor and
        // underlined so it reads as provisional rather than committed text.
        // conhost cannot do this — it leaves composition to a floating OS
        // window that does not line up with the terminal grid.
        let preedit_cells = if self.ime_preedit.is_empty() {
            0
        } else {
            self.draw_preedit(&mut surface, cursor)
        };

        paint_cursor(
            &mut surface,
            screen,
            CursorPaintSpec {
                cell_w: self.cell_w,
                cell_h: self.cell_h,
                default_bg: self.default_bg,
                cursor: self.term_cursor,
                font_size_px: self.font_size_px,
                left: self.content_left_px,
                top: self.content_top_px,
                scroll_offset: self.scroll_offset,
                preedit_cells,
                blink_visible: self.blink_visible,
            },
        );

        // Grid crosshair: a translucent row/column band plus 1px lines snapped
        // to the hovered cell, drawn over the content without erasing it.
        if self.crosshair_on
            && self.crosshair_active
            && let Some(cell) = self.crosshair_cell
        {
            let terminal_h = fh
                .saturating_sub(self.content_top_px)
                .saturating_sub(self.content_bottom_px);
            let term_w = fw.saturating_sub(self.content_left_px);
            let col_x = self
                .content_left_px
                .saturating_add(u32::from(cell.col).saturating_mul(self.cell_w));
            let row_y = self
                .content_top_px
                .saturating_add(u32::from(cell.row).saturating_mul(self.cell_h));
            // Follow the terminal foreground so the crosshair stays visible on
            // every theme (a hardcoded white vanished on the light Paper theme).
            let mark = self.default_fg;
            surface.blend_rect(
                self.content_left_px,
                row_y,
                term_w,
                self.cell_h,
                mark,
                0.035,
            );
            surface.blend_rect(
                col_x,
                self.content_top_px,
                self.cell_w,
                terminal_h,
                mark,
                0.035,
            );
            surface.blend_rect(col_x, self.content_top_px, 1, terminal_h, mark, 0.28);
            surface.blend_rect(self.content_left_px, row_y, term_w, 1, mark, 0.28);
        }

        // Scrollbar tones ride the terminal palette so they stay legible on
        // every theme (a fixed dark track was near-invisible on light Paper).
        let track_color = palette::blend(self.default_bg, self.default_fg, 0.12);
        let thumb_color = palette::blend(
            self.default_bg,
            self.default_fg,
            if scrollbar_active { 0.55 } else { 0.33 },
        );
        surface.fill_rect(
            scrollbar.track.left.max(0) as u32,
            scrollbar.track.top.max(0) as u32,
            scrollbar.track.width().max(0) as u32,
            scrollbar.track.height().max(0) as u32,
            track_color.to_xrgb(),
        );
        surface.fill_rect(
            scrollbar.thumb.left.max(0) as u32,
            scrollbar.thumb.top.max(0) as u32,
            scrollbar.thumb.width().max(0) as u32,
            scrollbar.thumb.height().max(0) as u32,
            thumb_color.to_xrgb(),
        );

        self.write_snapshot_if_requested();

        Ok(PixelWindowDirective::Continue)
    }

    pub(super) fn about_to_wait(
        &mut self,
        window: &PixelWindow,
        now: Instant,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        // (see impl ConTerminal::draw_preedit for the composition renderer)
        if self.exit {
            return Ok(PixelWindowDirective::Exit);
        }
        // A session with an exited child remains drawable and selectable. The
        // outer ConApp may still host live siblings; closing the entire GUI
        // here made an ordinary child failure indistinguishable from a host
        // crash and discarded unrelated terminals.
        if self.child_gone {
            return Ok(PixelWindowDirective::Wait);
        }

        // Three independent timers can all have work pending at once (a
        // resize settling, the cursor mid-blink, a scripted `wait_ms`), and
        // this callback can only return one deadline. Each contributes to a
        // shared "wake no later than" floor instead of returning early —
        // returning early on, say, blink would starve a scripted wait behind
        // blink's ~530ms cadence, making `wait_ms: 50` in a script actually
        // take up to 530ms.
        let mut redraw = false;
        let mut partial_redraw = false;
        let mut next_wake: Option<Instant> = None;
        let mut fold_wake = |deadline: Instant| {
            next_wake = Some(next_wake.map_or(deadline, |current| current.min(deadline)));
        };

        if let Some((deadline, _)) = self.pending_submit_enter.as_ref().map(|(d, b)| (*d, b)) {
            if now >= deadline {
                if let Some((_, bytes)) = self.pending_submit_enter.take() {
                    // Best effort: a child that died between the payload and the
                    // commit is an ordinary exit, not a submission failure.
                    let _ = self.write_pty(&bytes);
                }
            } else {
                fold_wake(deadline);
            }
        }

        if let Some((pw, ph, scale)) = self.pending_geometry {
            let deadline = self.last_geometry_at + RESIZE_DEBOUNCE;
            if now >= deadline {
                self.apply_resize(pw, ph, scale);
                self.pending_geometry = None;
                redraw = true;
            } else {
                fold_wake(deadline);
            }
        }

        // A steady cursor needs no timer at all. A blinking cursor that is
        // hidden or scrolled out of the live viewport also needs none: the
        // overlay would not paint, and unix transient backing full-rasters
        // every present.
        if self.cursor_blink_is_live() {
            if now.saturating_duration_since(self.last_blink_at) >= BLINK_INTERVAL {
                self.mark_cursor_change();
                self.blink_visible = !self.blink_visible;
                self.last_blink_at = now;
                partial_redraw = true;
            }
            fold_wake(self.last_blink_at + BLINK_INTERVAL);
        }

        if redraw {
            window.request_redraw();
        } else if partial_redraw {
            self.request_dirty_redraw(window);
        }

        Ok(next_wake.map_or(PixelWindowDirective::Wait, PixelWindowDirective::WaitUntil))
    }
}
