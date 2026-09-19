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
    /// Host the shell through ConPTY instead of the classic Windows console.
    ///
    /// Default off because mouse input does not currently survive our ConPTY
    /// path in either direction: measured on the court, the child's
    /// `ESC[?1000h` never reaches the host and the host's SGR reports are
    /// dropped coming back, so a hosted TUI never sees a click. The classic
    /// console owns a real console and delivers them — confirmed on a real
    /// machine, where the same program answered the mouse only on this path.
    ///
    /// The cause is NOT that ConPTY lacks mouse support: upstream added it in
    /// 2021 (microsoft/terminal#376, PR #9970 relays the child's mouse-mode
    /// request to the host). So the gap is on our side and is expected to be
    /// fixable — note that `ENABLE_MOUSE_INPUT` appears nowhere in our ConPTY
    /// setup. This flag keeps ConPTY reachable meanwhile, and is how you get
    /// back to the Microsoft-blessed path once it carries the mouse.
    pub(crate) conpty: bool,
}

/// Every name `--feature` accepts, with the one-line help shown in usage.
/// A name absent here is a typo, not an unsupported feature, and is rejected.
pub(crate) const FEATURES: &[(&str, &str)] = &[(
    "conpty",
    "Windows: host the shell through ConPTY instead of the classic\n\
     \x20                    console. Pick by what the program uses: the default\n\
     \x20                    carries the mouse to agent CLIs, this carries it to\n\
     \x20                    Vim-style programs. No terminal can do both — see\n\
     \x20                    microsoft/terminal#15083. Inert off Windows.",
)];

impl Features {
    /// Applies one `--feature` word. `no-<name>` turns a feature off, so a
    /// script can state its intent instead of relying on today's defaults.
    fn apply(&mut self, word: &str) -> Result<(), String> {
        let (name, enable) = match word.strip_prefix("no-") {
            Some(rest) => (rest, false),
            None => (word, true),
        };
        match name {
            "conpty" => self.conpty = enable,
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
                return Ok(parsed);
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

pub(crate) fn status_text() -> String {
    use std::fmt::Write as _;

    let mut text = String::with_capacity(512);
    let _ = writeln!(text, "minicon {}", env!("CARGO_PKG_VERSION"));
    let _ = writeln!(text);

    let backend = agenterm_platform::pty::backend_report();
    let _ = writeln!(text, "  pty backend    {}", backend.kind);
    if !backend.detail.is_empty() {
        let _ = writeln!(text, "                 {}", backend.detail);
    }

    match agenterm_platform::font::primary_face_report(DEFAULT_FONT_PX as u16) {
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
        agenterm_platform::diagnostics::log_path().map_or_else(
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
  ... send-mouse [--target TAB] --action press|release|move|click
                 --button none|left|middle|right --column N --row N
  ... send-wheel [--target TAB] --column N --row N --notches N [--ctrl]
  ... wait-text [--target TAB] [--timeout-ms N] TEXT
  ... wait-tab-exit --target TAB [--timeout-ms N]

Keys use names such as Enter, Escape, Tab, Up, F1 or modifiers such as Ctrl+C.
send-ui-keys follows current UI focus; send-keys always targets a terminal.
Mouse coordinates are zero-based terminal cells. Positive wheel notches scroll up.

  Ctrl+Shift+T       New root terminal
  Ctrl+Shift+N       New child terminal below the active tab
  Ctrl+Shift+W       Close active terminal (children are promoted)
  Ctrl+Shift+B       Collapse the tab sidebar to a rail, or expand it again
  Ctrl+Shift+[ / ]   Switch terminal tabs
  Ctrl+Shift+I       Focus the external input area
  Ctrl+Shift+P       Cycle the color theme (Neutral / Docs / Paper)
  Ctrl+Shift+,       Open or close the settings panel
  Ctrl+Shift+G       Toggle the grid crosshair
  Enter              Insert a soft newline in the input area
  Ctrl+O             Send the complete input-area draft
  Up / Down          Recall what you sent before, in the input area
  Click a tab to select it. Closing the final tab leaves the greeting page.

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
        control_examples = USAGE_CONTROL_EXAMPLES,
        shell_examples = USAGE_SHELL_EXAMPLES,
        config_location = USAGE_CONFIG_LOCATION,
    )
}

/// Flags that must not open a window. Returns `Some(exit_code)` when handled.
fn write_offline_stdout(text: &str) {
    let _ = agenterm_platform::parent_console::write_stdout(text);
}

fn write_offline_stderr(text: &str) {
    let _ = agenterm_platform::parent_console::write_stderr(text);
}

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
    let alone = args.len() == 1;
    match args.first().map(String::as_str) {
        Some("--version" | "-V") if alone => {
            let _ = agenterm_platform::parent_console::write_stdout(&format!(
                "minicon {}",
                env!("CARGO_PKG_VERSION")
            ));
            Some(0)
        }
        Some("--status") if alone => {
            let _ = agenterm_platform::parent_console::write_stdout(&status_text());
            Some(0)
        }
        Some("--help" | "-h") if alone => {
            let _ = agenterm_platform::parent_console::write_stdout(&usage_text());
            Some(0)
        }
        Some("install-cli") => Some(install_cli(&args[1..], true)),
        Some("uninstall-cli") => Some(install_cli(&args[1..], false)),
        Some("--version" | "-V" | "--help" | "-h" | "--status") => {
            let _ = agenterm_platform::parent_console::write_stderr(
                "error: --version/--status/--help must be used alone",
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
#[cfg(unix)]
fn install_cli(rest: &[String], install: bool) -> i32 {
    use std::os::unix::fs::symlink;

    let mut prefix = PathBuf::from("/usr/local/bin");
    let mut rest = rest.iter();
    while let Some(arg) = rest.next() {
        match arg.as_str() {
            "--prefix" => match rest.next() {
                Some(dir) => prefix = PathBuf::from(dir),
                None => {
                    let _ = agenterm_platform::parent_console::write_stderr(
                        "error: --prefix requires a directory\n",
                    );
                    return 2;
                }
            },
            other if other.starts_with("--prefix=") => {
                prefix = PathBuf::from(&other["--prefix=".len()..]);
            }
            unknown => {
                let _ = agenterm_platform::parent_console::write_stderr(&format!(
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
                let _ = agenterm_platform::parent_console::write_stdout(&format!(
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
                    let _ = agenterm_platform::parent_console::write_stdout(&format!(
                        "removed {}\n",
                        link.display()
                    ));
                    0
                }
                Err(error) => {
                    let _ = agenterm_platform::parent_console::write_stderr(&format!(
                        "error: cannot remove {}: {error}\n{}",
                        link.display(),
                        install_cli_permission_hint(&prefix)
                    ));
                    1
                }
            },
            Some(_) => {
                let _ = agenterm_platform::parent_console::write_stderr(&format!(
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
            let _ = agenterm_platform::parent_console::write_stderr(&format!(
                "error: cannot resolve this executable: {error}\n"
            ));
            return 1;
        }
    };
    if !prefix.is_dir() {
        let _ = agenterm_platform::parent_console::write_stderr(&format!(
            "error: {} is not a directory\ntry: minicon install-cli --prefix ~/.local/bin\n",
            prefix.display()
        ));
        return 1;
    }
    if let Some(meta) = existing {
        if !meta.file_type().is_symlink() {
            let _ = agenterm_platform::parent_console::write_stderr(&format!(
                "error: {} already exists and is not a symlink; refusing to replace it\n",
                link.display()
            ));
            return 1;
        }
        if let Err(error) = std::fs::remove_file(&link) {
            let _ = agenterm_platform::parent_console::write_stderr(&format!(
                "error: cannot replace {}: {error}\n{}",
                link.display(),
                install_cli_permission_hint(&prefix)
            ));
            return 1;
        }
    }
    match symlink(&target, &link) {
        Ok(()) => {
            let _ = agenterm_platform::parent_console::write_stdout(&format!(
                "linked {} -> {}\nopen a new shell, then `minicon --version`\n",
                link.display(),
                target.display()
            ));
            0
        }
        Err(error) => {
            let _ = agenterm_platform::parent_console::write_stderr(&format!(
                "error: cannot create {}: {error}\n{}",
                link.display(),
                install_cli_permission_hint(&prefix)
            ));
            1
        }
    }
}

#[cfg(unix)]
fn install_cli_permission_hint(prefix: &std::path::Path) -> String {
    format!(
        "{} may not be writable by this user. Re-run with sudo, or choose a \
         directory you own:\n  minicon install-cli --prefix ~/.local/bin\n",
        prefix.display()
    )
}

#[cfg(not(unix))]
fn install_cli(_rest: &[String], _install: bool) -> i32 {
    let _ = agenterm_platform::parent_console::write_stderr(
        "error: install-cli is a Unix convenience; on Windows add the folder \
         holding minicon.exe to PATH instead\n",
    );
    2
}
