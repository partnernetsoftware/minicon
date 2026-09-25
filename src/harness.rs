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
    // Refuse an unwired backend before anything else, including before the
    // credential check: demanding a key for a backend that cannot run would
    // name the wrong problem. `opencode-go` has its own codec, written
    // separately; it is deliberately **not** served by the DeepSeek codec,
    // because a wire format that merely resembles another one produces
    // plausible wrong requests instead of a clear failure.
    if request.backend == Backend::OpencodeGo {
        return Err(format!(
            "harness: --backend {} is not wired into this CLI yet: its own codec is a separate \
             module still being written, and it will not be served by the deepseek codec, which \
             speaks a different wire format. Use --backend {} today; tracked as H5 in \
             plan/plan-v0.2.0.md.",
            Backend::OpencodeGo.flag_name(),
            Backend::DeepSeek.flag_name()
        ));
    }
    // Refuse a missing credential next. Finding it here means no tool ever runs
    // for a task that could not have reached a model anyway, and the refusal
    // names the one variable the backend reads.
    let key_var = request.backend.key_var();
    let key = match std::env::var(key_var) {
        Ok(key) if !key.is_empty() => key,
        _ => {
            return Err(format!(
                "harness: {key_var} is unset or empty; --backend {} reads its key from that \
                 environment variable only",
                request.backend.flag_name()
            ));
        }
    };
    // Resolve the file tool's bound now, so an unusable --root is a bounded
    // error up front rather than a surprise on the model's first tool call.
    let file_tool = FileTool::new(&request.root)?;
    let exec_tool = ExecTool::new(file_tool.root_path(), &request.allow_cmd);
    crate::harness_wire::run_task(
        &crate::harness_wire::NetworkHttp,
        crate::harness_wire::DEEPSEEK_CHAT_URL,
        &key,
        crate::harness_wire::DEEPSEEK_MODEL,
        &request.task,
        &file_tool,
        &exec_tool,
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

// ---------------------------------------------------------------------------
// H2 -- the `file` tool
// ---------------------------------------------------------------------------

/// Largest file the `file` tool will read in one call. A tool result is
/// eventually going into a model prompt, so the bound is the read's, not the
/// caller's: an unbounded read would be an unbounded prompt.
pub const FILE_TOOL_MAX_READ_BYTES: usize = 256 * 1024;

/// Read and write, confined to one directory.
///
/// The bound is enforced by construction rather than by comparing resolved
/// prefixes, because a prefix comparison has to be right about symlinks at
/// every component and about paths that do not exist yet. Instead a relative
/// path is admitted only when it is made entirely of plain components and no
/// component that already exists is a symlink. Such a path cannot leave the
/// canonical root, so there is nothing left to clamp -- anything else is
/// refused.
#[derive(Clone, Debug)]
pub struct FileTool {
    root: std::path::PathBuf,
}

impl FileTool {
    /// Canonicalizes the root once. A root that is missing or is not a
    /// directory is refused here, so no later call has to wonder whether its
    /// bound exists.
    pub fn new(root: &str) -> Result<Self, String> {
        let root = std::fs::canonicalize(root)
            .map_err(|error| format!("file: --root {root:?} cannot be resolved: {error}"))?;
        if !root.is_dir() {
            return Err(format!(
                "file: --root {} is not a directory",
                root.display()
            ));
        }
        Ok(Self { root })
    }

    /// The canonical directory every path this tool accepts stays inside.
    pub fn root_path(&self) -> &std::path::Path {
        &self.root
    }

    /// Resolves one model-supplied relative path against the root, refusing
    /// anything that could denote a file outside it.
    ///
    /// Refused, never rewritten: an absolute path (including a Windows drive
    /// prefix), an empty path, any `..` or `.` component, and any existing
    /// component -- intermediate or final -- that is a symlink. The symlink
    /// check walks the path from the root downwards, so a symlinked directory
    /// halfway along cannot be traversed, and a leaf that does not exist yet
    /// (an ordinary create) is accepted only once every one of its existing
    /// ancestors has been checked.
    fn resolve(&self, relative: &str) -> Result<std::path::PathBuf, String> {
        use std::path::Component;

        if relative.is_empty() {
            return Err("file: path is empty; give a path relative to --root".to_owned());
        }
        let candidate = std::path::Path::new(relative);
        let mut resolved = self.root.clone();
        for component in candidate.components() {
            match component {
                Component::Normal(part) => resolved.push(part),
                Component::ParentDir => {
                    return Err(format!(
                        "file: path {relative:?} is refused: a `..` component would escape \
                         --root {}; paths are not clamped into the root",
                        self.root.display()
                    ));
                }
                Component::RootDir | Component::Prefix(_) => {
                    return Err(format!(
                        "file: path {relative:?} is refused: only paths relative to --root {} \
                         are accepted, never absolute ones",
                        self.root.display()
                    ));
                }
                // `.` carries no authority but also no meaning here; refusing
                // it keeps "every component is a plain name" true, which is
                // the property the symlink walk below relies on.
                Component::CurDir => {
                    return Err(format!(
                        "file: path {relative:?} is refused: remove the `.` component"
                    ));
                }
            }
            match std::fs::symlink_metadata(&resolved) {
                Ok(metadata) if metadata.file_type().is_symlink() => {
                    return Err(format!(
                        "file: path {relative:?} is refused: {} is a symlink, and the file \
                         tool never follows one -- a link could point outside --root {}",
                        resolved.display(),
                        self.root.display()
                    ));
                }
                // Absent is fine: a create names a leaf that does not exist,
                // and every ancestor it does have was checked on an earlier
                // turn of this loop.
                _ => {}
            }
        }
        Ok(resolved)
    }

    /// Reads one bounded UTF-8 file from inside the root.
    pub fn read(&self, relative: &str) -> Result<String, String> {
        let path = self.resolve(relative)?;
        let bytes =
            agenterm_platform::filesystem_read::read_bounded(&path, FILE_TOOL_MAX_READ_BYTES)
                .map_err(|error| format!("file: read {relative:?} failed: {error}"))?;
        String::from_utf8(bytes)
            .map_err(|_| format!("file: read {relative:?} failed: file is not valid UTF-8"))
    }

    /// Writes one file inside the root, atomically.
    ///
    /// `write_file_atomic` rather than `write_path_atomic_no_clobber`: the tool
    /// must be able to rewrite a file it just read, which no-clobber forbids,
    /// and unlike the path-based variant this one hands us the `fs::File` we
    /// want to write bytes into directly. Both publish a completed sibling by
    /// rename, so a refusal or a failure mid-write leaves the destination at
    /// its previous contents -- the "never a partial write" half of H2's safe
    /// failure.
    pub fn write(&self, relative: &str, contents: &str) -> Result<usize, String> {
        use std::io::Write as _;

        let path = self.resolve(relative)?;
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|error| format!("file: write {relative:?} failed: {error}"))?;
        }
        agenterm_platform::filesystem_publish::write_file_atomic(&path, |file| {
            file.write_all(contents.as_bytes())
        })
        .map_err(|error| format!("file: write {relative:?} failed: {}", error.detail()))?;
        Ok(contents.len())
    }
}

// ---------------------------------------------------------------------------
// H3 -- the `exec` tool
// ---------------------------------------------------------------------------

/// How long one `exec` call may run before it is terminated.
pub const EXEC_TOOL_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// What one finished `exec` call produced.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ExecOutcome {
    /// `None` when the process was terminated rather than exiting on its own
    /// (a signal, or this tool's own timeout).
    pub exit_code: Option<i32>,
    pub stdout: String,
    pub stderr: String,
    pub timed_out: bool,
}

/// Runs exactly one allow-listed command per call, as an argv vector.
///
/// There is no shell anywhere in this path: the program is executed directly,
/// so `;`, `&&`, `|`, `$(...)` and friends reach the process as ordinary
/// argument bytes and cannot become a second command.
#[derive(Clone, Debug)]
pub struct ExecTool {
    root: std::path::PathBuf,
    allow_cmd: Vec<String>,
}

impl ExecTool {
    pub fn new(root: &std::path::Path, allow_cmd: &[String]) -> Self {
        Self {
            root: root.to_path_buf(),
            allow_cmd: allow_cmd.to_vec(),
        }
    }

    /// Decides whether `program` may run, without running it.
    ///
    /// An empty allow-list does not make the tool disappear; it makes every
    /// call fail with this one error, naming the flag that would grant it. A
    /// model probing the tool therefore cannot distinguish "not allowed yet"
    /// from "tool absent".
    fn admit(&self, program: &str) -> Result<(), String> {
        if self.allow_cmd.is_empty() {
            return Err(
                "exec: refused: the allow-list is empty, so no command may run; pass \
                 --allow-cmd NAME (repeatable) to permit an executable basename"
                    .to_owned(),
            );
        }
        if program.is_empty() {
            return Err("exec: refused: no command given".to_owned());
        }
        // A basename, not a path: without this, `--allow-cmd ls` would also
        // admit `./ls` or `/tmp/evil/ls`, whose basename matches but whose
        // bytes name a different executable.
        if std::path::Path::new(program).components().count() != 1
            || program.contains('/')
            || program.contains('\\')
        {
            return Err(format!(
                "exec: refused: {program:?} is a path, not an executable basename; \
                 --allow-cmd names basenames and exec resolves them on PATH"
            ));
        }
        if !self.allow_cmd.iter().any(|allowed| allowed == program) {
            return Err(format!(
                "exec: refused: {program:?} is not in the allow-list ({}); pass \
                 --allow-cmd {program} to permit it",
                self.allow_cmd.join(", ")
            ));
        }
        Ok(())
    }

    /// Runs one command with its arguments, capturing output.
    pub fn run(&self, program: &str, args: &[String]) -> Result<ExecOutcome, String> {
        self.admit(program)?;
        self.spawn_contained(program, args)
    }

    /// The one place a child process is created.
    ///
    /// **Deviation, deliberate and reported:** the design of record names
    /// `agenterm_platform::contained_process::ContainedHeadlessCommand`, but
    /// that module is gated behind the crate's `contained-process-spawn`
    /// feature, which MiniCon's dependency does not enable. Enabling it is a
    /// one-line `Cargo.toml` change and it does compile, but that file is
    /// outside this change's ownership. Until it is enabled this uses
    /// `std::process::Command`, which gives the same argv-vector,
    /// no-shell guarantee that H3's invariant is actually about; what it does
    /// not give is the resource containment. Swapping the builder in is a
    /// change to this function body alone.
    fn spawn_contained(&self, program: &str, args: &[String]) -> Result<ExecOutcome, String> {
        use std::io::Read as _;

        let mut child = std::process::Command::new(program)
            .args(args)
            .current_dir(&self.root)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .map_err(|error| format!("exec: {program:?} could not be started: {error}"))?;

        // Drain both pipes on their own threads. Polling for the exit while a
        // full pipe blocks the child would deadlock the timeout this bound
        // exists to provide.
        let drain = |stream: Option<std::process::ChildStdout>| {
            std::thread::spawn(move || {
                let mut buffer = Vec::new();
                if let Some(mut stream) = stream {
                    let _ = stream.read_to_end(&mut buffer);
                }
                buffer
            })
        };
        let stdout = drain(child.stdout.take());
        let stderr = std::thread::spawn({
            let stream = child.stderr.take();
            move || {
                let mut buffer = Vec::new();
                if let Some(mut stream) = stream {
                    let _ = stream.read_to_end(&mut buffer);
                }
                buffer
            }
        });

        let deadline = std::time::Instant::now() + EXEC_TOOL_TIMEOUT;
        let mut timed_out = false;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break Some(status),
                Ok(None) => {}
                Err(error) => {
                    return Err(format!("exec: {program:?} could not be waited on: {error}"));
                }
            }
            if std::time::Instant::now() >= deadline {
                timed_out = true;
                let _ = child.kill();
                let _ = child.wait();
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };

        let collect = |handle: std::thread::JoinHandle<Vec<u8>>| {
            String::from_utf8_lossy(&handle.join().unwrap_or_default()).into_owned()
        };
        Ok(ExecOutcome {
            exit_code: status.and_then(|status| status.code()),
            stdout: collect(stdout),
            stderr: collect(stderr),
            timed_out,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A fresh empty directory under the system temp dir, unique per test.
    fn fixture(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "minicon-harness-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture root");
        std::fs::canonicalize(&root).expect("canonical fixture root")
    }

    #[test]
    fn file_tool_round_trips_inside_the_root() {
        let root = fixture("round-trip");
        let tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("root exists");
        tool.write("notes/todo.txt", "one\ntwo\n").expect("write");
        assert_eq!(tool.read("notes/todo.txt").expect("read"), "one\ntwo\n");
        // A rewrite must be allowed, and must replace rather than append.
        tool.write("notes/todo.txt", "three\n").expect("rewrite");
        assert_eq!(tool.read("notes/todo.txt").expect("reread"), "three\n");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn file_tool_refuses_parent_and_absolute_escapes_without_clamping() {
        let root = fixture("escape");
        let outside = root
            .parent()
            .expect("parent")
            .join("minicon-harness-outside.txt");
        std::fs::write(&outside, "secret\n").expect("outside fixture");
        let tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("root exists");

        let error = tool.read("../minicon-harness-outside.txt").unwrap_err();
        assert!(error.contains("refused") && error.contains(".."), "{error}");
        let error = tool
            .write("a/../../minicon-harness-outside.txt", "clobbered\n")
            .unwrap_err();
        assert!(error.contains("refused"), "{error}");
        let error = tool
            .read(outside.to_str().expect("utf-8 absolute"))
            .unwrap_err();
        assert!(
            error.contains("refused") && error.contains("absolute"),
            "{error}"
        );

        // Refused, not clamped: the outside file is untouched and no clamped
        // in-root twin was created either.
        assert_eq!(
            std::fs::read_to_string(&outside).expect("outside"),
            "secret\n"
        );
        assert!(!root.join("minicon-harness-outside.txt").exists());
        let _ = std::fs::remove_file(&outside);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn file_tool_refuses_a_symlink_at_the_leaf_and_at_an_intermediate_component() {
        let root = fixture("symlink");
        let outside_dir = root
            .parent()
            .expect("parent")
            .join("minicon-harness-outdir");
        let _ = std::fs::remove_dir_all(&outside_dir);
        std::fs::create_dir_all(&outside_dir).expect("outside dir");
        std::fs::write(outside_dir.join("loot.txt"), "secret\n").expect("outside file");

        // A leaf symlink, and a symlink at an intermediate component -- the
        // case a resolve-then-compare-prefixes check is easiest to get wrong.
        std::os::unix::fs::symlink(outside_dir.join("loot.txt"), root.join("leaf.txt"))
            .expect("leaf link");
        std::os::unix::fs::symlink(&outside_dir, root.join("bridge")).expect("dir link");

        let tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("root exists");
        let error = tool.read("leaf.txt").unwrap_err();
        assert!(error.contains("symlink"), "{error}");
        let error = tool.read("bridge/loot.txt").unwrap_err();
        assert!(error.contains("symlink"), "{error}");
        // A write through an intermediate link is refused too, and lands
        // nothing outside the root.
        let error = tool.write("bridge/planted.txt", "x\n").unwrap_err();
        assert!(error.contains("symlink"), "{error}");
        assert!(!outside_dir.join("planted.txt").exists());
        assert_eq!(
            std::fs::read_to_string(outside_dir.join("loot.txt")).expect("loot"),
            "secret\n"
        );

        let _ = std::fs::remove_dir_all(&outside_dir);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn exec_tool_refuses_every_call_when_the_allow_list_is_empty() {
        let root = fixture("exec-empty");
        let tool = ExecTool::new(&root, &[]);
        let error = tool.run("echo", &["hi".to_owned()]).unwrap_err();
        assert!(error.contains("allow-list is empty"), "{error}");
        assert!(error.contains("--allow-cmd"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn exec_tool_refuses_an_unlisted_command_and_a_path_spelling_of_a_listed_one() {
        let root = fixture("exec-allow");
        let tool = ExecTool::new(&root, &["echo".to_owned()]);
        let error = tool.run("cat", &[]).unwrap_err();
        assert!(error.contains("not in the allow-list"), "{error}");
        // A basename match is not enough: a path spelling names a different
        // executable and is refused before anything is spawned.
        let error = tool.run("/bin/echo", &[]).unwrap_err();
        assert!(error.contains("basename"), "{error}");
        let error = tool.run("./echo", &[]).unwrap_err();
        assert!(error.contains("basename"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[cfg(unix)]
    #[test]
    fn exec_tool_runs_one_command_and_never_interprets_shell_metacharacters() {
        let root = fixture("exec-run");
        let tool = ExecTool::new(&root, &["echo".to_owned()]);

        let outcome = tool.run("echo", &["hello".to_owned()]).expect("run echo");
        assert_eq!(outcome.exit_code, Some(0));
        assert_eq!(outcome.stdout.trim_end(), "hello");
        assert!(!outcome.timed_out);

        // `;` and `&&` arrive as argument bytes. If a shell were involved the
        // second command would run and the marker file would exist.
        let marker = root.join("pwned.txt");
        let outcome = tool
            .run(
                "echo",
                &[
                    "a; touch pwned.txt".to_owned(),
                    "&&".to_owned(),
                    "touch".to_owned(),
                    "pwned.txt".to_owned(),
                    "| tee pwned.txt".to_owned(),
                    "$(touch pwned.txt)".to_owned(),
                ],
            )
            .expect("run echo with metacharacters");
        assert_eq!(outcome.exit_code, Some(0));
        assert!(!marker.exists(), "a shell interpreted the arguments");
        assert!(outcome.stdout.contains("a; touch pwned.txt"), "{outcome:?}");
        assert!(outcome.stdout.contains("$(touch pwned.txt)"), "{outcome:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

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
