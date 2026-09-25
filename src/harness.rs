//! `harness`: a minimal bounded agent loop with exactly two tools, `file`
//! and `exec`. Not a plugin host, not a permission-policy framework -- the
//! two tools' own bounds (the `--root` path, the `--allow-cmd` list) are the
//! whole policy.
//!
//! Design of record: `prd/PRD_02_31_v0_2_horizon.md` ("harness -- detail").

/// What a `harness` invocation was asked to do, after flag parsing.
///
/// Parsed up front and whole, so a malformed invocation cannot half-run: the
/// same all-or-nothing contract `parse_control_keys` gives the control CLI.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct HarnessRequest {
    pub root: String,
    pub task: String,
    pub backend: Backend,
    /// Permitted executable basenames for the `exec` tool. Empty does **not**
    /// disable the tool -- it still gets advertised, and every call is refused
    /// naming the missing allow-list, so a model cannot tell "not allowed yet"
    /// from "tool absent" by probing.
    pub allow_cmd: Vec<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Backend {
    #[default]
    DeepSeek,
    OpencodeGo,
}

impl Backend {
    /// The environment variable this backend reads its key from. Environment
    /// only, by decision: a config-file credential store would grow MiniCon a
    /// general secrets-management feature for one CLI mode.
    pub fn key_var(self) -> &'static str {
        match self {
            Self::DeepSeek => "MINICON_DEEPSEEK_API_KEY",
            Self::OpencodeGo => "MINICON_OPENCODE_API_KEY",
        }
    }

    /// The spelling `--backend` accepts, so an error message names the flag
    /// value the caller actually typed rather than a Rust variant name.
    pub fn flag_name(self) -> &'static str {
        match self {
            Self::DeepSeek => "deepseek",
            Self::OpencodeGo => "opencode-go",
        }
    }
}

pub fn run_harness(args: &[String]) -> Result<String, String> {
    let request = parse_harness(args)?;
    // Refuse a missing credential before anything else. Finding it here means
    // no tool ever runs for a task that could not have reached a model anyway,
    // and the refusal names the one variable the backend reads.
    let key_var = request.backend.key_var();
    if std::env::var_os(key_var).is_none_or(|value| value.is_empty()) {
        return Err(format!(
            "harness: {key_var} is unset or empty; --backend {} reads its key from that \
             environment variable only",
            request.backend.flag_name()
        ));
    }
    Err(
        "harness: the tool loop is not implemented yet (tracked in plan/plan-v0.2.0.md, H2-H5)"
            .to_owned(),
    )
}

pub fn parse_harness(args: &[String]) -> Result<HarnessRequest, String> {
    let mut position = 0usize;
    match args.get(position).map(String::as_str) {
        Some("harness") => position += 1,
        _ => return Err(harness_usage()),
    }
    let mut root: Option<String> = None;
    let mut task: Option<String> = None;
    let mut backend: Option<Backend> = None;
    let mut allow_cmd = Vec::new();
    while let Some(flag) = args.get(position).map(String::as_str) {
        position += 1;
        let mut value = || -> Result<String, String> {
            let value = args
                .get(position)
                .ok_or_else(|| format!("{flag} requires a value"))?;
            position += 1;
            Ok(value.clone())
        };
        match flag {
            "--root" => root = Some(value()?),
            "--task" => task = Some(value()?),
            "--backend" => {
                let name = value()?;
                backend = Some(match name.as_str() {
                    "deepseek" => Backend::DeepSeek,
                    "opencode-go" => Backend::OpencodeGo,
                    other => {
                        return Err(format!(
                            "--backend {other:?} is not one of: deepseek, opencode-go"
                        ));
                    }
                });
            }
            "--allow-cmd" => allow_cmd.push(value()?),
            other => {
                return Err(format!(
                    "unexpected argument {other:?}\n{}",
                    harness_usage()
                ));
            }
        }
    }
    Ok(HarnessRequest {
        root: root.ok_or_else(|| {
            format!(
                "harness requires --root PATH (there is deliberately no implicit \
                 current-directory default, which would widen the file tool's bound \
                 silently)\n{}",
                harness_usage()
            )
        })?,
        task: task.ok_or_else(|| format!("harness requires --task TEXT\n{}", harness_usage()))?,
        backend: backend.unwrap_or_default(),
        allow_cmd,
    })
}

fn harness_usage() -> String {
    "usage: minicon harness --root PATH --task TEXT [--backend deepseek|opencode-go] \
     [--allow-cmd NAME]..."
        .to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_and_task_are_required_with_no_implicit_default() {
        let error = parse_harness(&["harness".to_owned(), "--task".to_owned(), "x".to_owned()])
            .unwrap_err();
        assert!(error.contains("--root"), "{error}");
        let error = parse_harness(&["harness".to_owned(), "--root".to_owned(), "/tmp".to_owned()])
            .unwrap_err();
        assert!(error.contains("--task"), "{error}");
    }

    #[test]
    fn allow_cmd_repeats_and_backend_defaults_to_deepseek() {
        let request = parse_harness(&[
            "harness".to_owned(),
            "--root".to_owned(),
            "/tmp/work".to_owned(),
            "--task".to_owned(),
            "do a thing".to_owned(),
            "--allow-cmd".to_owned(),
            "ls".to_owned(),
            "--allow-cmd".to_owned(),
            "cat".to_owned(),
        ])
        .expect("valid invocation");
        assert_eq!(request.allow_cmd, vec!["ls".to_owned(), "cat".to_owned()]);
        assert_eq!(request.backend, Backend::DeepSeek);
        assert_eq!(request.backend.key_var(), "MINICON_DEEPSEEK_API_KEY");
    }

    #[test]
    fn an_unknown_backend_is_a_bounded_error() {
        let error = parse_harness(&[
            "harness".to_owned(),
            "--root".to_owned(),
            "/tmp".to_owned(),
            "--task".to_owned(),
            "x".to_owned(),
            "--backend".to_owned(),
            "gpt".to_owned(),
        ])
        .unwrap_err();
        assert!(error.contains("not one of"), "{error}");
    }
}
