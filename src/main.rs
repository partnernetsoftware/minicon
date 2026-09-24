//! `minicon` — a minimal console host (conhost equivalent).
//!
//! Like Windows `conhost.exe`, it owns the terminal window, renders cells
//! into a pixel surface, and forwards keyboard input to shells running inside
//! independent PTYs. It has an in-window tab tree, but deliberately does not
//! implement a persisted workspace, Fleet, mux, server, or script runtime.
//!
//! Design priority: **stability**. The terminal that TUI agents and CLI tools
//! crash inside most often dies during resize storms or VT-sequence floods, so
//! resize is trailing-edge debounced, the PTY reader runs on its own thread
//! (never blocking the render path), and the VT parser is the same one the
//! product terminal already hardened.

// GUI subsystem: prevents conhost from attaching a console window.
// Earlier this was omitted to work around cmd.exe exit(0), but that root
// cause turned out to be PtyChild drop (Job Object kill), now fixed by
// keeping the child handle alive for the session lifetime.
#![cfg_attr(windows, windows_subsystem = "windows")]

mod a11y;
mod agent_interface;
use minicon_core::{composer, json, keymap};

mod host_paint;
mod host_ui;
use host_ui::{
    BUTTON_HINT_SIZE_PX, BUTTON_LABEL_SIZE_PX, HeaderIcon, host_ui_text_width, paint_button_label,
    paint_header_icon_button, paint_host_ui_text, paint_host_ui_text_parts,
    paint_host_ui_text_parts_clipped, paint_settings_panel, paint_status_bar,
    paint_two_line_button_label, scaled_host_ui_font, stroke_rect,
};
mod cli;
use cli::{ConArgs, offline_cli_exit, parse_args};
#[cfg(test)]
use cli::{FEATURES, status_text, usage_text};
mod clipboard_status;
mod control;
mod control_dispatch;
mod control_pending;
mod font;
mod ime_mode;
#[cfg(target_os = "linux")]
mod linux_startup;
mod palette;
mod perf;
mod raster_surface;
mod session_store;
#[cfg(windows)]
mod startup;
mod terminal;
mod terminal_paint;
pub(crate) use terminal::{ConTerminal, SessionSeed};
mod text_contrast;
mod theme;
mod ui;
mod workspace;

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use agent_interface::ScreenSnapshot;
use agenterm_platform::input::{
    KeyPressState, LogicalKey, ModifierState, NamedKey, NormalizedKeyEvent, PhysicalKeyCode,
};
use agenterm_platform::pty::{BoundedOutputPipe, ChildCommand, PtyChild, PtyMaster, TerminalSize};
use agenterm_platform::terminal_input::{self, TerminalKeyMode};
use agenterm_platform::window_host::{
    GeometryChange, LogicalPoint, LogicalSize, PixelBackingRetention, PixelFrameWrite,
    PixelPointerCursor, PixelRect as HostPixelRect, PixelWindow, PixelWindowApplication,
    PixelWindowDirective, PixelWindowError, PixelWindowEvent, PixelWindowOptions, PointerButton,
    PointerButtonState, WheelDelta, WindowWaker, XrgbPixelFrame, run_pixel_window,
};
use agenterm_ui_core::{DirtyRegion, DirtyRows, PixelRect};
use agenterm_ui_core::{
    ScrollbarHit, ScrollbarThumbDrag, scrollback_for_thumb_top, scrollbar_hit_test,
};

use control_pending::{PendingControl, WaitKind, WaitProbe};
use palette::Rgb;
use perf::PerfStats;
use raster_surface::{CellRect, Surface};
use session_store::SessionStore;
#[cfg(test)]
use terminal_paint::paint_cells;
use terminal_paint::{CursorPaintSpec, cursor_visible, paint_cells_at, paint_cursor};

/// VT callback storage for OSC sequences (window title, etc.) and terminal
/// query replies (see `unhandled_csi` below) that need to be written back
/// to the PTY.
#[derive(Default)]
struct ConCallbacks {
    title: Option<String>,
    /// Bytes queued by a terminal-query reply (DA1/CPR/DSR — see
    /// `unhandled_csi`), drained and written to the PTY by `drain_pty`
    /// right after the batch of input that produced them finishes
    /// processing. A callback only gets `&mut Screen`, not PTY write
    /// access, so this is the seam between "recognized a query" and
    /// "actually answered it."
    pending_replies: Vec<u8>,
}

impl vt100::Callbacks for ConCallbacks {
    fn set_window_title(&mut self, _: &mut vt100::Screen, title: &[u8]) {
        self.title = Some(String::from_utf8_lossy(title).trim().to_string());
    }

    /// Real, previously-missing terminal-query support — discovered as a
    /// genuine hang, not a cosmetic gap: `claude` (a modern, real-world
    /// Node/Ink TUI) run inside this binary via `-e` produced zero output
    /// and never returned, indefinitely, while the identical command via a
    /// plain `cmd.exe /c` outside minicon completed in under a
    /// second. Root cause, confirmed by reading vendored vt100's own
    /// `csi_dispatch`: neither DA1 (`CSI c`, "what are you") nor CPR
    /// (`CSI 6n`, "where is the cursor") is in its handled-final-byte list
    /// for the no-intermediate case — both fall through to
    /// `unhandled_csi`, which every terminal-facing callback in this
    /// codebase left as the trait's no-op default. A program that queries
    /// the terminal and *blocks* waiting for a reply before proceeding —
    /// exactly what sophisticated TUIs do to detect real capabilities —
    /// hangs forever against a terminal that never answers. This is very
    /// likely the deeper, more general version of "some effects don't
    /// render in real TUI programs": a program that never gets past its
    /// own capability probe never gets to rendering anything at all.
    fn unhandled_csi(
        &mut self,
        screen: &mut vt100::Screen,
        intermediate1: Option<u8>,
        _intermediate2: Option<u8>,
        params: &[&[u16]],
        final_byte: char,
    ) {
        // Private-mode sequences (`CSI ? ...`) and anything else with an
        // intermediate byte are a different, larger space (DEC private
        // mode queries, etc.) — out of scope for this fix, which targets
        // specifically the two queries proven to actually hang a real
        // program.
        if intermediate1.is_some() {
            return;
        }
        match final_byte {
            // DA1 (Primary Device Attributes). Real terminals differ in
            // exact capability bits; `\x1b[?1;2c` ("VT100 with Advanced
            // Video Option") is the same class of minimal-but-valid answer
            // xterm and other emulators have shipped as a baseline for
            // decades — enough for a program that just wants confirmation
            // something is listening before it proceeds.
            'c' => self.pending_replies.extend_from_slice(b"\x1b[?1;2c"),
            'n' => match params.first().and_then(|p| p.first()) {
                // CPR (Cursor Position Report), 1-indexed per the spec —
                // reads the screen's actual current cursor position, not a
                // placeholder, so a program that positions itself relative
                // to the reported location gets the truth.
                Some(6) => {
                    let (row, col) = screen.cursor_position();
                    let reply = format!("\x1b[{};{}R", row + 1, col + 1);
                    self.pending_replies.extend_from_slice(reply.as_bytes());
                }
                // DSR "are you OK?" -> "0n" (terminal OK, no malfunction).
                Some(5) => self.pending_replies.extend_from_slice(b"\x1b[0n"),
                _ => {}
            },
            _ => {}
        }
    }
}

use agenterm_ui_core::terminal_selection::{
    TerminalPoint, normalize_endpoints, terminal_selection_text, visible_row_selection,
    word_selection,
};

fn selection_should_auto_copy(selection: Option<(TerminalPoint, TerminalPoint)>) -> bool {
    selection.is_some_and(|(anchor, focus)| anchor != focus)
}

/// The span a left-button drag covered while the application owned the
/// mouse, if it covered any text.
///
/// A program that turns on mouse reporting (Claude Code, vim, htop) receives
/// every drag, so MiniCon never makes a local selection and the copy-on-select
/// every other drag gets never happened: the text the user just swept over
/// was not on the clipboard. MiniCon still knows where the drag began and
/// ended, and the screen under it, so it copies that span first and then
/// hands the release to the program. Only the left button, and only a drag
/// that moved -- a click is a click, and a right or middle drag is not a
/// selection gesture.
fn application_drag_copy_span(
    button: u8,
    anchor: Option<TerminalPoint>,
    release: TerminalPoint,
) -> Option<(TerminalPoint, TerminalPoint)> {
    const LEFT: u8 = 0;
    let anchor = anchor.filter(|_| button == LEFT)?;
    (anchor != release).then_some((anchor, release))
}

/// Extracts text from the VT screen between two points (inclusive).
/// Produces Windows CRLF line joins, trims trailing whitespace per row.
fn selection_text(screen: &vt100::Screen, a: TerminalPoint, b: TerminalPoint) -> String {
    terminal_selection_text(screen, a, b)
}

/// Trailing-edge debounce for resize: drag storms produce dozens of geometry
/// events per second. We keep only the latest metrics and apply a single resize
/// once the stream has been quiet for this long, so TUI apps see one clean
/// SIGWINCH/ConPTY resize instead of a redraw storm.
const RESIZE_DEBOUNCE: Duration = Duration::from_millis(60);

/// How long a composer submission's Enter is held back after its payload.
///
/// Measured with a raw-mode probe: writing the bracketed paste and the CR back
/// to back — even as two separate `write` calls — still arrives in the child's
/// *single* `read()`. Only an actual gap lets the child consume the paste first
/// and then see the commit as its own key press. See
/// [`composer_submission_parts`].
const COMPOSER_ENTER_DELAY: Duration = Duration::from_millis(12);

/// Read buffer for the PTY pump thread.
const READ_BUF: usize = 8192;

/// Cursor blink half-period, matching the Windows default caret blink rate
/// rather than reading GetCaretBlinkTime, for the same reason as above.
const BLINK_INTERVAL: Duration = Duration::from_millis(530);

/// Scrollback retained by the vt100 model.
const SCROLLBACK: usize = 4000;
const PTY_QUEUE_BYTES: usize = READ_BUF * 128;
const PTY_DRAIN_BUDGET_BYTES: usize = 128 * 1024;
const CONTROL_DRAIN_BUDGET_REQUESTS: usize = 2;

fn pty_drain_budget_per_session(session_count: usize) -> usize {
    if session_count == 0 {
        return 0;
    }
    (PTY_DRAIN_BUDGET_BYTES / session_count).max(1)
}

#[cfg(test)]
mod pty_budget_tests {
    use super::*;

    #[test]
    fn wake_budget_is_shared_without_multiplying_by_tab_count() {
        assert_eq!(pty_drain_budget_per_session(0), 0);
        assert_eq!(pty_drain_budget_per_session(1), PTY_DRAIN_BUDGET_BYTES);
        assert_eq!(pty_drain_budget_per_session(2), PTY_DRAIN_BUDGET_BYTES / 2);
        for count in 1..=256 {
            assert!(pty_drain_budget_per_session(count) * count <= PTY_DRAIN_BUDGET_BYTES);
        }
    }
}

/// Logical (DIP) font size. 15 px is approximately 11.25 pt at 96 DPI and
/// visually matches the 14 px tree labels. The previous value `11` was
/// pixels, not points, and therefore rendered smaller than intended.
const DEFAULT_FONT_PX: f64 = 15.0;

/// Composer text geometry at the default zoom, shared by the painter and the IME candidate
/// placement so the caret they each compute cannot land in two places.
const COMPOSER_TEXT_SIZE_PX: u16 = 15;
const COMPOSER_TEXT_INSET: u32 = 10;
const HOST_UI_HEADER_SIZE_PX: u16 = 14;
const HOST_UI_TAB_SIZE_PX: u16 = 16;
const HOST_UI_CLOSE_SIZE_PX: u16 = 13;
const HOST_UI_STATUS_SIZE_PX: u16 = 14;

#[allow(clippy::manual_clamp)] // f64::clamp retains the large float-format panic path.
fn clamp_font_size(value: f64) -> f64 {
    if value < 8.0 {
        8.0
    } else if value > 36.0 {
        36.0
    } else {
        value
    }
}

/// Configuration loaded from `minicon.json` (analogous to conhost
/// "Defaults" — persist font size, window geometry, etc. without a GUI dialog).
///
/// Location: platform user-config directory + `minicon.json`
/// (Windows Roaming AppData, Unix `~/.config`, see `runtime::user_config_directory`).
#[derive(Default)]
struct ConConfig {
    font_size: Option<f64>,
    cols: Option<u16>,
    rows: Option<u16>,
}

fn config_path() -> Option<std::path::PathBuf> {
    agenterm_platform::runtime::user_config_directory()
        .ok()
        .map(|directory| directory.join("minicon.json"))
}

#[inline(never)]
fn load_config() -> ConConfig {
    let Some(path) = config_path() else {
        return ConConfig::default();
    };
    let Ok(bytes) = agenterm_platform::filesystem_read::read_bounded(&path, json::MAX_INPUT_BYTES)
    else {
        return ConConfig::default();
    };
    let Ok(config) = json::parse_config(&bytes) else {
        return ConConfig::default();
    };
    ConConfig {
        font_size: config.font_size,
        cols: config.cols,
        rows: config.rows,
    }
}

/// Record a panic to the diagnostics log (and the parent console when one is
/// attached) before the default handler unwinds or aborts the process.
///
/// The release profile unwinds, but the message only reaches a console — which
/// a windowed launch does not have. This keeps the default behavior (the panic
/// still unwinds afterwards) and only adds a durable record. Everything it
/// touches is best-effort: a panic hook that can itself fail is worse than none.
fn install_panic_diagnostics() {
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let location = info
            .location()
            .map_or_else(|| "unknown".to_owned(), |location| location.to_string());
        let payload = info
            .payload()
            .downcast_ref::<&str>()
            .map(|text| (*text).to_owned())
            .or_else(|| info.payload().downcast_ref::<String>().cloned())
            .unwrap_or_else(|| "non-string panic payload".to_owned());
        let thread = std::thread::current();
        let thread_name = thread.name().unwrap_or("unnamed");
        let detail = format!("thread={thread_name} at={location} panic={payload}");
        agenterm_platform::diagnostics::record("panic", "unhandled_panic", &detail);
        // No `parent_console` write here: attaching the parent console from a
        // crash path is the one moment a windowed host must not touch the
        // console that launched it. The log line above is the durable record.
        default_hook(info);
    }));
}

fn main() {
    // A GUI host has nowhere to report a panic: launched from a shell icon it
    // owns no console, its window may already be gone, and the release binary
    // is stripped — so the user can only report "it vanished". Install a hook
    // that records the panic (location, message, and the thread name) before
    // the default handler runs, so the next "闪退" leaves a line that names
    // itself. The hook must never panic; `record` is written for a failure path.
    install_panic_diagnostics();
    let args = match agenterm_platform::runtime::application_arguments() {
        Ok(args) => args,
        Err(error) => {
            let message = format!("error: cannot read process arguments: {error}\n");
            let _ = agenterm_platform::parent_console::write_stderr(&message);
            std::process::exit(2);
        }
    };

    // Before anything else, including argument parsing. On a Windows build
    // without a pseudoconsole this binary re-executes itself to host a
    // child's hidden console; in that mode the process is not the terminal,
    // it only shares the file, and must not open a window or a control
    // endpoint.
    if let Some(code) = agenterm_platform::pty::run_if_console_agent(&args) {
        std::process::exit(code);
    }

    if let Some(code) = offline_cli_exit(&args) {
        std::process::exit(code);
    }

    let parsed = match parse_args(&args) {
        Ok(parsed) => parsed,
        Err(message) => {
            let _ = agenterm_platform::parent_console::write_stderr(&message);
            std::process::exit(2);
        }
    };
    #[cfg(target_os = "linux")]
    if let Err(message) = linux_startup::preflight() {
        let _ = agenterm_platform::parent_console::write_stderr(&format!("minicon: {message}\n"));
        std::process::exit(1);
    }
    let ConArgs {
        mut no_activate,
        working_dir,
        font_size,
        cols: initial_cols,
        rows: initial_rows,
        control_endpoint,
        command,
        snapshot_path,
        features,
        headless,
    } = parsed;
    // The classic Windows console is the default host because mouse input does
    // not survive our ConPTY path today, which left every hosted TUI blind to
    // clicks — confirmed on a real machine, where the same program answered the
    // mouse only once this path was selected. Upstream ConPTY has supported the
    // mouse since 2021, so this default is "the path we have proven", not a
    // verdict on ConPTY; `--feature conpty` selects it back.
    //
    // Applied here, and only here, because `set_var` is sound only while the
    // process is single-threaded: this runs before any window, PTY or reader
    // thread exists, and after argument parsing so the flag can be read.
    #[cfg(windows)]
    if !features.conpty {
        // SAFETY: nothing has been spawned yet; this is still single-threaded.
        unsafe { std::env::set_var("AGENTERM_FORCE_CONSOLE_AGENT", "1") };
    }
    #[cfg(not(windows))]
    let _ = features;
    no_activate |=
        agenterm_platform::runtime::ascii_environment_variable_present("AGENTERM_NO_ACTIVATE");

    // Load config file: CLI flags override config, config overrides defaults.
    let config = load_config();

    let mut app = ConApp::new(working_dir.clone(), control_endpoint);
    // Before the window: the endpoint belongs to the process, so a bind failure
    // is a startup failure with no window to close, and a client racing startup
    // finds a listener rather than a refused connection.
    if let Err(error) = app.bind_control_endpoint() {
        let _ = agenterm_platform::parent_console::write_stderr(&format!(
            "minicon: control endpoint unavailable: {error}\n"
        ));
        std::process::exit(1);
    }
    app.no_activate = no_activate;
    let session = app.active_session_mut().expect("initial terminal session");
    session.command = command;
    session.snapshot_path = snapshot_path;
    // CLI flags override config, config overrides defaults.
    session.apply_startup_size(
        [config.font_size, font_size],
        [config.cols, initial_cols],
        [config.rows, initial_rows],
    );
    // IME must stay on: without it CJK cannot be typed at all, which no
    // console host on Windows gets to call acceptable. An earlier fix disabled
    // it to recover keyboard input, but the actual cause was the missing
    // focus request in `opened` — see the Ime arm in `event` for the other
    // half (composed text never reached the PTY, which made IME look broken).
    app.pending_headless_session = headless;
    let options = PixelWindowOptions::new("minicon", LogicalSize::new(960.0, 600.0))
        .with_no_activate(no_activate)
        .with_ime_allowed(true)
        .with_start_detached(headless);

    if let Err(error) = run_pixel_window(options, Box::new(app)) {
        let _ = agenterm_platform::parent_console::write_stderr(&format!("minicon: {error}"));
        std::process::exit(1);
    }
}

/// The short name a program is known by: `C:\Windows\system32\cmd.exe` → `cmd`.
fn program_leaf(program: &str) -> &str {
    program
        .rsplit(['/', '\\'])
        .next()
        .filter(|leaf| !leaf.is_empty())
        .unwrap_or("")
}

fn program_stem(program: &str) -> String {
    // Command configuration and restored sessions may contain a path written
    // for a different host. `Path` only recognizes the current host's
    // separator, so using it made Windows paths render differently in a macOS
    // build and vice versa. Program labels are lexical, not filesystem I/O:
    // accept both separators on every target.
    let leaf = program_leaf(program);
    let stem = leaf
        .rsplit_once('.')
        .filter(|(stem, _extension)| !stem.is_empty())
        .map_or(leaf, |(stem, _extension)| stem);
    if stem.is_empty() {
        "terminal".to_owned()
    } else {
        stem.to_owned()
    }
}

/// What a tab should be called, given what the child said about itself.
///
/// A child names its own executable and calls that a title — `cmd.exe` sets
/// its window title to its own full path — and taking that literally is what
/// made every tab in the tree read `C:\Windows\system32\cmd.exe`. Long,
/// truncated in a narrow column, and identical across every tab, which defeats
/// the tab tree this product is built around.
///
/// A title that only repeats the program is not a title: it tells the user
/// nothing they did not already know from opening it. So it is treated as
/// absent, and the short program name is used instead. A title the child
/// genuinely sets — `title deploy` in cmd, or any shell's prompt escape — is
/// information, and it wins.
fn session_label(reported: &str, program_path: &str, program_label: &str) -> String {
    let trimmed = reported.trim();
    if trimmed.is_empty() {
        return program_label.to_owned();
    }
    let names_itself = trimmed.eq_ignore_ascii_case(program_path)
        || trimmed.eq_ignore_ascii_case(program_leaf(program_path));
    if names_itself {
        return program_label.to_owned();
    }
    trimmed.to_owned()
}

/// One lightweight GUI process containing several isolated terminal sessions.
///
/// The wrapper owns tree identity and routing only. A `ConTerminal` still owns
/// its own PTY, reader/waiter threads, parser, viewport and input state, so a
/// dead child or malformed output cannot corrupt another session's state.
struct ConApp {
    no_activate: bool,
    workspace: workspace::Workspace,
    sessions: SessionStore<ConTerminal>,
    /// Settings inherited when an empty workspace creates its next terminal.
    /// Closing the final tab updates this before the terminal is dropped, so
    /// the greeting page is a lifecycle boundary rather than a settings reset.
    session_seed: SessionSeed,
    composer: composer::ComposerState,
    /// The clipboard length the status bar shows.
    clipboard_status: clipboard_status::ClipboardStatus,
    /// True while the left button is held after pressing inside the composer,
    /// so pointer motion extends a mouse selection there instead of reaching
    /// the terminal. Cleared on release.
    composer_selecting: bool,
    /// Time, byte offset and streak of the last composer press, for
    /// double-click (word) and triple-click (all) selection — the composer's
    /// own counterpart to the terminal's click streak.
    composer_clicks: minicon_core::click::ClickCounter<usize>,
    /// The language MiniCon labels its own host UI in. Child output is never
    /// touched by this.
    ui_language: ui::UiLanguage,
    ui_theme: theme::ThemeChoice,
    /// Absolute node index the pointer is hovering in the tree, or None.
    /// The per-row close button shows only for the active or hovered row.
    hovered_tree_row: Option<usize>,
    /// The header icon button (New / Settings) the pointer is over, for hover
    /// feedback. `None` when the pointer is elsewhere.
    hovered_header: Option<HeaderIcon>,
    settings_open: bool,
    tree_scroll_offset: usize,
    sidebar_width_logical: f64,
    /// The sidebar is showing as a rail. Session-scoped on purpose: it is a
    /// view state, not a preference, and a restart should not strand a user in
    /// a collapsed sidebar they do not remember choosing.
    sidebar_collapsed: bool,
    sidebar_resizing: bool,
    exit: bool,
    control_endpoint: Option<String>,
    control_server: Option<control::ControlServer>,
    /// The event loop's waker, once a window exists to supply it.
    ///
    /// The control endpoint is bound before any window is, so its wake callback
    /// cannot capture a `WindowWaker` directly — it reads this slot instead. A
    /// wake with the slot empty is a no-op rather than an error: the request is
    /// already queued, and the loop drains every ready producer as soon as it
    /// starts. That indirection is what lets the endpoint's lifetime belong to
    /// the process rather than to the window.
    waker_slot: Arc<Mutex<Option<WindowWaker>>>,
    /// Reattaches a window after `detach-gui`. Deliberately not a `PixelWindow`:
    /// holding one of those across a detach would keep the native window alive
    /// and defeat the detach.
    attachment: Option<agenterm_platform::window_host::WindowAttachment>,
    /// `--headless`: the first session has not been started yet, because there
    /// was no window whose `opened` would have started it.
    pending_headless_session: bool,
    pending_control: PendingControl,
    pending_resize_requests: Vec<control::IncomingRequest>,
    pending_resize_deadline: Option<Instant>,
    pending_clipboard_paste: Option<PendingClipboardPaste>,
    pending_paste_review: Option<PendingPasteReview>,
    terminal_clipboard_error: Option<String>,
    /// A recoverable host-level notice (a tab that failed to open, an action
    /// that was refused). Kept separate from `terminal_clipboard_error` so a
    /// new-tab failure does not read as a clipboard problem, and surfaced in
    /// `ui-snapshot` like it. A per-tab failure must never end the process:
    /// the tab is rolled back and the host says why.
    host_notice: Option<String>,
    ime_status: Option<agenterm_platform::ime::ImeStatus>,
    ime_status_label: String,
    /// macOS cannot report an input method's internal latin/native toggle;
    /// this infers it from keystrokes (see `ime_mode`).
    ime_mode: ime_mode::ImeModeInference,
    control_pointer_owner: Option<workspace::TabId>,
    perf_stats: PerfStats,
    host_ui_dirty: DirtyRegion,
    frame_width: u32,
    frame_height: u32,
    frame_scale: f64,
    // `None` on hosts whose accessibility backend discards snapshots, so the
    // publish path costs nothing there. The platform crate owns that choice.
    a11y: Option<agenterm_platform::accessibility_publish::AccessibilityPublisher>,
    a11y_inbox: Arc<a11y::ActionInbox>,
    a11y_dirty: bool,
    current_window_title: String,
}

impl Drop for ConApp {
    fn drop(&mut self) {
        for request in self.pending_resize_requests.drain(..) {
            let _ = request.reply.send(Err(
                "terminal window closed while resize request was pending".to_owned(),
            ));
        }
        self.pending_control
            .cancel_all("terminal window closed while control request was pending");
    }
}

#[derive(Clone, Copy, Default)]
struct DrainOutcome {
    changed: bool,
    redraw: bool,
    backlog: bool,
    /// The child's exit was observed by this drain. The tab row dims when the
    /// shell is gone, and that row is host UI, not terminal cells, so a
    /// terminal-focused redraw would leave a live-looking tab behind.
    child_exited: bool,
    bytes: usize,
}

#[inline(never)]
fn single_field_json(name: &'static str, value: json::JsonValue) -> json::JsonValue {
    json::object(vec![(name, value)])
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct WheelOutcome {
    route: &'static str,
    delivered_notches: i16,
    changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MouseOutcome {
    route: &'static str,
    changed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct MouseReportOutcome {
    consumed: bool,
    wrote: bool,
}

struct PendingClipboardPaste {
    target: workspace::TabId,
    read: agenterm_platform::clipboard::ClipboardTextRead,
    review: bool,
}

/// Which modifier means "clipboard action" in this host's text areas.
///
/// macOS uses Command -- delivered as `meta` -- matching every native macOS
/// app, since a Mac keyboard has no Insert key and `Ctrl+C` there is not copy;
/// every other platform uses Control. Command and Control are distinct from
/// the terminal's own `Ctrl+C` (SIGINT) and `Ctrl+V` (readline quoted-insert),
/// so the clipboard keys never shadow them. Control is also accepted on macOS
/// so existing `Ctrl+C/V` keeps working.
///
/// This is the only `cfg` in the composer's key path. The rules themselves are
/// target-neutral and live in `minicon_core::keymap`, which takes this as a
/// value -- so the macOS rule is unit-tested on whatever machine runs the
/// tests, instead of only on a Mac.
#[cfg(target_os = "macos")]
const CLIPBOARD_MODIFIER: keymap::ClipboardModifier = keymap::ClipboardModifier::ControlOrMeta;
#[cfg(not(target_os = "macos"))]
const CLIPBOARD_MODIFIER: keymap::ClipboardModifier = keymap::ClipboardModifier::ControlOnly;

/// Translate a host key event into the neutral chord the keymap understands.
///
/// `None` means the table has no name for this key, which is not the same as
/// the table declining it: an unnamed key is simply not a composer key.
/// Multi-character logical text (an IME commit, say) has no single-character
/// spelling, so it is reported as a chord only when it is one character --
/// longer text reaches the draft through the insert path instead.
fn composer_chord(key: &NormalizedKeyEvent) -> Option<keymap::Chord> {
    let named = |k| Some(k);
    let key_name = match &key.logical {
        LogicalKey::Named(NamedKey::Enter) => named(keymap::Key::Enter),
        LogicalKey::Named(NamedKey::Backspace) => named(keymap::Key::Backspace),
        LogicalKey::Named(NamedKey::Delete) => named(keymap::Key::Delete),
        LogicalKey::Named(NamedKey::Escape) => named(keymap::Key::Escape),
        LogicalKey::Named(NamedKey::Space) => named(keymap::Key::Space),
        LogicalKey::Named(NamedKey::ArrowUp) => named(keymap::Key::Up),
        LogicalKey::Named(NamedKey::ArrowDown) => named(keymap::Key::Down),
        LogicalKey::Named(NamedKey::ArrowLeft) => named(keymap::Key::Left),
        LogicalKey::Named(NamedKey::ArrowRight) => named(keymap::Key::Right),
        LogicalKey::Named(NamedKey::Home) => named(keymap::Key::Home),
        LogicalKey::Named(NamedKey::End) => named(keymap::Key::End),
        LogicalKey::Named(NamedKey::PageUp) => named(keymap::Key::PageUp),
        LogicalKey::Named(NamedKey::PageDown) => named(keymap::Key::PageDown),
        LogicalKey::Character(text) => {
            let mut characters = text.chars();
            match (characters.next(), characters.next()) {
                (Some(character), None) => named(keymap::Key::Character(character)),
                _ => None,
            }
        }
        LogicalKey::Named(_) => None,
        // `LogicalKey` is non-exhaustive: a key this build has no name for is
        // not a composer key, which is the same answer as an unnamed one.
        _ => None,
    }?;
    Some(keymap::Chord {
        key: key_name,
        modifiers: keymap::Modifiers {
            control: key.modifiers.control,
            shift: key.modifiers.shift,
            alt: key.modifiers.alt,
            meta: key.modifiers.meta,
        },
    })
}

/// Where the caret sits, which is what decides whether Up moves or recalls.
fn composer_draft_shape(state: &composer::ComposerState) -> keymap::Draft {
    let caret = state.caret.min(state.text.len());
    let line = composer::line_index_at(&state.text, caret);
    keymap::Draft {
        caret_on_first_line: line == 0,
        caret_on_last_line: line + 1 >= composer::line_count(&state.text),
    }
}

/// Why a paste could not enter review. `Unsupported` has a defined fallback —
/// the platform never offered a review and the unreviewed path is what shipped
/// there — while `Failed` is a real failure the human must be shown.
enum PasteReviewRefusal {
    Unsupported,
    Failed(String),
}

/// The second half of a reviewed paste. It is a distinct state because the
/// event loop keeps running for as long as it is held: dropping it dismisses
/// the review and re-enables the owner, which is how tab and window teardown
/// stay safe without a separate cancel path.
struct PendingPasteReview {
    target: workspace::TabId,
    review: agenterm_platform::text_review::TextReview,
}

fn mouse_outcome_json(outcome: MouseOutcome) -> json::JsonValue {
    json::object(vec![
        ("delivered", true.into()),
        ("route", outcome.route.into()),
        ("changed", outcome.changed.into()),
    ])
}

fn wheel_outcome_json(outcome: WheelOutcome) -> json::JsonValue {
    json::object(vec![
        ("delivered_notches", outcome.delivered_notches.into()),
        ("route", outcome.route.into()),
        ("changed", outcome.changed.into()),
    ])
}

/// A child exit code as JSON: the number, or null while the child is live.
/// One projection, so `tab-exit` and the tab list cannot disagree on whether
/// "still running" is null or absent.
fn exit_code_json(exit_code: Option<i32>) -> json::JsonValue {
    exit_code.map_or(json::JsonValue::Null, |code| i64::from(code).into())
}

#[inline(never)]
fn tab_id_json(id: Option<workspace::TabId>) -> json::JsonValue {
    match id {
        Some(id) => json::JsonValue::TabId(id.get()),
        None => json::JsonValue::Null,
    }
}

fn tab_exit_json(id: workspace::TabId, exit_code: Option<i32>) -> json::JsonValue {
    json::object(vec![
        ("id", json::JsonValue::TabId(id.get())),
        ("child_alive", false.into()),
        ("child_exit_code", exit_code_json(exit_code)),
    ])
}

fn terminal_clipboard_target_is_current(
    target: workspace::TabId,
    active: Option<workspace::TabId>,
    composer_focused: bool,
) -> bool {
    active == Some(target) && !composer_focused
}

/// The single message the status strip shows, or `None` for the routing label.
/// A host notice (a tab that could not open) outranks a clipboard refusal; both
/// are recoverable and actionable, and the strip has one line.
fn status_strip_notice<'a>(
    host_notice: Option<&'a str>,
    clipboard_error: Option<&'a str>,
) -> Option<&'a str> {
    host_notice.or(clipboard_error)
}

fn ime_status_json(status: Option<&agenterm_platform::ime::ImeStatus>) -> json::JsonValue {
    let (known, name, available, open, native_mode, full_shape, label) = status.map_or_else(
        || (false, "", false, false, false, false, "IME: ?".to_owned()),
        |status| {
            (
                true,
                status.name.as_str(),
                status.available,
                status.open,
                status.native_mode,
                status.full_shape,
                status.label(),
            )
        },
    );
    json::object(vec![
        ("known", known.into()),
        ("name", name.into()),
        ("available", available.into()),
        ("open", open.into()),
        ("native_mode", native_mode.into()),
        ("full_shape", full_shape.into()),
        ("label", label.into()),
    ])
}

impl ConApp {
    fn new(working_dir: Option<String>, control_endpoint: Option<String>) -> Self {
        let mut workspace = workspace::Workspace::default();
        let initial = workspace
            .add_root("terminal".to_owned())
            .expect("an empty workspace accepts its initial tab");
        let mut sessions = SessionStore::default();
        let initial_session = ConTerminal::new(working_dir);
        let session_seed = SessionSeed::from_session(&initial_session);
        assert!(
            sessions.insert(initial, initial_session).is_ok(),
            "an empty session store accepts its initial tab"
        );
        Self {
            no_activate: false,
            workspace,
            sessions,
            session_seed,
            composer: composer::ComposerState::default(),
            composer_selecting: false,
            clipboard_status: clipboard_status::ClipboardStatus::new(),
            composer_clicks: Default::default(),
            ui_language: ui::UiLanguage::default(),
            ui_theme: theme::ThemeChoice::default(),
            hovered_tree_row: None,
            hovered_header: None,
            settings_open: false,
            tree_scroll_offset: 0,
            sidebar_width_logical: ui::SIDEBAR_WIDTH_DIP,
            sidebar_collapsed: false,
            sidebar_resizing: false,
            exit: false,
            control_endpoint,
            control_server: None,
            waker_slot: Arc::new(Mutex::new(None)),
            attachment: None,
            pending_headless_session: false,
            pending_control: PendingControl::default(),
            pending_resize_requests: Vec::new(),
            pending_resize_deadline: None,
            pending_clipboard_paste: None,
            pending_paste_review: None,
            terminal_clipboard_error: None,
            host_notice: None,
            ime_status: None,
            ime_status_label: "IME: ?".to_owned(),
            ime_mode: ime_mode::ImeModeInference::default(),
            control_pointer_owner: None,
            perf_stats: PerfStats::default(),
            host_ui_dirty: DirtyRegion::full(),
            frame_width: 0,
            frame_height: 0,
            frame_scale: 1.0,
            a11y: None,
            a11y_inbox: Arc::new(a11y::ActionInbox::default()),
            a11y_dirty: false,
            current_window_title: product_window_title(),
        }
    }

    fn active_session_mut(&mut self) -> Result<&mut ConTerminal, PixelWindowError> {
        let id = self.workspace.active().ok_or_else(|| {
            PixelWindowError::failed("con_session_missing", "no active terminal session")
        })?;
        self.sessions.get_mut(&id).ok_or_else(|| {
            PixelWindowError::failed(
                "con_session_missing",
                format!("active terminal session @{} is unavailable", id.get()),
            )
        })
    }

    fn refresh_ime_status(&mut self) -> bool {
        let mut next = agenterm_platform::ime::status();
        // macOS reports "an input method is selected" as native whatever its
        // internal Shift toggle says; apply the mode its keystrokes showed.
        if cfg!(target_os = "macos")
            && let Some(status) = next.as_mut()
        {
            self.ime_mode.observe_source(&status.name);
            if status.available
                && let Some(native) = self.ime_mode.native()
            {
                status.native_mode = native;
            }
        }
        if next == self.ime_status {
            return false;
        }
        self.ime_status_label = next.as_ref().map_or_else(
            || "IME: ?".to_owned(),
            agenterm_platform::ime::ImeStatus::label,
        );
        self.ime_status = next;
        self.mark_composer_dirty();
        true
    }

    fn active_session(&self) -> Result<&ConTerminal, PixelWindowError> {
        let id = self.workspace.active().ok_or_else(|| {
            PixelWindowError::failed("con_session_missing", "no active terminal session")
        })?;
        self.sessions.get(&id).ok_or_else(|| {
            PixelWindowError::failed(
                "con_session_missing",
                format!("active terminal session @{} is unavailable", id.get()),
            )
        })
    }

    /// The active session, or `None` when there is no active tab or its session
    /// has not been installed yet. Read-only projections (titles, IME preedit,
    /// dirty regions, seed capture) use this so the "no active session" shape
    /// lives in one place instead of being re-derived at each call site.
    fn active_session_opt(&self) -> Option<&ConTerminal> {
        self.workspace
            .active()
            .and_then(|id| self.sessions.get(&id))
    }

    /// The session for a specific tab, or `None` when the tab's session has not
    /// been installed yet. A projection over every tab (the tree rows, the
    /// `list-tabs` reply) reads it by id without naming the map itself.
    fn session_for(&self, id: workspace::TabId) -> Option<&ConTerminal> {
        self.sessions.get(&id)
    }

    fn cancel_pointer_gesture_for_tab(&mut self, window: &PixelWindow, id: workspace::TabId) {
        if self.control_pointer_owner == Some(id) {
            self.control_pointer_owner = None;
        }
        if let Some(session) = self.sessions.get_mut(&id) {
            session.cancel_pointer_gesture(window);
        }
    }

    fn cancel_pointer_gestures_for_activation(&mut self, window: &PixelWindow) {
        let owner = self.control_pointer_owner;
        if let Some(owner) = owner {
            self.cancel_pointer_gesture_for_tab(window, owner);
        }
        if let Some(active) = self.workspace.active()
            && Some(active) != owner
        {
            self.cancel_pointer_gesture_for_tab(window, active);
        }
    }

    /// Drops a tab's session and its tree node, carrying the closed terminal's
    /// settings forward as the seed the next tab inherits, and reports whether
    /// that left the workspace empty. Window-free by design: the caller decides
    /// what an empty window paints and titles, and a test can drive the state
    /// transition without a pixel window.
    fn detach_session(&mut self, id: workspace::TabId) -> bool {
        if let Some(session) = self.sessions.remove(&id) {
            self.session_seed = SessionSeed::from_session(&session);
            drop(session);
        }
        self.workspace.close(id);
        self.workspace.active().is_none()
    }

    /// Makes `id` the active tab and reports whether that changed anything. A
    /// re-selection of the already-active tab is a no-op, which is what keeps
    /// the windowed caller from cancelling pointer gestures the user is still
    /// performing.
    fn select_tab(&mut self, id: workspace::TabId) -> bool {
        if self.workspace.active() == Some(id) {
            return false;
        }
        self.workspace.set_active(id)
    }

    fn activate_session(&mut self, window: &PixelWindow, id: workspace::TabId) {
        if self.workspace.active() == Some(id) {
            return;
        }
        self.cancel_pointer_gestures_for_activation(window);
        self.select_tab(id);
    }

    fn refresh_title(&mut self, window: &PixelWindow) -> Result<(), PixelWindowError> {
        let title = self.active_session()?.window_title();
        window.set_title(&title);
        self.current_window_title = title;
        self.mark_a11y_dirty();
        Ok(())
    }

    fn close_active_session(&mut self, window: &PixelWindow) -> Result<(), PixelWindowError> {
        let id = self.workspace.active().ok_or_else(|| {
            PixelWindowError::failed("con_session_missing", "no active terminal session")
        })?;
        self.cancel_pointer_gestures_for_activation(window);
        self.cancel_control_for_tab(
            id,
            &format!(
                "terminal @{} closed while control request was pending",
                id.get()
            ),
        );
        let emptied = self.detach_session(id);
        if emptied {
            self.tree_scroll_offset = 0;
            self.composer = composer::ComposerState::default();
            self.current_window_title = product_window_title();
            window.set_title(&self.current_window_title);
            self.mark_host_ui_full_and_repaint(window);
            return Ok(());
        }
        self.mark_host_ui_full();
        let metrics = window.metrics()?;
        let sidebar_width = self.effective_sidebar_dip();
        let session = self.active_session_mut()?;
        Self::configure_host_ui(session, metrics.scale_factor, sidebar_width);
        session.apply_resize(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        self.composer.focused = false;
        self.refresh_title(window)?;
        window.request_redraw();
        Ok(())
    }

    fn configure_host_ui(session: &mut ConTerminal, scale: f64, sidebar_width_logical: f64) {
        let scale = scale.max(1.0);
        session.set_content_insets(
            minicon_core::numeric::round_f64(sidebar_width_logical * scale) as u32,
            0,
            ui::bottom_inset(scale),
        );
    }

    /// Applies the sidebar inset and the settled geometry to the active
    /// terminal in one place. Both follow the same window metrics and must move
    /// together: an inset from the old scale with a grid from the new one is
    /// what leaves the terminal content misaligned after a resize. A drag that
    /// wants a debounced PTY resize uses `queue_resize` and so keeps its own
    /// path.
    fn apply_active_geometry(
        &mut self,
        metrics: agenterm_platform::window_host::PixelWindowMetrics,
    ) -> Result<(), PixelWindowError> {
        let sidebar_width = self.sidebar_width_logical;
        let session = self.active_session_mut()?;
        Self::configure_host_ui(session, metrics.scale_factor, sidebar_width);
        session.apply_resize(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        Ok(())
    }

    fn layout(&self, width: u32, height: u32, scale: f64) -> ui::Layout {
        ui::Layout::with_sidebar(
            width,
            height,
            scale,
            self.sidebar_width_logical,
            self.sidebar_collapsed,
        )
    }

    /// The width the terminal must be inset by — the rail's width while
    /// collapsed, so the terminal reclaims the column instead of leaving a gap
    /// where the expanded sidebar used to be.
    fn effective_sidebar_dip(&self) -> f64 {
        if self.sidebar_collapsed {
            ui::SIDEBAR_RAIL_WIDTH_DIP
        } else {
            self.sidebar_width_logical
        }
    }

    /// Re-applies the terminal inset after the sidebar's width changed, so the
    /// terminal reclaims (or yields) the column in the same frame as the rail.
    fn apply_sidebar_width(&mut self, window: &PixelWindow) -> Result<(), PixelWindowError> {
        let metrics = window.metrics()?;
        let sidebar_width = self.effective_sidebar_dip();
        if let Ok(session) = self.active_session_mut() {
            Self::configure_host_ui(session, metrics.scale_factor, sidebar_width);
            session.queue_resize(
                metrics.physical_width,
                metrics.physical_height,
                metrics.scale_factor,
            );
        }
        Ok(())
    }

    /// Binds the control endpoint before any window exists.
    ///
    /// Called from `main` so a client that connects during startup is answered
    /// rather than refused, and so a bind failure is reported before a window
    /// is put on screen instead of after. The wake callback reaches the event
    /// loop through `waker_slot`, which `opened` fills.
    fn bind_control_endpoint(&mut self) -> Result<(), String> {
        let Some(endpoint) = self.control_endpoint.clone() else {
            return Ok(());
        };
        let slot = Arc::clone(&self.waker_slot);
        self.control_server = Some(control::ControlServer::bind(&endpoint, move || {
            if let Ok(slot) = slot.lock()
                && let Some(waker) = slot.as_ref()
            {
                let _ = waker.wake();
            }
        })?);
        Ok(())
    }

    fn mark_a11y_dirty(&mut self) {
        self.a11y_dirty = true;
    }

    fn publish_a11y(&mut self, window: &PixelWindow) {
        let Some(publisher) = self.a11y.as_ref() else {
            return;
        };
        publisher.set_window_handle(window.native_identity());
        if let Ok(metrics) = window.metrics()
            && metrics.is_drawable()
        {
            self.frame_width = metrics.physical_width;
            self.frame_height = metrics.physical_height;
            self.frame_scale = metrics.scale_factor;
        }
        let width = self.frame_width.max(1);
        let height = self.frame_height.max(1);
        let layout = self.layout(width, height, self.frame_scale.max(1.0));
        publisher.publish(a11y::tree(
            "minicon",
            &self.current_window_title,
            layout,
            width,
            height,
            self.composer.focused,
            &self.composer.text,
        ));
        self.a11y_dirty = false;
    }

    fn drain_a11y_actions(&mut self, window: &PixelWindow) -> Result<(), PixelWindowError> {
        let (requests, backlog) = self.a11y_inbox.pop_batch(a11y::ACTION_DRAIN_BUDGET);
        for request in requests {
            match (request.node, request.action) {
                (
                    agenterm_platform::accessibility_publish::NODE_COMMAND,
                    agenterm_platform::accessibility_publish::PublishedAction::SetText(text),
                ) => {
                    self.composer.focused = true;
                    self.composer.submit_error = None;
                    composer::replace_text(&mut self.composer, &text);
                    self.composer.preedit.clear();
                    let _ = self.update_composer_ime_anchor(window);
                    self.mark_composer_dirty();
                }
                (
                    agenterm_platform::accessibility_publish::NODE_COMMAND,
                    agenterm_platform::accessibility_publish::PublishedAction::Key(key),
                ) => {
                    self.composer.focused = true;
                    match agenterm_platform::accessibility_publish::published_key_effect(&key) {
                        agenterm_platform::accessibility_publish::KeyEffect::Insert(text) => {
                            self.composer.submit_error = None;
                            composer::insert(&mut self.composer, &text);
                        }
                        agenterm_platform::accessibility_publish::KeyEffect::Backspace => {
                            self.composer.submit_error = None;
                            composer::backspace(&mut self.composer);
                        }
                        agenterm_platform::accessibility_publish::KeyEffect::SelectAll => {
                            composer::select_all(&mut self.composer);
                        }
                        agenterm_platform::accessibility_publish::KeyEffect::Submit => {
                            self.submit_composer();
                        }
                        agenterm_platform::accessibility_publish::KeyEffect::Cancel => {
                            self.composer.cancel_focus();
                        }
                        agenterm_platform::accessibility_publish::KeyEffect::Ignore => {}
                    }
                    self.composer.preedit.clear();
                    let _ = self.update_composer_ime_anchor(window);
                    self.mark_composer_dirty();
                }
                (agenterm_platform::accessibility_publish::NODE_COMMAND, _) => {
                    self.composer.focused = true;
                    let _ = self.update_composer_ime_anchor(window);
                    self.mark_composer_dirty();
                }
                (
                    agenterm_platform::accessibility_publish::NODE_SEND,
                    agenterm_platform::accessibility_publish::PublishedAction::Click,
                ) => {
                    self.submit_composer();
                }
                (
                    agenterm_platform::accessibility_publish::NODE_SEND,
                    agenterm_platform::accessibility_publish::PublishedAction::Focus,
                )
                | (agenterm_platform::accessibility_publish::NODE_SESSION, _) => {
                    self.composer.focused = false;
                    self.mark_composer_dirty();
                }
                (agenterm_platform::accessibility_publish::NODE_FRAME, _) => {
                    window.focus();
                }
                _ => {}
            }
            self.mark_a11y_dirty();
        }
        if backlog {
            let _ = window.waker().wake();
        }
        Ok(())
    }

    fn mark_host_ui_full(&mut self) {
        self.host_ui_dirty.mark_full();
        self.mark_a11y_dirty();
    }

    /// Invalidates all host UI and asks the window to repaint. The two always
    /// travel together — marking without repainting leaves a stale frame until
    /// unrelated damage arrives, and repainting without marking can present an
    /// unchanged one — so the pair gets one spelling.
    /// The status bar's clipboard-length text, in the UI's language.
    fn clipboard_readout(&self) -> Option<String> {
        clipboard_status::readout(
            self.clipboard_status.length(),
            !matches!(self.ui_language, ui::UiLanguage::English),
        )
    }

    fn mark_host_ui_full_and_repaint(&mut self, window: &PixelWindow) {
        self.mark_host_ui_full();
        window.request_redraw();
    }

    fn mark_host_ui_rect(&mut self, x: u32, y: u32, width: u32, height: u32) {
        if self.frame_width == 0 || self.frame_height == 0 {
            self.mark_host_ui_full();
            return;
        }
        self.host_ui_dirty
            .mark_rect(PixelRect::from_xywh(x, y, width, height));
    }

    /// Marks one host-UI region dirty. Sugar over [`Self::mark_host_ui_rect`],
    /// which already falls back to the whole host UI before the first frame has
    /// dimensions; the point is that a caller passes a layout rectangle instead
    /// of four coordinates, so the rectangle and the marked area cannot drift.
    fn mark_host_ui_region(&mut self, region: ui::Rect) {
        self.mark_host_ui_rect(region.x, region.y, region.width, region.height);
    }

    fn mark_tree_dirty(&mut self) {
        let layout = self.layout(self.frame_width, self.frame_height, self.frame_scale);
        self.mark_host_ui_region(layout.sidebar);
    }

    fn mark_composer_dirty(&mut self) {
        self.mark_a11y_dirty();
        let layout = self.layout(self.frame_width, self.frame_height, self.frame_scale);
        // The whole composer band, not just the control row. The "SEND TO @N"
        // label is painted at `composer.y + 7` -- above `composer_input.y` --
        // and its color tracks `composer_focused`, so marking only the controls
        // left the label showing the previous focus state until some unrelated
        // damage happened to cover it. The band is the provable bound: every
        // pixel `paint_host_ui` derives from composer state lives inside it.
        let band = ui::Rect {
            x: layout.composer.x,
            y: layout.composer.y,
            width: self.frame_width.saturating_sub(layout.composer.x),
            height: self.frame_height.saturating_sub(layout.composer.y),
        };
        self.mark_host_ui_region(band);
    }

    fn note_frame_dimensions(&mut self, width: u32, height: u32, scale: f64) {
        if self.frame_width != width || self.frame_height != height || self.frame_scale != scale {
            self.mark_host_ui_full();
        }
        self.frame_width = width;
        self.frame_height = height;
        self.frame_scale = scale;
    }

    fn take_dirty_candidate(&mut self, width: u32, height: u32) -> DirtyRegion {
        let mut candidate = std::mem::take(&mut self.host_ui_dirty);
        if let Ok(session) = self.active_session_mut() {
            candidate = candidate.union(session.take_dirty());
        }
        candidate.clip(width, height)
    }

    fn request_dirty_redraw(&self, window: &PixelWindow) {
        let candidate = self.host_ui_dirty.union(
            self.active_session_opt()
                .map(|session| session.dirty)
                .unwrap_or_else(DirtyRegion::full),
        );
        request_candidate_redraw(window, candidate, self.frame_width, self.frame_height);
    }

    fn reveal_active_tree_row(&mut self, window: &PixelWindow) -> Result<(), PixelWindowError> {
        let Some(active) = self.workspace.active() else {
            return Ok(());
        };
        let Some(index) = self
            .workspace
            .nodes()
            .iter()
            .position(|node| node.id == active)
        else {
            return Ok(());
        };
        let metrics = window.metrics()?;
        let layout = self.layout(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        self.tree_scroll_offset = ui::reveal_tree_index(
            self.tree_scroll_offset,
            index,
            self.workspace.nodes().len(),
            layout.tree_capacity(),
        );
        Ok(())
    }

    fn open_session(&mut self, window: &PixelWindow, child: bool) -> Result<(), PixelWindowError> {
        let seed = self
            .active_session_opt()
            .map(SessionSeed::from_session)
            .unwrap_or_else(|| self.session_seed.clone());
        self.cancel_pointer_gestures_for_activation(window);
        let parent = self.workspace.active();
        let id = match (child, parent) {
            (true, Some(parent)) => self.workspace.add_child(parent, "terminal".to_owned()),
            _ => self.workspace.add_root("terminal".to_owned()),
        }
        .ok_or_else(|| {
            PixelWindowError::failed("con_tab_create", "active parent is unavailable")
        })?;

        let mut session = seed.create_session();
        session.apply_theme(self.ui_theme);
        Self::configure_host_ui(
            &mut session,
            window.metrics()?.scale_factor,
            self.effective_sidebar_dip(),
        );
        if let Err(error) = session.opened(window) {
            self.workspace.close(id);
            return Err(error);
        }
        if self.sessions.insert(id, session).is_err() {
            self.workspace.close(id);
            return Err(PixelWindowError::failed(
                "con_tab_create",
                "stable tab id is already in use",
            ));
        }
        self.session_seed = seed;
        self.mark_host_ui_full();
        self.reveal_active_tree_row(window)?;
        self.refresh_title(window)
    }

    /// Sets a recoverable host notice and repaints the strip that shows it.
    /// A per-tab failure uses this instead of returning `Err`, which the host
    /// would otherwise turn into an exit for every tab.
    fn note_host_notice(&mut self, message: String, window: &PixelWindow) {
        self.host_notice = Some(message);
        self.mark_host_ui_full_and_repaint(window);
    }

    /// Opens a tab the way a user gesture or a control request does: a shell
    /// that will not start is a notice, never a reason to end the process.
    /// The tab is already rolled back inside `open_session` on failure.
    fn open_session_contained(&mut self, window: &PixelWindow, child: bool) {
        if let Err(error) = self.open_session(window, child) {
            self.note_host_notice(format!("could not open a terminal: {error}"), window);
        } else if self.host_notice.take().is_some() {
            // A successful open retracts the previous refusal; the strip must
            // repaint, or the stale notice stays on screen.
            self.mark_host_ui_full_and_repaint(window);
        }
    }

    fn select_relative(
        &mut self,
        window: &PixelWindow,
        direction: isize,
    ) -> Result<(), PixelWindowError> {
        let ids: Vec<_> = self.workspace.nodes().iter().map(|node| node.id).collect();
        let Some(active) = self.workspace.active() else {
            return Ok(());
        };
        let Some(index) = ids.iter().position(|id| *id == active) else {
            return Err(PixelWindowError::failed(
                "con_session_missing",
                "active tab is not in the tree",
            ));
        };
        let next = (index as isize + direction).rem_euclid(ids.len() as isize) as usize;
        self.mark_host_ui_full();
        self.activate_session(window, ids[next]);
        self.reveal_active_tree_row(window)?;
        let metrics = window.metrics()?;
        self.apply_active_geometry(metrics)?;
        self.refresh_title(window)?;
        window.focus();
        window.request_redraw();
        Ok(())
    }

    /// Re-apply the active host-UI theme's terminal colors to every open
    /// terminal. Call after any `ui_theme` change so the terminal bodies recolor
    /// with the rest of the UI.
    fn apply_theme_to_all_sessions(&mut self) {
        let choice = self.ui_theme;
        for (_, session) in self.sessions.entries_mut() {
            session.apply_theme(choice);
        }
    }

    fn handle_workspace_shortcut(
        &mut self,
        window: &PixelWindow,
        key: &NormalizedKeyEvent,
    ) -> Result<bool, PixelWindowError> {
        if key.state != KeyPressState::Pressed || !key.modifiers.control || !key.modifiers.shift {
            return Ok(false);
        }
        let LogicalKey::Character(text) = &key.logical else {
            return Ok(false);
        };
        if text.eq_ignore_ascii_case("t") {
            self.open_session_contained(window, false);
            return Ok(true);
        }
        if text.eq_ignore_ascii_case("n") {
            self.open_session_contained(window, true);
            return Ok(true);
        }
        if text.eq_ignore_ascii_case("w") {
            if self.workspace.active().is_some() {
                self.close_active_session(window)?;
            }
            return Ok(true);
        }
        // Collapse/expand the sidebar. The pointer has the header button; this
        // is the same action for someone who does not want to leave the
        // keyboard, and it is what makes the rail reachable from the control
        // CLI, so the collapsed chrome can be machine-verified.
        if text.eq_ignore_ascii_case("b") {
            self.sidebar_collapsed = !self.sidebar_collapsed;
            self.settings_open = false;
            self.hovered_tree_row = None;
            self.apply_sidebar_width(window)?;
            self.mark_host_ui_full_and_repaint(window);
            return Ok(true);
        }
        if text.eq_ignore_ascii_case("i") {
            if self.workspace.active().is_none() {
                return Ok(true);
            }
            self.composer.focused = true;
            self.update_composer_ime_anchor(window)?;
            self.mark_composer_dirty();
            self.request_dirty_redraw(window);
            return Ok(true);
        }
        if text == "[" {
            self.select_relative(window, -1)?;
            return Ok(true);
        }
        if text == "]" {
            self.select_relative(window, 1)?;
            return Ok(true);
        }
        if text.eq_ignore_ascii_case("p") {
            self.ui_theme = self.ui_theme.next();
            self.apply_theme_to_all_sessions();
            self.mark_host_ui_full();
            self.request_dirty_redraw(window);
            return Ok(true);
        }
        if text == "," {
            // Open or close the settings panel.
            self.settings_open = !self.settings_open;
            self.mark_host_ui_full_and_repaint(window);
            return Ok(true);
        }
        if text.eq_ignore_ascii_case("g") {
            // Toggle the grid crosshair (lines + row/column band).
            if let Ok(session) = self.active_session_mut() {
                session.crosshair_on = !session.crosshair_on;
                session.dirty.mark_full();
            }
            self.mark_host_ui_full();
            self.request_dirty_redraw(window);
            return Ok(true);
        }
        Ok(false)
    }

    fn handle_tree_pointer(
        &mut self,
        window: &PixelWindow,
        position: &LogicalPoint,
    ) -> Result<bool, PixelWindowError> {
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        let physical_x = (position.x * scale).max(0.0) as u32;
        let physical_y = (position.y * scale).max(0.0) as u32;
        let ids: Vec<_> = self.workspace.nodes().iter().map(|node| node.id).collect();
        match ui::tree_hit(
            layout,
            physical_x,
            physical_y,
            self.tree_scroll_offset,
            ids.len(),
            scale,
        ) {
            ui::TreeHit::Outside => return Ok(false),
            ui::TreeHit::Background => return Ok(true),
            ui::TreeHit::NewRoot => {
                self.settings_open = false;
                self.open_session_contained(window, false);
                return Ok(true);
            }
            ui::TreeHit::SidebarToggle => {
                self.sidebar_collapsed = !self.sidebar_collapsed;
                // Collapsing while the settings panel is open would leave the
                // panel floating over a rail that no longer has room for it.
                self.settings_open = false;
                self.hovered_tree_row = None;
                self.apply_sidebar_width(window)?;
                self.mark_host_ui_full_and_repaint(window);
                return Ok(true);
            }
            ui::TreeHit::Settings => {
                self.settings_open = !self.settings_open;
                self.mark_host_ui_full_and_repaint(window);
                return Ok(true);
            }
            ui::TreeHit::Close(index) => {
                // The close button is only shown for the active or hovered row,
                // so a click on its region only closes when it was actually
                // visible; otherwise it selects the row like any other click.
                let close_visible = self.hovered_tree_row == Some(index)
                    || self.workspace.active() == Some(ids[index]);
                if close_visible {
                    self.activate_session(window, ids[index]);
                    self.mark_host_ui_full();
                    self.close_active_session(window)?;
                    self.tree_scroll_offset = ui::clamp_tree_scroll(
                        self.tree_scroll_offset,
                        self.workspace.nodes().len(),
                        layout.tree_capacity(),
                    );
                    return Ok(true);
                }
                self.activate_session(window, ids[index]);
                self.reveal_active_tree_row(window)?;
                self.mark_host_ui_full();
            }
            ui::TreeHit::Select(index) => {
                self.activate_session(window, ids[index]);
                self.reveal_active_tree_row(window)?;
                self.mark_host_ui_full();
            }
        }
        self.apply_active_geometry(metrics)?;
        self.composer.focused = false;
        self.refresh_title(window)?;
        window.focus();
        window.request_redraw();
        Ok(true)
    }

    fn composer_hit(
        &self,
        window: &PixelWindow,
        position: &LogicalPoint,
    ) -> Result<ui::ComposerHit, PixelWindowError> {
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        Ok(ui::composer_hit(
            layout,
            (position.x * scale).max(0.0) as u32,
            (position.y * scale).max(0.0) as u32,
        ))
    }

    fn update_composer_ime_anchor(&self, window: &PixelWindow) -> Result<(), PixelWindowError> {
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        // The composer paints a sliding horizontal window on each soft line,
        // so the caret anchor follows its visible logical row and column.
        // Measuring the whole buffer sent the IME candidate list off screen as
        // soon as the text outgrew the box, and did it with a hard-coded 8 px
        // advance that no font honours.
        let composer_font_size = scaled_host_ui_font(
            COMPOSER_TEXT_SIZE_PX,
            self.active_session()?.font_size_logical,
            scale,
        );
        let cell_width = font::cell_metrics(composer_font_size).width.max(1);
        let text_width = layout
            .composer_input
            .width
            .saturating_sub(COMPOSER_TEXT_INSET.saturating_mul(2));
        let line_height = font::cell_metrics(composer_font_size).height.max(1);
        let rows = (layout.composer_input.height.saturating_sub(8) / line_height).max(1) as usize;
        let lines = composer::visible_line_window(&self.composer.text, self.composer.caret, rows);
        let caret_line = lines.first_line + lines.caret_row;
        let range = composer::line_range(&self.composer.text, caret_line);
        let local_caret = self
            .composer
            .caret
            .saturating_sub(range.start)
            .min(range.len());
        let line = &self.composer.text[range];
        let visible = composer::visible_window(
            line,
            &self.composer.preedit,
            local_caret,
            1,
            (text_width / cell_width) as usize,
        );
        let caret = local_caret.clamp(visible.text, line.len());
        let caret_cells = usize::from(visible.truncated)
            + composer::cells(&line[visible.text..caret])
            + composer::cells(&self.composer.preedit[visible.preedit..]);
        let caret_offset = cell_width
            .saturating_mul(caret_cells as u32)
            .min(text_width);
        let x = f64::from(
            layout
                .composer_input
                .x
                .saturating_add(COMPOSER_TEXT_INSET)
                .saturating_add(caret_offset),
        ) / scale;
        let y = f64::from(
            layout
                .composer_input
                .y
                .saturating_add(4)
                .saturating_add(line_height.saturating_mul(lines.caret_row as u32)),
        ) / scale;
        let _ = window.set_ime_cursor_area(agenterm_platform::window_host::LogicalRect::new(
            x, y, 2.0, 20.0,
        ));
        Ok(())
    }

    /// Puts the caret where the pointer landed.
    ///
    /// The column is measured against the same window the painter used, so a
    /// click resolves to the character the user can actually see under the
    /// pointer rather than to an offset in the full buffer, which for a
    /// scrolled line is a different character entirely.
    /// The composer text byte offset a pointer position lands on, mapping a
    /// pixel through the same visible-line window the painter uses. Shared by
    /// click-to-place-caret and mouse selection (press/drag) so both agree on
    /// where the pointer is in the draft.
    fn composer_offset_at(
        &self,
        window: &PixelWindow,
        position: &LogicalPoint,
    ) -> Result<usize, PixelWindowError> {
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        let composer_font_size = scaled_host_ui_font(
            COMPOSER_TEXT_SIZE_PX,
            self.active_session()?.font_size_logical,
            scale,
        );
        let cell_width = font::cell_metrics(composer_font_size).width.max(1);
        let text_width = layout
            .composer_input
            .width
            .saturating_sub(COMPOSER_TEXT_INSET.saturating_mul(2));
        let line_height = font::cell_metrics(composer_font_size).height.max(1);
        let rows = (layout.composer_input.height.saturating_sub(8) / line_height).max(1) as usize;
        let lines = composer::visible_line_window(&self.composer.text, self.composer.caret, rows);
        let physical_x = minicon_core::numeric::round_f64(position.x * scale) as u32;
        let physical_y = minicon_core::numeric::round_f64(position.y * scale) as u32;
        let clicked_row = physical_y
            .saturating_sub(layout.composer_input.y.saturating_add(4))
            .checked_div(line_height)
            .unwrap_or(0)
            .min(lines.line_count.saturating_sub(1) as u32) as usize;
        let line_index = lines.first_line + clicked_row;
        let range = composer::line_range(&self.composer.text, line_index);
        let line = &self.composer.text[range.clone()];
        let local_caret =
            if line_index == composer::line_index_at(&self.composer.text, self.composer.caret) {
                self.composer
                    .caret
                    .saturating_sub(range.start)
                    .min(range.len())
            } else {
                line.len()
            };
        let visible = composer::visible_window(
            line,
            if line_index == composer::line_index_at(&self.composer.text, self.composer.caret) {
                &self.composer.preedit
            } else {
                ""
            },
            local_caret,
            1,
            (text_width / cell_width) as usize,
        );
        let origin = layout
            .composer_input
            .x
            .saturating_add(COMPOSER_TEXT_INSET)
            // The truncation marker occupies a leading cell that belongs to no
            // character, so a click must not be charged for it.
            .saturating_add(u32::from(visible.truncated).saturating_mul(cell_width));
        let cell = physical_x.saturating_sub(origin) / cell_width;
        Ok(range.start + composer::caret_at_cell(line, visible.text, cell as usize))
    }

    /// Composer click streak (1 → 2 → 3 → 1) for word / all selection. A repeat
    /// only counts on the same byte offset inside the multi-click window, the
    /// composer's counterpart to the terminal's `register_click`.
    fn register_composer_click(&mut self, offset: usize) -> u8 {
        self.composer_clicks.register(offset, Instant::now())
    }

    /// Extends the composer mouse selection to `position` during a drag. The
    /// anchor was pinned on press; motion only moves the caret, so
    /// `selection_bounds` reports the covered range.
    fn drag_composer_selection(
        &mut self,
        window: &PixelWindow,
        position: &LogicalPoint,
    ) -> Result<(), PixelWindowError> {
        let offset = self.composer_offset_at(window, position)?;
        self.composer.caret = offset;
        self.update_composer_ime_anchor(window)?;
        self.mark_composer_dirty();
        self.request_dirty_redraw(window);
        Ok(())
    }

    /// Pastes the clipboard into the composer at the caret (replacing any
    /// selection), the mouse counterpart to Ctrl/Cmd+V. Used by right-click and
    /// the composer's Paste button so non-keyboard users can paste too.
    fn paste_clipboard_into_composer(&mut self, window: &PixelWindow) {
        if let Ok(text) = agenterm_platform::clipboard::get_text(composer::PASTE_LIMIT_BYTES) {
            composer::paste(&mut self.composer, &text);
            let _ = self.update_composer_ime_anchor(window);
            self.mark_composer_dirty();
            self.request_dirty_redraw(window);
        }
    }

    fn handle_composer_key(&mut self, window: &PixelWindow, key: &NormalizedKeyEvent) -> bool {
        if key.state != KeyPressState::Pressed {
            return true;
        }
        let Some(chord) = composer_chord(key) else {
            return true;
        };
        let draft = composer_draft_shape(&self.composer);
        let Some(action) = keymap::action(chord, CLIPBOARD_MODIFIER, draft) else {
            // Named by the table, bound to nothing: consumed, so an unbound
            // chord cannot type itself into the draft.
            return true;
        };
        if action == keymap::Action::Decline {
            // Handed to the same terminal handler that runs when the terminal
            // has focus, so scrollback paging behaves identically in both
            // focus states instead of being swallowed while typing.
            return false;
        }
        self.composer.submit_error = None;
        match action {
            keymap::Action::Decline => unreachable!("returned above"),
            keymap::Action::SoftNewline => composer::insert(&mut self.composer, "\n"),
            keymap::Action::Send => self.submit_composer(),
            keymap::Action::SelectAll => composer::select_all(&mut self.composer),
            keymap::Action::Copy => {
                if let Some(text) = composer::selection_text(&self.composer) {
                    let _ = clipboard_status::set_text(text);
                }
            }
            keymap::Action::Cut => {
                if let Some(text) = composer::cut(&mut self.composer) {
                    let _ = clipboard_status::set_text(&text);
                }
            }
            keymap::Action::Paste => {
                if let Ok(text) =
                    agenterm_platform::clipboard::get_text(composer::PASTE_LIMIT_BYTES)
                {
                    composer::paste(&mut self.composer, &text);
                }
            }
            keymap::Action::Backspace => composer::backspace(&mut self.composer),
            keymap::Action::DeleteForward => composer::delete_forward(&mut self.composer),
            keymap::Action::DeleteWordBack => composer::delete_word_back(&mut self.composer),
            keymap::Action::DeleteWordForward => {
                composer::delete_word_forward(&mut self.composer)
            }
            keymap::Action::Move(movement) => composer::move_caret(&mut self.composer, movement),
            keymap::Action::Extend(movement) => {
                composer::extend_selection(&mut self.composer, movement)
            }
            keymap::Action::RecallPrevious => {
                self.composer.recall_previous();
            }
            keymap::Action::RecallNext => {
                self.composer.recall_next();
            }
            keymap::Action::CancelFocus => self.composer.cancel_focus(),
            keymap::Action::Insert => match &key.logical {
                LogicalKey::Named(NamedKey::Space) => composer::insert(&mut self.composer, " "),
                LogicalKey::Character(text) => composer::insert(&mut self.composer, text),
                _ => {}
            },
        }
        let _ = self.update_composer_ime_anchor(window);
        true
    }

    fn submit_composer(&mut self) {
        let Some(input) = self.composer.take_submission() else {
            return;
        };
        // Remembered before delivery is attempted: a line that failed to send
        // is exactly the one worth recalling.
        self.composer.remember(&input);
        let result = (|| {
            let session = self
                .active_session_mut()
                .map_err(|error| error.to_string())?;
            session.ensure_pty_input_open()?;
            let payload =
                composer_submission_payload(&input, session.parser.screen().bracketed_paste());
            session
                .write_pty(&payload)
                .map_err(|error| format!("terminal input failed: {error}"))?;
            // Committing a draft *is* a key press, so encode it the way a
            // physical Enter and `send-keys Enter` are encoded instead of
            // assuming a bare CR — it then follows whatever keyboard mode the
            // terminal has negotiated.
            let enter = session.encoded_enter();
            // Deliberately not written here: back-to-back writes still land in
            // one `read()`. Hold it briefly so the child consumes the paste
            // first and sees the commit arrive as its own key press.
            session.pending_submit_enter = Some((Instant::now() + COMPOSER_ENTER_DELAY, enter));
            // Submission crosses the PTY boundary and can change arbitrary
            // terminal cells; commit view state only after delivery succeeds.
            session.dirty.mark_full();
            session.scroll_to_bottom();
            Ok::<(), String>(())
        })();
        if let Err(error) = result {
            self.composer.restore_failed_submission(input, error);
        }
    }

    fn handle_composer_ime(
        &mut self,
        window: &PixelWindow,
        event: agenterm_platform::ime::ImeEvent,
    ) {
        use agenterm_platform::ime::{ImeAction, classify_event};
        match classify_event(event, true) {
            ImeAction::UpdatePreedit { text, .. } => self.composer.preedit = text,
            ImeAction::ClearPreedit => self.composer.preedit.clear(),
            ImeAction::CommitText(text) => {
                self.composer.submit_error = None;
                self.composer.preedit.clear();
                composer::insert(&mut self.composer, &text);
            }
            ImeAction::None => {}
            _ => self.composer.preedit.clear(),
        }
        let _ = self.update_composer_ime_anchor(window);
    }

    fn control_target(&self, target: Option<workspace::TabId>) -> Result<workspace::TabId, String> {
        let id = target
            .or_else(|| self.workspace.active())
            .ok_or_else(|| "no active terminal".to_owned())?;
        self.sessions
            .contains_key(&id)
            .then_some(id)
            .ok_or_else(|| format!("terminal @{} does not exist", id.get()))
    }

    fn control_session_mut(
        &mut self,
        target: Option<workspace::TabId>,
    ) -> Result<&mut ConTerminal, String> {
        let id = self.control_target(target)?;
        self.sessions
            .get_mut(&id)
            .ok_or_else(|| "terminal disappeared".to_owned())
    }

    /// Sends input to a control-addressed terminal and replies with `label`
    /// set to the byte count the action reports. The guard and the error
    /// wording live here so a new send command cannot forget to require an
    /// open PTY or report a dead child as a typed failure instead of a
    /// silent success.
    fn send_to_control_terminal(
        &mut self,
        target: Option<workspace::TabId>,
        label: &'static str,
        action: impl FnOnce(&mut ConTerminal) -> std::io::Result<usize>,
    ) -> Result<json::JsonValue, String> {
        self.control_session_mut(target).and_then(|session| {
            session.ensure_pty_input_open()?;
            let sent =
                action(session).map_err(|error| format!("terminal input failed: {error}"))?;
            Ok(single_field_json(label, sent.into()))
        })
    }

    /// Runs `action` against one terminal's retained session, then settles a
    /// clipboard-paste request it raised: require an open PTY, and if the key
    /// asked for a terminal paste, start it. Both `send-keys` and
    /// `send-ui-keys` reach a session this way, so the guard and the
    /// paste-request handoff are written once.
    fn forward_keys_to_terminal(
        &mut self,
        window: &PixelWindow,
        id: workspace::TabId,
        action: impl FnOnce(&mut ConTerminal) -> Result<(), String>,
    ) -> Result<(), String> {
        let requested = {
            let session = self
                .sessions
                .get_mut(&id)
                .ok_or_else(|| format!("terminal @{} is unavailable", id.get()))?;
            session.ensure_pty_input_open()?;
            action(session)?;
            session.take_clipboard_paste_request()
        };
        if requested {
            self.request_terminal_clipboard_paste(window, id, false)?;
        }
        Ok(())
    }

    /// Drops a pending terminal clipboard read and/or paste review, recording
    /// why. `target` selects one tab; `None` cancels every tab. Both pending
    /// kinds share this one path so a new one cannot be forgotten in either
    /// cancellation site.
    fn cancel_terminal_paste(&mut self, target: Option<workspace::TabId>, reason: &str) {
        let matches =
            |pending_target: workspace::TabId| target.is_none_or(|target| target == pending_target);
        if self
            .pending_clipboard_paste
            .as_ref()
            .is_some_and(|pending| matches(pending.target))
        {
            self.pending_clipboard_paste = None;
            self.terminal_clipboard_error = Some(reason.to_owned());
        }
        // Dropping the review dismisses it and restores the owner window, so a
        // tab that goes away cannot leave an editor addressing a dead terminal.
        if self
            .pending_paste_review
            .as_ref()
            .is_some_and(|pending| matches(pending.target))
        {
            self.pending_paste_review = None;
            self.terminal_clipboard_error = Some(reason.to_owned());
        }
    }

    fn cancel_control_for_tab(&mut self, id: workspace::TabId, reason: &str) {
        self.pending_control.cancel_for_tab(id, reason);
        self.cancel_terminal_paste(Some(id), reason);
    }

    fn cancel_all_control_requests(&mut self, reason: &str) {
        self.pending_resize_deadline = None;
        for request in self.pending_resize_requests.drain(..) {
            let _ = request.reply.send(Err(reason.to_owned()));
        }
        self.pending_control.cancel_all(reason);
        self.cancel_terminal_paste(None, reason);
    }

    fn request_terminal_clipboard_paste(
        &mut self,
        window: &PixelWindow,
        target: workspace::TabId,
        review: bool,
    ) -> Result<(), String> {
        if self.pending_clipboard_paste.is_some() || self.pending_paste_review.is_some() {
            return Err("a terminal clipboard paste is already pending".to_owned());
        }
        if !terminal_clipboard_target_is_current(
            target,
            self.workspace.active(),
            self.composer.focused,
        ) {
            return Err("terminal clipboard paste requires the active terminal".to_owned());
        }
        self.sessions
            .get(&target)
            .ok_or_else(|| format!("terminal @{} is unavailable", target.get()))?
            .ensure_pty_input_open()?;
        let waker = window.waker();
        let read = agenterm_platform::clipboard::read_text_async(
            terminal_input::TERMINAL_PASTE_LIMIT_BYTES,
            move || {
                let _ = waker.wake();
            },
        )
        .map_err(|error| format!("clipboard read failed: {error}"))?;
        self.pending_clipboard_paste = Some(PendingClipboardPaste {
            target,
            read,
            review,
        });
        self.terminal_clipboard_error = None;
        window.request_redraw();
        Ok(())
    }

    /// The confirmation half of a clipboard paste, shared by the reviewed and
    /// unreviewed paths. Every check runs after the human has finished, because
    /// a review the host keeps pumping through means the active tab and focus
    /// can have moved while it was open — which is exactly what the invariant
    /// in PRD 23 requires be revalidated.
    fn deliver_terminal_paste(
        &mut self,
        target: workspace::TabId,
        text: &str,
    ) -> Result<(), String> {
        if text.is_empty() {
            return Err("reviewed paste contains no pasteable characters".to_owned());
        }
        if text.len() > terminal_input::TERMINAL_PASTE_LIMIT_BYTES {
            return Err(format!(
                "reviewed paste exceeds the {}-byte limit",
                terminal_input::TERMINAL_PASTE_LIMIT_BYTES
            ));
        }
        if !terminal_clipboard_target_is_current(
            target,
            self.workspace.active(),
            self.composer.focused,
        ) {
            return Err(
                "clipboard paste was cancelled because the active input changed".to_owned(),
            );
        }
        let session = self
            .sessions
            .get_mut(&target)
            .ok_or_else(|| format!("terminal @{} is unavailable", target.get()))?;
        session.ensure_pty_input_open()?;
        session
            .paste_text(text)
            .map_err(|error| format!("terminal input failed: {error}"))
    }

    fn drain_terminal_clipboard_paste(&mut self, window: &PixelWindow) {
        let poll = match self.pending_clipboard_paste.as_ref() {
            Some(pending) => pending.read.try_poll(),
            None => return,
        };
        let agenterm_platform::clipboard::ClipboardTextReadPoll::Ready(result) = poll else {
            return;
        };
        let pending = self
            .pending_clipboard_paste
            .take()
            .expect("ready clipboard read remains owned until completion");
        let result = result
            .map_err(|error| format!("clipboard read failed: {error}"))
            .and_then(|text| {
                if pending.review {
                    // Hands the human the text in the host review control's own
                    // newline form (CRLF on Windows' multiline EDIT, LF on
                    // macOS/Linux) so a multiline paste paints as multiple rows
                    // everywhere; delivery re-normalizes to CR. The event loop
                    // keeps running while the review is open; completion arrives
                    // through `try_poll`.
                    let display = composer::paste_review_display_text(&text);
                    if display.is_empty() {
                        return Err("clipboard text contains no pasteable characters".to_owned());
                    }
                    match self.open_terminal_paste_review(window, pending.target, &display) {
                        // A host with no native review has always delivered
                        // this paste directly. Refusing it here would turn a
                        // Windows freeze fix into a Linux/macOS regression, so
                        // the unreviewed path stays exactly as shipped; the
                        // missing review is a platform gap owned by the
                        // adapter, not a reason to drop the human's paste.
                        Err(PasteReviewRefusal::Unsupported) => {}
                        Err(PasteReviewRefusal::Failed(error)) => return Err(error),
                        Ok(()) => return Ok(()),
                    }
                }
                let text = terminal_input::normalize_terminal_paste(&text);
                if text.is_empty() {
                    return Err("clipboard text contains no pasteable characters".to_owned());
                }
                self.deliver_terminal_paste(pending.target, &text)
            });
        self.terminal_clipboard_error = result.err();
        self.mark_host_ui_full_and_repaint(window);
    }

    /// Opens the editable confirmation without blocking the event loop.
    ///
    /// `Unsupported` is kept distinct from `Failed` so the caller can tell "this
    /// platform has no review" from "the review broke". They deserve different
    /// answers: the first is a known gap with a defined fallback, the second is
    /// a failure the human must see.
    fn open_terminal_paste_review(
        &mut self,
        window: &PixelWindow,
        target: workspace::TabId,
        text: &str,
    ) -> Result<(), PasteReviewRefusal> {
        if self.pending_paste_review.is_some() {
            return Err(PasteReviewRefusal::Failed(
                "a terminal paste review is already open".to_owned(),
            ));
        }
        let waker = window.waker();
        let review = agenterm_platform::text_review::open_review(
            window.native_identity(),
            "Review terminal paste",
            "Review or edit the text before it is sent to the active terminal.",
            text,
            move || {
                let _ = waker.wake();
            },
        )
        .map_err(|error| match error {
            agenterm_platform::text_review::TextReviewError::Unsupported { .. } => {
                PasteReviewRefusal::Unsupported
            }
            error => PasteReviewRefusal::Failed(format!("paste review failed: {error}")),
        })?;
        self.pending_paste_review = Some(PendingPasteReview { target, review });
        window.request_redraw();
        Ok(())
    }

    fn drain_terminal_paste_review(&mut self, window: &PixelWindow) {
        let poll = match self.pending_paste_review.as_mut() {
            Some(pending) => pending.review.try_poll(),
            None => return,
        };
        let agenterm_platform::text_review::TextReviewPoll::Ready(edited) = poll else {
            return;
        };
        let pending = self
            .pending_paste_review
            .take()
            .expect("a ready review remains owned until completion");
        // A cancelled review is the human declining, not a failure: it clears
        // the pending state and leaves no error in host UI.
        let result = edited.map_or(Ok(()), |edited| {
            let text = terminal_input::normalize_terminal_paste(&edited);
            self.deliver_terminal_paste(pending.target, &text)
        });
        self.terminal_clipboard_error = result.err();
        self.mark_host_ui_full_and_repaint(window);
    }

    fn flush_pending_resize(&mut self, window: &PixelWindow) {
        self.pending_resize_deadline = None;
        if !self.pending_resize_requests.is_empty() {
            let requests = std::mem::take(&mut self.pending_resize_requests);
            self.dispatch_resize_run(window, requests);
        }
    }

    fn dispatch_resize_run(
        &mut self,
        window: &PixelWindow,
        requests: Vec<control::IncomingRequest>,
    ) {
        let Some((last_width, last_height)) =
            requests.last().and_then(|request| match &request.command {
                control::CliCommand::ResizeWindow { width, height } => Some((*width, *height)),
                _ => None,
            })
        else {
            return;
        };
        let result = window
            .request_logical_inner_size(LogicalSize::new(
                f64::from(last_width),
                f64::from(last_height),
            ))
            .map_err(|error| error.to_string());
        for request in requests {
            self.perf_stats.control_requests = self.perf_stats.control_requests.saturating_add(1);
            let control::CliCommand::ResizeWindow { width, height } = request.command else {
                continue;
            };
            let reply = match &result {
                Ok(()) => Ok(json::object(vec![
                    ("width", width.into()),
                    ("height", height.into()),
                ])),
                Err(error) => Err(error.clone()),
            };
            let _ = request.reply.send(reply);
        }
    }

    fn handle_sidebar_resize(
        &mut self,
        window: &PixelWindow,
        event: &PixelWindowEvent,
    ) -> Result<bool, PixelWindowError> {
        let metrics = window.metrics()?;
        let scale = metrics.scale_factor.max(1.0);
        let layout = self.layout(
            metrics.physical_width,
            metrics.physical_height,
            metrics.scale_factor,
        );
        let physical = |position: &LogicalPoint| {
            (
                (position.x * scale).max(0.0) as u32,
                (position.y * scale).max(0.0) as u32,
            )
        };
        match event {
            PixelWindowEvent::PointerMoved { position, .. } if self.sidebar_resizing => {
                self.sidebar_width_logical =
                    ui::sidebar_width_from_pointer(position.x, metrics.logical_size.width);
                let sidebar_width = self.effective_sidebar_dip();
                if let Ok(session) = self.active_session_mut() {
                    Self::configure_host_ui(session, metrics.scale_factor, sidebar_width);
                    session.queue_resize(
                        metrics.physical_width,
                        metrics.physical_height,
                        metrics.scale_factor,
                    );
                }
                let _ = window.set_pointer_cursor(PixelPointerCursor::ResizeHorizontal);
                window.request_redraw();
                Ok(true)
            }
            PixelWindowEvent::PointerMoved { position, .. } => {
                let (x, y) = physical(position);
                let over_grip = layout.sidebar_resize_grip(scale).contains(x, y);
                let _ = window.set_pointer_cursor(if over_grip {
                    PixelPointerCursor::ResizeHorizontal
                } else {
                    PixelPointerCursor::Arrow
                });
                // Track the hovered tree row so the per-row close button appears
                // only for the hovered (or active) row. Repaint only when the
                // hovered row actually changes, so motion over the terminal or
                // an unchanged row costs no frame.
                let count = self.workspace.nodes().len();
                let hit = ui::tree_hit(layout, x, y, self.tree_scroll_offset, count, scale);
                let hovered = match hit {
                    ui::TreeHit::Select(index) | ui::TreeHit::Close(index) => Some(index),
                    _ => None,
                };
                let hovered_header = match hit {
                    ui::TreeHit::NewRoot => Some(HeaderIcon::NewRoot),
                    ui::TreeHit::Settings => Some(HeaderIcon::Settings),
                    ui::TreeHit::SidebarToggle => Some(HeaderIcon::SidebarToggle {
                        collapsed: layout.sidebar_collapsed,
                    }),
                    _ => None,
                };
                if hovered != self.hovered_tree_row || hovered_header != self.hovered_header {
                    self.hovered_tree_row = hovered;
                    self.hovered_header = hovered_header;
                    self.mark_host_ui_full();
                    window.request_redraw();
                }
                // Grid crosshair: track the hovered terminal cell. Repaint the
                // terminal (and the status readout) only when the cell changes.
                let fw = metrics.physical_width;
                let fh = metrics.physical_height;
                let crosshair_changed = self
                    .active_session_mut()
                    .map(|session| session.update_crosshair(position, fw, fh))
                    .unwrap_or(false);
                if crosshair_changed {
                    if let Ok(session) = self.active_session_mut() {
                        session.dirty.mark_full();
                    }
                    self.mark_host_ui_full();
                    window.request_redraw();
                }
                Ok(over_grip)
            }
            PixelWindowEvent::PointerButton {
                button: PointerButton::Left,
                state: PointerButtonState::Pressed,
                position: Some(position),
                ..
            } => {
                let (x, y) = physical(position);
                if !layout.sidebar_resize_grip(scale).contains(x, y) {
                    return Ok(false);
                }
                self.sidebar_resizing = true;
                let _ = window.set_pointer_capture(true);
                let _ = window.set_pointer_cursor(PixelPointerCursor::ResizeHorizontal);
                Ok(true)
            }
            PixelWindowEvent::PointerButton {
                button: PointerButton::Left,
                state: PointerButtonState::Released,
                ..
            } if std::mem::take(&mut self.sidebar_resizing) => {
                let _ = window.set_pointer_capture(false);
                Ok(true)
            }
            PixelWindowEvent::PointerCaptureLost if std::mem::take(&mut self.sidebar_resizing) => {
                Ok(true)
            }
            _ => Ok(false),
        }
    }
}

/// Native title product half: `MiniCon` plus the package version this binary
/// was built with. The empty-tab greeting window uses this alone; a live tab
/// prefixes its context. One helper so those two paths cannot drift.
fn product_window_title() -> String {
    format!("MiniCon {}", env!("CARGO_PKG_VERSION"))
}

// The composer owns submission; the child owns its negotiated paste mode.
// Keep the final Enter outside the paste so a TUI does not absorb it as text.
/// Splits a composer submission into the payload and the Enter that commits it.
///
/// They must reach the child as two separate writes. Sent as one buffer the
/// bracketed paste and its trailing CR land in a single `read()` — measured
/// with a raw-mode probe, which received
/// `ESC[200~hello-from-composer ESC[201~\r` as one chunk. A TUI that emits the
/// paste asynchronously (ink/React, Bubble Tea and friends) then handles the CR
/// first, while its input box is still empty: it submits nothing and the pasted
/// text is left sitting there unsent. That is why such programs "need a real
/// Enter press".
///
/// The payload deliberately does **not** keep a trailing CR of its own. Keeping
/// it and appending a second Enter would submit twice — harmless in a shell
/// (one blank prompt) but actively wrong in an agent TUI, where it sends an
/// extra empty message.
fn composer_submission_payload(submission: &str, bracketed: bool) -> Vec<u8> {
    let draft = submission.strip_suffix('\r').unwrap_or(submission);
    if !bracketed {
        // Without bracketed paste the draft is ordinary typed input.
        return draft.as_bytes().to_vec();
    }
    let normalized = terminal_input::normalize_terminal_paste(draft);
    terminal_input::terminal_paste_bytes(&normalized, true)
}

impl PixelWindowApplication for ConApp {
    fn opened(&mut self, window: &PixelWindow) -> Result<PixelWindowDirective, PixelWindowError> {
        let metrics = window.metrics()?;
        let sidebar_width = self.effective_sidebar_dip();
        Self::configure_host_ui(
            self.active_session_mut()?,
            metrics.scale_factor,
            sidebar_width,
        );
        let directive = self.active_session_mut()?.opened(window)?;
        // Background startup must not override the host no-activate option.
        // Later user actions retain their explicit focus requests.
        if !self.no_activate {
            window.focus();
        }
        let _ = self.refresh_ime_status();
        // The endpoint was bound before this window existed; hand the loop's
        // waker to the callback that has been waiting for one, then drain
        // anything a client sent while there was nothing to wake.
        if let Ok(mut slot) = self.waker_slot.lock() {
            *slot = Some(window.waker());
        }
        self.attachment = window.attachment();
        let _ = window.waker().wake();
        self.refresh_title(window)?;
        match agenterm_platform::accessibility_publish::start("minicon", window.native_identity()) {
            // Keep a reconnectable publisher even if the first bus connect
            // failed. Snapshots stay in the store and go out on reconnect.
            // A no-op backend reports retains_snapshots() == false.
            Ok(publisher) if publisher.retains_snapshots() => {
                let inbox = Arc::clone(&self.a11y_inbox);
                let waker = window.waker();
                publisher.set_handler(Arc::new(move |node, action| {
                    let outcome = inbox.push(a11y::Request { node, action });
                    if outcome.should_wake {
                        let _ = waker.wake();
                    }
                    outcome.accepted
                }));
                self.a11y = Some(publisher);
                self.a11y_dirty = true;
                self.publish_a11y(window);
            }
            Ok(_) => {}
            Err(error) => {
                // Runtime diagnostics go to the log, never to a console this GUI
                // process is not attached to: writing one through
                // `parent_console` attaches the parent console for the duration,
                // and a windowed host should not couple its lifecycle to the
                // console that happens to have launched it.
                agenterm_platform::diagnostics::record(
                    "a11y",
                    "publish_setup_failed",
                    &error.to_string(),
                );
            }
        }
        Ok(directive)
    }

    fn event(
        &mut self,
        window: &PixelWindow,
        event: PixelWindowEvent,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        if self.exit {
            return Ok(PixelWindowDirective::Exit);
        }
        if self.settings_open
            && matches!(
                &event,
                PixelWindowEvent::Keyboard(NormalizedKeyEvent {
                    state: KeyPressState::Pressed,
                    logical: LogicalKey::Named(NamedKey::Escape),
                    ..
                })
            )
        {
            self.settings_open = false;
            self.mark_host_ui_full_and_repaint(window);
            return Ok(PixelWindowDirective::Continue);
        }
        if matches!(event, PixelWindowEvent::Wake) {
            self.drain_a11y_actions(window)?;
            self.drain_terminal_clipboard_paste(window);
            self.drain_terminal_paste_review(window);
            // PTY readers and the control server share the native wake path.
            // A flooded PTY continuously reposts Wake while bounded output
            // remains, so waiting until about_to_wait would starve control
            // requests (including the request that closes that PTY).
            let _ = self.drain_control(window, Instant::now());
            if self.exit {
                return Ok(PixelWindowDirective::Exit);
            }
            // A screenshot reply is owned by the next rendered frame. Under
            // sustained output, draining every PTY and reposting Wake here can
            // keep a slower native event loop on the Wake path indefinitely,
            // even though the control request already asked for a redraw.
            // Yield immediately so redraw gets the frame before more terminal
            // backlog; the readers retain their data and will wake us again.
            if self.pending_control.has_pending_screenshot() {
                window.request_redraw();
                return Ok(PixelWindowDirective::Continue);
            }
            let active = self.workspace.active();
            let session_budget = pty_drain_budget_per_session(self.workspace.nodes().len());
            let mut active_redraw = false;
            let mut backlog = false;
            let mut child_exited = false;
            for (id, session) in self.sessions.entries_mut() {
                let outcome = session.drain_pty_with_budget(session_budget);
                self.perf_stats.pty_drained_bytes = self
                    .perf_stats
                    .pty_drained_bytes
                    .saturating_add(outcome.bytes as u64);
                self.perf_stats.pty_budget_yields = self
                    .perf_stats
                    .pty_budget_yields
                    .saturating_add(u64::from(outcome.backlog));
                active_redraw |= active == Some(*id) && outcome.redraw;
                backlog |= outcome.backlog;
                child_exited |= outcome.child_exited;
            }
            if child_exited {
                // A tab row changed appearance (the shell exited), and that is
                // host UI the terminal redraw does not cover.
                self.mark_tree_dirty();
            }
            if active_redraw {
                if self.active_session()?.dirty.is_empty() {
                    // Title-only output and child exit still need one render,
                    // but there is no pixel rectangle to invalidate.
                    window.request_redraw();
                } else {
                    self.active_session()?.request_dirty_redraw(window);
                }
            }
            if backlog {
                let _ = window.waker().wake();
            }
            return Ok(PixelWindowDirective::Continue);
        }
        if matches!(event, PixelWindowEvent::PointerCaptureLost) {
            self.cancel_pointer_gestures_for_activation(window);
            return Ok(PixelWindowDirective::Continue);
        }
        // Another program may have changed the clipboard while MiniCon was in
        // the background, on a host that has no counter to say so.
        if matches!(&event, PixelWindowEvent::FocusChanged(true)) {
            self.clipboard_status.request_refresh();
        }
        match &event {
            PixelWindowEvent::Keyboard(key) => {
                self.ime_mode.observe_key(key);
            }
            PixelWindowEvent::Ime(ime) => {
                self.ime_mode.observe_ime(ime);
            }
            _ => {}
        }
        if matches!(
            &event,
            PixelWindowEvent::FocusChanged(_)
                | PixelWindowEvent::Keyboard(_)
                | PixelWindowEvent::Ime(_)
        ) && self.refresh_ime_status()
        {
            self.request_dirty_redraw(window);
        }
        if self.handle_sidebar_resize(window, &event)? {
            self.mark_host_ui_full();
            return Ok(PixelWindowDirective::Continue);
        }
        if let PixelWindowEvent::GeometryChanged { metrics, .. } = &event {
            self.mark_host_ui_full();
            let sidebar_width = self.effective_sidebar_dip();
            if let Ok(session) = self.active_session_mut() {
                Self::configure_host_ui(session, metrics.scale_factor, sidebar_width);
            }
        }
        if let PixelWindowEvent::Keyboard(key) = &event
            && self.handle_workspace_shortcut(window, key)?
        {
            return Ok(PixelWindowDirective::Continue);
        }
        // Escape closes the settings panel — the expected modal dismissal —
        // before the key reaches the composer or terminal.
        if self.settings_open
            && let PixelWindowEvent::Keyboard(key) = &event
            && key.state == KeyPressState::Pressed
            && matches!(key.logical, LogicalKey::Named(NamedKey::Escape))
            && !key.modifiers.control
            && !key.modifiers.alt
        {
            self.settings_open = false;
            self.mark_host_ui_full_and_repaint(window);
            return Ok(PixelWindowDirective::Continue);
        }
        if let PixelWindowEvent::MouseWheel {
            delta,
            position: Some(position),
            ..
        } = &event
        {
            let metrics = window.metrics()?;
            let scale = metrics.scale_factor.max(1.0);
            let layout = self.layout(
                metrics.physical_width,
                metrics.physical_height,
                metrics.scale_factor,
            );
            // No Ctrl exemption: Ctrl+wheel no longer zooms, so a modifier must
            // not steal the sidebar's own scroll.
            if layout.sidebar.contains(
                (position.x * scale).max(0.0) as u32,
                (position.y * scale).max(0.0) as u32,
            ) {
                let rows = match delta {
                    WheelDelta::Lines { y, .. } => minicon_core::numeric::round_f32(*y) as isize,
                    WheelDelta::LogicalPixels { y, .. } => {
                        minicon_core::numeric::round_f64(*y / ui::TREE_ROW_HEIGHT_DIP) as isize
                    }
                    _ => 0,
                };
                self.tree_scroll_offset = ui::scroll_tree(
                    self.tree_scroll_offset,
                    -rows,
                    self.workspace.nodes().len(),
                    layout.tree_capacity(),
                );
                self.mark_tree_dirty();
                self.request_dirty_redraw(window);
                return Ok(PixelWindowDirective::Continue);
            }
        }
        // A composer mouse selection in progress owns pointer motion and the
        // left release, so a drag extends the selection instead of reaching the
        // terminal. Handled before the press block below and before the terminal
        // gets the event.
        if self.composer_selecting {
            if let PixelWindowEvent::PointerMoved { position, .. } = &event {
                self.drag_composer_selection(window, position)?;
                return Ok(PixelWindowDirective::Continue);
            }
            if let PixelWindowEvent::PointerButton {
                button: PointerButton::Left,
                state: PointerButtonState::Released,
                ..
            } = &event
            {
                self.composer_selecting = false;
                // A press with no drag collapses back to a plain caret so a
                // click does not leave a zero-width "selection" armed.
                if self.composer.anchor == Some(self.composer.caret) {
                    self.composer.anchor = None;
                }
                self.mark_composer_dirty();
                self.request_dirty_redraw(window);
                return Ok(PixelWindowDirective::Continue);
            }
        }
        // Right-click inside the composer pastes at the caret — the mouse
        // counterpart to Ctrl/Cmd+V, for users who never learn the chord.
        if let PixelWindowEvent::PointerButton {
            button: PointerButton::Right,
            state: PointerButtonState::Pressed,
            position: Some(position),
            ..
        } = &event
            && self.composer_hit(window, position)? == ui::ComposerHit::Input
        {
            self.composer.focused = true;
            self.paste_clipboard_into_composer(window);
            return Ok(PixelWindowDirective::Continue);
        }
        if let PixelWindowEvent::PointerButton {
            button: PointerButton::Left,
            state: PointerButtonState::Pressed,
            position: Some(position),
            ..
        } = &event
        {
            if self.settings_open {
                let metrics = window.metrics()?;
                let scale = metrics.scale_factor.max(1.0);
                let layout = self.layout(
                    metrics.physical_width,
                    metrics.physical_height,
                    metrics.scale_factor,
                );
                let x = (position.x * scale).max(0.0) as u32;
                let y = (position.y * scale).max(0.0) as u32;
                match ui::settings_hit(layout, x, y, scale) {
                    ui::SettingsHit::Language(lang) => {
                        if self.ui_language != lang {
                            self.ui_language = lang;
                            self.mark_host_ui_full_and_repaint(window);
                        }
                    }
                    ui::SettingsHit::FontDown => {
                        if let Ok(session) = self.active_session_mut() {
                            session.zoom_font(window, false);
                        }
                        self.mark_host_ui_full_and_repaint(window);
                    }
                    ui::SettingsHit::FontReset => {
                        if let Ok(session) = self.active_session_mut() {
                            session.reset_font(window);
                        }
                        self.mark_host_ui_full_and_repaint(window);
                    }
                    ui::SettingsHit::FontUp => {
                        if let Ok(session) = self.active_session_mut() {
                            session.zoom_font(window, true);
                        }
                        self.mark_host_ui_full_and_repaint(window);
                    }
                    ui::SettingsHit::Theme(index) => {
                        let choice = match index {
                            0 => theme::ThemeChoice::Neutral,
                            1 => theme::ThemeChoice::Docs,
                            _ => theme::ThemeChoice::Paper,
                        };
                        if self.ui_theme != choice {
                            self.ui_theme = choice;
                            self.apply_theme_to_all_sessions();
                            self.mark_host_ui_full_and_repaint(window);
                        }
                    }
                    ui::SettingsHit::Panel => {}
                    ui::SettingsHit::Outside => {
                        // A header click still works (toggle settings, new tab);
                        // anything else outside the panel closes it.
                        if layout.new_root.contains(x, y) || layout.settings.contains(x, y) {
                            let _ = self.handle_tree_pointer(window, position)?;
                        } else {
                            self.settings_open = false;
                            self.mark_host_ui_full_and_repaint(window);
                        }
                    }
                }
                return Ok(PixelWindowDirective::Continue);
            }
            if self.handle_tree_pointer(window, position)? {
                return Ok(PixelWindowDirective::Continue);
            }
            if self.workspace.active().is_none() {
                let metrics = window.metrics()?;
                let scale = metrics.scale_factor.max(1.0);
                let layout = self.layout(
                    metrics.physical_width,
                    metrics.physical_height,
                    metrics.scale_factor,
                );
                let x = (position.x * scale).max(0.0) as u32;
                let y = (position.y * scale).max(0.0) as u32;
                if layout
                    .empty_new_terminal(metrics.physical_width, metrics.physical_height, scale)
                    .contains(x, y)
                {
                    self.open_session_contained(window, false);
                    window.focus();
                    window.request_redraw();
                } else if layout
                    .empty_quit(metrics.physical_width, metrics.physical_height, scale)
                    .contains(x, y)
                {
                    self.exit = true;
                    return Ok(PixelWindowDirective::Exit);
                }
                return Ok(PixelWindowDirective::Continue);
            }
            {
                let metrics = window.metrics()?;
                let scale = metrics.scale_factor.max(1.0);
                let layout = self.layout(
                    metrics.physical_width,
                    metrics.physical_height,
                    metrics.scale_factor,
                );
                let x = (position.x * scale).max(0.0) as u32;
                let y = (position.y * scale).max(0.0) as u32;
                // The status bar is informational; consume a click on it so it
                // neither defocuses the composer nor reaches the terminal.
                if ui::status_hit(layout, x, y) == ui::StatusHit::Bar {
                    return Ok(PixelWindowDirective::Continue);
                }
            }
            match self.composer_hit(window, position)? {
                ui::ComposerHit::Input => {
                    self.composer.focused = true;
                    self.composer.select_all = false;
                    let offset = self.composer_offset_at(window, position)?;
                    self.composer.caret = offset;
                    match self.register_composer_click(offset) {
                        // Single press: seed a collapsed selection and begin a
                        // drag. Motion extends it; a release with no motion
                        // collapses it back to a caret.
                        1 => {
                            self.composer.anchor = Some(offset);
                            self.composer_selecting = true;
                        }
                        // Double: select the word (non-whitespace token) here.
                        2 => {
                            let (start, end) = composer::word_bounds(&self.composer.text, offset);
                            self.composer.anchor = Some(start);
                            self.composer.caret = end;
                            self.composer_selecting = false;
                        }
                        // Triple (and the 3→1 cycle stops here): select all.
                        _ => {
                            composer::select_all(&mut self.composer);
                            self.composer_selecting = false;
                        }
                    }
                    self.update_composer_ime_anchor(window)?;
                    self.mark_composer_dirty();
                    // A physical client click has already activated and
                    // focused this top-level window. Re-entering native focus
                    // here synchronously emits another focus-message chain
                    // from inside pointer dispatch and can disrupt painting.
                    self.request_dirty_redraw(window);
                    return Ok(PixelWindowDirective::Continue);
                }
                ui::ComposerHit::Send => {
                    self.submit_composer();
                    self.composer.focused = true;
                    self.update_composer_ime_anchor(window)?;
                    self.mark_composer_dirty();
                    self.request_dirty_redraw(window);
                    return Ok(PixelWindowDirective::Continue);
                }
                ui::ComposerHit::Newline => {
                    // A break in the text, not a submission: the line goes on
                    // being edited. Sending here would make the two buttons the
                    // same button with different labels.
                    composer::insert(&mut self.composer, "\n");
                    self.composer.focused = true;
                    self.update_composer_ime_anchor(window)?;
                    self.mark_composer_dirty();
                    self.request_dirty_redraw(window);
                    return Ok(PixelWindowDirective::Continue);
                }
                // The Copy / Paste / Cut buttons give mouse-only users the
                // clipboard actions without a shortcut. Copy and Cut act on the
                // current selection (no-op with none); Paste inserts at the
                // caret. The composer keeps focus so the caret stays put.
                ui::ComposerHit::Copy => {
                    if let Some(text) = composer::selection_text(&self.composer) {
                        let _ = clipboard_status::set_text(text);
                    }
                    self.composer.focused = true;
                    self.mark_composer_dirty();
                    self.request_dirty_redraw(window);
                    return Ok(PixelWindowDirective::Continue);
                }
                ui::ComposerHit::Paste => {
                    self.composer.focused = true;
                    self.paste_clipboard_into_composer(window);
                    return Ok(PixelWindowDirective::Continue);
                }
                ui::ComposerHit::Cut => {
                    if let Some(text) = composer::cut(&mut self.composer) {
                        let _ = clipboard_status::set_text(&text);
                    }
                    self.composer.focused = true;
                    self.update_composer_ime_anchor(window)?;
                    self.mark_composer_dirty();
                    self.request_dirty_redraw(window);
                    return Ok(PixelWindowDirective::Continue);
                }
                ui::ComposerHit::Outside => {}
            }
            self.composer.focused = false;
            self.mark_composer_dirty();
            self.request_dirty_redraw(window);
        }
        if self.composer.focused {
            match event {
                PixelWindowEvent::Keyboard(key) if self.handle_composer_key(window, &key) => {
                    self.mark_composer_dirty();
                    self.request_dirty_redraw(window);
                    return Ok(PixelWindowDirective::Continue);
                }
                PixelWindowEvent::Ime(ime) => {
                    self.handle_composer_ime(window, ime);
                    self.mark_composer_dirty();
                    self.request_dirty_redraw(window);
                    return Ok(PixelWindowDirective::Continue);
                }
                _ => {}
            }
        }
        if self.workspace.active().is_none() {
            // Closing must keep working with no tabs open. `CloseRequested` is
            // handled in the session layer, which this early return never
            // reaches — so without this the window could not be closed at all
            // and the only way out was killing the process.
            if matches!(event, PixelWindowEvent::CloseRequested) {
                self.exit = true;
                return Ok(PixelWindowDirective::Exit);
            }
            return Ok(PixelWindowDirective::Continue);
        }
        let active = self.workspace.active().ok_or_else(|| {
            PixelWindowError::failed("con_session_missing", "no active terminal session")
        })?;
        let directive = self.active_session_mut()?.event(window, event)?;
        let requested = self
            .sessions
            .get_mut(&active)
            .is_some_and(ConTerminal::take_clipboard_paste_request);
        if requested && let Err(error) = self.request_terminal_clipboard_paste(window, active, true)
        {
            self.terminal_clipboard_error = Some(error);
            self.mark_host_ui_full_and_repaint(window);
        }
        Ok(directive)
    }

    fn render(
        &mut self,
        window: &PixelWindow,
        frame: &mut XrgbPixelFrame<'_>,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        // Explicit window close is the only product path that sets `exit`.
        // Closing the final tab is a valid zero-session greeting state.
        if self.exit {
            return Ok(PixelWindowDirective::Exit);
        }
        self.perf_stats.sync_present_stats(window.present_stats());
        if let Some(target) = self
            .pending_control
            .prepare_screenshot(self.workspace.active())
        {
            self.workspace.set_active(target);
        }
        let width = frame.width();
        let height = frame.height();
        let frame_info = frame.info();
        let host_retains_pixels = matches!(
            frame_info.retention,
            PixelBackingRetention::RetainedAcrossFrames
        );
        if self.workspace.active().is_none() {
            let scale = window.metrics()?.scale_factor.max(1.0);
            self.note_frame_dimensions(width, height, scale);
            self.paint_empty_workspace(frame.pixels_mut(), width, height, scale);
            frame
                .commit(PixelFrameWrite::Full)
                .map_err(|error| PixelWindowError::failed("con_frame_commit", error.to_string()))?;
            self.host_ui_dirty = DirtyRegion::default();
            self.perf_stats.record_host_direct_frame();
            return Ok(PixelWindowDirective::Continue);
        }
        let scale = self.active_session()?.scale.max(1.0);
        self.note_frame_dimensions(width, height, scale);
        self.active_session_mut()?
            .note_frame_dimensions(width, height);
        // A transient host gives us a fresh frame. Raster directly into it
        // instead of retaining and copying a second full-window canvas. Native
        // retained backing can still use bounded partial updates when valid.
        if !host_retains_pixels || !frame_info.content_valid {
            self.host_ui_dirty.mark_full();
            self.active_session_mut()?.dirty.mark_full();
        }

        // Drain before consuming the candidate. PTY output can alter arbitrary
        // cells, cursor state, modes, scrollback, and selection, so it always
        // upgrades the candidate to full before raster starts.
        let (drain, wake_pending) = {
            let session = self.active_session_mut()?;
            let drain = session.drain_pty();
            let wake_pending = session.pty_wake_pending.load(Ordering::Acquire);
            (drain, wake_pending)
        };
        if drain.child_exited {
            // The active tab's row dims when its shell exits; keep the tree
            // damage with the drain that observed it.
            self.mark_tree_dirty();
            self.host_ui_dirty.mark_full();
        }
        self.perf_stats.pty_drained_bytes = self
            .perf_stats
            .pty_drained_bytes
            .saturating_add(drain.bytes as u64);
        self.perf_stats.pty_budget_yields = self
            .perf_stats
            .pty_budget_yields
            .saturating_add(u64::from(drain.backlog));
        if drain.backlog || wake_pending {
            // Output arrived while this render was being prepared, or the
            // bounded drain still has a tail. The current frame is made safe
            // with a full raster; the reader/waker will schedule the
            // next bounded drain without forcing an unconditional Wake full.
            self.active_session_mut()?.dirty.mark_full();
        }

        // The candidate is complete before either product surface starts
        // rasterizing. A late dirty state is therefore a programming error,
        // not an excuse to label a partial frame after the fact.
        let mut candidate = self.take_dirty_candidate(width, height);
        #[cfg(windows)]
        if !candidate.is_empty() && !candidate.is_full() {
            // The optimized clipped raster path can currently erase retained
            // pixels outside its dirty bounds after pointer/composer updates.
            // Correctness wins until that path is proven by native before/after
            // screenshots; Windows con redraws a complete frame meanwhile.
            candidate = DirtyRegion::full_frame(width, height);
        }
        if host_retains_pixels && !frame_info.content_valid {
            candidate = DirtyRegion::full_frame(width, height);
        }
        if host_retains_pixels && candidate.is_full() {
            // A late resize, PTY drain, or invalidation must widen the native
            // update region before a partial GDI present can be accepted.
            window.request_redraw();
        }
        let render_started = Instant::now();
        let active_id = match self.workspace.active() {
            Some(id) => id,
            None => {
                return Err(PixelWindowError::failed(
                    "con_session_missing",
                    "no active terminal session",
                ));
            }
        };
        let directive = {
            let session = self.sessions.get_mut(&active_id).ok_or_else(|| {
                PixelWindowError::failed("con_session_missing", "active terminal session missing")
            })?;
            let directive = session.render(window, frame.pixels_mut(), width, height, candidate)?;
            if !candidate.is_empty() {
                self.paint_host_ui(frame.pixels_mut(), width, height, candidate)?;
            }
            directive
        };
        let mut discard_capture_frame = false;
        if let Some(screenshot) = self.pending_control.take_screenshot() {
            let control_pending::ScreenshotWork {
                target,
                path,
                reply,
                restore_active,
            } = screenshot;
            let response_path = path.to_string_lossy().into_owned();
            let shared_reply = Arc::new(std::sync::Mutex::new(Some(reply)));
            let done = Arc::new(AtomicBool::new(false));
            self.pending_control.start_screenshot(
                target,
                Arc::clone(&shared_reply),
                Arc::clone(&done),
            );
            let waker = window.waker();
            agent_interface::submit_png_atomic(
                path,
                frame.pixels_mut(),
                width,
                height,
                Box::new(move |write_result| {
                    let result = write_result
                        .map(|encode_ns| {
                            json::object(vec![
                                ("path", response_path.into()),
                                ("width", width.into()),
                                ("height", height.into()),
                                ("encode_ns", encode_ns.into()),
                            ])
                        })
                        .map_err(|error| format!("write screenshot: {error}"));
                    if let Some(reply) = shared_reply
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .take()
                    {
                        let _ = reply.send(result);
                    }
                    done.store(true, Ordering::Release);
                    let _ = waker.wake();
                }),
            );
            if let Some(restore_active) = restore_active
                && self.sessions.contains_key(&restore_active)
            {
                self.workspace.set_active(restore_active);
                self.mark_host_ui_full();
                self.active_session_mut()?.dirty.mark_full();
                self.refresh_title(window)?;
                window.request_redraw();
                discard_capture_frame = true;
            }
        }
        if discard_capture_frame {
            if let Err(error) = frame.commit(PixelFrameWrite::Discard) {
                return Err(PixelWindowError::failed(
                    "con_capture_frame_discard",
                    error.to_string(),
                ));
            }
            self.perf_stats.discarded_capture_frames =
                self.perf_stats.discarded_capture_frames.saturating_add(1);
            self.perf_stats.record_frame(render_started.elapsed());
            self.perf_stats
                .record_raster_candidate(candidate, width, height);
            return Ok(directive);
        }
        let write = frame_write_for_candidate(
            frame_info.retention,
            frame_info.content_valid,
            candidate,
            width,
            height,
        );
        frame
            .commit(write)
            .map_err(|error| PixelWindowError::failed("con_frame_commit", error.to_string()))?;
        self.perf_stats.record_host_direct_frame();
        self.perf_stats.record_frame(render_started.elapsed());
        self.perf_stats
            .record_raster_candidate(candidate, width, height);
        Ok(directive)
    }

    fn detached(
        &mut self,
        waker: &WindowWaker,
        attachment: &agenterm_platform::window_host::WindowAttachment,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        // A headless process never ran `opened`, so this is where it learns how
        // to ask for its first window.
        if self.attachment.is_none() {
            self.attachment = Some(attachment.clone());
        }
        if self.pending_headless_session {
            self.pending_headless_session = false;
            if let Ok(mut slot) = self.waker_slot.lock() {
                *slot = Some(waker.clone());
            }
            // A bind failure already exited; a spawn failure here is the same
            // class of startup failure and must not leave a silent, empty
            // process behind.
            let session = self.active_session_mut()?;
            session.open_detached(waker)?;
        }
        self.detached_turn()
    }

    fn about_to_wait(
        &mut self,
        window: &PixelWindow,
        now: Instant,
    ) -> Result<PixelWindowDirective, PixelWindowError> {
        if self.exit {
            return Ok(PixelWindowDirective::Exit);
        }
        let control_deadline = self.drain_control(window, now);
        // A control request can explicitly close the window while draining.
        // Re-check before touching session state so close stays a normal host
        // exit rather than a synthetic `con_session_missing` failure.
        if self.exit {
            return Ok(PixelWindowDirective::Exit);
        }
        if self.pending_control.has_pending_screenshot() {
            window.request_redraw();
        }
        // Cheap enough per iteration: on Windows it reads a system counter and
        // only starts a (background) read when the clipboard really changed.
        let waker = window.waker();
        if self.clipboard_status.poll(|limit| {
            agenterm_platform::clipboard::read_text_async(limit, move || {
                let _ = waker.wake();
            })
            .ok()
        }) {
            self.mark_host_ui_full_and_repaint(window);
        }
        if self.a11y_dirty {
            self.publish_a11y(window);
        }
        if self.workspace.active().is_none() {
            return Ok(control_deadline
                .map_or(PixelWindowDirective::Wait, PixelWindowDirective::WaitUntil));
        }
        let directive = self.active_session_mut()?.about_to_wait(window, now)?;
        Ok(match (directive, control_deadline) {
            (PixelWindowDirective::Wait, Some(deadline)) => {
                PixelWindowDirective::WaitUntil(deadline)
            }
            (PixelWindowDirective::WaitUntil(current), Some(deadline)) => {
                PixelWindowDirective::WaitUntil(current.min(deadline))
            }
            (directive, _) => directive,
        })
    }
}

#[derive(Clone, Copy)]
enum InjectedKey {
    Named(NamedKey),
    Char(char),
}

fn injected_key_event(key: InjectedKey, ctrl: bool, alt: bool, shift: bool) -> NormalizedKeyEvent {
    let modifiers = ModifierState {
        control: ctrl,
        alt,
        shift,
        meta: false,
    };
    let logical = match key {
        InjectedKey::Named(named) => LogicalKey::Named(named),
        InjectedKey::Char(ch) => LogicalKey::Character(ch.to_string()),
    };
    let text = match key {
        InjectedKey::Named(_) => None,
        InjectedKey::Char(ch) if !ctrl && !alt => Some(ch.to_string()),
        InjectedKey::Char(_) => None,
    };
    NormalizedKeyEvent {
        logical,
        physical: PhysicalKeyCode::Other,
        text,
        state: KeyPressState::Pressed,
        repeat: false,
        modifiers,
    }
}

fn encode_child_exit_code(code: Option<i32>) -> u64 {
    code.map_or(0, |code| u64::from(code as u32) + 1)
}

fn decode_child_exit_code(encoded: u64) -> Option<i32> {
    encoded
        .checked_sub(1)
        .and_then(|bits| u32::try_from(bits).ok())
        .map(|bits| bits as i32)
}

#[derive(Clone, Copy)]
enum InjectedMouseButton {
    Left,
    Middle,
    Right,
}

/// Parses a whole key sequence before any of it is applied.
///
/// Both `SendKeys` and `SendUiKeys` used to parse inside their injection
/// loops, so a malformed key late in a sequence arrived after the earlier keys
/// had already been delivered. The caller then got an error and a partially
/// applied side effect, with no way to tell how much had happened. Validating
/// the whole sequence first makes the operation all-or-nothing with respect to
/// its input, which is the only contract a remote caller can reason about.
///
/// The remaining partial-failure window is injection itself, which can still
/// fail mid-sequence; that one is reported as `terminal input failed` and is
/// inherent to writing to a pty.
fn parse_control_keys(specs: &[String]) -> Result<Vec<(InjectedKey, bool, bool, bool)>, String> {
    specs.iter().map(|spec| parse_control_key(spec)).collect()
}

fn parse_control_key(spec: &str) -> Result<(InjectedKey, bool, bool, bool), String> {
    let mut parts: Vec<_> = spec.split('+').collect();
    let key_name = parts
        .pop()
        .filter(|value| !value.is_empty())
        .ok_or_else(|| format!("invalid key specification {spec:?}"))?;
    let mut ctrl = false;
    let mut alt = false;
    let mut shift = false;
    for modifier in parts {
        match modifier.to_ascii_lowercase().as_str() {
            "ctrl" | "control" => ctrl = true,
            "alt" => alt = true,
            "shift" => shift = true,
            _ => return Err(format!("unknown key modifier {modifier:?}")),
        }
    }
    let named = NamedKey::from_name(key_name);
    let key = if let Some(named) = named {
        InjectedKey::Named(named)
    } else {
        let mut chars = key_name.chars();
        let character = chars.next().ok_or_else(|| "empty key".to_owned())?;
        if chars.next().is_some() {
            return Err(format!("unknown key {key_name:?}"));
        }
        InjectedKey::Char(character)
    };
    Ok((key, ctrl, alt, shift))
}

fn control_mouse_button(button: control::MouseButton) -> Result<InjectedMouseButton, String> {
    match button {
        control::MouseButton::Left => Ok(InjectedMouseButton::Left),
        control::MouseButton::Middle => Ok(InjectedMouseButton::Middle),
        control::MouseButton::Right => Ok(InjectedMouseButton::Right),
        control::MouseButton::None => Err("press/release requires a mouse button".to_owned()),
    }
}

// ---------------------------------------------------------------------------
// Pixel helpers
// ---------------------------------------------------------------------------

/// A cell's pixel rectangle. The four values are always derived together from
/// the grid position, so passing them separately only invited transposition.

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum CandidateRedrawRequest {
    None,
    Full,
    Partial(HostPixelRect),
}

fn candidate_redraw_request(
    candidate: DirtyRegion,
    width: u32,
    height: u32,
) -> CandidateRedrawRequest {
    if candidate.is_full() || width == 0 || height == 0 {
        return CandidateRedrawRequest::Full;
    }
    let Some(bounds) = candidate.clip(width, height).bounds() else {
        return CandidateRedrawRequest::None;
    };
    if bounds.is_empty() {
        CandidateRedrawRequest::None
    } else {
        CandidateRedrawRequest::Partial(HostPixelRect::new(
            bounds.left,
            bounds.top,
            bounds.right,
            bounds.bottom,
        ))
    }
}

fn frame_write_for_candidate(
    retention: PixelBackingRetention,
    content_valid: bool,
    candidate: DirtyRegion,
    width: u32,
    height: u32,
) -> PixelFrameWrite {
    if matches!(retention, PixelBackingRetention::Transient) || !content_valid {
        return PixelFrameWrite::Full;
    }
    match candidate_redraw_request(candidate, width, height) {
        CandidateRedrawRequest::None => PixelFrameWrite::None,
        CandidateRedrawRequest::Full => PixelFrameWrite::Full,
        CandidateRedrawRequest::Partial(rect) => PixelFrameWrite::Partial(rect),
    }
}

fn request_candidate_redraw(window: &PixelWindow, candidate: DirtyRegion, width: u32, height: u32) {
    match candidate_redraw_request(candidate, width, height) {
        CandidateRedrawRequest::None => {}
        CandidateRedrawRequest::Full => window.request_redraw(),
        CandidateRedrawRequest::Partial(rect) => window.request_redraw_rect(rect),
    }
}

fn candidate_bounds(candidate: DirtyRegion, width: u32, height: u32) -> PixelRect {
    candidate
        .clip(width, height)
        .bounds()
        .unwrap_or_else(PixelRect::empty)
}

// ---------------------------------------------------------------------------
// Keyboard → PTY byte encoding
// ---------------------------------------------------------------------------
//
// The encoding tables themselves live in `agenterm_platform::terminal_input`
// so the GUI terminal and this console host cannot drift apart again. Only the
// host-specific policy (what counts as a local shortcut) stays here.

#[cfg(test)]
mod tests {
    use super::*;
    use crate::terminal::tests::{parser, prepared_pointer_terminal};

    /// A key sequence is validated as a whole before any of it is delivered.
    ///
    /// What this guards is the *separation*, not the parser: `SendKeys` and
    /// `SendUiKeys` must parse every key before the first one is applied. The
    /// parser alone is trivially total over a slice, so the interesting claim is
    /// that the keys a caller sees applied are exactly the ones that validated.
    /// This replays both shapes — validate-then-apply, and the older
    /// apply-as-you-parse — and asserts the first delivers nothing on a bad
    /// sequence while the second delivers a prefix. If the dispatch arms ever go
    /// back to parsing inside their loops, the second shape is what they become.
    #[test]
    fn a_key_sequence_is_validated_before_any_key_is_applied() {
        let sequence = [
            "ctrl+a".to_owned(),
            "shift+F1".to_owned(),
            "ctrl+".to_owned(),
            "b".to_owned(),
        ];

        // The shape the dispatch uses now: parse all, then apply all.
        let mut delivered = 0usize;
        if let Ok(parsed) = parse_control_keys(&sequence) {
            delivered += parsed.len();
        }
        assert_eq!(
            delivered, 0,
            "a malformed key anywhere must stop the whole sequence before any key is applied"
        );

        // The shape that was there before, which delivered a prefix.
        let mut legacy_delivered = 0usize;
        for spec in &sequence {
            if parse_control_key(spec).is_err() {
                break;
            }
            legacy_delivered += 1;
        }
        assert_eq!(
            legacy_delivered, 2,
            "the old apply-as-you-parse shape really did deliver a prefix, so the assertion \
             above is not vacuous"
        );

        // And a valid sequence still delivers everything, so the guard is not
        // passing by refusing all input.
        let good = ["ctrl+a".to_owned(), "b".to_owned()];
        assert_eq!(parse_control_keys(&good).map(|keys| keys.len()), Ok(2));
    }

    #[test]
    fn a_key_sequence_with_no_bad_member_parses_entirely() {
        let sequence = ["a".to_owned(), "ctrl+alt+Delete".to_owned()];
        let parsed = parse_control_keys(&sequence).expect("all keys are valid");
        assert_eq!(parsed.len(), sequence.len());
        // `InjectedKey` deliberately carries no `Debug`/`PartialEq`, so the
        // modifiers are what is checked directly and the key itself through the
        // event it produces.
        assert_eq!((parsed[1].1, parsed[1].2, parsed[1].3), (true, true, false));
        let event = injected_key_event(parsed[1].0, parsed[1].1, parsed[1].2, parsed[1].3);
        assert!(event.modifiers.control && event.modifiers.alt && !event.modifiers.shift);
        assert!(parse_control_keys(&[]).expect("empty is valid").is_empty());
    }

    /// The rules themselves are unit-tested in `minicon_core::keymap`, which
    /// is target-neutral. What can only be tested here is the translation: a
    /// host key event becoming the chord the table expects. A rule proved
    /// against the wrong chord proves nothing.
    #[test]
    fn a_host_key_event_becomes_the_chord_the_table_expects() {
        let enter = injected_key_event(InjectedKey::Named(NamedKey::Enter), false, false, false);
        assert_eq!(
            composer_chord(&enter),
            Some(keymap::Chord::plain(keymap::Key::Enter))
        );

        let ctrl_o = injected_key_event(InjectedKey::Char('o'), true, false, false);
        assert_eq!(
            composer_chord(&ctrl_o),
            Some(keymap::Chord {
                key: keymap::Key::Character('o'),
                modifiers: keymap::Modifiers {
                    control: true,
                    ..keymap::Modifiers::default()
                },
            })
        );

        // Every modifier is carried across; dropping one would silently turn a
        // selection into a movement.
        let shift_up =
            injected_key_event(InjectedKey::Named(NamedKey::ArrowUp), false, false, true);
        assert_eq!(
            composer_chord(&shift_up).map(|chord| chord.modifiers),
            Some(keymap::Modifiers {
                shift: true,
                ..keymap::Modifiers::default()
            })
        );

        // A key with no name in the table is not a composer key. Tab is one:
        // the table answers nothing for it and the host must not invent a
        // chord that the rules would then have to reject.
        let tab = injected_key_event(InjectedKey::Named(NamedKey::Tab), false, false, false);
        assert_eq!(composer_chord(&tab), None);
    }

    /// The end-to-end shape of the owner's 0.1.22 report, from a host event to
    /// the action the handler will run: pasted lines, caret in the middle, Up
    /// must move rather than replace the draft with history.
    #[test]
    fn up_on_a_pasted_multi_line_draft_resolves_to_a_caret_move() {
        let mut state = composer::ComposerState::default();
        composer::paste(&mut state, "first\nsecond\nthird");
        composer::move_caret(&mut state, composer::Move::Up);
        let shape = composer_draft_shape(&state);
        assert!(
            !shape.caret_on_first_line && !shape.caret_on_last_line,
            "the caret should be on the middle line of a three-line draft"
        );

        let up = injected_key_event(InjectedKey::Named(NamedKey::ArrowUp), false, false, false);
        let chord = composer_chord(&up).expect("Up is a composer key");
        assert_eq!(
            keymap::action(chord, CLIPBOARD_MODIFIER, shape),
            Some(keymap::Action::Move(composer::Move::Up))
        );

        // The invariant: an ordinary one-line draft still recalls.
        let mut single = composer::ComposerState::default();
        composer::insert(&mut single, "ls -l");
        assert_eq!(
            keymap::action(chord, CLIPBOARD_MODIFIER, composer_draft_shape(&single)),
            Some(keymap::Action::RecallPrevious)
        );
    }

    /// An empty draft has one line, not zero. Reported as "no last line" it
    /// would make Down move nowhere on the most common state the box is in.
    #[test]
    fn an_empty_draft_is_one_line() {
        let shape = composer_draft_shape(&composer::ComposerState::default());
        assert!(shape.caret_on_first_line && shape.caret_on_last_line);
    }

    /// The clipboard modifier is the composer's only platform branch, so what
    /// is checked here is that this build picked the right one. What that
    /// choice then means is `keymap`'s to test, on any machine.
    #[test]
    fn this_build_picked_its_platform_clipboard_modifier() {
        #[cfg(target_os = "macos")]
        assert_eq!(CLIPBOARD_MODIFIER, keymap::ClipboardModifier::ControlOrMeta);
        #[cfg(not(target_os = "macos"))]
        assert_eq!(CLIPBOARD_MODIFIER, keymap::ClipboardModifier::ControlOnly);
    }

    /// `candidate_bounds` clips a repaint candidate to the frame and turns it
    /// into the rectangle the surface painter receives. The three shapes the
    /// render loop can hold — full, empty, and a partial rectangle — must each
    /// produce a rectangle inside the frame, and a candidate entirely outside
    /// the frame must become empty rather than an out-of-bounds rect.
    #[test]
    fn candidate_bounds_clips_every_dirty_shape_to_the_frame() {
        // Full with no dimensions yet: clipped to the whole frame.
        assert_eq!(
            candidate_bounds(DirtyRegion::full(), 80, 24),
            PixelRect::from_xywh(0, 0, 80, 24)
        );
        // Empty: no rectangle at all.
        assert_eq!(
            candidate_bounds(DirtyRegion::empty(), 80, 24),
            PixelRect::empty()
        );
        // A partial rectangle inside the frame passes through unchanged.
        let mut partial = DirtyRegion::empty();
        partial.mark_rect(PixelRect::from_xywh(3, 4, 10, 5));
        assert_eq!(
            candidate_bounds(partial, 80, 24),
            PixelRect::from_xywh(3, 4, 10, 5)
        );
        // A partial rectangle straddling the edge is clipped to the frame.
        let mut overhang = DirtyRegion::empty();
        overhang.mark_rect(PixelRect::from_xywh(70, 20, 40, 40));
        assert_eq!(
            candidate_bounds(overhang, 80, 24),
            PixelRect::from_xywh(70, 20, 10, 4)
        );
        // A zero-size frame yields an empty rectangle even for a full region.
        assert_eq!(
            candidate_bounds(DirtyRegion::full(), 0, 0),
            PixelRect::empty()
        );
    }

    #[test]
    fn ime_status_snapshot_keeps_fixed_types_when_known_or_unknown() {
        assert_eq!(
            ime_status_json(None),
            json::object(vec![
                ("known", false.into()),
                ("name", "".into()),
                ("available", false.into()),
                ("open", false.into()),
                ("native_mode", false.into()),
                ("full_shape", false.into()),
                ("label", "IME: ?".into()),
            ])
        );
        let mut status = agenterm_platform::ime::ImeStatus::default();
        status.name = "Pinyin".to_owned();
        status.available = true;
        status.open = true;
        status.native_mode = true;
        status.full_shape = true;
        assert_eq!(
            ime_status_json(Some(&status)),
            json::object(vec![
                ("known", true.into()),
                ("name", "Pinyin".into()),
                ("available", true.into()),
                ("open", true.into()),
                ("native_mode", true.into()),
                ("full_shape", true.into()),
                ("label", "IME: Pinyin · native · full-width".into()),
            ])
        );
    }

    #[test]
    fn terminal_clipboard_completion_requires_same_active_terminal() {
        let target = workspace::TabId::new(7);
        assert!(terminal_clipboard_target_is_current(
            target,
            Some(target),
            false
        ));
        assert!(!terminal_clipboard_target_is_current(
            target,
            Some(workspace::TabId::new(8)),
            false
        ));
        assert!(!terminal_clipboard_target_is_current(target, None, false));
    }

    #[test]
    fn terminal_clipboard_completion_rejects_composer_focus() {
        let target = workspace::TabId::new(7);
        assert!(!terminal_clipboard_target_is_current(
            target,
            Some(target),
            true
        ));
    }

    /// The status strip has one line, so the priority is a real decision:
    /// a tab that could not open outranks a clipboard refusal, and with
    /// neither the strip falls back to the routing label.
    #[test]
    fn status_strip_prefers_the_host_notice() {
        assert_eq!(status_strip_notice(None, None), None);
        assert_eq!(status_strip_notice(None, Some("clip")), Some("clip"));
        assert_eq!(status_strip_notice(Some("tab"), None), Some("tab"));
        assert_eq!(status_strip_notice(Some("tab"), Some("clip")), Some("tab"));
    }

    /// Regression coverage for a real, confirmed hang: `claude` (a real
    /// modern Node/Ink TUI) run through `-e` produced zero output and never
    /// returned — indefinitely — while the identical command via a plain
    /// `cmd.exe /c` outside this binary completed in under a second. Root
    /// cause: neither DA1 (`CSI c`) nor CPR (`CSI 6n`) was answered, and a
    /// program that blocks waiting for either reply before proceeding hangs
    /// forever against a terminal that never responds. Confirmed fixed
    /// live (not just by this unit test): the same `claude --help`
    /// invocation that previously produced nothing now renders its full
    /// output through this binary.
    #[test]
    fn terminal_paint_respects_left_tree_inset() {
        let mut parser = vt100::Parser::new_with_callbacks(2, 4, 0, ConCallbacks::default());
        parser.process(b"A");
        let width = 64;
        let height = 32;
        let untouched = 0x0012_3456;
        let mut pixels = vec![untouched; (width * height) as usize];
        let mut surface = Surface::new(&mut pixels, width, height);
        paint_cells_at(
            &mut surface,
            parser.screen(),
            None,
            8,
            16,
            Rgb(0xEE, 0xEE, 0xEE),
            Rgb(0x00, 0x00, 0x00),
            palette::STANDARD_ANSI,
            12,
            24,
            0,
        );
        for row in surface.pixels.chunks_exact(width as usize) {
            assert!(row[..24].iter().all(|pixel| *pixel == untouched));
        }
        assert!((0..16).any(|y| {
            let row = &surface.pixels[y * width as usize..(y + 1) * width as usize];
            row[24..32].iter().any(|pixel| *pixel != untouched)
        }));
    }

    #[test]
    fn paste_review_keeps_visual_line_breaks_until_pty_delivery() {
        let clipboard = "first\nsecond\r\nthird";
        let expected_display = if cfg!(target_os = "windows") {
            "first\r\nsecond\r\nthird"
        } else {
            "first\nsecond\nthird"
        };
        assert_eq!(
            composer::paste_review_display_text(clipboard),
            expected_display
        );
        assert_eq!(
            terminal_input::normalize_terminal_paste(clipboard),
            "first\rsecond\rthird"
        );
        assert_eq!(
            terminal_input::normalize_terminal_paste(&composer::paste_review_display_text(
                clipboard
            )),
            "first\rsecond\rthird"
        );
    }

    #[test]
    fn composer_send_places_one_submit_enter_outside_negotiated_paste() {
        let mut parser = parser();
        for draft in ["hello", "first\nsecond", "中文\n", "one\n\ntwo"] {
            let mut composer = composer::ComposerState::default();
            composer::insert(&mut composer, draft);
            let submission = composer.take_submission().unwrap();
            let plain = submission.strip_suffix('\r').unwrap_or(&submission);

            let payload = composer_submission_payload(&submission, false);
            assert_eq!(payload, plain.as_bytes());

            parser.process(b"\x1b[?2004h");
            let payload =
                composer_submission_payload(&submission, parser.screen().bracketed_paste());
            let expected = format!("\x1b[200~{}\x1b[201~", draft.replace('\n', "\r"));
            assert_eq!(payload, expected.as_bytes());
            // The commit must not ride inside or immediately behind the paste:
            // a child reading the payload sees no CR at all, so it cannot
            // submit an empty input before the pasted text has landed.
            assert!(
                !payload.ends_with(b"\r"),
                "payload must not carry its own commit"
            );

            parser.process(b"\x1b[?2004l");
            let payload =
                composer_submission_payload(&submission, parser.screen().bracketed_paste());
            assert_eq!(payload, plain.as_bytes());
        }
    }

    /// Committing a composer draft must send the *same* bytes a physical Enter
    /// produces, taken from the shared key encoder rather than hardcoded. In a
    /// terminal there is no separate "key event" channel — a key press *is*
    /// those bytes — so the only way the commit stays a real Enter is to encode
    /// it through the same path `send-keys Enter` and a physical press use. If a
    /// negotiated keyboard mode ever changes what Enter means, this follows it.
    #[test]
    fn composer_commit_uses_the_real_enter_key_encoding() {
        let app = prepared_pointer_terminal();
        let mode = TerminalKeyMode {
            application_cursor: app.parser.screen().application_cursor(),
            ime_active: app.ime_attached,
        };
        let event = injected_key_event(InjectedKey::Named(NamedKey::Enter), false, false, false);
        let expected = terminal_input::key_event_to_bytes(&event, mode).expect("Enter encodes");
        assert_eq!(
            app.encoded_enter(),
            expected,
            "commit must use the key encoder"
        );
        assert_eq!(
            app.encoded_enter(),
            b"\r",
            "which today is a carriage return"
        );
    }

    #[test]
    fn a_left_drag_the_application_owns_is_copied_by_its_span() {
        let at = |row, col| TerminalPoint { row, col };
        // A drag that moved: its span is copied.
        assert_eq!(
            application_drag_copy_span(0, Some(at(2, 3)), at(4, 10)),
            Some((at(2, 3), at(4, 10)))
        );
        // A click is not a selection.
        assert_eq!(
            application_drag_copy_span(0, Some(at(2, 3)), at(2, 3)),
            None
        );
        // Middle and right drags are not selection gestures.
        assert_eq!(
            application_drag_copy_span(1, Some(at(0, 0)), at(3, 3)),
            None
        );
        assert_eq!(
            application_drag_copy_span(2, Some(at(0, 0)), at(3, 3)),
            None
        );
        // No recorded press, nothing to copy.
        assert_eq!(application_drag_copy_span(0, None, at(3, 3)), None);
    }

    #[test]
    fn auto_copy_requires_a_non_empty_local_selection() {
        let point = TerminalPoint { row: 2, col: 4 };
        assert!(!selection_should_auto_copy(None));
        assert!(!selection_should_auto_copy(Some((point, point))));
        assert!(selection_should_auto_copy(Some((
            point,
            TerminalPoint { row: 2, col: 7 },
        ))));
    }

    /// The reported "Ctrl+wheel zoom occasionally makes the window vanish
    /// with no dialog" crash, reproduced as a unit test.
    ///
    /// Zooming *in* grows the cell, which shrinks the column count, which
    /// makes `apply_resize` call `vt100::Screen::set_size` with fewer
    /// columns. Shrinking a row truncates its cell array — and if a wide
    /// (CJK/emoji) character straddled the new right edge, its continuation
    /// cell is dropped while the first half stays behind in the final
    /// column. From then on the row violates vt100's own invariant that a
    /// wide cell always has its continuation at `col + 1`, and the next
    /// narrow character written onto that orphan made `Screen::text`
    /// dereference `col + 1` and `unwrap()` a `None` — a panic, which under
    /// this binary's `panic = "abort"` release profile is a silent,
    /// dialog-free process exit. Exactly the reported symptom, exactly the
    /// reported direction (enlarging, not shrinking), and "occasional"
    /// because it needs a wide glyph to land on the new last column.
    ///
    /// A shell that prints CJK (a localized Windows shell banner, a path
    /// with Han characters, any CJK program output) hits this; a pure-ASCII
    /// session never does, which is why earlier ASCII-driven reproduction
    /// attempts came back clean.
    #[test]
    fn narrow_write_over_a_wide_cell_orphaned_by_a_zoom_in_resize_survives() {
        let mut parser =
            vt100::Parser::<ConCallbacks>::new_with_callbacks(2, 6, 0, ConCallbacks::default());
        // Three wide chars fill columns 0-1, 2-3, 4-5 exactly.
        parser.process("你好吗".as_bytes());
        assert!(parser.screen().cell(0, 4).expect("col 4 exists").is_wide());

        // One Ctrl+wheel notch's worth of zoom-in: the same call
        // `apply_resize` makes, with one column fewer. Column 5 (the
        // continuation half) is truncated away; column 4 keeps the first
        // half and is now an orphan.
        parser.screen_mut().set_size(2, 5);

        // The shell then prints one ordinary narrow character onto that
        // cell — a cursor move to row 1, column 5 (1-indexed) and an 'x'.
        // Before the fix this aborted the process here.
        parser.process(b"\x1b[1;5Hx");

        assert_eq!(
            parser
                .screen()
                .cell(0, 4)
                .expect("col 4 still exists")
                .contents(),
            "x",
            "the narrow write must land, not just avoid panicking"
        );
    }

    /// The same invariant, checked one level down: a shrinking row resize
    /// must never leave a wide cell without its continuation. This is the
    /// property the fix actually restores, independent of which write
    /// happens to trip over the violation afterwards.
    #[test]
    fn shrinking_a_grid_never_leaves_a_wide_cell_without_its_continuation() {
        for cols in 2u16..=12 {
            let mut parser = vt100::Parser::<ConCallbacks>::new_with_callbacks(
                2,
                12,
                0,
                ConCallbacks::default(),
            );
            // Offset by one narrow char so the wide pairs straddle both odd
            // and even column boundaries as `cols` sweeps down.
            parser.process("a你好吗你".as_bytes());
            parser.screen_mut().set_size(2, cols);
            let last = cols - 1;
            let cell = parser.screen().cell(0, last).expect("last column exists");
            assert!(
                !cell.is_wide(),
                "cols={cols}: wide cell orphaned in the final column by the resize"
            );
        }
    }

    fn argv(values: &[&str]) -> Vec<String> {
        values.iter().map(|value| (*value).to_owned()).collect()
    }

    #[test]
    fn dash_e_takes_the_rest_of_the_line_verbatim() {
        // Flags belonging to the hosted program must reach it untouched, not
        // be parsed (or rejected) by this host.
        let parsed = parse_args(&argv(&["-e", "ssh", "host", "-p", "22"])).expect("parses");
        assert_eq!(parsed.command, Some(argv(&["ssh", "host", "-p", "22"])));

        // Host flags before -e still apply.
        let parsed =
            parse_args(&argv(&["--cols", "100", "-e", "pwsh", "-NoLogo"])).expect("parses");
        assert_eq!(parsed.cols, Some(100));
        assert_eq!(parsed.command, Some(argv(&["pwsh", "-NoLogo"])));
    }

    #[test]
    fn dash_e_without_a_program_is_an_error() {
        assert!(parse_args(&argv(&["-e"])).is_err());
    }

    #[test]
    fn bad_numeric_values_are_reported_rather_than_silently_dropped() {
        // The previous parser used `.ok()`, so `--cols twenty` was ignored and
        // the user got a default-sized window with no explanation.
        let error = parse_args(&argv(&["--cols", "twenty"])).expect_err("should reject");
        assert!(error.contains("--cols"), "{error}");
        assert!(error.contains("twenty"), "{error}");

        assert!(parse_args(&argv(&["--font-size"])).is_err());
        assert!(parse_args(&argv(&["--working-dir"])).is_err());
    }

    #[test]
    fn conpty_is_off_unless_a_feature_asks_for_it() {
        // The product invariant behind this release: the classic Windows
        // console hosts the shell by default, because ConPTY cannot carry
        // mouse input. If this ever defaults back to true, every hosted TUI
        // silently goes blind to clicks again.
        assert!(!ConArgs::default().features.conpty);
        assert!(
            !parse_args(&argv(&["--cols", "80"]))
                .expect("parses")
                .features
                .conpty
        );
    }

    #[test]
    fn features_accept_both_spellings_and_can_be_turned_back_off() {
        for argv_form in [
            vec!["--feature", "conpty"],
            vec!["--feature=conpty"],
            // Comma-separated and repeated forms must agree, including when a
            // later word overrides an earlier one.
            vec!["--feature", "no-conpty,conpty"],
            vec!["--feature", "no-conpty", "--feature", "conpty"],
        ] {
            let parsed = parse_args(&argv(&argv_form)).expect("parses");
            assert!(parsed.features.conpty, "{argv_form:?} should enable conpty");
        }
        for argv_form in [
            vec!["--feature", "no-conpty"],
            vec!["--feature", "conpty,no-conpty"],
            vec!["--feature", "conpty", "--feature", "no-conpty"],
        ] {
            let parsed = parse_args(&argv(&argv_form)).expect("parses");
            assert!(
                !parsed.features.conpty,
                "{argv_form:?} should disable conpty"
            );
        }
    }

    #[test]
    fn an_unknown_feature_is_a_typo_and_names_the_known_set() {
        // Silently ignoring an unrecognised name is how a user believes a
        // switch is on while it never was — the exact failure that made the
        // Windows mouse look unfixable.
        let error = parse_args(&argv(&["--feature", "conpyt"])).expect_err("should reject");
        assert!(error.contains("conpyt"), "{error}");
        assert!(error.contains("conpty"), "must list the known set: {error}");
        // A bad name anywhere in the list fails the whole list.
        assert!(parse_args(&argv(&["--feature", "conpty,nope"])).is_err());
        assert!(parse_args(&argv(&["--feature=no-nope"])).is_err());
    }

    #[test]
    fn usage_lists_every_feature_so_none_is_undiscoverable() {
        let usage = usage_text();
        assert!(usage.contains("--feature"), "{usage}");
        for (name, _) in FEATURES {
            assert!(usage.contains(name), "usage omits feature {name}");
        }
    }

    #[test]
    fn unknown_flags_are_rejected_with_usage() {
        let error = parse_args(&argv(&["--nope"])).expect_err("should reject");
        assert!(error.contains("--nope"), "{error}");
        assert!(error.contains("Usage:"), "{error}");
    }

    #[test]
    fn every_valued_flag_reports_a_missing_value() {
        // Each flag that takes a value must name itself when nothing follows,
        // rather than silently defaulting or panicking on an empty iterator.
        for (flag, needle) in [
            ("--font-size", "--font-size"),
            ("--cols", "--cols"),
            ("--rows", "--rows"),
            ("--control", "--control"),
            ("--emit-snapshot", "--emit-snapshot"),
            ("--feature", "--feature"),
        ] {
            let error = parse_args(&argv(&[flag])).expect_err(flag);
            assert!(
                error.contains(needle),
                "{flag} must name itself in: {error}"
            );
        }
    }

    #[test]
    fn inline_value_forms_match_their_separate_forms() {
        // `--working-dir=` and `--font-size=` are the two flags with an inline
        // spelling; the value must land in the same field as the split form.
        let inline = parse_args(&argv(&["--working-dir=C:/work dir", "--font-size=18.5"]))
            .expect("inline forms parse");
        assert_eq!(inline.working_dir.as_deref(), Some("C:/work dir"));
        assert_eq!(inline.font_size, Some(18.5));

        let split = parse_args(&argv(&[
            "--working-dir",
            "C:/work dir",
            "--font-size",
            "18.5",
        ]))
        .expect("split forms parse");
        assert_eq!(inline.working_dir, split.working_dir);
        assert_eq!(inline.font_size, split.font_size);
    }

    #[test]
    fn inline_numeric_flags_reject_garbage() {
        // The inline spelling must validate like the split one; `.ok()` once
        // let `--font-size=big` through as an ignored value.
        let error = parse_args(&argv(&["--font-size=big"])).expect_err("should reject");
        assert!(error.contains("--font-size"), "{error}");
        assert!(error.contains("big"), "{error}");
    }

    #[test]
    fn clipped_surface_matches_full_terminal_rect_operations() {
        let width = 16u32;
        let height = 8u32;
        let candidate = PixelRect::from_xywh(2, 1, 12, 6);
        let mut full_pixels = vec![0u32; (width * height) as usize];
        let mut full = Surface::new(&mut full_pixels, width, height);
        full.fill_rect(0, 0, width, height, 0);
        full.fill_rect(2, 1, 4, 3, 0x0011_2233);
        full.fill_rect(8, 4, 5, 2, 0x0044_5566);
        full.fill_rect(3, 6, 8, 1, 0x0077_8899);

        let mut partial_pixels = vec![0u32; (width * height) as usize];
        let mut partial = Surface::with_clip(&mut partial_pixels, width, height, candidate);
        partial.fill_rect(0, 0, width, height, 0);
        partial.fill_rect(2, 1, 4, 3, 0x0011_2233);
        partial.fill_rect(8, 4, 5, 2, 0x0044_5566);
        partial.fill_rect(3, 6, 8, 1, 0x0077_8899);

        assert_eq!(partial_pixels, full_pixels);
        assert_eq!(partial_pixels[0], 0);
        assert_eq!(partial_pixels[(7 * width + 15) as usize], 0);
    }

    #[test]
    fn direct_host_target_pixels_match_retained_raster_pixel_for_pixel() {
        let width = 9u32;
        let height = 5u32;
        let mut retained_pixels = vec![0u32; (width * height) as usize];
        let mut direct_pixels = vec![0u32; (width * height) as usize];
        for pixels in [&mut retained_pixels, &mut direct_pixels] {
            let mut surface = Surface::new(pixels, width, height);
            surface.fill_rect(0, 0, width, height, 0x0001_0203);
            surface.fill_rect(2, 1, 4, 2, 0x000A_0B0C);
            surface.fill_rect(6, 3, 2, 1, 0x000D_0E0F);
        }
        assert_eq!(direct_pixels, retained_pixels);
    }

    #[test]
    fn stress_raster_every_printable_ascii_and_cjk_at_every_clamped_size() {
        // font::raster clamps internally to [8,72], but sweep the full clamped
        // range against a wide character set in case some specific glyph's
        // outline panics ab_glyph's rasterizer at a particular size — the kind
        // of bug that would only show up for "large font + this app's prompt
        // happens to contain that glyph," matching a real-use-only report.
        let mut chars: Vec<char> = (32u8..=126).map(char::from).collect();
        chars.extend([
            '中', '文', '字', '形', '日', '本', '語', '한', '국', '어', '➜', '★', '你',
        ]);
        for size in 8u16..=72 {
            for &ch in &chars {
                let _ = font::raster(ch, size);
            }
        }
    }

    #[test]
    fn stress_paint_cells_with_shell_like_output_across_font_sizes() {
        // End-to-end: real PTY-shaped bytes (prompt, CJK, colors) through the
        // full paint path at every clamped font size, at a window size small
        // enough to force the grid toward its floor while the font is large.
        let bytes: &[u8] =
            b"C:/dev/agenterm> echo \xe4\xbd\xa0\xe5\xa5\xbd \x1b[1;32mok\x1b[0m\r\n\x1b[4munderline\x1b[0m ";
        for size in 8u16..=72 {
            let cell_w = 10u32.max(u32::from(size) / 2);
            let cell_h = 20u32.max(u32::from(size));
            let cols = (200u32 / cell_w).clamp(2, 512) as u16;
            let rows = (200u32 / cell_h).clamp(2, 512) as u16;
            let mut parser = vt100::Parser::<ConCallbacks>::new_with_callbacks(
                rows,
                cols,
                0,
                ConCallbacks::default(),
            );
            parser.process(bytes);
            let fw = u32::from(cols) * cell_w;
            let fh = u32::from(rows) * cell_h;
            let mut pixels = vec![0u32; (fw * fh) as usize];
            let mut surface = Surface::new(&mut pixels, fw, fh);
            paint_cells(
                &mut surface,
                parser.screen(),
                None,
                cell_w,
                cell_h,
                Rgb(0xCC, 0xCC, 0xCC),
                Rgb(0, 0, 0),
                palette::STANDARD_ANSI,
                size,
            );
        }
    }

    /// The IME preedit reports how many cells it drew so the caller can push the
    /// cursor past the composition. The count must be exactly the cells that
    /// landed on screen: a preedit running past the right edge or below the
    /// surface stops there and reports only what was drawn, so the cursor is
    /// never pushed off-screen and never lands on unwritten cells.
    /// Resolving a control target is the server-side check that turns a stale or
    /// missing tab into a typed refusal instead of a panic or a silent retarget.
    /// Cover an explicit live tab, the `None` fallback to the active tab, a
    /// stale id, and `None` with no active tab at all.
    /// Selecting a tab reports whether it changed the active tab, so the
    /// windowed caller can skip the pointer-gesture cancel when the user
    /// re-selected the tab they are already on. A stale id is refused by
    /// `set_active` and reports no change rather than silently retargeting.
    /// Closing an ancestor must not disturb the tab the user is actually in:
    /// the descendant keeps its session and stays active, while the closed
    /// node's own session is the one dropped. Only the node's slot moves.
    #[test]
    fn closing_an_ancestor_leaves_an_active_descendant_selected() {
        let mut app = ConApp::new(None, None);
        let root = app.workspace.active().expect("one tab to start");
        let parent = app.workspace.add_root("parent".to_owned()).unwrap();
        let child = app.workspace.add_child(parent, "child".to_owned()).unwrap();
        assert!(
            app.sessions.insert(parent, ConTerminal::new(None)).is_ok(),
            "a fresh store accepts the parent"
        );
        assert!(
            app.sessions.insert(child, ConTerminal::new(None)).is_ok(),
            "a fresh store accepts the child"
        );
        assert!(app.workspace.set_active(child));

        // Close the parent, not the tab that is active.
        assert!(
            !app.detach_session(parent),
            "two tabs remain, so the workspace is not empty"
        );

        // The descendant is promoted to a root and is still the active tab.
        assert_eq!(
            app.workspace.node(child).and_then(|node| node.parent),
            None,
            "the child is promoted out of the closed branch"
        );
        assert_eq!(
            app.workspace.active(),
            Some(child),
            "an active descendant must stay selected"
        );
        // The closed node's session went; the descendant's stayed.
        assert!(!app.sessions.contains_key(&parent));
        assert!(
            app.sessions.contains_key(&child),
            "the active descendant keeps its session"
        );
        assert_eq!(app.workspace.nodes().len(), 2);
        assert!(app.workspace.nodes().iter().any(|node| node.id == root));
    }

    /// The other order: close the child first, then its parent. Neither close
    /// may leave a dangling session or a node whose parent is gone, and after
    /// both the workspace holds exactly the untouched tab.
    #[test]
    fn closing_a_child_then_its_parent_leaves_no_dangling_session() {
        let mut app = ConApp::new(None, None);
        let root = app.workspace.active().expect("one tab to start");
        let parent = app.workspace.add_root("parent".to_owned()).unwrap();
        let child = app.workspace.add_child(parent, "child".to_owned()).unwrap();
        for id in [parent, child] {
            assert!(
                app.sessions.insert(id, ConTerminal::new(None)).is_ok(),
                "a fresh store accepts @{}",
                id.get()
            );
        }

        assert!(app.workspace.set_active(child));
        assert!(!app.detach_session(child), "the parent still remains");
        assert!(!app.sessions.contains_key(&child));
        // The parent is still there, and now has no children.
        assert!(app.workspace.node(parent).is_some());
        assert!(
            !app.workspace
                .nodes()
                .iter()
                .any(|node| node.parent == Some(child)),
            "no node may point at the closed child"
        );

        assert!(!app.detach_session(parent), "the untouched root remains");
        assert!(!app.sessions.contains_key(&parent));
        // Every surviving node's parent, if any, exists — no dangling links.
        for node in app.workspace.nodes() {
            if let Some(parent) = node.parent {
                assert!(
                    app.workspace.node(parent).is_some(),
                    "node @{} points at absent parent @{}",
                    node.id.get(),
                    parent.get()
                );
            }
        }
        assert_eq!(app.workspace.nodes().len(), 1);
        assert_eq!(app.workspace.nodes()[0].id, root);
        assert_eq!(app.workspace.active(), Some(root));
    }

    /// Closing a tab that was never given a session (the tree and the session
    /// store can be out of step mid-startup) still closes its node and reports
    /// the emptied workspace, rather than leaving the node behind.
    #[test]
    fn closing_a_tab_with_no_session_still_closes_its_node() {
        let mut app = ConApp::new(None, None);
        let only = app.workspace.active().unwrap();
        // Drop the session, leaving the node orphaned in the store's view.
        app.sessions.remove(&only);
        assert!(
            app.detach_session(only),
            "the last node closing empties the workspace"
        );
        assert!(app.workspace.nodes().is_empty());
        assert!(app.workspace.active().is_none());
    }

    #[test]
    fn selecting_a_tab_reports_whether_the_active_tab_changed() {
        let mut app = ConApp::new(None, None);
        let first = app.workspace.active().expect("one tab to start");
        let second = app.workspace.add_root("second".to_owned()).unwrap();
        assert!(app.workspace.set_active(first));

        // Re-selecting the active tab changes nothing.
        assert!(!app.select_tab(first), "already active is a no-op");
        assert_eq!(app.workspace.active(), Some(first));
        // Selecting a different tab changes the active tab and reports it.
        assert!(app.select_tab(second), "a new tab is a change");
        assert_eq!(app.workspace.active(), Some(second));
        // And back again.
        assert!(app.select_tab(first));
        assert_eq!(app.workspace.active(), Some(first));

        // An id that no longer exists is refused: no change is reported and the
        // selection stays where it was, rather than moving to nothing.
        let stale = workspace::TabId::new(9_999);
        assert!(!app.select_tab(stale), "an unknown id cannot become active");
        assert_eq!(
            app.workspace.active(),
            Some(first),
            "a refused selection must not disturb the current one"
        );
    }

    /// Closing a tab drops its session and node, and reports whether that left
    /// the workspace empty — the case the windowed caller answers with a
    /// greeting page instead of a new active session. It also carries the
    /// closed terminal's settings forward, so the next tab opens configured
    /// like the one that just closed.
    #[test]
    fn detaching_a_session_reports_an_emptied_workspace_and_seeds_the_next_tab() {
        let mut app = ConApp::new(None, None);
        let only = app.workspace.active().expect("one tab to start");
        // One tab in the tree, one session: closing it empties the workspace.
        assert_eq!(app.workspace.nodes().len(), 1);
        assert!(app.sessions.contains_key(&only));
        assert!(
            app.detach_session(only),
            "closing the only tab must report an empty workspace"
        );
        assert!(app.workspace.nodes().is_empty());
        assert!(app.workspace.active().is_none());
        assert!(
            !app.sessions.contains_key(&only),
            "the session must be gone"
        );

        // With a second tab left behind, the workspace is not empty and the
        // remaining tab becomes active.
        let mut app = ConApp::new(None, None);
        let first = app.workspace.active().unwrap();
        let second = app.workspace.add_root("second".to_owned()).unwrap();
        assert!(
            app.sessions.insert(second, ConTerminal::new(None)).is_ok(),
            "a fresh store accepts the second tab"
        );
        assert!(app.sessions.contains_key(&second));
        assert!(app.workspace.set_active(first));
        assert!(
            !app.detach_session(first),
            "a surviving tab means the workspace is not empty"
        );
        assert_eq!(app.workspace.active(), Some(second));
        assert_eq!(app.workspace.nodes().len(), 1);

        // The seed carries the closed terminal forward: a terminal seeded from
        // it opens at the same size the closed one had.
        let mut app = ConApp::new(None, None);
        let only = app.workspace.active().unwrap();
        let session = app.sessions.get_mut(&only).expect("the session exists");
        session.cols = 120;
        session.rows = 40;
        assert!(app.detach_session(only));
        let reopened = app.session_seed.create_session();
        assert_eq!(reopened.cols, 120, "the next tab inherits the closed size");
        assert_eq!(reopened.rows, 40);
    }

    #[test]
    fn a_control_target_resolves_live_tabs_and_refuses_the_rest() {
        let mut app = ConApp::new(None, None);
        let live = app.workspace.active().expect("a new app has one tab");

        // An explicit live tab resolves to itself.
        assert_eq!(app.control_target(Some(live)), Ok(live));
        // No target falls back to the active tab.
        assert_eq!(app.control_target(None), Ok(live));

        // A stale id — a tab that has since closed — is refused by name.
        let stale = workspace::TabId::new(9_999);
        assert_eq!(
            app.control_target(Some(stale)),
            Err(format!("terminal @{} does not exist", stale.get()))
        );

        // A tab in the tree with no session yet is refused the same way: the
        // authority is the session store, not the workspace.
        let treeless = app
            .workspace
            .add_root("pending".to_owned())
            .expect("a second root is accepted");
        assert_eq!(
            app.control_target(Some(treeless)),
            Err(format!("terminal @{} does not exist", treeless.get()))
        );

        // With no active tab, a `None` target has nothing to fall back to.
        app.workspace.close(live);
        if let Some(pending) = app.workspace.active() {
            // The freshly added root became active; close it too so the
            // workspace is empty for the no-active case.
            app.workspace.close(pending);
        }
        assert_eq!(app.workspace.active(), None, "the workspace must be empty");
        assert_eq!(
            app.control_target(None),
            Err("no active terminal".to_owned())
        );
    }

    #[test]
    fn offline_help_and_version_are_solo() {
        assert_eq!(offline_cli_exit(&["--version".to_owned()]), Some(0));
        assert_eq!(offline_cli_exit(&["--help".to_owned()]), Some(0));
        assert_eq!(offline_cli_exit(&["--status".to_owned()]), Some(0));
        assert_eq!(
            offline_cli_exit(&["--version".to_owned(), "x".to_owned()]),
            Some(2)
        );
        assert_eq!(
            offline_cli_exit(&["--status".to_owned(), "x".to_owned()]),
            Some(2)
        );
    }

    /// The defect this exists for: `cmd.exe` reports its own path as its
    /// window title, so every tab in the tree read the same long string and
    /// the tree could not tell its tabs apart.
    #[test]
    fn a_child_naming_itself_is_not_a_title() {
        let path = r"C:\Windows\system32\cmd.exe";
        assert_eq!(session_label(path, path, "cmd"), "cmd", "the full path");
        assert_eq!(
            session_label("cmd.exe", path, "cmd"),
            "cmd",
            "the file name"
        );
        assert_eq!(
            session_label(r"C:\WINDOWS\SYSTEM32\CMD.EXE", path, "cmd"),
            "cmd",
            "Windows paths are not case sensitive and neither is this"
        );
        assert_eq!(session_label("   ", path, "cmd"), "cmd", "blank is absent");
    }

    /// A title the child genuinely set is information the user asked for, and
    /// must win. Suppressing it would be the opposite defect.
    #[test]
    fn a_real_title_from_the_child_is_kept() {
        let path = r"C:\Windows\system32\cmd.exe";
        assert_eq!(session_label("deploy", path, "cmd"), "deploy");
        assert_eq!(session_label("  build 3  ", path, "cmd"), "build 3");
        // Contains the program name but says more than it: still a title.
        assert_eq!(
            session_label("cmd.exe — release", path, "cmd"),
            "cmd.exe — release"
        );
    }

    #[test]
    fn a_program_is_known_by_its_short_name() {
        assert_eq!(program_stem(r"C:\Windows\system32\cmd.exe"), "cmd");
        assert_eq!(program_stem("/bin/bash"), "bash");
        assert_eq!(program_stem(r"bin\bash"), "bash");
        assert_eq!(program_stem("pwsh"), "pwsh");
        // Never empty: an unnamed tab is worse than a generic one.
        assert_eq!(program_stem(""), "terminal");
        assert_eq!(program_stem("/"), "terminal");
    }

    /// `--status` exists to end a round trip, so it has to carry the facts a
    /// round trip would otherwise have to ask for. A status line that omits
    /// one of them just produces a second question.
    #[test]
    fn status_reports_the_facts_a_bug_report_needs() {
        let status = status_text();
        assert!(
            status.starts_with(&format!("minicon {}", env!("CARGO_PKG_VERSION"))),
            "the build identifies itself first: {status}"
        );
        assert!(status.contains("pty backend"), "{status}");
        assert!(status.contains("font"), "{status}");
        assert!(status.contains("diagnostics"), "{status}");
    }

    /// The backend line must name one of the backends that exist, not a
    /// placeholder. Whichever this machine has, it is the answer that decides
    /// where to look first on an old Windows.
    #[test]
    fn status_names_a_real_pty_backend() {
        let status = status_text();
        assert!(
            ["conpty", "console-agent", "unix-pty"]
                .iter()
                .any(|kind| status.contains(kind)),
            "no known backend named: {status}"
        );
    }

    /// The status must describe the run a user gets, not a capability the
    /// machine has. On Windows the default is the classic console (mouse
    /// input is proven there), and the switch is set later in startup, so a
    /// bare `--status` used to answer "conpty" for sessions that ran on the
    /// console agent.
    #[cfg(windows)]
    #[test]
    fn status_names_the_backend_a_default_windows_run_uses() {
        let status = crate::cli::offline_status_text_for_test();
        assert!(
            status.contains("console-agent"),
            "a default Windows run hosts the classic console: {status}"
        );
        assert!(
            status.contains("--feature conpty"),
            "the status must name the way to ConPTY: {status}"
        );
    }

    /// The font line reports a measurement, not just a name. The name alone
    /// cannot distinguish "resolved the right face" from "resolved a face that
    /// is the wrong shape for a grid", which is the failure it exists to
    /// diagnose.
    #[test]
    fn status_reports_measured_font_width_not_only_a_face_name() {
        let status = status_text();
        assert!(
            status.contains("half/full width correct")
                || status.contains("FULL WIDTH IS NOT DOUBLE")
                || status.contains("width unmeasured")
                || status.contains("font           unavailable"),
            "the font line carries no measurement: {status}"
        );
    }

    #[test]
    fn child_exit_code_encoding_preserves_complete_signed_domain() {
        for code in [
            None,
            Some(i32::MIN),
            Some(-1),
            Some(0),
            Some(1),
            Some(i32::MAX),
        ] {
            assert_eq!(decode_child_exit_code(encode_child_exit_code(code)), code);
        }
    }
}
