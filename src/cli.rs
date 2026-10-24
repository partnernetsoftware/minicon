//! Command-line surface: argument parsing, feature selection, usage and
//! status text, and the flags that must be served without opening a window.
//!
//! Moved out of `main.rs` verbatim; no behavior change.

use std::path::PathBuf;

use minicon_core::json;

use crate::DEFAULT_FONT_PX;
use crate::control;

/// Command-line options, parsed out of `main` so the precedence and
/// passthrough rules are unit-testable rather than only observable by
/// launching a window.
#[derive(Debug, Default, PartialEq)]
pub(crate) struct ConArgs {
    pub(crate) no_activate: bool,
    pub(crate) working_dir: Option<String>,
    pub(crate) font_size: Option<f64>,
    pub(crate) cols: Option<u16>,
    pub(crate) rows: Option<u16>,
    pub(crate) control_endpoint: Option<String>,
    pub(crate) command: Option<Vec<String>>,
    /// `--emit-snapshot`: see `agent_interface` module docs.
    pub(crate) snapshot_path: Option<PathBuf>,
    /// `--feature` selections, already resolved to their final on/off state.
    pub(crate) features: Features,
    /// `--headless`: start with no window. The sessions, the PTYs and the
    /// control endpoint all run; only the surface is absent until `attach-gui`.
    pub(crate) headless: bool,
}

/// Backend selections chosen with `--feature`, as distinct from the user
/// preferences in `minicon.json`. A preference (font size, geometry) is
/// something a person sets once and wants remembered; a feature selects which
/// implementation hosts the shell, which belongs to a single invocation and
/// must be visible in the command line that reproduces a bug report. Keeping
/// them in one namespace is what lets a future switch land without inventing
/// another flag.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Features {
    /// Host the shell through ConPTY. Mouse input on this path arrives at the
    /// child as the terminal's SGR bytes passed through as VT key characters
    /// (measured 2026-10-22), not as synthesized console mouse records, so a
    /// record-reading program sees no clicks here.
    pub(crate) conpty: bool,
    /// Host the shell through the owned Rust console (minicon-console). The
    /// Windows default since 0.2.20 where the build carries it.
    pub(crate) rust_console: bool,
    /// Host the shell through the classic console agent. The pre-0.2.20
    /// default, kept selectable as the rollback path.
    pub(crate) console_agent: bool,
}

/// The Windows terminal backends a session can be hosted by.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum Backend {
    ConsoleAgent,
    Conpty,
    RustConsole,
}

impl Backend {
    /// The spelling shared by `--feature`, `minicon.json`'s `backend` key and
    /// `--status`.
    pub(crate) fn name(self) -> &'static str {
        match self {
            Self::ConsoleAgent => "console-agent",
            Self::Conpty => "conpty",
            Self::RustConsole => "rust-console",
        }
    }
}

/// Every name `--feature` accepts, with the one-line help shown in usage.
/// A name absent here is a typo, not an unsupported feature, and is rejected.
pub(crate) const FEATURES: &[(&str, &str)] = &[
    (
        "conpty",
        "Windows: host the shell through ConPTY. Mouse reaches the program\n\
     \x20                    as VT bytes rather than console records. Inert off\n\
     \x20                    Windows.",
    ),
    (
        "rust-console",
        "Windows: the owned Rust console (minicon-console) -- the default on\n\
     \x20                    builds that carry it. Later backend selections win.",
    ),
    (
        "console-agent",
        "Windows: the classic console through a hidden console agent; the\n\
     \x20                    pre-0.2.20 default, kept as the rollback path.",
    ),
];

impl Features {
    /// The backend this command line names, if any. The three backend words
    /// are mutually exclusive in `apply`, so at most one is set.
    pub(crate) fn explicit_backend(self) -> Option<Backend> {
        if self.rust_console {
            Some(Backend::RustConsole)
        } else if self.conpty {
            Some(Backend::Conpty)
        } else if self.console_agent {
            Some(Backend::ConsoleAgent)
        } else {
            None
        }
    }

    /// Applies one `--feature` word. `no-<name>` turns a feature off, so a
    /// script can state its intent instead of relying on today's defaults.
    fn apply(&mut self, word: &str) -> Result<(), String> {
        let (name, enable) = match word.strip_prefix("no-") {
            Some(rest) => (rest, false),
            None => (word, true),
        };
        match name {
            "conpty" => {
                self.conpty = enable;
                if enable {
                    self.rust_console = false;
                    self.console_agent = false;
                }
            }
            "rust-console" => {
                self.rust_console = enable;
                if enable {
                    self.conpty = false;
                    self.console_agent = false;
                }
            }
            "console-agent" => {
                self.console_agent = enable;
                if enable {
                    self.conpty = false;
                    self.rust_console = false;
                }
            }
            _ => {
                let known: Vec<&str> = FEATURES.iter().map(|(name, _)| *name).collect();
                return Err(format!(
                    "error: unknown feature '{word}'; known features: {}\n",
                    known.join(", ")
                ));
            }
        }
        Ok(())
    }
}

/// Parses arguments, returning the message to print on failure.
#[inline(never)]
pub(crate) fn parse_args(args: &[String]) -> Result<ConArgs, String> {
    let mut parsed = ConArgs::default();
    let mut rest = args.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--no-activate" => parsed.no_activate = true,
            "--headless" => parsed.headless = true,
            "--working-dir" => {
                parsed.working_dir = Some(
                    rest.next()
                        .cloned()
                        .ok_or_else(|| "error: --working-dir requires a path\n".to_owned())?,
                );
            }
            other if other.starts_with("--working-dir=") => {
                parsed.working_dir = Some(other["--working-dir=".len()..].to_owned());
            }
            "--font-size" => parsed.font_size = next_decimal(&mut rest, "--font-size")?,
            other if other.starts_with("--font-size=") => {
                parsed.font_size = Some(parse_decimal(
                    &other["--font-size=".len()..],
                    "--font-size",
                )?);
            }
            // Comma-separated and repeatable, so `--feature a,b` and
            // `--feature a --feature b` mean the same thing. Later words win,
            // which is what makes a wrapper script able to append an override.
            "--feature" => {
                let list = rest
                    .next()
                    .cloned()
                    .ok_or_else(|| "error: --feature requires a name\n".to_owned())?;
                for word in list.split(',').map(str::trim).filter(|w| !w.is_empty()) {
                    parsed.features.apply(word)?;
                }
            }
            other if other.starts_with("--feature=") => {
                for word in other["--feature=".len()..]
                    .split(',')
                    .map(str::trim)
                    .filter(|w| !w.is_empty())
                {
                    parsed.features.apply(word)?;
                }
            }
            "--cols" => parsed.cols = next_value(&mut rest, "--cols")?,
            "--rows" => parsed.rows = next_value(&mut rest, "--rows")?,
            "--control" => {
                parsed.control_endpoint = Some(rest.next().cloned().ok_or_else(|| {
                    "error: --control requires pipe:<name> or unix:<absolute-path>\n".to_owned()
                })?);
            }
            "--emit-snapshot" => {
                parsed.snapshot_path =
                    Some(PathBuf::from(rest.next().cloned().ok_or_else(|| {
                        "error: --emit-snapshot requires a path\n".to_owned()
                    })?));
            }
            // Everything after -e is the command line, verbatim. Consuming the
            // remainder is what lets `-e ssh host -p 22` pass `-p 22` through
            // rather than having this parser reject it as an unknown flag.
            "-e" | "--command" => {
                let argv: Vec<String> = rest.cloned().collect();
                if argv.is_empty() {
                    return Err("error: -e requires a program to run\n".to_owned());
                }
                parsed.command = Some(argv);
                break;
            }
            unknown => {
                return Err(format!(
                    "error: unknown argument '{unknown}'

{}",
                    usage_text()
                ));
            }
        }
    }
    if parsed.features.rust_console && !cfg!(all(windows, feature = "experimental-rust-console")) {
        return Err(
            "error: rust-console requires Windows and a build with experimental-rust-console\n"
                .to_owned(),
        );
    }
    Ok(parsed)
}

/// Reads the next argument as `T`, reporting the flag name on failure rather
/// than silently ignoring a typo — the old parser dropped bad values on the
/// floor, so `--cols twenty` quietly did nothing.
fn next_value<'a, T: std::str::FromStr>(
    rest: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<Option<T>, String> {
    let raw = rest
        .next()
        .ok_or_else(|| format!("error: {flag} requires a value\n"))?;
    parse_value(raw, flag).map(Some)
}

fn parse_value<T: std::str::FromStr>(raw: &str, flag: &str) -> Result<T, String> {
    raw.parse()
        .map_err(|_| format!("error: {flag} expects a number, got '{raw}'\n"))
}

fn next_decimal<'a>(
    rest: &mut impl Iterator<Item = &'a String>,
    flag: &str,
) -> Result<Option<f64>, String> {
    let raw = rest
        .next()
        .ok_or_else(|| format!("error: {flag} requires a value\n"))?;
    parse_decimal(raw, flag).map(Some)
}

fn parse_decimal(raw: &str, flag: &str) -> Result<f64, String> {
    json::parse_finite_decimal(raw)
        .ok_or_else(|| format!("error: {flag} expects a finite number, got '{raw}'\n"))
}

#[cfg(windows)]
const USAGE_CONTROL_EXAMPLES: &str = "\
  minicon --control pipe:\\\\.\\pipe\\minicon-test
  minicon cli --control pipe:\\\\.\\pipe\\minicon-test list-tabs";

#[cfg(unix)]
const USAGE_CONTROL_EXAMPLES: &str = "\
  minicon --control unix:$TMPDIR/minicon-test/control.sock
  minicon cli --control unix:$TMPDIR/minicon-test/control.sock list-tabs
  (Unix socket parents may be under /tmp; the host resolves symlink roots such
   as macOS /tmp → /private/tmp so control works like a Windows named pipe.)";

#[cfg(windows)]
const USAGE_SHELL_EXAMPLES: &str = "\
                   minicon -e pwsh -NoLogo
                   minicon --working-dir C:\\src -e cargo test";

#[cfg(unix)]
const USAGE_SHELL_EXAMPLES: &str = "\
                   minicon -e /bin/zsh -l
                   minicon --working-dir ~/src -e cargo test";

#[cfg(windows)]
const USAGE_CONFIG_LOCATION: &str = "Configuration: create minicon.json under the user config directory\n\
     (Windows: %APPDATA%\\minicon.json via runtime::user_config_directory).";

#[cfg(unix)]
const USAGE_CONFIG_LOCATION: &str = "Configuration: create minicon.json under the user config directory\n\
     (Unix: ~/.config/minicon.json via runtime::user_config_directory).";

/// The facts a bug report needs, gathered from the machine it runs on.
///
/// Every line here answers a question that has actually cost a round trip:
/// which binary is this, which Windows is it on, which PTY backend did it
/// choose, which font face did the system really give it, and where does it
/// write when something fails. None of these can be answered by reading the
/// source, because all of them depend on the machine.
///
/// Opens no window and starts no session — the point is to be runnable on a
/// machine where starting a session is the thing that does not work.
/// The status a bare `--status` prints, including the Windows default that
/// the offline CLI applies before reporting. Tests call this rather than
/// `status_text`, which reports whatever the environment already says.
#[cfg(all(test, windows))]
pub(crate) fn offline_status_text_for_test() -> String {
    let selection = crate::resolved_backend(Features::default(), &crate::ConConfig::default());
    #[cfg(windows)]
    crate::apply_backend_environment(selection.backend);
    status_text_for(selection.backend)
}

/// The host-interface contract version this binary speaks. Independent of
/// `CARGO_PKG_VERSION` -- bumped minor for an additive capability, major for
/// a breaking change. See `plan/archive/plan-hostif-v1.md`.
const HOSTIF_VERSION: &str = "1.0";

/// This build's asset-name suffix, exactly the substring
/// `candidate-manifest.json`'s `assets[].name` uses
/// (`minicon-<version>-<suffix>.<ext>`). Computed once here so a launcher
/// never has to map `os`/`arch` to it itself -- macOS ships one universal
/// asset regardless of the running arch, which breaks any naive 1:1 mapping.
fn hostif_asset_suffix() -> String {
    match std::env::consts::OS {
        "macos" => "macos-universal".to_owned(),
        "windows" => format!(
            "windows-{}",
            match std::env::consts::ARCH {
                "aarch64" => "arm64",
                other => other,
            }
        ),
        "linux" => format!("linux-{}", std::env::consts::ARCH),
        other => format!("{other}-{}", std::env::consts::ARCH),
    }
}

/// `minicon --version --json` body. Raw `std::env::consts::OS`/`ARCH`, no
/// renaming -- a launcher-side alias table is one more place to drift.
fn hostif_version_json() -> String {
    let value = json::object(vec![
        ("version", env!("CARGO_PKG_VERSION").into()),
        ("hostif", HOSTIF_VERSION.into()),
        ("os", std::env::consts::OS.into()),
        ("arch", std::env::consts::ARCH.into()),
        ("asset", hostif_asset_suffix().into()),
    ]);
    String::from_utf8(json::to_vec(&value)).unwrap_or_default()
}

/// `minicon --hostif-handshake` body. Includes `version` so a launcher needs
/// exactly one call; `capabilities` is a flat list a launcher only checks
/// membership against, never parses as a version string.
fn hostif_handshake_json() -> String {
    let value = json::object(vec![
        ("hostif", HOSTIF_VERSION.into()),
        ("version", env!("CARGO_PKG_VERSION").into()),
        (
            "capabilities",
            json::JsonValue::Array(
                ["exec", "mux", "pty"]
                    .into_iter()
                    .map(json::JsonValue::from)
                    .collect(),
            ),
        ),
    ]);
    String::from_utf8(json::to_vec(&value)).unwrap_or_default()
}

/// Accept only feature flags beside `--status`; parsing owns their values.
///
/// `--status` answers for the run the same flags would produce, so it reads
/// them; every other offline verb still stands alone.
fn status_feature_request(args: &[String]) -> Option<()> {
    let mut rest = args.iter();
    if rest.next().map(String::as_str) != Some("--status") {
        return None;
    }
    while let Some(argument) = rest.next() {
        if argument == "--feature" {
            let _ = rest.next()?;
        } else if !argument.starts_with("--feature=") {
            return None;
        }
    }
    Some(())
}

/// The status of a run with no `--feature` and no saved preference: the
/// product default, which is what this pure form reports.
#[cfg(test)]
pub(crate) fn status_text() -> String {
    let selection = crate::resolved_backend(Features::default(), &crate::ConConfig::default());
    status_text_for(selection.backend)
}

fn status_text_for(selected: Backend) -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(512);
    let _ = writeln!(text, "minicon {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(text);

    let backend = minicon_platform::pty::backend_report();
    let rust = selected == Backend::RustConsole;
    let _ = writeln!(
        text,
        "  pty backend    {}",
        if rust { "rust-console" } else { backend.kind }
    );
    if rust {
        let _ = writeln!(
            text,
            "                 owned Rust console (minicon-console); MiniCon's default on Windows"
        );
        let _ = writeln!(
            text,
            "                 `--feature console-agent` or `--feature conpty` selects another backend"
        );
    } else if !backend.detail.is_empty() {
        let _ = writeln!(text, "                 {}", backend.detail);
    }
    // The platform reports the mechanism ("forced by ...=1"); a user needs the
    // product's reason and the way back to the default.
    #[cfg(windows)]
    if !rust {
        let way = if crate::RUST_CONSOLE_AVAILABLE {
            "`--feature rust-console` selects MiniCon's default"
        } else {
            "this build carries no rust-console; `--feature conpty` selects ConPTY"
        };
        let _ = writeln!(text, "                 {way}");
    }

    match minicon_platform::font::primary_face_report(DEFAULT_FONT_PX as u16) {
        Ok(report) => {
            let _ = writeln!(
                text,
                "  font           {} ({}x{} cells)",
                report.face, report.cell_width, report.cell_height
            );
            // The measurement, not just the name: a face can be resolved and
            // still be the wrong shape for a character grid, and that is
            // exactly the failure a user describes as "the font is off".
            let width = match report.full_width_is_double() {
                Some(true) => "half/full width correct".to_owned(),
                Some(false) => format!(
                    "FULL WIDTH IS NOT DOUBLE (ascii {:?}, full {:?})",
                    report.ascii_advance, report.full_width_advance
                ),
                None => "width unmeasured on this platform".to_owned(),
            };
            let _ = writeln!(text, "                 {width}");
        }
        Err(error) => {
            let _ = writeln!(text, "  font           unavailable: {error}");
        }
    }

    let _ = writeln!(
        text,
        "  diagnostics    {}",
        minicon_platform::diagnostics::log_path().map_or_else(
            || "unavailable".to_owned(),
            |path| path.display().to_string()
        )
    );
    text
}

pub(crate) fn usage_text() -> String {
    let feature_list = FEATURES
        .iter()
        .map(|(name, help)| format!("  --feature {name}  {help}"))
        .collect::<Vec<String>>()
        .join("\n");
    format!(
        "\
Usage: minicon [--no-activate] [--headless] [--working-dir DIR]
                   [--font-size N] [--cols N] [--rows N]
                   [--control ENDPOINT] [--emit-snapshot PATH]
                   [--feature NAME[,NAME...]] [-e PROGRAM [ARGS...]]
       minicon --version
       minicon --version --json
       minicon --hostif-handshake
       minicon --status
       minicon --help
       minicon cli --control ENDPOINT COMMAND [ARGS...]
       minicon install-cli [--prefix DIR]     (link `minicon` onto PATH)
       minicon uninstall-cli [--prefix DIR]

A standalone console host (conhost equivalent). No server, mux, or Fleet.

Features select which implementation hosts the shell, as opposed to the user
preferences in minicon.json. Repeatable and comma-separated; prefix a name with
`no-` to state the off position explicitly:
{feature_list}

Control endpoint and CLI (TAB is a stable @ID; omitted target means active tab):
  minicon cli list-commands
{control_examples}
  ... ui-snapshot | perf-stats | reset-perf-stats | cancel-pointer | close-window
  ... detach-gui | attach-gui   (release the window; the process keeps running)

--headless starts with no window at all: sessions and --control run, and a
window appears only when something sends attach-gui.
  ... resize-window --width N --height N
  ... new-tab [--parent TAB]
  ... select-tab --target TAB | close-tab --target TAB
  ... capture-pane [--target TAB] [--max-bytes N] [--output PATH]
  ... screenshot-pane [--target TAB] --output PATH
  ... send-text [--target TAB] TEXT|--file PATH
  ... send-paste [--target TAB] TEXT|--file PATH
  ... send-keys [--target TAB] KEY...
  ... send-ui-ime enabled|disabled|preedit TEXT [--cursor N]|commit TEXT
  ... send-ui-keys KEY...
  ... send-ui-mouse --action press|release|move|click
                   --button none|left|middle|right --column N --row N
  ... send-mouse [--target TAB] --action press|release|move|click
                 --button none|left|middle|right --column N --row N
  ... send-wheel [--target TAB] --column N --row N --notches N [--ctrl]
  ... wait-text [--target TAB] [--timeout-ms N] TEXT
  ... wait-tab-exit --target TAB [--timeout-ms N]

Keys use names such as Enter, Escape, Tab, Up, F1 or modifiers such as Ctrl+C.
send-ui-keys follows current UI focus; send-keys always targets a terminal.
Mouse coordinates are zero-based terminal cells. Positive wheel notches scroll up.
send-ui-mouse coordinates are window logical pixels and follow the real event
path: hover, click targets and focus all resolve exactly as with a real mouse.

  Ctrl+Shift+T       New root terminal
  Ctrl+Shift+O       Open or return to the browser tab
  Ctrl+Shift+N       New child terminal below the active tab
  Ctrl+Shift+W       Close active tab (children are promoted)
  Ctrl+Shift+B       Collapse the tab sidebar to a rail, or expand it again
  Ctrl+Shift+[ / ]   Switch tabs
  Ctrl+Shift+I       Focus terminal input or the browser address field
  Ctrl+L / Cmd+L     Edit the browser address; Enter navigates, Escape returns to page
  Cmd+[ / ] / R     On macOS: browser Back / Forward / Reload
  Ctrl+Shift+P       Cycle the color theme (Neutral / Docs / Paper / Dark)
  Ctrl+Shift+,       Open or close the settings panel
  Ctrl+Shift+G       Toggle the grid crosshair
  Ctrl+Shift+H       Open or close the sent-history panel
{composer_keys}
  Click a tab to select it. Closing the final tab leaves the greeting page.
  Choose New -> Browser to preview a URL; Retry or Edit address recovers a failed page.

  -e, --command  Run PROGRAM instead of the default shell. Everything after
                 -e is passed through verbatim, so it must come last:
{shell_examples}

  --emit-snapshot PATH
                 Write a JSON snapshot of screen text/cursor/selection to
                 PATH after each render (atomic write). For CLI clients, tests,
                 and other agents that need to inspect a session without
                 capturing pixels.

{config_location}
Keys: font_size, cols, rows (all optional).
CLI flags override config; config overrides defaults.
The header's ? opens its shortcut guide; adjacent size buttons shrink, reset,
and grow the whole interface.",
        composer_keys = composer_key_help(),
        control_examples = USAGE_CONTROL_EXAMPLES,
        shell_examples = USAGE_SHELL_EXAMPLES,
        config_location = USAGE_CONFIG_LOCATION,
    )
}

fn write_offline_stdout(text: &str) {
    let _ = minicon_platform::parent_console::write_stdout(text);
}

fn write_offline_stderr(text: &str) {
    let _ = minicon_platform::parent_console::write_stderr(text);
}

/// Flags that must not open a window. Returns `Some(exit_code)` when handled.
pub(crate) fn offline_cli_exit(args: &[String]) -> Option<i32> {
    if args.first().is_some_and(|arg| arg == "cli") {
        return Some(match control::run_cli(args) {
            Ok(output) => {
                if !output.is_empty() {
                    write_offline_stdout(&output);
                }
                0
            }
            Err(error) => {
                write_offline_stderr(&format!("minicon cli: {error}\n"));
                2
            }
        });
    }
    if args.first().is_some_and(|arg| arg == "harness") {
        return Some(match crate::harness::run_harness(args) {
            Ok(output) => {
                if !output.is_empty() {
                    write_offline_stdout(&output);
                }
                0
            }
            Err(error) => {
                write_offline_stderr(&format!("minicon harness: {error}\n"));
                2
            }
        });
    }
    if args.first().is_some_and(|arg| arg == "mux") {
        return Some(match crate::mux::run_mux(args) {
            Ok(output) => {
                if !output.is_empty() {
                    write_offline_stdout(&output);
                }
                0
            }
            Err(error) => {
                write_offline_stderr(&format!("minicon mux: {error}\n"));
                2
            }
        });
    }
    let alone = args.len() == 1;
    match args.first().map(String::as_str) {
        Some("--version" | "-V") if alone => {
            let _ = minicon_platform::parent_console::write_stdout(&format!(
                "minicon {}",
                env!("CARGO_PKG_VERSION")
            ));
            Some(0)
        }
        Some("--version" | "-V") if args.len() == 2 && args[1] == "--json" => {
            let _ = minicon_platform::parent_console::write_stdout(&hostif_version_json());
            Some(0)
        }
        Some("--hostif-handshake") if alone => {
            let _ = minicon_platform::parent_console::write_stdout(&hostif_handshake_json());
            Some(0)
        }
        Some("--status") if status_feature_request(args).is_some() => {
            let features = match parse_args(&args[1..]) {
                Ok(parsed) => parsed.features,
                Err(error) => {
                    let _ = minicon_platform::parent_console::write_stderr(&error);
                    return Some(2);
                }
            };
            // `--status` describes the run these same flags (and the saved
            // preference) would produce, through the same decision startup
            // uses. It once reported "conpty" while every session ran on the
            // console agent (measured in the Windows ARM court, 2026-09-23),
            // because it computed the answer its own way.
            let selection = crate::resolved_backend(features, &crate::load_config());
            // The offline CLI runs before any window, PTY or reader thread
            // exists, so the environment edit is sound here.
            #[cfg(windows)]
            crate::apply_backend_environment(selection.backend);
            let mut status = status_text_for(selection.backend);
            if let Some(notice) = selection.notice {
                status.push_str(&format!("  notice         {notice}\n"));
            }
            let _ = minicon_platform::parent_console::write_stdout(&status);
            Some(0)
        }
        Some("--help" | "-h") if alone => {
            let _ = minicon_platform::parent_console::write_stdout(&usage_text());
            Some(0)
        }
        Some("install-cli") => Some(install_cli(&args[1..], true)),
        Some("uninstall-cli") => Some(install_cli(&args[1..], false)),
        Some("--version" | "-V" | "--help" | "-h" | "--status") => {
            let _ = minicon_platform::parent_console::write_stderr(
                "error: --version/--help must be used alone; --status takes only --feature",
            );
            Some(2)
        }
        _ => None,
    }
}

/// Links this executable as `minicon` on `PATH`, or removes that link.
///
/// A macOS `.app` bundle keeps its executable at `Contents/MacOS/minicon`,
/// which is deliberately not on `PATH` — but MiniCon is also a control CLI
/// (`minicon cli ...`), so a GUI install still needs one step to be usable from
/// a shell. This is the VS Code `code` model: an explicit symlink into a prefix
/// the user names, never an installer silently writing into the system.
/// The symlink mechanism itself is the platform's seam: Unix makes one, Windows
/// answers "add the folder to PATH" so this function needs no platform branch.
fn install_cli(rest: &[String], install: bool) -> i32 {
    let mut prefix = PathBuf::from("/usr/local/bin");
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--prefix" => match rest.next() {
                Some(dir) => prefix = PathBuf::from(dir),
                None => {
                    let _ = minicon_platform::parent_console::write_stderr(
                        "error: --prefix requires a directory\n",
                    );
                    return 2;
                }
            },
            other if other.starts_with("--prefix=") => {
                prefix = PathBuf::from(&other["--prefix=".len()..]);
            }
            unknown => {
                let _ = minicon_platform::parent_console::write_stderr(&format!(
                    "error: unknown argument '{unknown}'\n"
                ));
                return 2;
            }
        }
    }

    let link = prefix.join("minicon");
    let existing = std::fs::symlink_metadata(&link).ok();

    if !install {
        return match existing {
            None => {
                let _ = minicon_platform::parent_console::write_stdout(&format!(
                    "nothing to remove: {} does not exist\n",
                    link.display()
                ));
                0
            }
            // Only ever remove a symlink. A real file there belongs to someone
            // else — a package manager, a hand-placed copy — and deleting it is
            // not this command's business.
            Some(meta) if meta.file_type().is_symlink() => match std::fs::remove_file(&link) {
                Ok(()) => {
                    let _ = minicon_platform::parent_console::write_stdout(&format!(
                        "removed {}\n",
                        link.display()
                    ));
                    0
                }
                Err(error) => {
                    let _ = minicon_platform::parent_console::write_stderr(&format!(
                        "error: cannot remove {}: {error}\n{}",
                        link.display(),
                        install_cli_permission_hint(&prefix)
                    ));
                    1
                }
            },
            Some(_) => {
                let _ = minicon_platform::parent_console::write_stderr(&format!(
                    "error: {} exists and is not a symlink; refusing to delete a real file\n",
                    link.display()
                ));
                1
            }
        };
    }

    let target = match std::env::current_exe().and_then(|path| path.canonicalize()) {
        Ok(path) => path,
        Err(error) => {
            let _ = minicon_platform::parent_console::write_stderr(&format!(
                "error: cannot resolve this executable: {error}\n"
            ));
            return 1;
        }
    };
    if !prefix.is_dir() {
        let _ = minicon_platform::parent_console::write_stderr(&format!(
            "error: {} is not a directory\ntry: minicon install-cli --prefix ~/.local/bin\n",
            prefix.display()
        ));
        return 1;
    }
    if let Some(meta) = existing {
        if !meta.file_type().is_symlink() {
            let _ = minicon_platform::parent_console::write_stderr(&format!(
                "error: {} already exists and is not a symlink; refusing to replace it\n",
                link.display()
            ));
            return 1;
        }
        if let Err(error) = std::fs::remove_file(&link) {
            let _ = minicon_platform::parent_console::write_stderr(&format!(
                "error: cannot replace {}: {error}\n{}",
                link.display(),
                install_cli_permission_hint(&prefix)
            ));
            return 1;
        }
    }
    match minicon_platform::filesystem_publish::install_cli_symlink(&target, &link) {
        Ok(()) => {
            let _ = minicon_platform::parent_console::write_stdout(&format!(
                "linked {} -> {}\nopen a new shell, then `minicon --version`\n",
                link.display(),
                target.display()
            ));
            0
        }
        Err(error) => {
            let _ = minicon_platform::parent_console::write_stderr(&format!(
                "error: cannot create {}: {error}\n{}",
                link.display(),
                install_cli_permission_hint(&prefix)
            ));
            1
        }
    }
}

fn install_cli_permission_hint(prefix: &std::path::Path) -> String {
    format!(
        "{} may not be writable by this user. Re-run with sudo, or choose a \
         directory you own:\n  minicon install-cli --prefix ~/.local/bin\n",
        prefix.display()
    )
}

/// The composer's key list, rendered from the table that also decides what
/// those keys do.
///
/// It used to be typed out here as prose. Prose cannot be wrong in a way the
/// compiler notices, so the help promised `Up / Down  Recall what you sent
/// before` for three releases after Shift+Up had already started selecting by
/// line. Generating the lines means a key's behaviour and its description are
/// the same edit.
fn composer_key_help() -> String {
    minicon_core::keymap::help_lines()
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod hostif_tests {
    use super::*;

    #[test]
    fn gui_owned_console_feature_selection_is_build_gated_and_exclusive() {
        let args = |words: &[&str]| words.iter().map(|s| (*s).to_owned()).collect::<Vec<_>>();
        assert!(!parse_args(&[]).unwrap().features.rust_console);
        for words in [
            vec!["--feature=rust-console,no-rust-console"],
            vec!["--feature=rust-console,conpty", "-e", "unused-program"],
        ] {
            assert!(!parse_args(&args(&words)).unwrap().features.rust_console);
        }
        assert!(
            !parse_args(&args(&["--feature=no-rust-console"]))
                .unwrap()
                .features
                .rust_console
        );
        let enabled = parse_args(&args(&["--feature=conpty,rust-console"]));
        if cfg!(all(windows, feature = "experimental-rust-console")) {
            let features = enabled.unwrap().features;
            assert!(features.rust_console && !features.conpty);
            assert_eq!(features.explicit_backend(), Some(Backend::RustConsole));
            assert!(status_text_for(Backend::RustConsole).contains("pty backend    rust-console"));
            let agent = parse_args(&args(&["--feature=rust-console,console-agent"]))
                .unwrap()
                .features;
            assert_eq!(
                agent.explicit_backend(),
                Some(Backend::ConsoleAgent),
                "the rollback word is selectable and later words win"
            );
            for words in [
                vec!["--feature=rust-console,conpty"],
                vec!["--feature", "rust-console", "--feature", "conpty"],
            ] {
                let features = parse_args(&args(&words)).unwrap().features;
                assert!(!features.rust_console && features.conpty);
            }
            let features = parse_args(&args(&["--feature=rust-console,no-conpty"]))
                .unwrap()
                .features;
            assert!(features.rust_console && !features.conpty);
            let features = parse_args(&args(&["--feature=rust-console,no-rust-console"]))
                .unwrap()
                .features;
            assert!(!features.rust_console && !features.conpty);
        } else {
            assert!(
                parse_args(&args(&["--feature=rust-console", "-e", "unused-program"])).is_err()
            );
            assert!(
                enabled
                    .unwrap_err()
                    .contains("requires Windows and a build with experimental-rust-console")
            );
        }
        assert!(usage_text().contains("--feature rust-console"));
        let mut session = crate::ConTerminal::new(None, None);
        session.rust_console = true;
        let seeded = crate::SessionSeed::from_session(&session).create_session();
        assert!(seeded.rust_console);
        assert_eq!(seeded.pty_backend(), None);
    }

    /// Both bodies must be object literals a launcher can actually parse,
    /// not just human-readable text that happens to look JSON-ish.
    fn assert_is_json_object(body: &str) {
        let trimmed = body.trim();
        assert!(
            trimmed.starts_with('{') && trimmed.ends_with('}'),
            "not a JSON object: {body:?}"
        );
        assert!(
            serde_json::from_str::<serde_json::Value>(trimmed).is_ok(),
            "serde_json rejected: {body:?}"
        );
    }

    #[test]
    fn version_json_is_parseable_and_carries_both_versions() {
        let body = hostif_version_json();
        assert_is_json_object(&body);
        assert!(body.contains(&format!("\"version\":\"{}\"", env!("CARGO_PKG_VERSION"))));
        assert!(body.contains("\"hostif\":\"1.0\""));
        assert!(body.contains(&format!("\"os\":\"{}\"", std::env::consts::OS)));
        assert!(body.contains(&format!("\"arch\":\"{}\"", std::env::consts::ARCH)));
    }

    #[test]
    fn handshake_json_is_parseable_and_carries_capabilities() {
        let body = hostif_handshake_json();
        assert_is_json_object(&body);
        assert!(body.contains("\"hostif\":\"1.0\""));
        assert!(body.contains(&format!("\"version\":\"{}\"", env!("CARGO_PKG_VERSION"))));
        assert!(body.contains("\"exec\""));
        assert!(body.contains("\"mux\""));
        assert!(body.contains("\"pty\""));
    }

    #[test]
    fn asset_suffix_matches_candidate_manifest_naming() {
        let suffix = hostif_asset_suffix();
        match std::env::consts::OS {
            "macos" => assert_eq!(suffix, "macos-universal"),
            "windows" => assert!(suffix == "windows-x86_64" || suffix == "windows-arm64"),
            "linux" => assert!(suffix.starts_with("linux-")),
            _ => {}
        }
    }

    /// `offline_cli_exit` must never fall into `parse_args`/window startup
    /// for these two verbs -- both branches must be reached and exit 0 with
    /// no ambiguity about whether an old binary would have opened a window.
    #[test]
    fn offline_cli_exit_handles_hostif_verbs_without_opening_a_window() {
        assert_eq!(
            offline_cli_exit(&["--version".to_owned(), "--json".to_owned()]),
            Some(0)
        );
        assert_eq!(
            offline_cli_exit(&["--hostif-handshake".to_owned()]),
            Some(0)
        );
        // A malformed combination still exits 2, not None (which would fall
        // through into parse_args and, on an older build, GUI startup).
        assert_eq!(
            offline_cli_exit(&[
                "--version".to_owned(),
                "--json".to_owned(),
                "extra".to_owned()
            ]),
            Some(2)
        );
    }
}
