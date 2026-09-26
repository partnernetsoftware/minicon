//! `mux`: a tmux-CLI-compatible verb surface over the existing `--control`
//! endpoint (`control.rs`). Each verb is a thin translation into the
//! existing `CliCommand` wire protocol -- `mux` opens no new transport and
//! adds no new server-side capability.
//!
//! Design of record: `prd/PRD_02_31_v0_2_horizon.md` ("mux -- detail").

use agenterm_platform::input::NamedKey;

/// Substitution variables `list-windows -F` accepts. Unlike tmux, an unknown
/// substitution is a bounded error rather than a blank field, so a script
/// never silently trusts empty output.
const KNOWN_FORMAT_VARS: &[&str] = &["window_id", "window_index", "window_name", "window_active"];

const DEFAULT_FORMAT: &str = "#{window_index}: #{window_name}#{window_active}";

/// Substitution variables `list-panes -F` accepts. MiniCon has exactly one
/// pane per tab (`split-window` is not implemented), so `list-panes` reports
/// the same tabs `list-windows` does, narrowed to tmux's pane-shaped fields.
const KNOWN_PANE_FORMAT_VARS: &[&str] = &[
    "pane_id",
    "pane_width",
    "pane_height",
    "pane_active",
    "pane_dead",
];

const DEFAULT_PANE_FORMAT: &str =
    "#{pane_id} #{pane_width} #{pane_height} #{pane_active} #{pane_dead}";

/// The session names a `-t` target may name. A MiniCon instance IS its one
/// session, so this is a comparison against the one real session (this
/// process), not a lookup among several. `0` is accepted because that is what
/// tmux calls its own first session, and `minicon` because a script that wants
/// to be explicit should have something to write.
const SESSION_NAMES: &[&str] = &["minicon", "0"];

pub fn run_mux(args: &[String]) -> Result<String, String> {
    let mut cursor = MuxCursor::new(args);
    cursor.require("mux")?;
    // `--control` may be omitted when this process was itself spawned inside
    // a MiniCon instance started with `--control`: that instance exports its
    // own endpoint as `MINICON_CONTROL`, the same role `$TMUX` plays for
    // tmux, so a script does not need to be told the endpoint explicitly.
    let control = match cursor.optional_value("--control")? {
        Some(explicit) if !explicit.starts_with("--") => explicit.to_owned(),
        Some(explicit) => return Err(format!("--control requires a value, got {explicit:?}")),
        None => std::env::var("MINICON_CONTROL").map_err(|_| {
            format!(
                "mux: --control requires a value (or set MINICON_CONTROL)\n{}",
                mux_usage()
            )
        })?,
    };
    let verb = cursor.next().ok_or_else(mux_usage)?.to_owned();

    match verb.as_str() {
        "list-windows" => run_list_windows(&control, &mut cursor),
        "list-panes" => run_list_panes(&control, &mut cursor),
        "select-window" => run_select_window(&control, &mut cursor),
        "new-window" => run_new_window(&control, &mut cursor),
        "kill-window" => run_kill_window(&control, &mut cursor),
        "send-keys" => run_send_keys(&control, &mut cursor),
        "capture-pane" => run_capture_pane(&control, &mut cursor),
        other => Err(format!("mux: unknown verb {other:?}\n{}", mux_usage())),
    }
}

fn run_list_windows(control: &str, cursor: &mut MuxCursor<'_>) -> Result<String, String> {
    let format = cursor
        .optional_value("-F")?
        .map(str::to_owned)
        .unwrap_or_else(|| DEFAULT_FORMAT.to_owned());
    cursor.finish()?;
    validate_format(&format, KNOWN_FORMAT_VARS)?;

    let tabs = fetch_tabs(control)?;
    let mut lines = Vec::with_capacity(tabs.len());
    for (index, tab) in tabs.iter().enumerate() {
        lines.push(render_window_format(&format, index, tab));
    }
    Ok(lines.join("\n"))
}

fn run_list_panes(control: &str, cursor: &mut MuxCursor<'_>) -> Result<String, String> {
    let format = cursor
        .optional_value("-F")?
        .map(str::to_owned)
        .unwrap_or_else(|| DEFAULT_PANE_FORMAT.to_owned());
    cursor.finish()?;
    validate_format(&format, KNOWN_PANE_FORMAT_VARS)?;

    let tabs = fetch_tabs(control)?;
    let mut lines = Vec::with_capacity(tabs.len());
    for tab in &tabs {
        lines.push(render_pane_format(&format, tab));
    }
    Ok(lines.join("\n"))
}

fn run_select_window(control: &str, cursor: &mut MuxCursor<'_>) -> Result<String, String> {
    let target = required_target("select-window", cursor)?;
    let id = resolve_target(control, &target)?;
    control_cli(control, &["select-tab", "--target", &id])
}

fn run_new_window(control: &str, cursor: &mut MuxCursor<'_>) -> Result<String, String> {
    // A `-t` target names the new tab's parent. MiniCon's tabs are a tree and
    // `new-tab --parent` is the only placement it has, so this is the honest
    // reading of tmux's positional `-t`; tmux's own "insert at this index"
    // meaning has no MiniCon equivalent and is not approximated.
    let mut target: Option<String> = None;
    while let Some(flag) = cursor.next() {
        match flag {
            "-t" => target = Some(cursor.value_for("-t")?.to_owned()),
            "-n" => {
                return Err(format!(
                    "new-window -n {:?}: MiniCon's `new-tab` takes no window name, and dropping \
                     the name silently would leave a script believing it had set one",
                    cursor.value_for("-n").unwrap_or("")
                ));
            }
            other => return Err(unsupported_flag("new-window", other)),
        }
    }
    match target {
        Some(target) => {
            let id = resolve_target(control, &target)?;
            control_cli(control, &["new-tab", "--parent", &id])
        }
        None => control_cli(control, &["new-tab"]),
    }
}

fn run_kill_window(control: &str, cursor: &mut MuxCursor<'_>) -> Result<String, String> {
    // tmux's `kill-window` defaults to the current window, but MiniCon's
    // `close-tab` demands an explicit handle. Resolving the active tab here
    // keeps the tmux default working without teaching the protocol a second
    // meaning for "no target".
    let target = optional_target("kill-window", cursor)?;
    let id = match target {
        Some(target) => resolve_target(control, &target)?,
        None => active_tab(control)?,
    };
    control_cli(control, &["close-tab", "--target", &id])
}

fn run_send_keys(control: &str, cursor: &mut MuxCursor<'_>) -> Result<String, String> {
    let mut target: Option<String> = None;
    let mut literal = false;
    let mut keys: Vec<String> = Vec::new();
    while let Some(flag) = cursor.next() {
        match flag {
            "-t" => target = Some(cursor.value_for("-t")?.to_owned()),
            "-l" => literal = true,
            other if other.starts_with('-') && other.len() > 1 => {
                return Err(unsupported_flag("send-keys", other));
            }
            key => keys.push(key.to_owned()),
        }
    }
    if keys.is_empty() {
        return Err("send-keys requires at least one KEY (or text, with -l)".to_owned());
    }
    let id = match target {
        Some(target) => Some(resolve_target(control, &target)?),
        None => None,
    };
    let mut argv: Vec<String> = Vec::new();
    if literal {
        // `-l` is text, not keys: it goes through `send-text`, which performs
        // no key-name resolution at all.
        argv.push("send-text".to_owned());
        if let Some(id) = &id {
            argv.push("--target".to_owned());
            argv.push(id.clone());
        }
        argv.push(keys.concat());
    } else {
        argv.push("send-keys".to_owned());
        if let Some(id) = &id {
            argv.push("--target".to_owned());
            argv.push(id.clone());
        }
        // Translate every key before sending any of it, so a bad name late in
        // the sequence cannot arrive after the earlier keys already landed --
        // the same all-or-nothing contract `parse_control_keys` gives the
        // control CLI.
        for key in &keys {
            argv.push(tmux_key_to_spec(key)?);
        }
    }
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    control_cli(control, &borrowed)
}

fn run_capture_pane(control: &str, cursor: &mut MuxCursor<'_>) -> Result<String, String> {
    let mut target: Option<String> = None;
    while let Some(flag) = cursor.next() {
        match flag {
            "-t" => target = Some(cursor.value_for("-t")?.to_owned()),
            // `-p` means "print to stdout", which is the only thing this CLI
            // can do, so accepting it is honest rather than a no-op pretending
            // to switch something.
            "-p" => {}
            "-S" | "-E" => {
                return Err(format!(
                    "capture-pane {flag}: BLOCKED on carried-debt item C1 \
                     (plan/plan-carried-debt.md) -- MiniCon's scrollback semantics are not \
                     decided yet, so a history range is refused rather than approximated from \
                     the visible screen"
                ));
            }
            other => return Err(unsupported_flag("capture-pane", other)),
        }
    }
    let mut argv = vec!["capture-pane".to_owned()];
    if let Some(target) = target {
        let id = resolve_target(control, &target)?;
        argv.push("--target".to_owned());
        argv.push(id);
    }
    let borrowed: Vec<&str> = argv.iter().map(String::as_str).collect();
    control_cli(control, &borrowed)
}

/// Runs one `minicon cli` verb against the same endpoint, which is all any
/// `mux` verb ultimately is.
fn control_cli(control: &str, argv: &[&str]) -> Result<String, String> {
    let mut args = vec!["cli".to_owned(), "--control".to_owned(), control.to_owned()];
    args.extend(argv.iter().map(|value| (*value).to_owned()));
    crate::control::run_cli(&args)
}

fn unsupported_flag(verb: &str, flag: &str) -> String {
    format!(
        "{verb}: {flag:?} is not one of the tmux flags MiniCon implements; see \
         prd/PRD_02_31_v0_2_horizon.md's verb table for the supported subset"
    )
}

fn required_target(verb: &str, cursor: &mut MuxCursor<'_>) -> Result<String, String> {
    optional_target(verb, cursor)?.ok_or_else(|| format!("{verb} requires -t TARGET"))
}

fn optional_target(verb: &str, cursor: &mut MuxCursor<'_>) -> Result<Option<String>, String> {
    let mut target = None;
    while let Some(flag) = cursor.next() {
        match flag {
            "-t" => target = Some(cursor.value_for("-t")?.to_owned()),
            other => return Err(unsupported_flag(verb, other)),
        }
    }
    Ok(target)
}

fn fetch_tabs(control: &str) -> Result<Vec<TabRow>, String> {
    parse_tabs(&control_cli(control, &["list-tabs"])?)
}

fn active_tab(control: &str) -> Result<String, String> {
    fetch_tabs(control)?
        .into_iter()
        .find(|tab| tab.active)
        .map(|tab| tab.id)
        .ok_or_else(|| "no window is active".to_owned())
}

/// Resolves a tmux `[session:]window[.pane]` target into a MiniCon `@ID`.
fn resolve_target(control: &str, value: &str) -> Result<String, String> {
    let (session, window_pane) = match value.split_once(':') {
        Some((session, rest)) => (Some(session), rest),
        None => (None, value),
    };
    if let Some(session) = session
        && !SESSION_NAMES.contains(&session)
    {
        return Err(format!(
            "target {value:?}: no session named {session:?}; a MiniCon instance IS its one \
             session (write it as {}, or leave the session off), and MiniCon implements no \
             lookup among several sessions",
            SESSION_NAMES.join(" or ")
        ));
    }
    let (window, pane) = match window_pane.split_once('.') {
        Some((window, pane)) => (window, Some(pane)),
        None => (window_pane, None),
    };
    if let Some(pane) = pane
        && pane != "0"
    {
        return Err(format!(
            "target {value:?}: no pane {pane:?}; a MiniCon window has exactly one pane (index 0, \
             the tab's own terminal area) because `split-window` is not implemented"
        ));
    }
    if window.is_empty() {
        return Err(format!("target {value:?}: names no window"));
    }
    if let Some(digits) = window.strip_prefix('@') {
        // An `@ID` is MiniCon's own canonical handle. Pass it through without
        // resolving it so a stale handle is refused by the server that owns
        // the tab list, with its own message, rather than by a guess here.
        if digits.is_empty() || !digits.bytes().all(|byte| byte.is_ascii_digit()) {
            return Err(format!(
                "target {value:?}: {window:?} is not a valid @ID handle"
            ));
        }
        return Ok(window.to_owned());
    }
    let index: usize = window.parse().map_err(|_| {
        format!(
            "target {value:?}: {window:?} is neither a MiniCon @ID handle nor a tmux window index"
        )
    })?;
    let tabs = fetch_tabs(control)?;
    tabs.get(index).map(|tab| tab.id.clone()).ok_or_else(|| {
        format!(
            "target {value:?}: window index {index} is out of range; {} window(s) are open. \
             Note that tmux-style indices renumber when a window closes, which is why @ID is \
             MiniCon's canonical handle",
            tabs.len()
        )
    })
}

/// Translates one tmux key name into the `ctrl+`/`alt+`/`shift+` spec the
/// control CLI's `send-keys` already parses.
///
/// tmux writes modifiers as `C-`/`M-`/`S-` prefixes and has its own spellings
/// for a few keys (`BSpace`, `DC`, `NPage`); MiniCon's own spec predates this
/// verb and is what the terminal injector understands. This is the whole
/// translation layer, and it is deliberately not a second key table: every
/// base name still has to satisfy `NamedKey::from_name`.
fn tmux_key_to_spec(key: &str) -> Result<String, String> {
    let mut rest = key;
    let mut modifiers: Vec<&str> = Vec::new();
    loop {
        let (modifier, tail) = if let Some(tail) = rest.strip_prefix("C-") {
            ("ctrl", tail)
        } else if let Some(tail) = rest.strip_prefix("M-") {
            ("alt", tail)
        } else if let Some(tail) = rest.strip_prefix("S-") {
            ("shift", tail)
        } else {
            break;
        };
        if tail.is_empty() {
            return Err(format!(
                "send-keys: {key:?} ends with a modifier and no key"
            ));
        }
        if !modifiers.contains(&modifier) {
            modifiers.push(modifier);
        }
        rest = tail;
    }
    let base = tmux_key_name(rest, key)?;
    modifiers.push(&base);
    Ok(modifiers.join("+"))
}

/// tmux's own spellings for keys `NamedKey::from_name` does not know, plus the
/// single-character case.
fn tmux_key_name(name: &str, whole: &str) -> Result<String, String> {
    let aliased = match name.to_ascii_lowercase().as_str() {
        "bspace" => Some("backspace"),
        "dc" => Some("delete"),
        "ic" => Some("insert"),
        "npage" | "pgdn" => Some("pagedown"),
        "ppage" | "pgup" => Some("pageup"),
        _ => None,
    };
    if let Some(aliased) = aliased {
        return Ok(aliased.to_owned());
    }
    if NamedKey::from_name(name).is_some() {
        return Ok(name.to_owned());
    }
    let mut chars = name.chars();
    if let (Some(_), None) = (chars.next(), chars.next()) {
        return Ok(name.to_owned());
    }
    Err(format!(
        "send-keys: {whole:?} is neither a tmux key name nor a single character. tmux would send \
         an unknown name as literal text; MiniCon refuses it instead, so a script cannot mistype \
         a key name and silently send it as characters -- pass -l to send text literally"
    ))
}

/// One tab as reported by `list-tabs`, narrowed to the fields `mux` exposes.
/// `width`/`height` and `dead` back `list-panes`'s `pane_*` fields;
/// `id`/`name`/`active` back `list-windows`'s `window_*` fields.
struct TabRow {
    id: String,
    name: String,
    active: bool,
    dead: bool,
    width: u16,
    height: u16,
}

fn parse_tabs(response: &str) -> Result<Vec<TabRow>, String> {
    let value: serde_json::Value = serde_json::from_str(response)
        .map_err(|error| format!("list-tabs: bad response: {error}"))?;
    let tabs = value
        .get("tabs")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "list-tabs: response missing \"tabs\" array".to_owned())?;
    tabs.iter()
        .map(|tab| {
            let id = tab
                .get("id")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "list-tabs: tab missing \"id\"".to_owned())?
                .to_owned();
            let name = tab
                .get("title")
                .and_then(serde_json::Value::as_str)
                .unwrap_or("")
                .to_owned();
            let active = tab
                .get("active")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let dead = !tab
                .get("child_exit_code")
                .is_none_or(serde_json::Value::is_null);
            let width = tab
                .get("cols")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u16;
            let height = tab
                .get("rows")
                .and_then(serde_json::Value::as_u64)
                .unwrap_or(0) as u16;
            Ok(TabRow {
                id,
                name,
                active,
                dead,
                width,
                height,
            })
        })
        .collect()
}

fn validate_format(format: &str, known_vars: &[&str]) -> Result<(), String> {
    let mut rest = format;
    while let Some(start) = rest.find("#{") {
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("-F {format:?}: unterminated #{{...}} substitution"))?;
        let var = &after[..end];
        if !known_vars.contains(&var) {
            return Err(format!(
                "-F {format:?}: unknown substitution #{{{var}}}; known: {}",
                known_vars.join(", ")
            ));
        }
        rest = &after[end + 1..];
    }
    Ok(())
}

fn render_window_format(format: &str, index: usize, tab: &TabRow) -> String {
    render_with(format, |var| match var {
        "window_id" => tab.id.clone(),
        "window_index" => index.to_string(),
        "window_name" => tab.name.clone(),
        "window_active" => (if tab.active { "*" } else { "" }).to_owned(),
        other => unreachable!("validate_format admitted unknown var {other:?}"),
    })
}

fn render_pane_format(format: &str, tab: &TabRow) -> String {
    render_with(format, |var| match var {
        "pane_id" => tab.id.clone(),
        "pane_width" => tab.width.to_string(),
        "pane_height" => tab.height.to_string(),
        "pane_active" => (if tab.active { "1" } else { "0" }).to_owned(),
        "pane_dead" => (if tab.dead { "1" } else { "0" }).to_owned(),
        other => unreachable!("validate_format admitted unknown var {other:?}"),
    })
}

/// Shared `#{var}` substitution walk; `validate_format` already proved every
/// substitution in `format` is well-formed, so `resolve` need not handle an
/// unknown or malformed one.
fn render_with(format: &str, resolve: impl Fn(&str) -> String) -> String {
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(start) = rest.find("#{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}').expect("validated format");
        let var = &after[..end];
        out.push_str(&resolve(var));
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn mux_usage() -> String {
    "usage: minicon mux --control ENDPOINT <verb> [flags]\n\
     verbs: list-windows [-F FORMAT]\n\
     \x20      list-panes [-F FORMAT]\n\
     \x20      select-window -t TARGET\n\
     \x20      new-window [-t TARGET]\n\
     \x20      kill-window [-t TARGET]\n\
     \x20      send-keys [-t TARGET] [-l] KEY...\n\
     \x20      capture-pane [-t TARGET] [-p]\n\
     a TARGET is [session:]window[.pane]; window is an @ID or a tmux index"
        .to_owned()
}

/// A minimal arg cursor, mirroring `control::Cursor`'s shape closely enough
/// to read but kept private to this module -- `mux` translates into
/// `control`'s own CLI args rather than sharing its (non-pub) cursor type.
struct MuxCursor<'a> {
    args: &'a [String],
    position: usize,
}

impl<'a> MuxCursor<'a> {
    fn new(args: &'a [String]) -> Self {
        Self { args, position: 0 }
    }

    fn next(&mut self) -> Option<&'a str> {
        let value = self.args.get(self.position)?.as_str();
        self.position += 1;
        Some(value)
    }

    fn require(&mut self, expected: &str) -> Result<(), String> {
        match self.next() {
            Some(value) if value == expected => Ok(()),
            _ => Err(mux_usage()),
        }
    }

    /// The value of a flag this caller has already consumed, for the verbs
    /// that accept tmux's flags in any order and so cannot peek.
    fn value_for(&mut self, flag: &str) -> Result<&'a str, String> {
        self.next()
            .ok_or_else(|| format!("{flag} requires a value"))
    }

    fn optional_value(&mut self, flag: &str) -> Result<Option<&'a str>, String> {
        if self
            .args
            .get(self.position)
            .is_none_or(|value| value != flag)
        {
            return Ok(None);
        }
        self.position += 1;
        self.next()
            .ok_or_else(|| format!("{flag} requires a value"))
            .map(Some)
    }

    fn finish(&self) -> Result<(), String> {
        if self.position == self.args.len() {
            Ok(())
        } else {
            Err(format!(
                "unexpected argument {:?}",
                self.args[self.position]
            ))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_verb_is_bounded_error() {
        let args = vec![
            "mux".to_owned(),
            "--control".to_owned(),
            "unix:/tmp/x".to_owned(),
            "split-window".to_owned(),
        ];
        let error = run_mux(&args).unwrap_err();
        assert!(error.contains("unknown verb"), "{error}");
    }

    #[test]
    fn parse_tabs_reads_id_title_active() {
        let response = r#"{"tabs":[{"id":"@1","parent":null,"title":"bash","active":true,"child_alive":true,"child_exit_code":null,"cols":80,"rows":24},{"id":"@2","parent":null,"title":"vim","active":false,"child_alive":false,"child_exit_code":0,"cols":100,"rows":40}]}"#;
        let tabs = parse_tabs(response).expect("valid list-tabs response");
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].id, "@1");
        assert_eq!(tabs[0].name, "bash");
        assert!(tabs[0].active);
        assert!(!tabs[0].dead);
        assert_eq!((tabs[0].width, tabs[0].height), (80, 24));
        assert!(!tabs[1].active);
        assert!(tabs[1].dead);
        assert_eq!((tabs[1].width, tabs[1].height), (100, 40));
    }

    #[test]
    fn render_window_format_default_marks_active_window() {
        let tab = TabRow {
            id: "@1".to_owned(),
            name: "bash".to_owned(),
            active: true,
            dead: false,
            width: 80,
            height: 24,
        };
        assert_eq!(render_window_format(DEFAULT_FORMAT, 0, &tab), "0: bash*");
    }

    #[test]
    fn render_pane_format_default_matches_tmux_field_order() {
        let tab = TabRow {
            id: "@1".to_owned(),
            name: "bash".to_owned(),
            active: true,
            dead: false,
            width: 80,
            height: 24,
        };
        assert_eq!(
            render_pane_format(DEFAULT_PANE_FORMAT, &tab),
            "@1 80 24 1 0"
        );
        let dead_tab = TabRow {
            active: false,
            dead: true,
            ..tab
        };
        assert_eq!(
            render_pane_format(DEFAULT_PANE_FORMAT, &dead_tab),
            "@1 80 24 0 1"
        );
    }

    #[test]
    fn unknown_substitution_is_bounded_error() {
        let error = validate_format("#{pane_id}", KNOWN_FORMAT_VARS).unwrap_err();
        assert!(error.contains("unknown substitution"), "{error}");
    }

    #[test]
    fn list_panes_format_rejects_window_vars() {
        let error = validate_format("#{window_id}", KNOWN_PANE_FORMAT_VARS).unwrap_err();
        assert!(error.contains("unknown substitution"), "{error}");
    }

    // `resolve_target` only reaches the endpoint for an integer index, so every
    // `@ID`, session and pane case below is exercised without a live instance.
    // A bogus endpoint proves it: these must fail on the target, not a connect.

    #[test]
    fn an_at_id_target_resolves_without_reaching_the_endpoint() {
        assert_eq!(resolve_target("unix:/nonexistent", "@7").unwrap(), "@7");
        assert_eq!(resolve_target("unix:/nonexistent", "0:@7").unwrap(), "@7");
        assert_eq!(
            resolve_target("unix:/nonexistent", "minicon:@7.0").unwrap(),
            "@7"
        );
    }

    #[test]
    fn a_nonzero_pane_names_the_missing_split_window() {
        let error = resolve_target("unix:/nonexistent", "@7.1").unwrap_err();
        assert!(error.contains("split-window"), "{error}");
        assert!(error.contains("one pane"), "{error}");
    }

    #[test]
    fn a_foreign_session_name_is_refused_not_ignored() {
        let error = resolve_target("unix:/nonexistent", "other:@7").unwrap_err();
        assert!(error.contains("no session named"), "{error}");
    }

    #[test]
    fn a_malformed_at_id_is_refused_before_any_connect() {
        let error = resolve_target("unix:/nonexistent", "@").unwrap_err();
        assert!(error.contains("not a valid @ID"), "{error}");
        let error = resolve_target("unix:/nonexistent", "@x1").unwrap_err();
        assert!(error.contains("not a valid @ID"), "{error}");
    }

    #[test]
    fn a_window_that_is_neither_handle_nor_index_is_refused() {
        let error = resolve_target("unix:/nonexistent", "name").unwrap_err();
        assert!(error.contains("neither a MiniCon @ID handle"), "{error}");
    }

    #[test]
    fn tmux_modifier_prefixes_become_minicon_key_specs() {
        assert_eq!(tmux_key_to_spec("C-c").unwrap(), "ctrl+c");
        assert_eq!(tmux_key_to_spec("Enter").unwrap(), "Enter");
        assert_eq!(tmux_key_to_spec("M-x").unwrap(), "alt+x");
        assert_eq!(tmux_key_to_spec("C-M-a").unwrap(), "ctrl+alt+a");
        assert_eq!(tmux_key_to_spec("S-Tab").unwrap(), "shift+Tab");
    }

    #[test]
    fn tmux_only_key_spellings_are_translated() {
        assert_eq!(tmux_key_to_spec("BSpace").unwrap(), "backspace");
        assert_eq!(tmux_key_to_spec("DC").unwrap(), "delete");
        assert_eq!(tmux_key_to_spec("NPage").unwrap(), "pagedown");
        assert_eq!(tmux_key_to_spec("PPage").unwrap(), "pageup");
    }

    #[test]
    fn an_unknown_key_name_is_refused_rather_than_sent_as_text() {
        let error = tmux_key_to_spec("Entr").unwrap_err();
        assert!(error.contains("neither a tmux key name"), "{error}");
        assert!(error.contains("-l"), "{error}");
        let error = tmux_key_to_spec("C-").unwrap_err();
        assert!(error.contains("modifier and no key"), "{error}");
    }

    #[test]
    fn capture_pane_history_range_is_blocked_on_carried_debt_c1() {
        for flag in ["-S", "-E"] {
            let args = vec![
                "mux".to_owned(),
                "--control".to_owned(),
                "unix:/nonexistent".to_owned(),
                "capture-pane".to_owned(),
                flag.to_owned(),
                "-100".to_owned(),
            ];
            let error = run_mux(&args).unwrap_err();
            assert!(error.contains("C1"), "{flag}: {error}");
            assert!(error.contains("scrollback"), "{flag}: {error}");
        }
    }

    #[test]
    fn new_window_refuses_to_silently_drop_a_window_name() {
        let args = vec![
            "mux".to_owned(),
            "--control".to_owned(),
            "unix:/nonexistent".to_owned(),
            "new-window".to_owned(),
            "-n".to_owned(),
            "build".to_owned(),
        ];
        let error = run_mux(&args).unwrap_err();
        assert!(error.contains("takes no window name"), "{error}");
    }

    #[test]
    fn an_unsupported_tmux_flag_names_the_implemented_subset() {
        let args = vec![
            "mux".to_owned(),
            "--control".to_owned(),
            "unix:/nonexistent".to_owned(),
            "select-window".to_owned(),
            "-a".to_owned(),
        ];
        let error = run_mux(&args).unwrap_err();
        assert!(error.contains("not one of the tmux flags"), "{error}");
    }

    #[test]
    fn select_window_requires_a_target() {
        let args = vec![
            "mux".to_owned(),
            "--control".to_owned(),
            "unix:/nonexistent".to_owned(),
            "select-window".to_owned(),
        ];
        let error = run_mux(&args).unwrap_err();
        assert!(error.contains("requires -t TARGET"), "{error}");
    }
}
