//! `mux`: a tmux-CLI-compatible verb surface over the existing `--control`
//! endpoint (`control.rs`). Each verb is a thin translation into the
//! existing `CliCommand` wire protocol -- `mux` opens no new transport and
//! adds no new server-side capability.
//!
//! Design of record: `prd/PRD_02_31_v0_2_horizon.md` ("mux -- detail").

/// Substitution variables `list-windows -F` accepts. Unlike tmux, an unknown
/// substitution is a bounded error rather than a blank field, so a script
/// never silently trusts empty output.
const KNOWN_FORMAT_VARS: &[&str] = &["window_id", "window_index", "window_name", "window_active"];

const DEFAULT_FORMAT: &str = "#{window_index}: #{window_name}#{window_active}";

pub fn run_mux(args: &[String]) -> Result<String, String> {
    let mut cursor = MuxCursor::new(args);
    cursor.require("mux")?;
    let control = cursor.required_value("--control")?.to_owned();
    let verb = cursor.next().ok_or_else(mux_usage)?.to_owned();

    match verb.as_str() {
        "list-windows" => run_list_windows(&control, &mut cursor),
        "select-window" | "new-window" | "kill-window" | "send-keys" | "capture-pane" => Err(
            format!("mux {verb}: not implemented yet (tracked in plan/plan-v0.2.0.md, M3/M3b)"),
        ),
        other => Err(format!("mux: unknown verb {other:?}\n{}", mux_usage())),
    }
}

fn run_list_windows(control: &str, cursor: &mut MuxCursor<'_>) -> Result<String, String> {
    let format = cursor
        .optional_value("-F")?
        .map(str::to_owned)
        .unwrap_or_else(|| DEFAULT_FORMAT.to_owned());
    cursor.finish()?;
    validate_format(&format)?;

    let cli_args = vec![
        "cli".to_owned(),
        "--control".to_owned(),
        control.to_owned(),
        "list-tabs".to_owned(),
    ];
    let response = crate::control::run_cli(&cli_args)?;
    let tabs = parse_tabs(&response)?;

    let mut lines = Vec::with_capacity(tabs.len());
    for (index, tab) in tabs.iter().enumerate() {
        lines.push(render_format(&format, index, tab));
    }
    Ok(lines.join("\n"))
}

/// One tab as reported by `list-tabs`, narrowed to the fields `mux` exposes.
struct TabRow {
    id: String,
    name: String,
    active: bool,
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
            Ok(TabRow { id, name, active })
        })
        .collect()
}

fn validate_format(format: &str) -> Result<(), String> {
    let mut rest = format;
    while let Some(start) = rest.find("#{") {
        let after = &rest[start + 2..];
        let end = after
            .find('}')
            .ok_or_else(|| format!("-F {format:?}: unterminated #{{...}} substitution"))?;
        let var = &after[..end];
        if !KNOWN_FORMAT_VARS.contains(&var) {
            return Err(format!(
                "-F {format:?}: unknown substitution #{{{var}}}; known: {}",
                KNOWN_FORMAT_VARS.join(", ")
            ));
        }
        rest = &after[end + 1..];
    }
    Ok(())
}

fn render_format(format: &str, index: usize, tab: &TabRow) -> String {
    let mut out = String::with_capacity(format.len());
    let mut rest = format;
    while let Some(start) = rest.find("#{") {
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        // `validate_format` already proved every substitution is well-formed
        // and known, so this unwrap and lookup cannot fail here.
        let end = after.find('}').expect("validated format");
        let var = &after[..end];
        out.push_str(&match var {
            "window_id" => tab.id.clone(),
            "window_index" => index.to_string(),
            "window_name" => tab.name.clone(),
            "window_active" => (if tab.active { "*" } else { "" }).to_owned(),
            other => unreachable!("validate_format admitted unknown var {other:?}"),
        });
        rest = &after[end + 1..];
    }
    out.push_str(rest);
    out
}

fn mux_usage() -> String {
    "usage: minicon mux --control ENDPOINT <verb> [flags]\n\
     verbs: list-windows [-F FORMAT]"
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

    fn required_value(&mut self, flag: &str) -> Result<&'a str, String> {
        match self.next() {
            Some(value) if value == flag => self
                .next()
                .filter(|value| !value.starts_with("--"))
                .ok_or_else(|| format!("{flag} requires a value")),
            Some(value) => Err(format!("expected {flag}, got {value:?}")),
            None => Err(format!("{flag} requires a value")),
        }
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
    fn unimplemented_verb_names_itself_not_a_crash() {
        let args = vec![
            "mux".to_owned(),
            "--control".to_owned(),
            "unix:/tmp/x".to_owned(),
            "kill-window".to_owned(),
        ];
        let error = run_mux(&args).unwrap_err();
        assert!(error.contains("kill-window"), "{error}");
    }

    #[test]
    fn parse_tabs_reads_id_title_active() {
        let response = r#"{"tabs":[{"id":"@1","parent":null,"title":"bash","active":true,"child_alive":true,"child_exit_code":null},{"id":"@2","parent":null,"title":"vim","active":false,"child_alive":true,"child_exit_code":null}]}"#;
        let tabs = parse_tabs(response).expect("valid list-tabs response");
        assert_eq!(tabs.len(), 2);
        assert_eq!(tabs[0].id, "@1");
        assert_eq!(tabs[0].name, "bash");
        assert!(tabs[0].active);
        assert!(!tabs[1].active);
    }

    #[test]
    fn render_format_default_marks_active_window() {
        let tab = TabRow {
            id: "@1".to_owned(),
            name: "bash".to_owned(),
            active: true,
        };
        assert_eq!(render_format(DEFAULT_FORMAT, 0, &tab), "0: bash*");
    }

    #[test]
    fn unknown_substitution_is_bounded_error() {
        let error = validate_format("#{pane_id}").unwrap_err();
        assert!(error.contains("unknown substitution"), "{error}");
    }
}
