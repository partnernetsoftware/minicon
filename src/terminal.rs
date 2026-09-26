//! One terminal session: its PTY, its VT screen, and everything a user does to
//! it -- keys, IME, pointer, selection, scrollback -- plus how it paints.
//!
//! Moved out of `main.rs` unchanged. Its fields and methods are `pub(super)`
//! because `ConApp` in the crate root reads and drives them directly, exactly as
//! it did when both lived in one file; narrowing that surface is the next step,
//! not this one.

use super::*;

pub(super) struct ConTerminal {
    working_dir: Option<String>,

    /// This instance's `--control` endpoint, exactly as bound (e.g.
    /// `"pipe:foo"` or `"unix:/path"`). Exported into every spawned shell as
    /// `MINICON_CONTROL` so a script running inside a pane can drive `minicon
    /// mux`/`minicon cli` against its own instance without being told the
    /// endpoint -- the same role `$TMUX` plays for tmux. `None` when the
    /// instance was not started with `--control`, in which case nothing is
    /// exported and scripts must be told the endpoint explicitly, same as
    /// today.
    control_endpoint: Option<String>,

    /// Program to host, from `-e`. `None` runs the user's default shell.
    pub(super) command: Option<Vec<String>>,

    /// Mirrors whatever the window title was last set to (default or an OSC
    /// title change), so `--emit-snapshot` can report it without needing to
    /// steal the one-shot `.take()` the render loop uses to notify the OS
    /// window.
    pub(super) current_title: String,
    /// The program this session runs, and its short name. Kept so a title the
    /// child sets can be distinguished from the child naming itself.
    program_path: String,
    program_label: String,

    /// `--emit-snapshot`: written after each render when set. See
    /// `agent_interface` module docs.
    pub(super) snapshot_path: Option<PathBuf>,

    /// VT model. Resized in lock-step with the PTY (see `apply_resize`).
    pub(super) parser: vt100::Parser<ConCallbacks>,

    /// PTY master (input writes + resize). `None` until `opened` spawns it.
    master: Option<PtyMaster>,

    /// PTY child handle. MUST stay alive for the session lifetime: dropping it
    /// closes the platform-owned Job Object
    /// (`JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE`), which kills the shell tree.
    child: Option<PtyChild>,

    /// Preallocated bounded handoff from the PTY reader thread.
    pty_output: Arc<BoundedOutputPipe>,
    /// Coalesces reader notifications so a burst produces one GUI wake until
    /// the event thread has consumed its bounded share.
    pub(super) pty_wake_pending: Arc<AtomicBool>,

    /// Set once by the waiter thread when the child process actually exits
    /// (via Windows' process-exit notification, not PTY EOF — see `spawn_pty`).
    /// The existing window wake transports notification; this atomic owns only
    /// the completion state, so no general-purpose channel is required.
    child_exit_pending: Arc<AtomicBool>,
    /// Encoded optional `ExitStatus::code`, published before
    /// `child_exit_pending` with release ordering.
    child_exit_code_encoded: Arc<AtomicU64>,
    pub(super) child_exit_code: Option<i32>,

    /// Logical font size in DIPs. Adjusted by the tab column's zoom buttons.
    pub(super) font_size_logical: f64,
    /// Startup/configured font size restored by the `0` control.
    pub(super) font_size_baseline: f64,

    /// Physical cell metrics, recomputed whenever the font size or scale changes.
    pub(super) cell_w: u32,
    pub(super) cell_h: u32,
    font_size_px: u16,
    /// The backend's last refusal to resize, surfaced in the snapshot.
    /// `None` once a resize succeeds.
    backend_resize_error: Option<String>,
    backend_resize_failures: u64,

    pub(super) cols: u16,
    pub(super) rows: u16,

    /// Latest un-applied geometry (coalesced). Applied once the stream settles.
    pending_geometry: Option<(u32, u32, f64)>,
    last_geometry_at: Instant,
    /// A composer submission's Enter, held back so the child does not read it
    /// as trailing bytes of the paste. See [`COMPOSER_ENTER_DELAY`].
    pub(super) pending_submit_enter: Option<(Instant, Vec<u8>)>,

    default_fg: Rgb,
    default_bg: Rgb,
    /// Terminal cursor color and the themable 16 ANSI colors, kept in sync with
    /// `ui_theme` (see `ConTerminal::apply_theme`). The 6x6x6 cube and grayscale ramp
    /// stay standard.
    term_cursor: Rgb,
    term_ansi: [Rgb; 16],

    /// Set when the reader thread exits (PTY EOF or error).
    pub(super) child_gone: bool,
    exit: bool,

    /// Scrollback scroll offset (0 = bottom/live). Positive = scrolled up.
    scroll_offset: usize,
    /// Accumulated wheel delta (fractional lines pending application).
    wheel_accumulator: f32,
    scrollbar_drag: Option<ScrollbarThumbDrag>,

    /// Text selection: anchor + focus in terminal cell coordinates.
    /// None = no selection; Some = active or completed selection.
    selection: Option<(TerminalPoint, TerminalPoint)>,
    /// True while left mouse button is held during a drag.
    selecting: bool,
    /// True while the application (not local selection) owns a button gesture.
    /// Keeps press/release paired so TUI buttons do not get a stuck-down state.
    mouse_dragging: bool,
    /// Last cell reported to the application, used to collapse motion spam.
    last_reported_cell: Option<TerminalPoint>,
    /// Button code of the in-flight application gesture, so the release
    /// reports the same button that was pressed.
    active_button: Option<u8>,
    /// Where the in-flight application gesture was pressed, so its span can
    /// be copied on release (`application_drag_copy_span`).
    application_drag_anchor: Option<TerminalPoint>,
    clipboard_paste_requested: bool,

    /// Whether the cursor is in its "on" phase of the blink cycle. Ignored
    /// entirely when `screen.cursor_blinking()` is false (a steady cursor).
    blink_visible: bool,
    /// When `blink_visible` last flipped, for pacing the next flip.
    last_blink_at: Instant,

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
    clicks: minicon_core::click::ClickCounter<TerminalPoint>,
    /// Current scale factor (for pointer hit-test DIP→pixel conversion).
    pub(super) scale: f64,
    /// Physical space owned by the outer tab tree and composer.
    pub(super) content_left_px: u32,
    pub(super) content_top_px: u32,
    pub(super) content_bottom_px: u32,

    /// Conservative raster-candidate evidence for retained pixels and native
    /// redraw requests. Unknown damage remains full rather than guessed.
    pub(super) dirty: DirtyRegion,
    last_cursor: Option<TerminalPoint>,
    /// Grid crosshair: the terminal cell the pointer last hovered (persists so
    /// the status readout does not blank when the pointer leaves), whether the
    /// pointer is currently over the grid (gates drawing the lines), and whether
    /// the crosshair is enabled at all (Ctrl+Shift+G).
    pub(super) crosshair_cell: Option<TerminalPoint>,
    pub(super) crosshair_active: bool,
    pub(super) crosshair_on: bool,
    frame_width: u32,
    frame_height: u32,
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

    fn shutdown_pty(&mut self) {
        // First release product backpressure, then transfer both ownership
        // halves. ClosePseudoConsole may block while a flooded client drains,
        // so native teardown must never run on the GUI event thread.
        self.pty_output.close();
        let master = self.master.take();
        let child = self.child.take();
        let _ = agenterm_platform::pty::shutdown_session_detached(master, child);
    }

    /// Apply startup sizes in rising priority (config, then CLI flags); the
    /// last given value wins. The font size that results becomes the baseline
    /// that zoom reset returns to.
    pub(super) fn apply_startup_size(
        &mut self,
        font_sizes: [Option<f64>; 2],
        cols: [Option<u16>; 2],
        rows: [Option<u16>; 2],
    ) {
        if let Some(size) = font_sizes.into_iter().flatten().last() {
            self.font_size_logical = clamp_font_size(size);
        }
        if let Some(value) = cols.into_iter().flatten().last() {
            self.cols = value.max(2);
        }
        if let Some(value) = rows.into_iter().flatten().last() {
            self.rows = value.max(2);
        }
        self.font_size_baseline = self.font_size_logical;
    }

    pub(super) fn new(working_dir: Option<String>, control_endpoint: Option<String>) -> Self {
        let pty_output = Arc::new(BoundedOutputPipe::new(PTY_QUEUE_BYTES));
        pty_output.close();
        Self {
            working_dir,
            control_endpoint,
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
            backend_resize_error: None,
            backend_resize_failures: 0,
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

    fn mark_cell(&mut self, point: TerminalPoint) {
        if !self.mark_cursor_position((point.row, point.col)) {
            self.dirty.mark_full();
        }
    }

    fn mark_cursor_position(&mut self, position: (u16, u16)) -> bool {
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

    fn mark_terminal_rows(&mut self, rows: vt100::RowRange) -> bool {
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

    fn mark_vt_damage(&mut self, damage: vt100::ScreenDamage) {
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

    fn mark_cursor_change(&mut self) {
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
    fn cursor_blink_is_live(&self) -> bool {
        self.parser.screen().cursor_blinking()
            && cursor_visible(self.parser.screen(), self.scroll_offset, true)
    }

    fn mark_ime_bounds(&mut self) {
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
    fn set_ime_preedit(&mut self, text: String) {
        self.mark_ime_bounds();
        self.ime_preedit = text;
        self.mark_ime_bounds();
    }

    fn mark_selection(&mut self, selection: Option<(TerminalPoint, TerminalPoint)>) {
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

    fn mark_selection_change(
        &mut self,
        previous: Option<(TerminalPoint, TerminalPoint)>,
        current: Option<(TerminalPoint, TerminalPoint)>,
    ) {
        self.mark_selection(previous);
        self.mark_selection(current);
    }

    fn mark_scrollbar_bounds(&mut self) {
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
    fn compute_grid(phys_w: u32, phys_h: u32, cell_w: u32, cell_h: u32) -> (u16, u16) {
        let cols = (phys_w / cell_w.max(1)).clamp(2, 512) as u16;
        let rows = (phys_h / cell_h.max(1)).clamp(2, 512) as u16;
        (cols, rows)
    }

    /// (Re)computes physical cell metrics from the logical font size and scale.
    fn recompute_metrics(&mut self, scale: f64) {
        self.font_size_px =
            minicon_core::numeric::round_f64(self.font_size_logical * scale).max(8.0) as u16;
        let m = font::cell_metrics(self.font_size_px);
        self.cell_w = m.width.max(1);
        self.cell_h = m.height.max(1);
    }

    /// Spawns the shell PTY and the reader thread. Called once from `opened`.
    fn spawn_pty(&mut self, waker: &WindowWaker) -> Result<(), PixelWindowError> {
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

        if let Some(endpoint) = &self.control_endpoint {
            command = command.env("MINICON_CONTROL", endpoint);
        }

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
            // Recorded, not discarded. The model converges either way, but a
            // backend that could not apply the size may have left its own
            // window in a state that shows nothing, and a silent failure there
            // is indistinguishable from a product that stopped painting.
            let outcome = master
                .resize(TerminalSize { rows, cols })
                .map_err(|error| error.to_string());
            self.record_resize_outcome(cols, rows, outcome);
        }
        self.parser.screen_mut().set_size(rows, cols);
    }

    /// What the session remembers about a resize the backend answered.
    ///
    /// The message is the current state and clears on the next success; the
    /// count is the history and never clears. They differ exactly when a
    /// failure was followed by a success, which is the case that reads as a
    /// healthy session and is not one: run 35992342157 found a blank Windows
    /// terminal whose error field was null, and a null field cannot say
    /// whether nothing ever failed or something failed and was papered over.
    ///
    /// Separated from `apply_resize` so it can be tested without a PTY.
    fn record_resize_outcome(&mut self, cols: u16, rows: u16, outcome: Result<(), String>) {
        self.backend_resize_error = match outcome {
            Ok(()) => None,
            Err(error) => {
                self.backend_resize_failures = self.backend_resize_failures.saturating_add(1);
                Some(format!("resize to {cols}x{rows} failed: {error}"))
            }
        };
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
    fn handle_ime(&mut self, window: &PixelWindow, event: agenterm_platform::ime::ImeEvent) {
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
    fn register_click(&mut self, point: TerminalPoint) -> u8 {
        self.clicks.register(point, Instant::now())
    }

    /// Expands to the word around `point`, or `None` if that cell is blank.
    fn word_at(&self, point: TerminalPoint) -> Option<(TerminalPoint, TerminalPoint)> {
        word_selection(self.parser.screen(), point)
    }

    /// Triple-click owns one visible terminal row; soft-wrapped neighbors are
    /// separate selectable rows, matching the professional-selection contract.
    fn line_at(&self, point: TerminalPoint) -> Option<(TerminalPoint, TerminalPoint)> {
        visible_row_selection(self.parser.screen(), point.row)
    }

    /// Draws the in-progress composition starting at the cursor cell and
    /// returns how many cells it occupied, so the caller can push the cursor
    /// past it. Wide (CJK) characters take two cells, matching the grid.
    fn draw_preedit(&self, surface: &mut Surface<'_>, cursor: (u16, u16)) -> u32 {
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
    fn update_ime_anchor(&self, window: &PixelWindow) {
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
            // Windows binds Ctrl+V to paste everywhere, including its own
            // terminals, and a shell there has no readline quoted-insert to
            // shadow: before this, Ctrl+V reached cmd.exe as 0x16 and printed
            // "^V" (measured in the ARM court, 2026-09-23), which is what a
            // user reads as "paste does not work". Ctrl+Shift+V keeps working
            // for anyone with the habit, and a program that wants a literal
            // 0x16 can still receive it through the control CLI.
            #[cfg(windows)]
            if control
                && !event.modifiers.alt
                && !event.modifiers.shift
                && text.eq_ignore_ascii_case("v")
            {
                self.request_clipboard_paste();
                return Ok(());
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

    fn forward_key(&mut self, event: &NormalizedKeyEvent) {
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
    fn scroll_by(&mut self, lines: isize) {
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

    fn scrollback_bounds(&mut self) -> (usize, usize) {
        if self.parser.screen().alternate_screen() {
            return (0, 0);
        }
        let offset = self.parser.screen().scrollback();
        let maximum = self.parser.screen().scrollback_len();
        self.scroll_offset = offset;
        (offset, maximum)
    }

    fn set_scrollback(&mut self, requested: usize) {
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

    fn scrollbar_geometry(
        &mut self,
        width: u32,
        height: u32,
    ) -> (agenterm_ui_core::ScrollbarGeometry, usize, usize) {
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

    fn handle_scrollbar_event(
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
    fn hit_test(&self, pos: &LogicalPoint) -> TerminalPoint {
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
    fn terminal_point_to_logical(&self, point: TerminalPoint) -> LogicalPoint {
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
    fn active_selection(&self) -> Option<(TerminalPoint, TerminalPoint)> {
        self.selection.filter(|(anchor, focus)| anchor != focus)
    }

    fn copy_selection(&mut self) {
        let Some((start, end)) = self.active_selection() else {
            return;
        };
        let text = selection_text(self.parser.screen(), start, end);
        self.copy_text(&text);
    }

    /// Every copy MiniCon makes goes through here, so the status bar's
    /// clipboard length follows it without waiting to re-read the clipboard.
    fn copy_text(&mut self, text: &str) {
        if text.is_empty() {
            return;
        }
        let _ = clipboard_status::set_text(text);
    }

    fn request_clipboard_paste(&mut self) {
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
            backend_resize_error: self.backend_resize_error.clone(),
            backend_resize_failures: self.backend_resize_failures,
        }
    }

    /// Writes the current snapshot to `--emit-snapshot`'s path, if set.
    /// Errors are deliberately swallowed: a full disk or a test harness that
    /// deleted the target directory mid-run must not crash the session it is
    /// trying to observe.
    fn write_snapshot_if_requested(&mut self) {
        if let Some(path) = self.snapshot_path.clone() {
            let _ = agent_interface::write_snapshot_atomic(&path, &self.build_snapshot());
        }
    }

    /// Current mouse reporting contract negotiated by the running application.
    fn mouse_mode(
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
    fn report_mouse_checked(
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

    fn report_mouse(
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
    fn handle_wheel(
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
    fn handle_pointer_button_checked(
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

    fn handle_pointer_button(
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
    fn handle_pointer_moved_checked(
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
    fn pointer_moved_outcome(
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

    fn handle_pointer_moved(
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

    fn take_cancelled_pointer_release(&mut self) -> Option<(u8, TerminalPoint)> {
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

#[derive(Clone)]
pub(super) struct SessionSeed {
    working_dir: Option<String>,
    control_endpoint: Option<String>,
    command: Option<Vec<String>>,
    font_size_logical: f64,
    font_size_baseline: f64,
    cols: u16,
    rows: u16,
}

impl SessionSeed {
    pub(super) fn from_session(session: &ConTerminal) -> Self {
        Self {
            working_dir: session.working_dir.clone(),
            control_endpoint: session.control_endpoint.clone(),
            command: session.command.clone(),
            font_size_logical: session.font_size_logical,
            font_size_baseline: session.font_size_baseline,
            cols: session.cols,
            rows: session.rows,
        }
    }

    /// The zoom the next session opens at; host chrome scales with it.
    pub(super) fn font_size_logical(&self) -> f64 {
        self.font_size_logical
    }

    pub(super) fn create_session(&self) -> ConTerminal {
        let mut session = ConTerminal::new(self.working_dir.clone(), self.control_endpoint.clone());
        session.command = self.command.clone();
        session.font_size_logical = self.font_size_logical;
        session.font_size_baseline = self.font_size_baseline;
        session.cols = self.cols;
        session.rows = self.rows;
        session
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn parser() -> vt100::Parser<ConCallbacks> {
        vt100::Parser::<ConCallbacks>::new_with_callbacks(24, 80, 0, ConCallbacks::default())
    }
    /// Renders one screen and returns (pixel buffer, cell_w, cell_h) for exact
    /// pixel assertions — the deterministic alternative to eyeballing a
    /// screenshot, which is what actually caught this bug: a screenshot
    /// suggested underline/background/inverse were shifted by a couple of
    /// columns, but that could just as easily have been the screenshot
    /// harness. This settles it in-process.
    pub(crate) fn render_to_buffer(bytes: &[u8], cols: u16, rows: u16) -> (Vec<u32>, u32, u32) {
        let cell_w = 10u32;
        let cell_h = 20u32;
        let mut screen_parser = vt100::Parser::<ConCallbacks>::new_with_callbacks(
            rows,
            cols,
            0,
            ConCallbacks::default(),
        );
        screen_parser.process(bytes);
        let fw = u32::from(cols) * cell_w;
        let fh = u32::from(rows) * cell_h;
        let mut pixels = vec![Rgb(0, 0, 0).to_xrgb(); (fw * fh) as usize];
        let mut surface = Surface::new(&mut pixels, fw, fh);
        paint_cells(
            &mut surface,
            screen_parser.screen(),
            None,
            cell_w,
            cell_h,
            Rgb(0xCC, 0xCC, 0xCC),
            Rgb(0, 0, 0),
            palette::STANDARD_ANSI,
            10,
        );
        (pixels, cell_w, cell_h)
    }
    pub(crate) fn prepared_pointer_terminal() -> ConTerminal {
        let mut app = ConTerminal::new(None, None);
        app.frame_width = 800;
        app.frame_height = 400;
        app.cell_w = 8;
        app.cell_h = 16;
        app.cols = 80;
        app.rows = 24;
        app.scale = 1.0;
        app.dirty = DirtyRegion::empty();
        app
    }
    /// Builds a terminal whose cell metrics, grid and content insets are derived
    /// the same way `opened`/`configure_host_ui` derive them at runtime, so a
    /// coordinate test exercises the real scale pipeline rather than hand-picked
    /// numbers. The bottom inset reserves composer + status, matching
    /// [`ui::bottom_inset`].
    pub(crate) fn pointer_terminal_at(scale: f64, frame_w: u32, frame_h: u32) -> ConTerminal {
        let mut app = ConTerminal::new(None, None);
        app.scale = scale;
        app.recompute_metrics(scale);
        let left = minicon_core::numeric::round_f64(ui::SIDEBAR_WIDTH_DIP * scale) as u32;
        let bottom = ui::bottom_inset(scale);
        app.set_content_insets(left, 0, bottom);
        app.frame_width = frame_w;
        app.frame_height = frame_h;
        let usable_w = frame_w
            .saturating_sub(left)
            .saturating_sub(ui::terminal_scrollbar_width(scale));
        let usable_h = frame_h
            .saturating_sub(app.content_top_px)
            .saturating_sub(bottom);
        let (cols, rows) = ConTerminal::compute_grid(usable_w, usable_h, app.cell_w, app.cell_h);
        app.cols = cols;
        app.rows = rows;
        app.dirty = DirtyRegion::empty();
        app
    }
    pub(crate) fn preedit_surface<'a>(
        pixels: &'a mut [u32],
        width: u32,
        height: u32,
    ) -> Surface<'a> {
        Surface::new(pixels, width, height)
    }
    #[test]
    fn vt_damage_rows_map_to_clamped_content_and_cursor_endpoints() {
        let mut app = ConTerminal::new(None, None);
        app.dirty = DirtyRegion::empty();
        app.frame_width = 100;
        app.frame_height = 60;
        app.content_left_px = 10;
        app.content_top_px = 8;
        app.content_bottom_px = 12;
        app.cell_w = 8;
        app.cell_h = 10;
        app.cols = 8;
        app.rows = 4;
        app.parser.screen_mut().set_size(4, 8);
        let _ = app.parser.take_damage();

        app.parser.process(b"\x1b[2J");
        let damage = app.parser.take_damage();
        assert!(!damage.needs_full_raster());
        app.mark_vt_damage(damage);
        let rows = app.dirty.bounds().expect("row damage has a pixel bound");
        assert_eq!(rows.left, 10);
        assert_eq!(rows.top, 8);
        assert_eq!(rows.right, 74);
        assert_eq!(rows.bottom, 48);
        assert!(!app.dirty.is_full());

        app.dirty = DirtyRegion::empty();
        app.parser.process(b"A");
        let _ = app.parser.take_damage();
        app.parser.process(b"\x1b[1;1H");
        let damage = app.parser.take_damage();
        assert_eq!(damage.cursor_before(), Some((0, 1)));
        assert_eq!(damage.cursor_after(), Some((0, 0)));
        app.mark_vt_damage(damage);
        let cursor = app.dirty.bounds().expect("cursor endpoints are dirty");
        assert_eq!(cursor.left, 10);
        assert_eq!(cursor.right, 34);
        assert_eq!(cursor.top, 8);
        assert_eq!(cursor.bottom, 18);
        assert!(!app.dirty.is_full());
    }
    #[test]
    fn pty_drain_consumes_vt_damage_without_unconditional_full() {
        let mut app = ConTerminal::new(None, None);
        app.pty_output = Arc::new(BoundedOutputPipe::new(1024));
        app.dirty = DirtyRegion::empty();
        app.frame_width = 640;
        app.frame_height = 400;
        app.content_left_px = 10;
        app.content_top_px = 8;
        app.content_bottom_px = 12;
        app.cell_w = 8;
        app.cell_h = 16;
        app.pty_output.push_blocking(b"ASCII").expect("pipe open");

        let outcome = app.drain_pty();
        assert!(outcome.changed);
        assert!(outcome.redraw);
        assert!(!app.dirty.is_full());
        assert!(app.dirty.bounds().is_some());
    }
    #[test]
    fn full_vt_damage_is_the_explicit_safe_fallback() {
        let mut app = ConTerminal::new(None, None);
        app.dirty = DirtyRegion::empty();
        app.frame_width = 640;
        app.frame_height = 400;
        app.parser.screen_mut().mark_full_damage();

        let outcome = app.drain_pty();
        assert!(outcome.redraw);
        assert!(app.dirty.is_full());
    }
    #[test]
    fn scrollback_bounds_uses_read_only_vt_length() {
        let mut app = ConTerminal::new(None, None);
        app.parser.screen_mut().set_size(3, 10);
        let _ = app.parser.take_damage();
        app.parser.process(b"a\r\nb\r\nc\r\nd");
        let _ = app.parser.take_damage();

        let before = app.parser.screen().scrollback();
        let expected = app.parser.screen().scrollback_len();
        let (offset, maximum) = app.scrollback_bounds();
        assert_eq!(offset, before);
        assert_eq!(maximum, expected);
        assert_eq!(app.parser.screen().scrollback(), before);
    }
    #[test]
    fn da1_query_gets_a_reply_queued_for_the_pty() {
        let mut parser = parser();
        parser.process(b"\x1b[c");
        assert_eq!(parser.callbacks().pending_replies, b"\x1b[?1;2c");
    }
    #[test]
    fn cpr_query_reports_the_real_current_cursor_position() {
        let mut parser = parser();
        // Two lines of output move the cursor to row 1 (0-indexed), col 0 —
        // reported 1-indexed per the CPR spec, so row 2, col 1.
        parser.process(b"hello\r\nworld");
        parser.callbacks_mut().pending_replies.clear();
        parser.process(b"\x1b[6n");
        assert_eq!(parser.callbacks().pending_replies, b"\x1b[2;6R");
    }
    #[test]
    fn dsr_ok_query_gets_a_reply_queued() {
        let mut parser = parser();
        parser.process(b"\x1b[5n");
        assert_eq!(parser.callbacks().pending_replies, b"\x1b[0n");
    }
    #[test]
    fn unrecognized_csi_queries_are_left_unanswered_not_guessed_at() {
        // Anything with an intermediate byte (private-mode queries, etc.)
        // or an unrecognized final byte must not get a made-up reply —
        // silence is the correct, honest answer for a query this binary
        // does not actually understand, not a guess that could mislead the
        // caller into thinking a real capability exists.
        let mut parser = parser();
        parser.process(b"\x1b[?15n"); // DEC-private status (printer), unhandled
        assert!(parser.callbacks().pending_replies.is_empty());
    }
    /// The escape-sequence tables are covered exhaustively in
    /// `agenterm_platform::contract::terminal_input`. What matters here is this
    /// host's own policy: that it reads the modes the application negotiated
    /// and hands the shared encoder the right ones.
    #[test]
    fn key_encoding_is_driven_by_live_screen_mode() {
        let mut parser = parser();
        let up = NormalizedKeyEvent {
            logical: LogicalKey::Named(NamedKey::ArrowUp),
            physical: agenterm_platform::input::PhysicalKeyCode::Other,
            text: None,
            state: KeyPressState::Pressed,
            repeat: false,
            modifiers: ModifierState::default(),
        };

        let mode = TerminalKeyMode {
            application_cursor: parser.screen().application_cursor(),
            ime_active: false,
        };
        assert_eq!(
            terminal_input::key_event_to_bytes(&up, mode),
            Some(b"\x1b[A".to_vec()),
            "default mode must use CSI"
        );

        // The application turns on DECCKM; the same keypress must now encode as
        // SS3. Ignoring this is what made vim/less misread arrow keys.
        parser.process(b"\x1b[?1h");
        let mode = TerminalKeyMode {
            application_cursor: parser.screen().application_cursor(),
            ime_active: false,
        };
        assert_eq!(
            terminal_input::key_event_to_bytes(&up, mode),
            Some(b"\x1bOA".to_vec()),
            "DECCKM must switch cursor keys to SS3"
        );
    }
    #[test]
    fn paste_framing_follows_the_application_bracketed_paste_mode() {
        let mut parser = parser();
        assert!(!parser.screen().bracketed_paste());
        let text = terminal_input::normalize_terminal_paste("a\nb");
        assert_eq!(
            terminal_input::terminal_paste_bytes(&text, parser.screen().bracketed_paste()),
            b"a\rb".to_vec()
        );

        parser.process(b"\x1b[?2004h");
        assert!(parser.screen().bracketed_paste());
        assert_eq!(
            terminal_input::terminal_paste_bytes(&text, parser.screen().bracketed_paste()),
            b"\x1b[200~a\rb\x1b[201~".to_vec()
        );
    }
    #[test]
    fn mouse_mode_maps_the_vt100_variants_a_tui_actually_requests() {
        let mut app = ConTerminal::new(None, None);
        assert_eq!(
            app.mouse_mode(),
            (
                terminal_input::ApplicationMouseMode::None,
                terminal_input::MouseReportEncoding::Default
            )
        );

        // ?1002h + ?1006h is what a modern TUI asks for.
        app.parser.process(b"\x1b[?1002h\x1b[?1006h");
        assert_eq!(
            app.mouse_mode(),
            (
                terminal_input::ApplicationMouseMode::ButtonMotion,
                terminal_input::MouseReportEncoding::Sgr
            )
        );
    }
    #[test]
    fn selection_text_joins_rows_with_crlf_and_trims_trailing_blanks() {
        let mut parser = parser();
        parser.process(b"ab\r\ncd");
        let text = selection_text(
            parser.screen(),
            TerminalPoint { row: 0, col: 0 },
            TerminalPoint { row: 1, col: 79 },
        );
        assert_eq!(text, "ab\r\ncd");
    }
    #[test]
    fn scrolling_clamps_to_available_scrollback() {
        let mut app = ConTerminal::new(None, None);
        // Nothing scrolled off yet, so the viewport cannot move up...
        app.scroll_by(10);
        assert_eq!(app.scroll_offset, 0);
        // ...and scrolling down from the bottom must not underflow.
        app.scroll_by(-10);
        assert_eq!(app.scroll_offset, 0);
    }
    #[test]
    fn queued_resize_coalesces_without_synchronously_mutating_the_grid() {
        let mut terminal = ConTerminal::new(None, None);
        let original_grid = (terminal.cols, terminal.rows);
        terminal.queue_resize(900, 600, 1.0);
        terminal.queue_resize(1200, 800, 1.25);
        assert_eq!((terminal.cols, terminal.rows), original_grid);
        assert_eq!(terminal.pending_geometry, Some((1200, 800, 1.25)));
    }
    #[test]
    fn scrolling_up_actually_moves_once_real_content_is_off_screen() {
        // Complements `scrolling_clamps_to_available_scrollback`, which only
        // ever exercises a terminal with nothing scrolled off — a case where
        // "clamped to 0 because there's nothing to see" and "clamped to 0
        // because the bound was computed wrong" are indistinguishable, and
        // did not catch a real bug: `scroll_by`'s old bound was
        // `screen().scrollback() + scroll_offset`, but vendored vt100's
        // `Screen::scrollback()` returns the *current* offset (its own doc
        // comment says so), not the available range — so the bound was
        // always `2 * scroll_offset`, i.e. always 0 from a fresh view, and
        // wheel-up silently never worked in a live session. Only caught by
        // a black-box control `send-wheel` test against a real session with
        // actual scrolled-off lines; this pins the same fact as a fast unit
        // test so it can't regress silently again.
        let mut app = ConTerminal::new(None, None);
        app.parser.screen_mut().set_size(4, 40);
        for line in 0..20 {
            app.parser.process(format!("line{line}\r\n").as_bytes());
        }
        assert_eq!(app.scroll_offset, 0);

        app.scroll_by(3);
        assert_eq!(
            app.scroll_offset, 3,
            "3 lines of real scrollback exist; scrolling up must move"
        );

        // Overshooting clamps to what's actually buffered, not to 0.
        app.scroll_by(1000);
        let max = app.scroll_offset;
        assert!(
            max > 3,
            "clamp must be the real available scrollback, not stuck at the first move"
        );

        app.scroll_by(-1000);
        assert_eq!(
            app.scroll_offset, 0,
            "scrolling back down must return to the bottom"
        );
    }
    /// The same crash one level up, driven the way the product drives it:
    /// a full Ctrl+wheel zoom-in sweep through `apply_resize` against a
    /// shell that keeps printing CJK. Every notch shrinks the column count,
    /// and the CJK text guarantees wide characters sit near whatever the new
    /// right edge turns out to be. Deterministic — no window, no timing.
    ///
    /// The reason this angle went unnoticed for two rounds of investigation
    /// is that the existing zoom stress tests either resize without any
    /// output in flight, or push output through a fixed grid; only doing
    /// both, with *wide* characters, reaches the broken invariant.
    #[test]
    fn zoom_in_sweep_while_printing_cjk_never_aborts() {
        // A localized Windows shell banner is CJK, so this is what a real
        // session looks like from its very first frame — not an exotic case.
        let chunks: [&[u8]; 3] = [
            "Microsoft Windows [版本 10.0.20348.1006]\r\n".as_bytes(),
            "(c) Microsoft Corporation。保留所有权利。\r\n".as_bytes(),
            "C:\\dev> 编译 中文日本語 한국어 ██▒░\r\n".as_bytes(),
        ];
        for &(phys_w, phys_h) in &[(960u32, 600u32), (1280, 400), (420, 900)] {
            for scale_tenths in [10u32, 15, 25] {
                let scale = f64::from(scale_tenths) / 10.0;
                let mut app = ConTerminal::new(None, None);
                app.apply_resize(phys_w, phys_h, scale);
                // One notch per step across the whole clamp range, exactly
                // as `zoom_font` walks it, with output in flight throughout.
                for step in 0..=28u32 {
                    app.font_size_logical = (8.0 + f64::from(step)).clamp(8.0, 36.0);
                    app.apply_resize(phys_w, phys_h, scale);
                    for chunk in &chunks {
                        app.parser.process(chunk);
                    }
                }
                assert!(app.cols >= 2 && app.rows >= 2);
            }
        }
    }
    #[test]
    fn double_click_uses_shared_terminal_word_classes() {
        let mut app = ConTerminal::new(None, None);
        app.parser.screen_mut().set_size(4, 40);
        app.parser.process(b"cd /usr/local/bin (note)");

        // Inside the path: the whole path is one word, because '/', '.', '-'
        // and ':' are word characters here — more useful than conhost's
        // space-only rule.
        let hit = TerminalPoint { row: 0, col: 8 };
        let (start, end) = app.word_at(hit).expect("word under a path cell");
        assert_eq!((start.col, end.col), (3, 16));

        // Parentheses are delimiters, so "note" selects without them.
        let hit = TerminalPoint { row: 0, col: 19 };
        let (start, end) = app.word_at(hit).expect("word inside parens");
        assert_eq!((start.col, end.col), (19, 22));

        // Whitespace is its own terminal word class rather than being folded
        // into either adjacent command token.
        let (start, end) = app
            .word_at(TerminalPoint { row: 0, col: 2 })
            .expect("blank run is selectable");
        assert_eq!((start.col, end.col), (2, 2));
    }
    #[test]
    fn triple_click_selects_only_the_visible_row() {
        let mut app = ConTerminal::new(None, None);
        app.parser.screen_mut().set_size(4, 10);
        // 15 characters over a 10-column grid soft-wraps onto row 1.
        app.parser.process(b"abcdefghijklmno");
        assert!(
            app.parser.screen().row_wrapped(0),
            "row 0 should be wrapped"
        );

        let (start, end) = app
            .line_at(TerminalPoint { row: 1, col: 2 })
            .expect("visible row");
        assert_eq!((start.row, start.col), (1, 0));
        assert_eq!((end.row, end.col), (1, 9));
    }
    #[test]
    fn click_counting_requires_the_same_cell_within_the_window() {
        let mut app = ConTerminal::new(None, None);
        let here = TerminalPoint { row: 1, col: 1 };
        let elsewhere = TerminalPoint { row: 5, col: 5 };

        assert_eq!(app.register_click(here), 1);
        assert_eq!(app.register_click(here), 2);
        assert_eq!(app.register_click(here), 3);
        // A fourth click cycles back to character selection.
        assert_eq!(app.register_click(here), 1);

        // Moving restarts the count, so a fast click in two places cannot
        // accidentally select a word.
        assert_eq!(app.register_click(here), 2);
        assert_eq!(app.register_click(elsewhere), 1);
    }
    /// A plain click seeds a drag anchor, not a selection. Rendering, copying,
    /// and the Ctrl+C / right-click branches must all agree that a degenerate
    /// range is nothing — otherwise one click leaves its cell inverted forever,
    /// bare Ctrl+C copies an empty string instead of interrupting the child,
    /// and right-click stops pasting.
    #[test]
    fn a_click_without_a_drag_is_not_a_selection() {
        let mut app = ConTerminal::new(None, None);
        let point = TerminalPoint { row: 2, col: 4 };

        app.selection = Some((point, point));
        assert_eq!(
            app.active_selection(),
            None,
            "an anchor-only range covers no cells and must not render or copy"
        );

        let dragged = TerminalPoint { row: 2, col: 7 };
        app.selection = Some((point, dragged));
        assert_eq!(
            app.active_selection(),
            Some((point, dragged)),
            "a real drag stays a selection"
        );

        // The stored state agrees with what consumers see, so the next
        // right-click pastes rather than copying nothing.
        app.selection = Some((point, point));
        app.selecting = true;
        if !selection_should_auto_copy(app.selection) {
            app.selection = None;
        }
        assert_eq!(app.selection, None);
    }
    #[test]
    fn underline_paints_under_the_correct_columns_not_shifted() {
        // "AA" plain, then underlined "BB". If underline were misplaced (the
        // shift a screenshot seemed to show), it would land under "AA".
        let (pixels, cell_w, cell_h) = render_to_buffer(b"AA\x1b[4mBB\x1b[0m", 10, 1);
        let underline_y = cell_h - 2;
        let bg = Rgb(0, 0, 0).to_xrgb();

        // No underline under the plain run (cols 0-1).
        for col in 0..2u32 {
            let x = col * cell_w + cell_w / 2;
            assert_eq!(
                pixels[(underline_y * cell_w * 10 + x) as usize],
                bg,
                "col {col} must not be underlined"
            );
        }
        // Underline present under the attributed run (cols 2-3).
        for col in 2..4u32 {
            let x = col * cell_w + cell_w / 2;
            assert_ne!(
                pixels[(underline_y * cell_w * 10 + x) as usize],
                bg,
                "col {col} must be underlined"
            );
        }
    }
    #[test]
    fn background_fill_spans_exactly_the_attributed_columns() {
        // "XX" plain, then two red-background blanks, then plain "YY" again —
        // the fill must start exactly at column 2 and end exactly at column 3.
        // The attributed cells are blank because the sample is the cell's
        // centre pixel: with a glyph there, whether a stroke covers that pixel
        // depends on the font (Cascadia Mono at 10 px on Windows does), and
        // this test is about the fill, not the glyph.
        let (pixels, cell_w, cell_h) = render_to_buffer(b"XX\x1b[41m  \x1b[0mYY", 10, 1);
        let mid_y = cell_h / 2;
        let row_base = (mid_y * cell_w * 10) as usize;
        let red = palette::resolve(
            vt100::Color::Idx(1),
            Rgb(0, 0, 0),
            &palette::STANDARD_ANSI,
            false,
        )
        .to_xrgb();

        let sample = |col: u32| pixels[row_base + (col * cell_w + cell_w / 2) as usize];
        assert_ne!(sample(0), red, "col 0 (plain) must not be red");
        assert_ne!(sample(1), red, "col 1 (plain) must not be red");
        assert_eq!(sample(2), red, "col 2 must be red");
        assert_eq!(sample(3), red, "col 3 must be red");
        assert_ne!(sample(4), red, "col 4 (plain again) must not be red");
    }
    #[test]
    fn inverse_swaps_the_full_attributed_span_not_one_cell() {
        let (pixels, cell_w, cell_h) = render_to_buffer(b"NN\x1b[7mIIII\x1b[0m", 10, 1);
        let mid_y = cell_h / 2;
        let row_base = (mid_y * cell_w * 10) as usize;
        let fg = Rgb(0xCC, 0xCC, 0xCC).to_xrgb();

        // Inverse fills the background with the swapped color across all 4
        // attributed cells (2..6), not just the first one.
        for col in 2..6u32 {
            assert_eq!(
                pixels[row_base + (col * cell_w + cell_w / 2) as usize],
                fg,
                "col {col} must show the inverted background"
            );
        }
    }
    /// A backend that refuses a resize and then accepts the next one leaves a
    /// session that looks healthy and is not: on Windows the console can be
    /// left at the one-cell rectangle the resize sequence shrinks it to, so
    /// every later scrape reads a blank screen while the last resize reports
    /// success. The message answers "now"; only the count answers "ever".
    #[test]
    fn a_healed_resize_still_admits_that_one_was_refused() {
        let mut app = ConTerminal::new(None, None);
        assert_eq!(app.backend_resize_failures, 0);
        assert!(app.backend_resize_error.is_none());

        app.record_resize_outcome(141, 40, Err("bad rect".to_owned()));
        assert_eq!(
            app.backend_resize_error.as_deref(),
            Some("resize to 141x40 failed: bad rect")
        );
        assert_eq!(app.backend_resize_failures, 1);

        app.record_resize_outcome(128, 40, Ok(()));
        assert!(
            app.backend_resize_error.is_none(),
            "a success clears the message, because it describes the present"
        );
        assert_eq!(
            app.backend_resize_failures, 1,
            "and never the count, because that is the only surviving witness"
        );

        app.record_resize_outcome(141, 40, Err("bad rect".to_owned()));
        assert_eq!(app.backend_resize_failures, 2, "failures accumulate");
    }

    #[test]
    fn stress_apply_resize_across_extreme_scale_and_window_sizes() {
        // Reproduce a reported crash: "font grows past a certain size and the
        // program exits." Sweep scale factors (simulating high-DPI displays
        // this dev machine does not have) crossed with window sizes from tiny
        // to large, at every font size in the allowed range, and confirm
        // apply_resize never panics and never produces a zero-sized grid.
        for scale_tenths in 5..=40 {
            let scale = f64::from(scale_tenths) / 10.0;
            for logical in [8.0, 20.0, 36.0] {
                for &(w, h) in &[(1u32, 1u32), (50, 50), (960, 600), (3840, 2160)] {
                    let mut app = ConTerminal::new(None, None);
                    app.font_size_logical = logical;
                    app.apply_resize(w, h, scale);
                    assert!(
                        app.cols >= 2,
                        "cols degenerated at scale={scale} logical={logical} w={w} h={h}"
                    );
                    assert!(
                        app.rows >= 2,
                        "rows degenerated at scale={scale} logical={logical} w={w} h={h}"
                    );
                    assert!(app.cell_w > 0);
                    assert!(app.cell_h > 0);
                }
            }
        }
    }
    #[test]
    fn decscusr_selects_shape_and_blink() {
        let mut parser = parser();
        // Default before any DECSCUSR: blinking block.
        assert_eq!(parser.screen().cursor_shape(), vt100::CursorShape::Block);
        assert!(parser.screen().cursor_blinking());

        parser.process(b"\x1b[6 q"); // steady bar (insert-mode convention)
        assert_eq!(parser.screen().cursor_shape(), vt100::CursorShape::Bar);
        assert!(!parser.screen().cursor_blinking());

        parser.process(b"\x1b[3 q"); // blinking underline
        assert_eq!(
            parser.screen().cursor_shape(),
            vt100::CursorShape::Underline
        );
        assert!(parser.screen().cursor_blinking());

        parser.process(b"\x1b[2 q"); // steady block
        assert_eq!(parser.screen().cursor_shape(), vt100::CursorShape::Block);
        assert!(!parser.screen().cursor_blinking());

        // Out-of-range resets to the default rather than leaving stale state.
        parser.process(b"\x1b[9 q");
        assert_eq!(parser.screen().cursor_shape(), vt100::CursorShape::Block);
        assert!(parser.screen().cursor_blinking());
    }
    #[test]
    fn blink_toggles_on_the_configured_interval_and_resets_on_keystroke() {
        let mut app = ConTerminal::new(None, None);
        assert!(app.blink_visible);
        let start = app.last_blink_at;

        // Simulate the interval having elapsed by moving the recorded time
        // into the past rather than sleeping — deterministic and instant.
        app.last_blink_at = start - BLINK_INTERVAL - Duration::from_millis(1);
        let due = app.last_blink_at;
        let now = Instant::now();
        assert!(now.saturating_duration_since(due) >= BLINK_INTERVAL);

        // A keystroke must force the cursor back to visible immediately,
        // regardless of blink phase — this is what stops "did that key even
        // register?" moments.
        app.blink_visible = false;
        let key = NormalizedKeyEvent {
            logical: LogicalKey::Character("a".to_owned()),
            physical: agenterm_platform::input::PhysicalKeyCode::Other,
            text: Some("a".to_owned()),
            state: KeyPressState::Pressed,
            repeat: false,
            modifiers: ModifierState::default(),
        };
        app.forward_key(&key);
        assert!(app.blink_visible);
    }
    /// Windows binds Ctrl+V to paste everywhere, so a terminal that forwards
    /// it as 0x16 looks broken: cmd.exe prints "^V" and nothing is pasted,
    /// which is exactly what a user reported against 0.1.22. Ctrl+Shift+V
    /// stays for the habit, and Ctrl+V with Alt still belongs to the program.
    #[cfg(windows)]
    #[test]
    fn ctrl_v_asks_for_a_paste_on_windows_instead_of_reaching_the_shell() {
        let mut app = prepared_pointer_terminal();
        let press = |text: &str, control: bool, shift: bool, alt: bool| NormalizedKeyEvent {
            logical: LogicalKey::Character(text.to_owned()),
            physical: agenterm_platform::input::PhysicalKeyCode::Other,
            text: Some(text.to_owned()),
            state: KeyPressState::Pressed,
            repeat: false,
            modifiers: ModifierState {
                control,
                shift,
                alt,
                ..ModifierState::default()
            },
        };

        app.forward_key_checked(&press("v", true, false, false))
            .expect("ctrl+v");
        assert!(
            app.take_clipboard_paste_request(),
            "Ctrl+V must ask the host for a paste"
        );

        app.forward_key_checked(&press("v", true, true, false))
            .expect("ctrl+shift+v");
        assert!(
            app.take_clipboard_paste_request(),
            "Ctrl+Shift+V must keep asking for a paste"
        );

        // Ctrl+Alt+V belongs to the program: it is forwarded, so in this
        // fixture it reaches a PTY that was never opened. That error is the
        // proof it was not consumed as a host shortcut.
        let forwarded = app.forward_key_checked(&press("v", true, false, true));
        assert!(
            !app.take_clipboard_paste_request(),
            "Ctrl+Alt+V belongs to the program, not the host"
        );
        assert!(
            forwarded.is_err(),
            "Ctrl+Alt+V must reach the program's PTY"
        );
    }

    /// A pointer or control coordinate past the grid's right/bottom edge must
    /// land on the last cell, not an off-grid column or row. Row already
    /// clamped; the column did not, so a coordinate to the right of the grid
    /// produced an out-of-grid cell that could seed a phantom-width selection.
    #[test]
    fn hit_test_clamps_both_axes_to_the_last_cell() {
        let app = prepared_pointer_terminal(); // 80x24 grid, 8x16 cells, scale 1
        // In-grid coordinates map straight through (no clamp applied).
        let inside = app.hit_test(&LogicalPoint { x: 100.0, y: 160.0 });
        assert_eq!((inside.col, inside.row), (12, 10));
        // Far past the right and bottom edges: clamp to the last col and row.
        let outside = app.hit_test(&LogicalPoint {
            x: 100_000.0,
            y: 100_000.0,
        });
        assert_eq!((outside.col, outside.row), (79, 23));
        // Exactly on the trailing edge of the last cell stays on the last cell.
        let edge = app.hit_test(&LogicalPoint { x: 640.0, y: 384.0 });
        assert_eq!((edge.col, edge.row), (79, 23));
    }
    /// Every grid cell must survive a `terminal_point_to_logical` →
    /// `hit_test` round trip at any DPI scale. This is the coordinate-consistency
    /// invariant behind mouse forwarding: a cell's center, converted to a logical
    /// pointer and hit-tested back, must return that same cell — otherwise a real
    /// click on a mouse-tracking TUI reports the wrong cell (or none). The scales
    /// include the fractional values real Windows displays report, where a
    /// physical/logical mix-up would surface.
    #[test]
    fn every_cell_round_trips_through_logical_at_every_scale() {
        for scale in [1.0, 1.25, 1.5, 2.0, 2.5] {
            let app = pointer_terminal_at(scale, 1600, 900);
            assert!(app.cols >= 2 && app.rows >= 2, "degenerate grid at {scale}");
            for row in 0..app.rows {
                for col in 0..app.cols {
                    let point = TerminalPoint { row, col };
                    let logical = app.terminal_point_to_logical(point);
                    let back = app.hit_test(&logical);
                    assert_eq!(
                        (back.col, back.row),
                        (col, row),
                        "cell ({col},{row}) failed the logical round trip at scale {scale}"
                    );
                }
            }
        }
    }
    /// The last physically-clickable pixel of the terminal viewport — one pixel
    /// above the reserved host-UI band — must map to the last grid cell, and the
    /// first pixel of that band must NOT (it belongs to the composer). Together
    /// with the `ui::tests` tiling invariant this proves the terminal's clickable
    /// area meets the composer with neither a dead strip nor an overlap, at scale.
    #[test]
    fn the_terminal_viewport_meets_the_host_ui_band_exactly() {
        for scale in [1.0, 1.5, 2.0] {
            let app = pointer_terminal_at(scale, 1600, 900);
            let viewport_bottom_px = app.frame_height.saturating_sub(app.content_bottom_px);
            // One physical pixel above the band, converted to a logical pointer.
            let inside = LogicalPoint {
                x: f64::from(app.content_left_px + 2) / scale,
                y: f64::from(viewport_bottom_px - 1) / scale,
            };
            let cell = app.hit_test(&inside);
            assert_eq!(
                cell.row,
                app.rows - 1,
                "last viewport pixel row must hit the last grid row at scale {scale}"
            );
        }
    }
    #[test]
    fn preedit_advance_counts_only_the_cells_it_drew() {
        // A surface 5 cells wide and 2 rows tall, with a 1-cell cursor row.
        let mut app = prepared_pointer_terminal();
        app.content_left_px = 0;
        app.content_top_px = 0;

        // A short preedit that fits entirely: every cell is drawn.
        app.ime_preedit = "abc".to_owned();
        let mut pixels = vec![0u32; 40 * 32];
        let advance = app.draw_preedit(&mut preedit_surface(&mut pixels, 40, 32), (0, 0));
        assert_eq!(advance, 3, "three one-cell characters occupy three cells");

        // A double-width character counts two, not one.
        app.ime_preedit = "a\u{4e2d}".to_owned();
        let mut pixels = vec![0u32; 40 * 32];
        let advance = app.draw_preedit(&mut preedit_surface(&mut pixels, 40, 32), (0, 0));
        assert_eq!(advance, 3, "a wide glyph owns two cells");

        // Ten cells of preedit into a five-cell wide surface: only the cells
        // that fit are drawn and reported; the rest are dropped.
        app.ime_preedit = "abcdefghij".to_owned();
        let mut pixels = vec![0u32; 40 * 32];
        let advance = app.draw_preedit(&mut preedit_surface(&mut pixels, 40, 32), (0, 0));
        assert_eq!(
            advance, 5,
            "a preedit wider than the surface must stop at its edge"
        );

        // A preedit beginning at the last column has room for exactly one.
        app.ime_preedit = "abcd".to_owned();
        let mut pixels = vec![0u32; 40 * 32];
        let advance = app.draw_preedit(&mut preedit_surface(&mut pixels, 40, 32), (0, 4));
        assert_eq!(advance, 1, "only the last column fits");

        // A cursor row below the surface draws nothing and advances nothing.
        app.ime_preedit = "abcd".to_owned();
        let mut pixels = vec![0u32; 40 * 32];
        let advance = app.draw_preedit(&mut preedit_surface(&mut pixels, 40, 32), (99, 0));
        assert_eq!(advance, 0, "an off-surface row cannot draw");

        // An empty preedit is zero cells and writes nothing.
        app.ime_preedit = String::new();
        let mut pixels = vec![0u32; 40 * 32];
        let advance = app.draw_preedit(&mut preedit_surface(&mut pixels, 40, 32), (0, 0));
        assert_eq!(advance, 0);
        assert!(
            pixels.iter().all(|pixel| *pixel == 0),
            "an empty preedit must not paint"
        );
    }
    #[test]
    fn idle_pointer_motion_does_not_dirty_an_unchanged_selection() {
        let mut app = prepared_pointer_terminal();
        app.selection = Some((
            TerminalPoint { row: 0, col: 0 },
            TerminalPoint { row: 0, col: 8 },
        ));
        let position = app.terminal_point_to_logical(TerminalPoint { row: 2, col: 4 });
        let (outcome, needs_redraw) = app
            .pointer_moved_outcome(position, &ModifierState::default())
            .expect("hover over a live terminal");
        assert_eq!(outcome.route, "noop");
        assert!(!outcome.changed);
        assert!(!needs_redraw);
        assert!(app.dirty.is_empty());
    }
    #[test]
    fn selection_drag_dirties_only_when_the_focus_cell_changes() {
        let mut app = prepared_pointer_terminal();
        let anchor = TerminalPoint { row: 0, col: 0 };
        app.selecting = true;
        app.selection = Some((anchor, TerminalPoint { row: 0, col: 2 }));
        let same = app.terminal_point_to_logical(TerminalPoint { row: 0, col: 2 });
        let (outcome, needs_redraw) = app
            .pointer_moved_outcome(same, &ModifierState::default())
            .expect("same-cell drag");
        assert_eq!(outcome.route, "selection");
        assert!(!outcome.changed);
        assert!(!needs_redraw);
        assert!(app.dirty.is_empty());

        let next = app.terminal_point_to_logical(TerminalPoint { row: 0, col: 5 });
        let (outcome, needs_redraw) = app
            .pointer_moved_outcome(next, &ModifierState::default())
            .expect("cell-changing drag");
        assert_eq!(outcome.route, "selection");
        assert!(outcome.changed);
        assert!(needs_redraw);
        assert!(!app.dirty.is_empty());
    }
    #[test]
    fn hidden_or_scrolled_cursor_does_not_arm_the_blink_timer() {
        let mut app = ConTerminal::new(None, None);
        assert!(app.cursor_blink_is_live());
        app.scroll_offset = 3;
        assert!(!app.cursor_blink_is_live());
        app.scroll_offset = 0;
        app.parser.process(b"\x1b[?25l");
        assert!(!app.cursor_blink_is_live());
        app.parser.process(b"\x1b[?25h");
        assert!(app.cursor_blink_is_live());
        app.parser.process(b"\x1b[2 q");
        assert!(!app.cursor_blink_is_live());
    }
    #[test]
    fn cursor_shape_default_is_block_absent_any_decscusr() {
        // Regression guard: paint_cells and the cursor overlay must agree
        // with vt100's own default, or a fresh terminal would draw the wrong
        // cursor shape from the very first frame.
        let parser = parser();
        assert_eq!(parser.screen().cursor_shape(), vt100::CursorShape::Block);
    }
    #[test]
    fn arrow_left_key_command_produces_the_expected_csi_bytes() {
        // Isolates the encoder from the ConPTY/cmd.exe environment: if this
        // passes but a real session's cursor still does not move, the bug is
        // downstream of write_pty, not in event construction or encoding.
        let mut app = ConTerminal::new(None, None);
        app.master = None; // no real PTY; we only care what bytes WOULD be sent
        // Reconstruct exactly what inject_key builds, bypassing
        // forward_key's PTY write so we can inspect the encoder's output
        // directly via the same TerminalKeyMode computation forward_key uses.
        let mode = TerminalKeyMode {
            application_cursor: app.parser.screen().application_cursor(),
            ime_active: app.ime_attached,
        };
        let event = NormalizedKeyEvent {
            logical: LogicalKey::Named(NamedKey::ArrowLeft),
            physical: PhysicalKeyCode::Other,
            text: None,
            state: KeyPressState::Pressed,
            repeat: false,
            modifiers: ModifierState::default(),
        };
        let bytes = terminal_input::key_event_to_bytes(&event, mode);
        assert_eq!(bytes, Some(b"\x1b[D".to_vec()));
    }
    #[test]
    fn capture_loss_cancels_local_selection_and_pairs_raw_mouse_release() {
        let mut app = ConTerminal::new(None, None);
        app.mouse_dragging = true;
        app.selecting = true;
        app.active_button = Some(2);
        app.last_reported_cell = Some(TerminalPoint { row: 7, col: 11 });

        assert_eq!(
            app.take_cancelled_pointer_release(),
            Some((2, TerminalPoint { row: 7, col: 11 }))
        );
        assert!(!app.mouse_dragging);
        assert!(!app.selecting);
        assert_eq!(app.active_button, None);
        assert_eq!(app.take_cancelled_pointer_release(), None);
    }
    #[test]
    fn application_mouse_failure_does_not_commit_reported_cell() {
        let mut app = ConTerminal::new(None, None);
        app.parser.process(b"\x1b[?1000h");
        let point = TerminalPoint { row: 2, col: 3 };

        let error = app
            .report_mouse_checked(0, point, true, false, &ModifierState::default())
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(app.last_reported_cell, None);
        assert!(!app.mouse_dragging);
        assert_eq!(app.active_button, None);
    }
    #[test]
    fn alternate_screen_wheel_propagates_closed_pty() {
        let mut app = ConTerminal::new(None, None);
        app.parser.process(b"\x1b[?1049h");

        let error = app
            .handle_wheel(-1.0, &ModifierState::default(), None)
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
    }
    #[test]
    fn injected_terminal_key_propagates_closed_pty() {
        let mut app = ConTerminal::new(None, None);

        let error = app
            .inject_key(InjectedKey::Char('a'), false, false, false)
            .unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(app.scroll_offset, 0);
    }
    #[test]
    fn failed_terminal_paste_does_not_commit_live_view_scroll() {
        let mut app = ConTerminal::new(None, None);
        app.scroll_offset = 7;

        let error = app.paste_text("retry me").unwrap_err();

        assert_eq!(error.kind(), std::io::ErrorKind::BrokenPipe);
        assert_eq!(app.scroll_offset, 7);
    }
    /// One builder, because two of them drifted: the OSC path and the
    /// activation path formatted the window title independently, so the same
    /// window read differently depending on which had written it last.
    #[test]
    fn every_path_builds_the_same_window_title() {
        let mut terminal = ConTerminal::new(None, None);
        let product = product_window_title();
        assert_eq!(product, format!("MiniCon {}", env!("CARGO_PKG_VERSION")));
        terminal.current_title = "deploy".to_owned();
        assert_eq!(terminal.window_title(), format!("deploy — {product}"));
        terminal.current_title = "cmd".to_owned();
        assert_eq!(terminal.window_title(), format!("cmd — {product}"));
        assert!(
            terminal.window_title().contains(env!("CARGO_PKG_VERSION")),
            "a taskbar title names the MiniCon version this binary was built with"
        );
        assert!(
            !terminal.window_title().contains("新宋体") && !terminal.window_title().contains('@'),
            "a taskbar title carries neither a font diagnostic nor a machine id"
        );
    }
    /// A new tab inherits the active terminal's launch configuration through
    /// `SessionSeed`, which copies the fields by hand in two places. Pin the
    /// round trip so adding a field to `ConTerminal` and forgetting it here
    /// fails this test instead of silently giving new tabs a different config.
    #[test]
    fn session_seed_round_trips_every_inherited_field() {
        let mut source = ConTerminal::new(Some("C:\\work".to_owned()), None);
        source.command = Some(vec!["cmd.exe".to_owned(), "/K".to_owned()]);
        source.font_size_logical = 21.5;
        source.font_size_baseline = 18.0;
        source.cols = 101;
        source.rows = 37;

        let seeded = SessionSeed::from_session(&source).create_session();
        assert_eq!(seeded.working_dir, source.working_dir);
        assert_eq!(seeded.command, source.command);
        assert_eq!(seeded.font_size_logical, source.font_size_logical);
        assert_eq!(seeded.font_size_baseline, source.font_size_baseline);
        assert_eq!(seeded.cols, source.cols);
        assert_eq!(seeded.rows, source.rows);

        // A fresh session starts with no PTY until `opened` spawns one; the
        // seed must not carry a live handle across tabs.
        assert!(seeded.master.is_none());
        assert!(seeded.child.is_none());
    }
}
