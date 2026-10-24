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
    pub gateway: Option<String>,
    pub client_key_env: String,
    /// Permitted executable basenames for the `exec` tool. Empty does **not**
    /// disable the tool -- it still gets advertised, and every call is refused
    /// naming the missing allow-list, so a model cannot tell "not allowed yet"
    /// from "tool absent" by probing.
    pub allow_cmd: Vec<String>,
    /// Names a **new** session to record this run's task/answer under.
    /// Mutually exclusive with `continue_session`. Refused if a session by
    /// this name already exists, so a typo cannot silently overwrite one.
    pub new_session: Option<String>,
    /// Resumes an existing session: its recorded turns are folded into this
    /// run's task as prior context, and this turn is appended back. Refused
    /// -- a bounded CLI error, never a silent fresh start -- if the named
    /// session does not exist or its file is corrupt.
    pub continue_session: Option<String>,
}

#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum Backend {
    #[default]
    DeepSeek,
    OpencodeGo,
}

impl Backend {
    /// The environment variable this backend reads its key from. Environment
    /// only for the legacy direct-provider mode. OAuth accounts and the
    /// explicit local gateway are owned by PRD 02.31.01.
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

/// Private file the GUI writes after a DeepSeek code exchange.
///
/// `HOME` wins. Windows profiles often have `USERPROFILE` and no `HOME`.
pub fn oauth_bearer_path() -> Option<std::path::PathBuf> {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .or_else(|| std::env::var_os("USERPROFILE").filter(|value| !value.is_empty()))?;
    Some(
        std::path::PathBuf::from(home)
            .join(".minicon-embed")
            .join("deepseek-bearer"),
    )
}

#[cfg(all(test, unix))]
fn store_oauth_bearer_at(path: &std::path::Path, token: &str) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
        minicon_platform::filesystem::protect_private_directory(parent)
            .map_err(|error| error.to_string())?;
    }
    minicon_platform::filesystem::write_private_atomic(path, token.as_bytes())
        .map_err(|error| error.to_string())
}

pub fn load_oauth_bearer() -> Option<String> {
    let path = oauth_bearer_path()?;
    load_oauth_bearer_at(&path)
}

fn load_oauth_bearer_at(path: &std::path::Path) -> Option<String> {
    let bytes = minicon_platform::filesystem::read_private_bounded(path, 4096).ok()?;
    let token = String::from_utf8(bytes).ok()?;
    let token = token.trim();
    if token.is_empty() || token.len() > 256 || !token.is_ascii() {
        None
    } else {
        Some(token.to_owned())
    }
}

pub fn run_harness(args: &[String]) -> Result<String, String> {
    let request = parse_harness(args)?;
    let cancellation = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let budget = InvocationBudget::new(cancellation.clone());
    // Refuse a missing credential first. Finding it here means no tool ever runs
    // for a task that could not have reached a model anyway, and the refusal
    // names the one variable the backend reads.
    let key_var = if request.gateway.is_some() {
        request.client_key_env.as_str()
    } else {
        request.backend.key_var()
    };
    let env_key = std::env::var(key_var).ok();
    let oauth_token = if request.gateway.is_none() && request.backend == Backend::DeepSeek {
        load_oauth_bearer()
    } else {
        None
    };
    let key = crate::harness_wire::resolve_deepseek_bearer(
        env_key.as_deref(),
        oauth_token.as_deref(),
    )
    .map(str::to_owned)
    .map_err(|()| {
        format!(
            "harness: {key_var} is unset or empty; {} reads its key from that environment variable",
            if request.gateway.is_some() {
                "--gateway"
            } else {
                request.backend.flag_name()
            }
        )
    })?;
    let _console = minicon_platform::parent_console::ParentConsoleSession::attach();
    let _interrupt = crate::cli_interrupt::InterruptWatch::start(cancellation.clone())?;
    // Resolve the file tool's bound now, so an unusable --root is a bounded
    // error up front rather than a surprise on the model's first tool call.
    let file_tool = FileTool::new(&request.root)?;
    let exec_tool = ExecTool::new(file_tool.root_path(), &request.allow_cmd)
        .with_cancellation(cancellation.clone())
        .with_deadline(budget.deadline);

    let session_id = request
        .new_session
        .as_deref()
        .or(request.continue_session.as_deref());
    let is_continue = request.continue_session.is_some();
    let mut state = SessionState::default();
    if let Some(id) = session_id {
        validate_session_id(id)?;
        let existing = load_session(&file_tool, id)?;
        if is_continue && existing.is_none() {
            return Err(format!(
                "harness: --continue {id:?} refused: no session by that name exists under \
                 --root; start one first with --session {id:?}"
            ));
        }
        if !is_continue && existing.is_some() {
            return Err(format!(
                "harness: --session {id:?} refused: a session by that name already exists; \
                 use --continue {id:?} to resume it instead of overwriting it"
            ));
        }
        state = existing.unwrap_or_default();
    }
    let gateway = if let Some(base) = &request.gateway {
        let session = state
            .gateway_binding
            .as_ref()
            .map(|binding| binding.session.clone())
            .map(Ok)
            .unwrap_or_else(crate::harness_gateway::new_session_id)?;
        let transport = crate::harness_gateway::GatewayTransport {
            base: base.clone(),
            session,
            cancelled: cancellation,
        };
        let binding =
            transport.binding(&key, budget.check().map_err(|error| error.to_string())?)?;
        if is_continue && state.gateway_binding.as_ref() != Some(&binding) {
            return Err("harness: session gateway binding changed; start a new session".into());
        }
        state.gateway_binding = Some(binding);
        Some(transport)
    } else {
        if state.gateway_binding.is_some() {
            return Err("harness: gateway session cannot resume through a direct provider".into());
        }
        None
    };
    // A session saved before the tree working memory existed carries `turns`
    // but no `tree`: rebuild it deterministically from `turns` rather than
    // starting the tree over -- lossless, so this is not the silent-fresh-
    // start this leaf's safe-failure rule forbids (that rule is about a
    // *corrupt* file, not an older-format one).
    if state.tree.is_empty() && !state.turns.is_empty() {
        state.tree = tree_from_turns(&state.turns);
    }
    let task = compose_resumed_task(&state.tree, &request.task);

    // Each backend runs its OWN codec. `opencode-go` is deliberately not
    // served by the DeepSeek codec: a wire format that merely resembles
    // another one produces plausible wrong requests instead of a clear
    // failure. The `Transport` seam is the only thing the two share, wrapped
    // here as a single `&str -> Result<String, String>` closure so `{LOOP}`
    // (below) can drive it through several state-machine turns without
    // knowing which backend it is -- the same "dumb transport" contract
    // `{WIRE}` already guards.
    let mut call = |task: &str| -> Result<String, String> {
        if let Some(transport) = &gateway {
            return crate::harness_wire::run_task_with_budget(
                transport,
                &format!("{}/chat/completions", transport.base),
                &key,
                "minicon-selected",
                task,
                &file_tool,
                &exec_tool,
                &budget,
            )
            .map_err(|error| error.to_string());
        }
        match request.backend {
            Backend::DeepSeek => crate::harness_wire::run_task_with_budget(
                &crate::harness_wire::NetworkHttp,
                crate::harness_wire::DEEPSEEK_CHAT_URL,
                &key,
                crate::harness_wire::DEEPSEEK_MODEL,
                task,
                &file_tool,
                &exec_tool,
                &budget,
            )
            .map_err(|error| error.to_string()),
            Backend::OpencodeGo => {
                let url = crate::harness_opencode::opencode_chat_url()?;
                let model = crate::harness_opencode::opencode_model();
                crate::harness_opencode::opencode_run_task_with_budget(
                    &crate::harness_wire::NetworkHttp,
                    &url,
                    &key,
                    &model,
                    task,
                    &file_tool,
                    &exec_tool,
                    &budget,
                )
                .map_err(|error| error.to_string())
            }
        }
    };
    let mut trace = Vec::new();
    let answer = run_loop(&mut call, &task, &mut trace)?;

    // Only a turn that produced a final answer is remembered -- a refused or
    // timed-out turn never reaches here, so a broken turn cannot pollute the
    // resumed context with a partial exchange.
    if let Some(id) = session_id {
        let index = state.turns.len();
        state.turns.push((request.task.clone(), answer.clone()));
        state
            .tree
            .push(turn_node(index, &request.task, trace, &answer));
        save_session(&file_tool, id, &state)?;
    }
    Ok(answer)
}

pub const INVOCATION_MAX_MODEL_TURNS: usize = 16;
pub const INVOCATION_MAX_TOOL_CALLS: usize = 64;
pub const INVOCATION_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10 * 60);

/// A fatal invocation boundary, separate from a recoverable tool refusal.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum InvocationStop {
    ModelTurns,
    ToolCalls,
    Deadline,
    Cancelled,
}

impl std::fmt::Display for InvocationStop {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let reason = match self {
            Self::ModelTurns => "invocation model-turn budget exhausted (16)",
            Self::ToolCalls => "invocation tool-call budget exhausted (64)",
            Self::Deadline => "invocation deadline exhausted (10 minutes)",
            Self::Cancelled => "invocation cancelled",
        };
        write!(formatter, "harness: {reason}; the task did not finish")
    }
}

#[derive(Debug)]
pub enum HarnessFailure {
    Stopped(InvocationStop),
    Backend(String),
}

impl From<InvocationStop> for HarnessFailure {
    fn from(stop: InvocationStop) -> Self {
        Self::Stopped(stop)
    }
}

impl From<String> for HarnessFailure {
    fn from(error: String) -> Self {
        Self::Backend(error)
    }
}

impl std::fmt::Display for HarnessFailure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stopped(stop) => stop.fmt(formatter),
            Self::Backend(error) => formatter.write_str(error),
        }
    }
}

/// One owner spans outer-loop re-entry and both codecs. Session resumption
/// starts a fresh invocation; counters never reset inside a codec call.
pub struct InvocationBudget {
    model_turns: std::cell::Cell<usize>,
    tool_calls: std::cell::Cell<usize>,
    deadline: std::time::Instant,
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl InvocationBudget {
    pub fn new(cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>) -> Self {
        Self {
            model_turns: std::cell::Cell::new(0),
            tool_calls: std::cell::Cell::new(0),
            deadline: std::time::Instant::now() + INVOCATION_TIMEOUT,
            cancelled,
        }
    }

    pub fn check(&self) -> Result<std::time::Duration, InvocationStop> {
        if self.cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err(InvocationStop::Cancelled);
        }
        self.deadline
            .checked_duration_since(std::time::Instant::now())
            .filter(|remaining| !remaining.is_zero())
            .ok_or(InvocationStop::Deadline)
    }

    pub fn take_model_turn(&self) -> Result<std::time::Duration, InvocationStop> {
        let remaining = self.check()?;
        if self.model_turns.get() >= INVOCATION_MAX_MODEL_TURNS {
            return Err(InvocationStop::ModelTurns);
        }
        self.model_turns.set(self.model_turns.get() + 1);
        Ok(remaining)
    }

    pub fn take_tool_call(&self) -> Result<(), InvocationStop> {
        self.check()?;
        if self.tool_calls.get() >= INVOCATION_MAX_TOOL_CALLS {
            return Err(InvocationStop::ToolCalls);
        }
        self.tool_calls.set(self.tool_calls.get() + 1);
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// {LOOP} -- the bounded five-state micro-workflow
// (plan/plan-harness-context-engineering.md)
// ---------------------------------------------------------------------------

/// Ceiling on how many times `{LOOP}` may jump back to `categorize` within
/// one invocation. A live model is never trusted to end a loop on its own
/// judgement alone (mirrors `exec`'s allow-list posture): once this many
/// draft/execute rounds have run, `decide-continue` force-ends regardless of
/// what the latest answer says.
const LOOP_MAX_ITERATIONS: usize = 5;

/// Ceiling on how many times `decide-pick` may send a draft back for a
/// redraft within one iteration, before it force-accepts the latest draft
/// rather than looping forever on a model that keeps asking to redo its own
/// work.
const LOOP_MAX_PLAN_REVISIONS: usize = 3;

/// The one literal marker `decide-pick`'s bounded rule checks for. This is a
/// fixed string comparison in MiniCon's own code, not model judgement --
/// finding the marker in a draft is a fact `decide-pick` reacts to
/// mechanically, same as `exec`'s allow-list checking a fixed name rather
/// than reasoning about intent.
const LOOP_REDRAFT_MARKER: &str = "PLAN: REDRAFT";

/// The one literal marker `decide-continue`'s bounded rule checks for. A
/// model that believes the task is done must say so with this exact phrase;
/// anything else is read as "not yet", bounded by `LOOP_MAX_ITERATIONS` so a
/// model that never says it is never trusted to loop forever either.
const LOOP_STOP_PHRASE: &str = "TASK COMPLETE";

/// One of `{LOOP}`'s five states. Every transition between them is logged as
/// a tree node (`run_loop`'s `trace`), so a session replay shows *why* the
/// loop moved on, not just what it did.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum LoopState {
    Categorize,
    Draft,
    DecidePick,
    Execute,
    DecideContinue,
}

impl LoopState {
    fn label(self) -> &'static str {
        match self {
            Self::Categorize => "categorize",
            Self::Draft => "draft",
            Self::DecidePick => "decide-pick",
            Self::Execute => "execute",
            Self::DecideContinue => "decide-continue",
        }
    }
}

/// Appends one transition node to `trace`, named so ids never collide across
/// iterations or states within one invocation.
fn log_transition(trace: &mut Vec<TreeNode>, iteration: usize, state: LoopState, note: &str) {
    trace.push(TreeNode {
        id: format!("loop-{iteration}-{}", state.label()),
        label: format!("state: {} -- {note}", state.label()),
        evidence: None,
        failure: None,
        children: Vec::new(),
    });
}

/// Drives `categorize -> draft -> decide-pick -> execute -> decide-continue`,
/// jumping back to `categorize` until `decide-continue`'s bounded rule ends
/// it. `call` is the backend closure `run_harness` builds; `{Draft}` and
/// `{Execute}` both go through it because the backend codec's own turn loop
/// (`harness_wire::run_task`/`harness_opencode::opencode_run_task`) already
/// performs tool-calling end to end in one round trip -- `{LOOP}` does not
/// reimplement that, it only decides when to call it again and with what
/// task text, per `{WIRE}`'s "both codecs stay dumb transports" rule.
fn run_loop(
    call: &mut dyn FnMut(&str) -> Result<String, String>,
    initial_task: &str,
    trace: &mut Vec<TreeNode>,
) -> Result<String, String> {
    let mut current_task = initial_task.to_owned();
    for iteration in 0..LOOP_MAX_ITERATIONS {
        log_transition(
            trace,
            iteration,
            LoopState::Categorize,
            "routed to the invocation's fixed --backend",
        );

        let mut revisions = 0usize;
        let mut draft = call(&current_task)?;
        log_transition(trace, iteration, LoopState::Draft, "model drafted a step");

        while draft.contains(LOOP_REDRAFT_MARKER) && revisions < LOOP_MAX_PLAN_REVISIONS {
            log_transition(
                trace,
                iteration,
                LoopState::DecidePick,
                &format!(
                    "reject, redraft {}/{LOOP_MAX_PLAN_REVISIONS}",
                    revisions + 1
                ),
            );
            revisions += 1;
            draft = call(&format!(
                "Your previous draft asked for a redraft. Try again for task:\n{current_task}"
            ))?;
        }
        log_transition(trace, iteration, LoopState::DecidePick, "accept");

        // Execution already happened inside `call` (the backend codec's own
        // tool loop); this state exists to log that fact as its own node,
        // not to run the tools a second time.
        log_transition(
            trace,
            iteration,
            LoopState::Execute,
            "ran via the backend's own file/exec tool loop",
        );

        let at_ceiling = iteration + 1 >= LOOP_MAX_ITERATIONS;
        let stopped = draft.contains(LOOP_STOP_PHRASE);
        if stopped || at_ceiling {
            let reason = if stopped {
                "stop phrase"
            } else {
                "iteration ceiling"
            };
            log_transition(
                trace,
                iteration,
                LoopState::DecideContinue,
                &format!("end ({reason})"),
            );
            return Ok(draft);
        }
        log_transition(trace, iteration, LoopState::DecideContinue, "jump");
        // Carries the ORIGINAL task forward every iteration, not just the
        // latest draft: a jump that dropped the initial instructions would
        // leave iteration 2+ with no idea what the task even is, since
        // `call` (the backend codec) is stateless across these calls -- it
        // is not the resumed-session fold (`compose_resumed_task`, a
        // different seam) which only runs across separate invocations.
        current_task = format!(
            "Original task:\n{initial_task}\n\nYour prior step's result:\n{draft}\n\nContinue \
             the task, building on that result."
        );
    }
    unreachable!("the ceiling branch above always returns before this point")
}

/// How many prior turns a resumed session carries forward. Bounded, like
/// every other harness limit: an unbounded transcript would eventually make
/// every resumed task prompt itself the thing that exhausts
/// `HARNESS_MAX_TURNS`/`OPENCODE_MAX_TURNS`. Oldest turns are dropped first.
const HARNESS_SESSION_MAX_TURNS: usize = 10;

/// Where session files live, relative to `--root` -- inside the `file` tool's
/// own bound, so session storage gets that tool's existing path-escape and
/// symlink protection for free instead of a second copy of it.
const HARNESS_SESSION_DIR: &str = ".minicon-harness-sessions";

/// Refuses a session id that is not a safe, flat filename component: no
/// separators, no `.`/`..`, nothing that could be read as a path.
fn validate_session_id(id: &str) -> Result<(), String> {
    let ok = !id.is_empty()
        && id.len() <= 64
        && id
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if ok {
        Ok(())
    } else {
        Err(format!(
            "harness: session id {id:?} is refused: use only ASCII letters, digits, `-` or \
             `_`, up to 64 characters"
        ))
    }
}

/// One node of the session's tree-DAG working memory
/// (`plan/plan-harness-context-engineering.md`, leaf `{CTX}`). Rendered as an
/// indented Markdown bullet list -- the same shape this repo's own planning
/// method uses -- so it is both what gets sent to the model and what a human
/// reading a session file sees, with no second representation to keep in
/// sync.
///
/// `evidence`/`failure` are optional single-token annotations (no spaces),
/// carried as trailing `@evidence=...`/`#failure=...` markers on the node's
/// line; either, both or neither may be present. There is no third "status"
/// field yet -- `{LOOP}`'s decision states will need one, deliberately left
/// for that leaf rather than guessed at here.
#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct TreeNode {
    pub id: String,
    pub label: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub evidence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<String>,
    #[serde(default)]
    pub children: Vec<TreeNode>,
}

/// A session's on-disk contents: `turns` is the flat history HS originally
/// shipped (kept so an existing 0.2.x session file still parses), `tree` is
/// the working memory this leaf adds. A file with no `tree` key at all --
/// every session saved before this leaf existed -- deserializes with
/// `tree: Vec::new()` via `#[serde(default)]`, never an error.
#[derive(Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
struct SessionState {
    turns: Vec<(String, String)>,
    #[serde(default)]
    tree: Vec<TreeNode>,
    /// `{PALACE}`'s Mermaid flowchart, recomputed and persisted alongside
    /// `tree` on every save -- never hand-edited, never read back into
    /// `tree` -- so a session file always carries a palace matching its own
    /// tree rather than one that can silently drift out of sync with it.
    /// `#[serde(default)]` so a pre-`{PALACE}` session file (no `tree`
    /// either, or `tree` without a `palace`) still deserializes.
    #[serde(default)]
    palace: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    gateway_binding: Option<crate::harness_gateway::Binding>,
}

/// Loads a session's state. `Ok(None)` means no session by this name exists
/// yet -- distinct from a corrupt file, which is a hard error: per this
/// leaf's safe-failure contract, a broken resume must never look like a
/// fresh start. A file written by HS before this leaf existed is a bare
/// `[[task, answer], ...]` JSON array rather than `{"turns": [...]}`; that
/// shape is tried second, not treated as corrupt.
fn load_session(file_tool: &FileTool, id: &str) -> Result<Option<SessionState>, String> {
    let relative = format!("{HARNESS_SESSION_DIR}/{id}.json");
    match file_tool.read(&relative) {
        Ok(contents) => {
            let state = serde_json::from_str::<SessionState>(&contents)
                .or_else(|_| {
                    serde_json::from_str::<Vec<(String, String)>>(&contents).map(|turns| {
                        SessionState {
                            turns,
                            ..Default::default()
                        }
                    })
                })
                .map_err(|error| {
                    format!(
                        "harness: session {id:?} is corrupt ({error}); refusing to fall back to a \
                     silent fresh start -- move or delete {relative:?} under --root to abandon it"
                    )
                })?;
            Ok(Some(state))
        }
        Err(_) => Ok(None),
    }
}

/// Persists a session's state, keeping only the most recent
/// `HARNESS_SESSION_MAX_TURNS` of both `turns` and top-level `tree` nodes --
/// the two stay the same length by construction (`run_harness` appends one of
/// each per turn), so this bounds them identically rather than risking them
/// drifting apart.
fn save_session(file_tool: &FileTool, id: &str, state: &SessionState) -> Result<(), String> {
    let turns_start = state.turns.len().saturating_sub(HARNESS_SESSION_MAX_TURNS);
    let tree_start = state.tree.len().saturating_sub(HARNESS_SESSION_MAX_TURNS);
    let bounded_tree = state.tree[tree_start..].to_vec();
    let bounded = SessionState {
        turns: state.turns[turns_start..].to_vec(),
        palace: render_palace(&bounded_tree),
        tree: bounded_tree,
        gateway_binding: state.gateway_binding.clone(),
    };
    let relative = format!("{HARNESS_SESSION_DIR}/{id}.json");
    let contents = serde_json::to_string(&bounded)
        .map_err(|error| format!("harness: could not serialize session {id:?}: {error}"))?;
    file_tool.write(&relative, &contents)?;
    Ok(())
}

/// Builds this turn's tree node: the task, `{LOOP}`'s own state-transition
/// trace as children (empty for a session upgraded from before `{LOOP}`
/// existed), then the final answer as the last child -- named so
/// `turn-{index}`/`turn-{index}-answer` never collides with a different
/// turn's ids within one session.
fn turn_node(index: usize, task: &str, trace: Vec<TreeNode>, answer: &str) -> TreeNode {
    let mut children = trace;
    children.push(TreeNode {
        id: format!("turn-{index}-answer"),
        label: format!("assistant: {answer}"),
        evidence: None,
        failure: None,
        children: Vec::new(),
    });
    TreeNode {
        id: format!("turn-{index}"),
        label: format!("task: {task}"),
        evidence: None,
        failure: None,
        children,
    }
}

/// Rebuilds a tree from a pre-`{CTX}` session's flat turns, so an older
/// session file upgrades deterministically instead of losing its history the
/// first time it is resumed after this leaf landed.
fn tree_from_turns(turns: &[(String, String)]) -> Vec<TreeNode> {
    turns
        .iter()
        .enumerate()
        .map(|(index, (task, answer))| turn_node(index, task, Vec::new(), answer))
        .collect()
}

/// Renders a tree as the indented Markdown bullet list `parse_tree` reads
/// back. Two spaces per depth level, an `{id}` tag right after the bullet so
/// a node's identity survives the round trip, then its label, then any
/// `evidence`/`failure` annotation as a trailing token.
fn render_tree(nodes: &[TreeNode]) -> String {
    let mut out = String::new();
    render_tree_at(nodes, 0, &mut out);
    out
}

fn render_tree_at(nodes: &[TreeNode], depth: usize, out: &mut String) {
    for node in nodes {
        out.push_str(&"  ".repeat(depth));
        out.push_str("- {");
        out.push_str(&node.id);
        out.push_str("} ");
        out.push_str(&node.label);
        if let Some(evidence) = &node.evidence {
            out.push_str(" @evidence=");
            out.push_str(evidence);
        }
        if let Some(failure) = &node.failure {
            out.push_str(" #failure=");
            out.push_str(failure);
        }
        out.push('\n');
        render_tree_at(&node.children, depth + 1, out);
    }
}

/// Parses `render_tree`'s own output back into a tree. Indentation must be a
/// whole number of 2-space levels and a child's depth must have a parent
/// already open above it -- both violations are named, bounded errors, never
/// a best-effort guess at the intended shape, since this is working memory a
/// resumed task's next turn reasons over: a silently misparsed tree is a
/// silently wrong task, not merely a cosmetic one.
///
/// Not yet called from `run_harness` -- the tree is stored and reloaded as
/// JSON, never as this Markdown rendering, so nothing needs to parse it back
/// today. It exists now because `{CTX}`'s own evidence contract is the
/// render/parse round trip itself (`plan/plan-harness-context-engineering.md`),
/// and `{LOOP}` will need this exact parser to read a model's tree edits back
/// off the wire.
#[allow(dead_code)]
fn parse_tree(text: &str) -> Result<Vec<TreeNode>, String> {
    let mut roots: Vec<TreeNode> = Vec::new();
    let mut stack: Vec<(usize, TreeNode)> = Vec::new();
    for raw_line in text.lines() {
        if raw_line.trim().is_empty() {
            continue;
        }
        let indent = raw_line.chars().take_while(|c| *c == ' ').count();
        if indent % 2 != 0 {
            return Err(format!(
                "harness: tree line has odd indentation ({indent} spaces), expected a multiple \
                 of 2: {raw_line:?}"
            ));
        }
        let depth = indent / 2;
        let node = parse_tree_line(raw_line.trim_start())?;

        while let Some(&(top_depth, _)) = stack.last() {
            if top_depth < depth {
                break;
            }
            let (_, finished) = stack.pop().expect("just peeked");
            match stack.last_mut() {
                Some((_, parent)) => parent.children.push(finished),
                None => roots.push(finished),
            }
        }
        if depth > 0 && stack.is_empty() {
            return Err(format!(
                "harness: tree line is indented with no parent line above it: {raw_line:?}"
            ));
        }
        stack.push((depth, node));
    }
    while let Some((_, finished)) = stack.pop() {
        match stack.last_mut() {
            Some((_, parent)) => parent.children.push(finished),
            None => roots.push(finished),
        }
    }
    Ok(roots)
}

/// Parses one already-trimmed tree line's `- {id} label [@evidence=x] [#failure=y]`.
fn parse_tree_line(line: &str) -> Result<TreeNode, String> {
    let rest = line
        .strip_prefix("- ")
        .ok_or_else(|| format!("harness: tree line is not a `- ` bullet: {line:?}"))?;
    let rest = rest
        .strip_prefix('{')
        .ok_or_else(|| format!("harness: tree line has no {{id}} tag: {line:?}"))?;
    let (id, rest) = rest
        .split_once('}')
        .ok_or_else(|| format!("harness: tree line's {{id}} tag is never closed: {line:?}"))?;
    if id.is_empty() {
        return Err(format!(
            "harness: tree line has an empty {{id}} tag: {line:?}"
        ));
    }
    let mut tokens: Vec<&str> = rest.split(' ').filter(|token| !token.is_empty()).collect();
    let mut evidence = None;
    let mut failure = None;
    while let Some(&last) = tokens.last() {
        if let Some(value) = last.strip_prefix("@evidence=") {
            evidence = Some(value.to_owned());
            tokens.pop();
        } else if let Some(value) = last.strip_prefix("#failure=") {
            failure = Some(value.to_owned());
            tokens.pop();
        } else {
            break;
        }
    }
    Ok(TreeNode {
        id: id.to_owned(),
        label: tokens.join(" "),
        evidence,
        failure,
        children: Vec::new(),
    })
}

/// Renders `{PALACE}`'s Mermaid flowchart for a tree: one node per
/// `TreeNode`, in the tree's own `{id}` namespace (so a tree leaf and its
/// palace node are the same identity, never two names for one fact, per
/// `plan/plan-harness-context-engineering.md`'s `{PALACE}` dependency on
/// `{CTX}`), plus one edge per tree parent/child relationship. This is the
/// whole palace for now -- shared-prerequisite and kill-path edges beyond
/// the tree's own hierarchy are `{LOOP}`'s job to add once there is a
/// decision state that can mark one, not invented here ahead of that
/// dependency.
fn render_palace(tree: &[TreeNode]) -> String {
    let mut out = String::from("flowchart TD\n");
    render_palace_nodes(tree, &mut out);
    render_palace_edges(tree, &mut out);
    out
}

fn render_palace_nodes(nodes: &[TreeNode], out: &mut String) {
    for node in nodes {
        out.push_str("    ");
        out.push_str(&node.id);
        out.push_str("[\"");
        out.push_str(&node.label.replace('"', "'"));
        out.push_str("\"]\n");
        render_palace_nodes(&node.children, out);
    }
}

fn render_palace_edges(nodes: &[TreeNode], out: &mut String) {
    for node in nodes {
        for child in &node.children {
            out.push_str("    ");
            out.push_str(&node.id);
            out.push_str(" --> ");
            out.push_str(&child.id);
            out.push('\n');
        }
        render_palace_edges(&node.children, out);
    }
}

/// Extracts every node id `render_palace` declared, in declaration order --
/// the counterpart evidence needs to assert the palace's node ids match the
/// tree's `{id}`-tagged nodes 1:1 in both directions, without re-parsing
/// Mermaid syntax generally (this repo's palace is one fixed shape it wrote
/// itself, not an arbitrary diagram to interpret).
#[cfg(test)]
fn palace_node_ids(rendered: &str) -> Vec<String> {
    rendered
        .lines()
        .filter_map(|line| {
            let trimmed = line.trim();
            let (id, _) = trimmed.split_once('[')?;
            if id.is_empty() || id.contains("-->") {
                None
            } else {
                Some(id.to_owned())
            }
        })
        .collect()
}

#[cfg(test)]
fn tree_ids(nodes: &[TreeNode], out: &mut Vec<String>) {
    for node in nodes {
        out.push(node.id.clone());
        tree_ids(&node.children, out);
    }
}

/// Folds a session's tree working memory into the text sent as this run's
/// task, so neither backend codec (`harness_wire`/`harness_opencode`) needs
/// to know sessions or trees exist at all -- a plain composed string
/// round-trips through both unchanged, which is the dependency this leaf's
/// PRD entry named (`{WIRE}` in the context-engineering plan). Raw
/// message-object replay was considered and rejected: it would couple the
/// stored format to one backend's wire shape.
fn compose_resumed_task(tree: &[TreeNode], new_task: &str) -> String {
    if tree.is_empty() {
        return new_task.to_owned();
    }
    let mut composed = String::from("Resumed session -- working memory as a tree, oldest first:\n");
    composed.push_str(&render_tree(tree));
    composed.push_str("\nNew task:\n");
    composed.push_str(new_task);
    composed
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
    let mut gateway = None;
    let mut client_key_env = None;
    let mut allow_cmd = Vec::new();
    let mut new_session: Option<String> = None;
    let mut continue_session: Option<String> = None;
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
            "--gateway" => gateway = Some(crate::harness_gateway::validate_url(&value()?)?),
            "--client-key-env" => {
                let name = value()?;
                crate::harness_gateway::validate_env_name(&name)?;
                client_key_env = Some(name);
            }
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
            "--session" => new_session = Some(value()?),
            "--continue" => continue_session = Some(value()?),
            other => {
                return Err(format!(
                    "unexpected argument {other:?}\n{}",
                    harness_usage()
                ));
            }
        }
    }
    if new_session.is_some() && continue_session.is_some() {
        return Err(format!(
            "--session and --continue are mutually exclusive: --session starts a new one, \
             --continue resumes an existing one\n{}",
            harness_usage()
        ));
    }
    if gateway.is_some() && backend.is_some() {
        return Err("harness: --gateway and explicit --backend are mutually exclusive".into());
    }
    if gateway.is_none() && client_key_env.is_some() {
        return Err("harness: --client-key-env requires --gateway".into());
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
        gateway,
        client_key_env: client_key_env.unwrap_or_else(|| "MINICON_LOCAL_API_KEY".into()),
        allow_cmd,
        new_session,
        continue_session,
    })
}

fn harness_usage() -> String {
    "usage: minicon harness --root PATH --task TEXT [--backend deepseek|opencode-go | \
     --gateway http://127.0.0.1:PORT/v1 [--client-key-env NAME]] \
     [--allow-cmd NAME]... [--session ID | --continue ID]"
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
            minicon_platform::filesystem_read::read_bounded(&path, FILE_TOOL_MAX_READ_BYTES)
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
        minicon_platform::filesystem_publish::write_file_atomic(&path, |file| {
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

/// How long a killed child is given to actually leave after
/// `terminate_and_wait`, on top of `EXEC_TOOL_TIMEOUT` itself.
const EXEC_TOOL_KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

pub const EXEC_TOOL_MAX_OUTPUT_BYTES: usize = 1024 * 1024;
const EXEC_TOOL_TRUNCATION_MARKER: &str = "\n[exec: combined output truncated]\n";

/// Exact inherited names permitted for exec. Provider/client/OAuth variables,
/// proxies, loader hooks and shell startup variables are deliberately absent.
/// HOME/USERPROFILE locate ordinary native user configuration; this is an
/// environment boundary, not filesystem isolation from the current user.
const EXEC_TOOL_ENVIRONMENT: &[&str] = &[
    "PATH",
    "SystemRoot",
    "WINDIR",
    "COMSPEC",
    "PATHEXT",
    "USERPROFILE",
    "HOME",
    "TMP",
    "TEMP",
    "TMPDIR",
    "LANG",
    "LC_ALL",
    "LC_CTYPE",
    "TZ",
];

/// Hard native ceilings installed on every contained `exec` child, so a
/// bounded local task cannot exhaust host memory, disk or process slots
/// even if the allow-listed command itself is misbehaving.
#[cfg_attr(target_os = "macos", allow(dead_code))]
const EXEC_TOOL_MEMORY_BYTES: u64 = 512 * 1024 * 1024;
const EXEC_TOOL_FILE_SIZE_BYTES: u64 = 64 * 1024 * 1024;
const EXEC_TOOL_OPEN_FILES: u64 = 256;
const EXEC_TOOL_ACTIVE_PROCESSES: u32 = 32;

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
    cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    deadline: Option<std::time::Instant>,
}

impl ExecTool {
    pub fn new(root: &std::path::Path, allow_cmd: &[String]) -> Self {
        Self {
            root: root.to_path_buf(),
            allow_cmd: allow_cmd.to_vec(),
            cancelled: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            deadline: None,
        }
    }

    /// Shares an invocation owner's cancellation signal without changing the
    /// existing constructor or codec callers. Cancellation is terminal for an
    /// exec call and never returns a successful partial tool result.
    pub fn with_cancellation(
        mut self,
        cancelled: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> Self {
        self.cancelled = cancelled;
        self
    }

    fn with_deadline(mut self, deadline: std::time::Instant) -> Self {
        self.deadline = Some(deadline);
        self
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
    /// Uses `minicon_platform::contained_process::ContainedHeadlessCommand`
    /// (the crate's `contained-process-spawn` feature, enabled in
    /// `Cargo.toml`), not `std::process::Command`. This keeps the same
    /// argv-vector, no-shell guarantee H3's invariant is about and adds the
    /// resource containment the design of record actually asked for: a
    /// Windows kill-on-close Job Object or a Unix process group owns the
    /// child's whole descendant tree, so a runaway grandchild cannot outlive
    /// this call's own timeout kill.
    fn spawn_contained(&self, program: &str, args: &[String]) -> Result<ExecOutcome, String> {
        self.spawn_with_timeout(program, args, EXEC_TOOL_TIMEOUT)
    }

    fn spawn_with_timeout(
        &self,
        program: &str,
        args: &[String],
        timeout: std::time::Duration,
    ) -> Result<ExecOutcome, String> {
        use minicon_platform::contained_process::{
            ContainedHeadlessCommand, ContainedProcessLimits,
        };

        if self.cancelled.load(std::sync::atomic::Ordering::Acquire) {
            return Err("exec: cancelled before spawn".to_owned());
        }
        let exec_deadline = std::time::Instant::now() + timeout;
        let deadline = self
            .deadline
            .map_or(exec_deadline, |deadline| deadline.min(exec_deadline));
        if std::time::Instant::now() >= deadline {
            return Err("exec: invocation deadline exhausted before spawn".to_owned());
        }

        let mut command = ContainedHeadlessCommand::new(program);
        command
            .args(args.iter().cloned())
            .current_dir(&self.root)
            .env_clear()
            .capture_output()
            .limits(ContainedProcessLimits {
                cpu_seconds: Some(EXEC_TOOL_TIMEOUT.as_secs()),
                // minicon_platform's macOS contained_process refuses any
                // `memory_bytes` limit outright (RLIMIT_AS cannot be set
                // below the process-wide dyld mapping there), so asking for
                // one made every spawn fail on macOS. Other limits are still
                // enforced there.
                #[cfg(target_os = "macos")]
                memory_bytes: None,
                #[cfg(not(target_os = "macos"))]
                memory_bytes: Some(EXEC_TOOL_MEMORY_BYTES),
                file_size_bytes: cfg!(unix).then_some(EXEC_TOOL_FILE_SIZE_BYTES),
                open_files: cfg!(unix).then_some(EXEC_TOOL_OPEN_FILES),
                active_processes: Some(EXEC_TOOL_ACTIVE_PROCESSES),
            });
        for name in EXEC_TOOL_ENVIRONMENT {
            if let Some(value) = std::env::var_os(name) {
                command.env(*name, value);
            }
        }

        let mut child = command
            .spawn()
            .map_err(|error| format!("exec: {program:?} could not be started: {error}"))?;

        // No blocking reader workers: the same deadline owns root wait and
        // both pipes, even if the leader exits while a descendant holds them.
        // At most one chunk per stream per pass keeps stderr and cancellation
        // responsive under an infinite stdout flood.
        let mut stdout = child.take_stdout();
        let mut stderr = child.take_stderr();
        let mut capture = ExecCapture::default();
        let mut timed_out = false;
        let mut exited = false;
        let mut failure = None;
        loop {
            if self.cancelled.load(std::sync::atomic::Ordering::Acquire) {
                failure = Some("exec: cancelled".to_owned());
                break;
            }
            if std::time::Instant::now() >= deadline {
                timed_out = true;
                break;
            }
            let observed = (|| -> Result<bool, String> {
                if !exited {
                    exited = child
                        .exit_observed()
                        .map_err(|error| format!("exec: wait failed: {error}"))?;
                }
                let out = capture.poll(&mut stdout, false)?;
                let err = capture.poll(&mut stderr, true)?;
                Ok(out || err)
            })();
            match observed {
                Err(error) => {
                    failure = Some(error);
                    break;
                }
                Ok(_) if exited && stdout.is_none() && stderr.is_none() => break,
                Ok(false) => std::thread::sleep(std::time::Duration::from_millis(1)),
                Ok(true) => {}
            }
        }
        // Terminate descendants on every terminal path, including successful
        // leader exit. Reap and pipe closure share one further five-second
        // cleanup allowance; there is no thread join or read left to finish.
        let cleanup = child.terminate_and_wait(EXEC_TOOL_KILL_GRACE);
        let status = child.try_wait();
        drop(stdout);
        drop(stderr);
        drop(child);
        if let Err(error) = cleanup {
            return Err(format!(
                "{}; exec: cleanup failed: {error}",
                failure.as_deref().unwrap_or("exec: command finished")
            ));
        }
        if let Some(error) = failure {
            return Err(error);
        }
        let status = status.map_err(|error| format!("exec: final wait failed: {error}"))?;
        let (stdout, stderr) = capture.finish();
        Ok(ExecOutcome {
            exit_code: if timed_out {
                None
            } else {
                status.and_then(|status| status.conventional_code().map(|code| code as i32))
            },
            stdout,
            stderr,
            timed_out,
        })
    }
}

#[derive(Default)]
struct ExecCapture {
    stdout: Vec<u8>,
    stderr: Vec<u8>,
    truncated: bool,
}

impl ExecCapture {
    fn poll(
        &mut self,
        stream: &mut Option<minicon_platform::contained_process::ContainedChildOutput>,
        is_stderr: bool,
    ) -> Result<bool, String> {
        let Some(source) = stream else {
            return Ok(false);
        };
        let mut chunk = [0; 8192];
        match source.read_timeout(&mut chunk, std::time::Duration::ZERO) {
            Ok(0) => {
                *stream = None;
                Ok(false)
            }
            Ok(count) => {
                self.append(&chunk[..count], is_stderr);
                Ok(true)
            }
            Err(error)
                if matches!(
                    error.kind(),
                    std::io::ErrorKind::TimedOut
                        | std::io::ErrorKind::Interrupted
                        | std::io::ErrorKind::WouldBlock
                ) =>
            {
                Ok(false)
            }
            Err(error) => Err(format!("exec: output read failed: {error}")),
        }
    }

    fn append(&mut self, bytes: &[u8], is_stderr: bool) {
        let remaining =
            EXEC_TOOL_MAX_OUTPUT_BYTES.saturating_sub(self.stdout.len() + self.stderr.len());
        let keep = remaining.min(bytes.len());
        self.truncated |= keep < bytes.len();
        let target = if is_stderr {
            &mut self.stderr
        } else {
            &mut self.stdout
        };
        target.extend_from_slice(&bytes[..keep]);
    }

    fn finish(self) -> (String, String) {
        let mut stdout = String::from_utf8_lossy(&self.stdout).into_owned();
        let mut stderr = String::from_utf8_lossy(&self.stderr).into_owned();
        if self.truncated || stdout.len() + stderr.len() > EXEC_TOOL_MAX_OUTPUT_BYTES {
            // Account for UTF-8 replacement expansion and the marker itself.
            let available = EXEC_TOOL_MAX_OUTPUT_BYTES - EXEC_TOOL_TRUNCATION_MARKER.len();
            truncate_utf8(&mut stdout, available);
            truncate_utf8(&mut stderr, available - stdout.len());
            stderr.push_str(EXEC_TOOL_TRUNCATION_MARKER);
        }
        (stdout, stderr)
    }
}

fn truncate_utf8(text: &mut String, max_bytes: usize) {
    let mut end = text.len().min(max_bytes);
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
}

#[cfg(test)]
mod invocation_budget_tests {
    use super::*;
    use crate::harness_wire::Transport;
    use std::{
        cell::{Cell, RefCell},
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };

    enum ReplyMode {
        Redraft,
        WriteBatch,
        Cancel,
    }

    struct ModelFixture {
        mode: ReplyMode,
        requests: Cell<usize>,
        offered_tools: Cell<usize>,
        timeout: RefCell<Option<Duration>>,
        cancellation: Arc<AtomicBool>,
    }

    impl ModelFixture {
        fn new(mode: ReplyMode, cancellation: Arc<AtomicBool>) -> Self {
            Self {
                mode,
                requests: Cell::new(0),
                offered_tools: Cell::new(0),
                timeout: RefCell::new(None),
                cancellation,
            }
        }
    }

    fn text_reply(text: &str) -> String {
        serde_json::json!({"choices":[{"message":{"role":"assistant","content":text}}]}).to_string()
    }

    impl Transport for ModelFixture {
        fn post_json(
            &self,
            _url: &str,
            _bearer: &str,
            body: &str,
            _headers: &[(&str, &str)],
        ) -> Result<String, String> {
            self.requests.set(self.requests.get() + 1);
            match self.mode {
                ReplyMode::Redraft => Ok(text_reply(LOOP_REDRAFT_MARKER)),
                ReplyMode::WriteBatch | ReplyMode::Cancel => {
                    let request: serde_json::Value = serde_json::from_str(body).unwrap();
                    if request["messages"].as_array().unwrap().last().unwrap()["role"] == "tool" {
                        return Ok(text_reply("step finished"));
                    }
                    let count = if matches!(self.mode, ReplyMode::Cancel) {
                        1
                    } else {
                        16
                    };
                    let start = self.offered_tools.get();
                    self.offered_tools.set(start + count);
                    let calls: Vec<_> = (start..start + count).map(|index| serde_json::json!({
                        "id":format!("call-{index}"), "type":"function", "function":{
                            "name":"file", "arguments":serde_json::json!({"op":"write","path":format!("effect-{index}"),"contents":"bounded effect"}).to_string()
                        }
                    })).collect();
                    if matches!(self.mode, ReplyMode::Cancel) {
                        self.cancellation.store(true, Ordering::Release);
                    }
                    Ok(serde_json::json!({"choices":[{"message":{"role":"assistant","content":null,"tool_calls":calls}}]}).to_string())
                }
            }
        }

        fn post_json_with_timeout(
            &self,
            url: &str,
            bearer: &str,
            body: &str,
            headers: &[(&str, &str)],
            timeout: Duration,
        ) -> Result<String, String> {
            *self.timeout.borrow_mut() = Some(timeout);
            self.post_json(url, bearer, body, headers)
        }
    }

    fn tools(tag: &str, backend: Backend) -> (std::path::PathBuf, FileTool, ExecTool) {
        let root = std::env::temp_dir().join(format!(
            "minicon-budget-{tag}-{}-{}-{:?}",
            backend.flag_name(),
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&root).unwrap();
        let root = root.canonicalize().unwrap();
        let file = FileTool::new(root.to_str().unwrap()).unwrap();
        let exec = ExecTool::new(&root, &[]);
        (root, file, exec)
    }

    fn call(
        backend: Backend,
        model: &dyn Transport,
        budget: &InvocationBudget,
        file: &FileTool,
        exec: &ExecTool,
        task: &str,
    ) -> Result<String, HarnessFailure> {
        match backend {
            Backend::DeepSeek => crate::harness_wire::run_task_with_budget(
                model,
                "http://127.0.0.1/fixture",
                "synthetic",
                "fixture",
                task,
                file,
                exec,
                budget,
            ),
            Backend::OpencodeGo => crate::harness_opencode::opencode_run_task_with_budget(
                model,
                "http://127.0.0.1/fixture",
                "synthetic",
                "fixture",
                task,
                file,
                exec,
                budget,
            ),
        }
    }

    fn through_outer_loop(
        backend: Backend,
        model: &dyn Transport,
        budget: &InvocationBudget,
        file: &FileTool,
        exec: &ExecTool,
    ) -> InvocationStop {
        let stopped = Cell::new(None);
        let mut call = |task: &str| {
            call(backend, model, budget, file, exec, task).map_err(|error| {
                if let HarnessFailure::Stopped(stop) = &error {
                    stopped.set(Some(*stop));
                }
                error.to_string()
            })
        };
        run_loop(&mut call, "bounded task", &mut Vec::new())
            .expect_err("outer re-entry must exhaust the shared budget");
        stopped
            .get()
            .expect("a typed fatal stop, not a codec or tool string")
    }

    #[test]
    fn global_model_turn_limit_survives_codec_and_outer_loop_reentry() {
        for backend in [Backend::DeepSeek, Backend::OpencodeGo] {
            let (root, file, exec) = tools("turns", backend);
            let cancel = Arc::new(AtomicBool::new(false));
            let budget = InvocationBudget::new(cancel.clone());
            let model = ModelFixture::new(ReplyMode::Redraft, cancel);
            assert_eq!(
                through_outer_loop(backend, &model, &budget, &file, &exec),
                InvocationStop::ModelTurns
            );
            assert_eq!(model.requests.get(), 16);
            assert_eq!(budget.model_turns.get(), 16);
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn global_tool_budget_blocks_the_sixty_fifth_effect_across_reentry() {
        for backend in [Backend::DeepSeek, Backend::OpencodeGo] {
            let (root, file, exec) = tools("tools", backend);
            let cancel = Arc::new(AtomicBool::new(false));
            let budget = InvocationBudget::new(cancel.clone());
            let model = ModelFixture::new(ReplyMode::WriteBatch, cancel);
            assert_eq!(
                through_outer_loop(backend, &model, &budget, &file, &exec),
                InvocationStop::ToolCalls
            );
            assert_eq!(budget.tool_calls.get(), 64);
            for index in 0..64 {
                assert_eq!(
                    std::fs::read_to_string(root.join(format!("effect-{index}"))).unwrap(),
                    "bounded effect"
                );
            }
            assert!(
                !root.join("effect-64").exists(),
                "tool 65 ran after budget exhaustion"
            );
            assert_eq!(
                model.requests.get(),
                9,
                "fatal exhaustion must not be fed back as another model turn"
            );
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn cancelled_or_expired_invocations_make_no_model_request() {
        for backend in [Backend::DeepSeek, Backend::OpencodeGo] {
            for expired in [false, true] {
                let (root, file, exec) =
                    tools(if expired { "expired" } else { "cancelled" }, backend);
                let cancel = Arc::new(AtomicBool::new(!expired));
                let mut budget = InvocationBudget::new(cancel.clone());
                if expired {
                    budget.deadline = Instant::now();
                }
                let model = ModelFixture::new(ReplyMode::WriteBatch, cancel);
                let error =
                    call(backend, &model, &budget, &file, &exec, "refused task").unwrap_err();
                assert!(
                    matches!(error, HarnessFailure::Stopped(stop) if stop == if expired { InvocationStop::Deadline } else { InvocationStop::Cancelled })
                );
                assert_eq!(model.requests.get(), 0);
                assert_eq!(budget.tool_calls.get(), 0);
                assert!(!root.join("effect-0").exists());
                std::fs::remove_dir_all(root).unwrap();
            }
        }
    }

    #[test]
    fn cancellation_after_inference_is_fatal_before_any_tool_effect() {
        for backend in [Backend::DeepSeek, Backend::OpencodeGo] {
            let (root, file, exec) = tools("cancel-reply", backend);
            let cancel = Arc::new(AtomicBool::new(false));
            let budget = InvocationBudget::new(cancel.clone());
            let model = ModelFixture::new(ReplyMode::Cancel, cancel);
            let error = call(
                backend,
                &model,
                &budget,
                &file,
                &exec,
                "cancel during model request",
            )
            .unwrap_err();
            assert!(matches!(
                error,
                HarnessFailure::Stopped(InvocationStop::Cancelled)
            ));
            assert_eq!(model.requests.get(), 1);
            assert_eq!(budget.tool_calls.get(), 0);
            assert!(!root.join("effect-0").exists());
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn each_codec_passes_only_remaining_invocation_time_to_transport() {
        for backend in [Backend::DeepSeek, Backend::OpencodeGo] {
            let (root, file, exec) = tools("remaining", backend);
            let cancel = Arc::new(AtomicBool::new(false));
            let mut budget = InvocationBudget::new(cancel.clone());
            let model = ModelFixture::new(ReplyMode::Redraft, cancel);
            budget.deadline = Instant::now() + Duration::from_secs(1);
            call(backend, &model, &budget, &file, &exec, "short deadline").unwrap();
            assert!(model.timeout.borrow().is_some_and(
                |remaining| remaining > Duration::ZERO && remaining <= Duration::from_secs(1)
            ));
            std::fs::remove_dir_all(root).unwrap();
        }
    }

    #[test]
    fn native_http_exchange_cannot_outlive_remaining_invocation_time() {
        use std::{
            io::{Read as _, Write as _},
            net::TcpListener,
        };
        let (root, file, exec) = tools("native-http", Backend::DeepSeek);
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}/fixture", listener.local_addr().unwrap());
        listener.set_nonblocking(true).unwrap();
        let peer = std::thread::spawn(move || {
            let until = Instant::now() + Duration::from_secs(5);
            let mut socket = loop {
                match listener.accept() {
                    Ok((socket, _)) => break socket,
                    Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                        assert!(Instant::now() < until, "native HTTP client did not connect");
                        std::thread::sleep(Duration::from_millis(1));
                    }
                    Err(error) => panic!("accept failed: {error}"),
                }
            };
            socket.set_nonblocking(false).unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            socket
                .set_write_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut head = Vec::new();
            let mut byte = [0];
            while !head.ends_with(b"\r\n\r\n") {
                assert!(head.len() < 16384);
                assert_eq!(socket.read(&mut byte).unwrap(), 1);
                head.push(byte[0]);
            }
            let reply = text_reply("late answer");
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                reply.len()
            )
            .unwrap();
            socket.flush().unwrap();
            std::thread::sleep(Duration::from_secs(2));
            let _ = socket.write_all(reply.as_bytes()); // Client has already cancelled its deadline.
        });
        let mut budget = InvocationBudget::new(Arc::new(AtomicBool::new(false)));
        budget.deadline = Instant::now() + Duration::from_millis(200);
        let start = Instant::now();
        let result = crate::harness_wire::run_task_with_budget(
            &crate::harness_wire::NetworkHttp,
            &url,
            "synthetic",
            "fixture",
            "bounded HTTP",
            &file,
            &exec,
            &budget,
        );
        let elapsed = start.elapsed();
        peer.join().unwrap();
        assert!(
            matches!(
                result,
                Err(HarnessFailure::Stopped(InvocationStop::Deadline))
            ),
            "{result:?}"
        );
        assert!(
            elapsed < Duration::from_secs(1),
            "native HTTP ignored remaining deadline: {elapsed:?}"
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}

#[cfg(test)]
mod worker_exec_tests {
    use super::*;
    use std::{
        io::Write as _,
        sync::{
            Arc,
            atomic::{AtomicBool, Ordering},
        },
        time::{Duration, Instant},
    };

    const PROBE: &str = "harness::worker_exec_tests::exec_probe_child";
    const SECRETS: &[&str] = &[
        "MINICON_DEEPSEEK_API_KEY",
        "MINICON_OPENCODE_API_KEY",
        "OPENAI_API_KEY",
        "ANTHROPIC_API_KEY",
        "MINICON_LOCAL_CLIENT_TOKEN",
        "MINICON_OAUTH_STATE",
        "CUSTOM_SECRET",
    ];

    fn fixture(tag: &str, mode: &str) -> std::path::PathBuf {
        let path = std::env::temp_dir().join(format!(
            "minicon-worker-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        std::fs::create_dir_all(&path).unwrap();
        let path = path.canonicalize().unwrap();
        std::fs::write(path.join("worker-probe-mode"), mode).unwrap();
        path
    }

    fn probe_args() -> Vec<String> {
        ["--exact", PROBE, "--nocapture", "--test-threads=1"]
            .map(str::to_owned)
            .to_vec()
    }

    /// Runs only inside a deliberately selected fresh cwd. The parent tests
    /// launch this same executable, so fixture setup needs no shell/compiler.
    #[test]
    fn exec_probe_child() {
        let Ok(mode) = std::fs::read_to_string("worker-probe-mode") else {
            return;
        };
        match mode.as_str() {
            "flood" => {
                let mut out = std::io::stdout().lock();
                let mut err = std::io::stderr().lock();
                for _ in 0..256 {
                    out.write_all(&[b'o'; 8192]).unwrap();
                    err.write_all(&[b'e'; 8192]).unwrap();
                }
            }
            "leader" => {
                let nested = std::env::current_dir().unwrap().join("descendant");
                std::fs::create_dir_all(&nested).unwrap();
                std::fs::write(nested.join("worker-probe-mode"), "hold").unwrap();
                // This controlled leader must exit before its finite child.
                // The outer court owns the native identity and proves the
                // containment group kills it after leader exit.
                #[allow(clippy::zombie_processes)]
                let mut descendant = std::process::Command::new(std::env::current_exe().unwrap())
                    .args(probe_args())
                    .current_dir(&nested)
                    .spawn()
                    .unwrap();
                // Keep the native child handle alive until the root exits.
                // Inherit both stdout and stderr, so the parent must not treat
                // leader exit as completion of output draining.
                let until = Instant::now() + Duration::from_secs(5);
                while !nested.join("ready").is_file() {
                    assert!(
                        descendant.try_wait().unwrap().is_none(),
                        "descendant exited before ready"
                    );
                    assert!(Instant::now() < until, "descendant setup expired");
                    std::thread::sleep(Duration::from_millis(1));
                }
                std::fs::write("leader-ready", "ready").unwrap();
            }
            "hold" => {
                let pid = std::process::id();
                let identity = minicon_platform::process_observation::start_identity(pid).unwrap();
                std::fs::write("ready", serde_json::to_vec(&(pid, identity)).unwrap()).unwrap();
                // Finite even if the termination guard is mutated away.
                std::thread::sleep(Duration::from_secs(6));
            }
            "environment-parent" => {
                for name in SECRETS {
                    assert_eq!(std::env::var(name).unwrap(), "synthetic-secret-sentinel");
                }
                let nested = std::env::current_dir().unwrap().join("child");
                std::fs::create_dir_all(&nested).unwrap();
                std::fs::write(nested.join("worker-probe-mode"), "environment-child").unwrap();
                let exe = std::env::current_exe().unwrap();
                let basename = exe.file_name().unwrap().to_str().unwrap();
                let tool = ExecTool::new(&nested, &[basename.to_owned()]);
                let outcome = tool.run(basename, &probe_args()).unwrap();
                assert_eq!(outcome.exit_code, Some(0), "{}", outcome.stderr);
                assert!(outcome.stdout.contains("minimal-environment-verified"));
            }
            "environment-child" => {
                for name in SECRETS {
                    assert!(std::env::var_os(name).is_none(), "forwarded {name}");
                }
                for (name, _) in std::env::vars_os() {
                    assert!(
                        EXEC_TOOL_ENVIRONMENT
                            .iter()
                            .any(|allowed| name.to_string_lossy().eq_ignore_ascii_case(allowed)),
                        "unexpected child environment key {name:?}"
                    );
                }
                assert!(std::env::var_os("PATH").is_some());
                println!("minimal-environment-verified");
            }
            other => panic!("unknown probe {other}"),
        }
    }

    #[test]
    fn combined_output_flood_is_capped_and_marked() {
        let root = fixture("flood", "flood");
        let tool = ExecTool::new(&root, &[]);
        let outcome = tool
            .spawn_contained(
                std::env::current_exe().unwrap().to_str().unwrap(),
                &probe_args(),
            )
            .unwrap();
        assert_eq!(outcome.exit_code, Some(0));
        assert!(!outcome.timed_out);
        assert!(outcome.stdout.contains("oooo"));
        assert!(
            outcome.stderr.contains("eeee"),
            "stderr starved behind stdout"
        );
        assert!(outcome.stderr.ends_with(EXEC_TOOL_TRUNCATION_MARKER));
        assert!(outcome.stdout.len() + outcome.stderr.len() <= EXEC_TOOL_MAX_OUTPUT_BYTES);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_utf8_expansion_and_marker_share_the_same_output_cap() {
        let mut capture = ExecCapture::default();
        capture.append(&vec![0xff; EXEC_TOOL_MAX_OUTPUT_BYTES / 2], false);
        capture.append(&vec![0xff; EXEC_TOOL_MAX_OUTPUT_BYTES / 2], true);
        capture.append(&[0xff; 8192], true);
        assert!(capture.stdout.len() + capture.stderr.len() <= EXEC_TOOL_MAX_OUTPUT_BYTES);
        assert!(capture.truncated);
        let (out, err) = capture.finish();
        assert!(out.len() + err.len() <= EXEC_TOOL_MAX_OUTPUT_BYTES);
        assert!(err.ends_with(EXEC_TOOL_TRUNCATION_MARKER));
    }

    fn wait_for(path: &std::path::Path) {
        let until = Instant::now() + Duration::from_secs(10);
        while !path.is_file() {
            assert!(
                Instant::now() < until,
                "probe did not become ready: {}",
                path.display()
            );
            std::thread::sleep(Duration::from_millis(2));
        }
    }

    fn retain_owned_process(
        path: &std::path::Path,
    ) -> minicon_platform::process_reference::ProcessReference {
        let (pid, identity): (u32, String) =
            serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(
            minicon_platform::process_observation::verify_identity(pid, &identity),
            minicon_platform::process_observation::IdentityVerdict::Live
        );
        let reference = minicon_platform::process_reference::ProcessReference::open(pid).unwrap();
        assert_eq!(
            minicon_platform::process_observation::verify_identity(pid, &identity),
            minicon_platform::process_observation::IdentityVerdict::Live
        );
        reference
    }

    fn assert_owned_process_exited(
        reference: &minicon_platform::process_reference::ProcessReference,
    ) {
        assert_eq!(
            reference
                .wait_for_exit(Some(Duration::from_secs(1)))
                .unwrap(),
            minicon_platform::process_reference::ProcessWait::Exited,
            "owned process survived cleanup"
        );
    }

    #[test]
    fn leader_exit_does_not_end_the_pipe_deadline() {
        let root = fixture("leader", "leader");
        let tool = ExecTool::new(&root, &[]);
        // Retain the exact descendant before cleanup, so a vanished numeric
        // PID or an observation failure cannot masquerade as proof of exit.
        let start = Instant::now();
        let worker = std::thread::spawn(move || {
            tool.spawn_with_timeout(
                std::env::current_exe().unwrap().to_str().unwrap(),
                &probe_args(),
                Duration::from_secs(2),
            )
        });
        wait_for(&root.join("leader-ready"));
        let descendant = retain_owned_process(&root.join("descendant/ready"));
        let outcome = worker.join().unwrap().unwrap();
        assert!(
            root.join("leader-ready").is_file(),
            "leader setup failed: {}",
            outcome.stderr
        );
        assert!(outcome.timed_out, "leader exit hid an inherited open pipe");
        assert_eq!(outcome.exit_code, None);
        assert!(
            start.elapsed() < Duration::from_secs(4),
            "output drain escaped its deadline"
        );
        assert_owned_process_exited(&descendant);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn cancellation_terminates_and_reaps_an_owned_process() {
        let root = fixture("cancel", "hold");
        let cancel = Arc::new(AtomicBool::new(false));
        let tool = ExecTool::new(&root, &[]).with_cancellation(cancel.clone());
        let worker = std::thread::spawn(move || {
            tool.spawn_contained(
                std::env::current_exe().unwrap().to_str().unwrap(),
                &probe_args(),
            )
        });
        wait_for(&root.join("ready"));
        let owned = retain_owned_process(&root.join("ready"));
        let start = Instant::now();
        cancel.store(true, Ordering::Release);
        let error = worker.join().unwrap().unwrap_err();
        assert!(error.contains("cancelled"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(2));
        assert_owned_process_exited(&owned);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn pre_cancelled_exec_has_no_child_effect() {
        let root = fixture("pre-cancel", "hold");
        let tool = ExecTool::new(&root, &[]).with_cancellation(Arc::new(AtomicBool::new(true)));
        let error = tool
            .spawn_contained(
                std::env::current_exe().unwrap().to_str().unwrap(),
                &probe_args(),
            )
            .unwrap_err();
        assert!(error.contains("cancelled before spawn"));
        assert!(!root.join("ready").exists());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn exec_minimal_environment_excludes_provider_and_client_secrets() {
        let root = fixture("environment", "environment-parent");
        let exe = std::env::current_exe().unwrap();
        let path =
            std::env::join_paths(std::iter::once(exe.parent().unwrap().to_path_buf()).chain(
                std::env::split_paths(&std::env::var_os("PATH").unwrap_or_default()),
            ))
            .unwrap();
        let mut parent = std::process::Command::new(exe);
        parent
            .args(probe_args())
            .current_dir(&root)
            .env("PATH", path);
        for name in SECRETS {
            parent.env(name, "synthetic-secret-sentinel");
        }
        let output = parent.output().unwrap();
        assert!(
            output.status.success(),
            "parent probe failed: {} {}",
            String::from_utf8_lossy(&output.stdout),
            String::from_utf8_lossy(&output.stderr)
        );
        std::fs::remove_dir_all(root).unwrap();
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

    #[cfg(unix)]
    #[test]
    fn bearer_file_stays_private_and_refuses_a_symlink() {
        use std::os::unix::fs::{PermissionsExt, symlink};
        let root = fixture("bearer");
        let path = root.join("deepseek-bearer");
        std::fs::write(&path, b"old").unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        store_oauth_bearer_at(&path, "token-value").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode, 0o600);
        assert_eq!(load_oauth_bearer_at(&path).as_deref(), Some("token-value"));

        let elsewhere = root.join("elsewhere");
        std::fs::write(&elsewhere, b"secret").unwrap();
        std::fs::remove_file(&path).unwrap();
        symlink(&elsewhere, &path).unwrap();
        let error = store_oauth_bearer_at(&path, "new-token").unwrap_err();
        assert!(!error.is_empty());
        assert_eq!(std::fs::read(&elsewhere).unwrap(), b"secret");
        assert!(load_oauth_bearer_at(&path).is_none());
        let _ = std::fs::remove_dir_all(&root);
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

    #[test]
    fn session_and_continue_together_is_a_bounded_error() {
        let error = parse_harness(&[
            "harness".to_owned(),
            "--root".to_owned(),
            "/tmp".to_owned(),
            "--task".to_owned(),
            "x".to_owned(),
            "--session".to_owned(),
            "a".to_owned(),
            "--continue".to_owned(),
            "a".to_owned(),
        ])
        .unwrap_err();
        assert!(error.contains("mutually exclusive"), "{error}");
    }

    #[test]
    fn session_id_charset_is_bounded() {
        validate_session_id("plan-021_HB").expect("plain id");
        let error = validate_session_id("../escape").unwrap_err();
        assert!(error.contains("refused"), "{error}");
        let error = validate_session_id("").unwrap_err();
        assert!(error.contains("refused"), "{error}");
        let error = validate_session_id(&"x".repeat(65)).unwrap_err();
        assert!(error.contains("64"), "{error}");
    }

    #[test]
    fn compose_resumed_task_folds_prior_turns_ahead_of_the_new_one() {
        assert_eq!(compose_resumed_task(&[], "first task"), "first task");
        let tree = tree_from_turns(&[("earlier task".to_owned(), "earlier answer".to_owned())]);
        let composed = compose_resumed_task(&tree, "new task");
        assert!(composed.contains("earlier task"), "{composed}");
        assert!(composed.contains("earlier answer"), "{composed}");
        assert!(composed.ends_with("new task"), "{composed}");
    }

    #[test]
    fn a_session_round_trips_through_load_and_save_and_stays_bounded() {
        let root = fixture("session-round-trip");
        let tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("root exists");

        assert_eq!(load_session(&tool, "s1").expect("no session yet"), None);

        let mut turns = Vec::new();
        for index in 0..(HARNESS_SESSION_MAX_TURNS + 3) {
            turns.push((format!("task {index}"), format!("answer {index}")));
        }
        let tree = tree_from_turns(&turns);
        save_session(
            &tool,
            "s1",
            &SessionState {
                turns,
                tree,
                palace: String::new(),
                gateway_binding: None,
            },
        )
        .expect("save");

        let loaded = load_session(&tool, "s1")
            .expect("load")
            .expect("session exists now");
        assert_eq!(loaded.turns.len(), HARNESS_SESSION_MAX_TURNS);
        assert_eq!(loaded.tree.len(), HARNESS_SESSION_MAX_TURNS);
        // Oldest turns are the ones dropped, not the newest.
        assert_eq!(loaded.turns.first().unwrap().0, "task 3");
        assert_eq!(loaded.tree.first().unwrap().label, "task: task 3");
        assert_eq!(
            loaded.turns.last().unwrap().0,
            format!("task {}", HARNESS_SESSION_MAX_TURNS + 2)
        );
        // save_session always recomputes palace from the bounded tree, never
        // trusts whatever palace the caller passed in.
        assert!(
            loaded.palace.starts_with("flowchart TD"),
            "{}",
            loaded.palace
        );
        assert!(loaded.palace.contains("turn-3"), "{}", loaded.palace);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pre_palace_session_with_no_palace_key_still_deserializes() {
        let root = fixture("session-pre-palace");
        let tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("root exists");
        let pre_palace = serde_json::json!({
            "turns": [["old task", "old answer"]],
            "tree": [],
        })
        .to_string();
        tool.write(&format!("{HARNESS_SESSION_DIR}/old.json"), &pre_palace)
            .expect("write pre-palace session");

        let loaded = load_session(&tool, "old")
            .expect("load")
            .expect("session exists");
        assert_eq!(
            loaded.palace, "",
            "missing key defaults to empty, not an error"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn palace_node_ids_match_the_trees_id_tagged_nodes_one_to_one() {
        let tree = vec![TreeNode {
            id: "root".to_owned(),
            label: "root task".to_owned(),
            evidence: None,
            failure: None,
            children: vec![
                TreeNode {
                    id: "root-answer".to_owned(),
                    label: "root answer".to_owned(),
                    evidence: None,
                    failure: None,
                    children: Vec::new(),
                },
                TreeNode {
                    id: "sibling".to_owned(),
                    label: "a second subgoal".to_owned(),
                    evidence: None,
                    failure: None,
                    children: Vec::new(),
                },
            ],
        }];

        let rendered = render_palace(&tree);
        let mut expected_ids = Vec::new();
        tree_ids(&tree, &mut expected_ids);
        let mut palace_ids = palace_node_ids(&rendered);
        let mut expected_sorted = expected_ids.clone();
        expected_sorted.sort();
        palace_ids.sort();
        assert_eq!(
            palace_ids, expected_sorted,
            "no orphan node either direction: {rendered}"
        );
        assert!(rendered.contains("root --> root-answer"), "{rendered}");
        assert!(rendered.contains("root --> sibling"), "{rendered}");
    }

    #[test]
    fn a_corrupt_session_file_is_a_bounded_error_not_a_silent_fresh_start() {
        let root = fixture("session-corrupt");
        let tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("root exists");
        tool.write(
            &format!("{HARNESS_SESSION_DIR}/broken.json"),
            "not json at all",
        )
        .expect("write garbage");

        let error = load_session(&tool, "broken").unwrap_err();
        assert!(error.contains("corrupt"), "{error}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_pre_ctx_bare_array_session_upgrades_to_a_tree_on_load() {
        let root = fixture("session-upgrade");
        let tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("root exists");
        let bare = serde_json::to_string(&vec![("old task".to_owned(), "old answer".to_owned())])
            .expect("serialize bare array");
        tool.write(&format!("{HARNESS_SESSION_DIR}/legacy.json"), &bare)
            .expect("write legacy session");

        let loaded = load_session(&tool, "legacy")
            .expect("load")
            .expect("session exists");
        assert_eq!(
            loaded.turns,
            vec![("old task".to_owned(), "old answer".to_owned())]
        );
        assert!(
            loaded.tree.is_empty(),
            "upgrade happens at use, not at load"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_tree_round_trips_through_render_and_parse_preserving_order_and_annotations() {
        let tree = vec![
            TreeNode {
                id: "a".to_owned(),
                label: "first subgoal".to_owned(),
                evidence: Some("test-x".to_owned()),
                failure: None,
                children: vec![TreeNode {
                    id: "a-1".to_owned(),
                    label: "a detail".to_owned(),
                    evidence: None,
                    failure: Some("timeout".to_owned()),
                    children: Vec::new(),
                }],
            },
            TreeNode {
                id: "b".to_owned(),
                label: "second subgoal".to_owned(),
                evidence: None,
                failure: None,
                children: Vec::new(),
            },
        ];

        let rendered = render_tree(&tree);
        let parsed = parse_tree(&rendered).expect("parse own output");
        assert_eq!(
            parsed, tree,
            "round trip must preserve order and annotations"
        );

        // Mutate one node and confirm the mutation alone survives a second round trip.
        let mut mutated = parsed;
        mutated[0].children[0].failure = None;
        mutated[0].children[0].evidence = Some("retried-ok".to_owned());
        let rendered_again = render_tree(&mutated);
        let parsed_again = parse_tree(&rendered_again).expect("parse mutated output");
        assert_eq!(parsed_again, mutated);
        assert_eq!(parsed_again[1].id, "b", "sibling order preserved");
    }

    #[test]
    fn parse_tree_rejects_odd_indentation_and_orphaned_children() {
        let error = parse_tree(" - {a} bad indent").unwrap_err();
        assert!(error.contains("odd indentation"), "{error}");

        let error = parse_tree("  - {a} orphaned child").unwrap_err();
        assert!(error.contains("no parent line above it"), "{error}");
    }

    #[test]
    fn run_loop_ends_on_the_explicit_stop_phrase_after_all_five_states() {
        let mut trace = Vec::new();
        let mut call = |_task: &str| Ok(format!("done, {LOOP_STOP_PHRASE}"));
        let answer = run_loop(&mut call, "task", &mut trace).expect("loop ends");
        assert!(answer.contains(LOOP_STOP_PHRASE), "{answer}");
        let labels: Vec<&str> = trace.iter().map(|node| node.label.as_str()).collect();
        assert!(labels.iter().any(|l| l.starts_with("state: categorize")));
        assert!(labels.iter().any(|l| l.starts_with("state: draft")));
        assert!(labels.iter().any(|l| l.starts_with("state: decide-pick")));
        assert!(labels.iter().any(|l| l.starts_with("state: execute")));
        assert!(
            labels
                .iter()
                .any(|l| l.starts_with("state: decide-continue") && l.contains("end"))
        );
        // Exactly one iteration: the stop phrase was on the first answer.
        assert_eq!(trace.len(), 5, "{trace:?}");
    }

    #[test]
    fn run_loop_jumps_when_no_stop_phrase_then_ends_on_the_iteration_ceiling() {
        let mut trace = Vec::new();
        let mut call = |_task: &str| Ok("still working, no stop phrase yet".to_owned());
        let answer = run_loop(&mut call, "task", &mut trace).expect("loop ends on ceiling");
        assert!(!answer.contains(LOOP_STOP_PHRASE));
        let jumps = trace
            .iter()
            .filter(|node| node.label.contains("decide-continue") && node.label.contains("jump"))
            .count();
        assert_eq!(jumps, LOOP_MAX_ITERATIONS - 1, "{trace:?}");
        let ends = trace
            .iter()
            .filter(|node| {
                node.label.contains("decide-continue") && node.label.contains("iteration ceiling")
            })
            .count();
        assert_eq!(ends, 1, "{trace:?}");
    }

    #[test]
    fn run_loop_bounds_redrafts_then_force_accepts() {
        let mut trace = Vec::new();
        let mut call = |_task: &str| Ok(format!("{LOOP_REDRAFT_MARKER}, {LOOP_STOP_PHRASE}"));
        run_loop(&mut call, "task", &mut trace).expect("loop ends despite redraft marker");
        let rejects = trace
            .iter()
            .filter(|node| node.label.contains("decide-pick") && node.label.contains("reject"))
            .count();
        assert_eq!(rejects, LOOP_MAX_PLAN_REVISIONS, "{trace:?}");
        let accepts = trace
            .iter()
            .filter(|node| node.label.contains("decide-pick") && node.label.contains("accept"))
            .count();
        assert_eq!(accepts, 1, "{trace:?}");
    }

    #[test]
    fn run_loop_jump_carries_the_prior_answer_into_the_next_call() {
        let mut trace = Vec::new();
        let mut calls: Vec<String> = Vec::new();
        let mut call = |task: &str| {
            calls.push(task.to_owned());
            if calls.len() == 1 {
                Ok("first-step-result".to_owned())
            } else {
                Ok(format!("built on it, {LOOP_STOP_PHRASE}"))
            }
        };
        let answer = run_loop(&mut call, "start", &mut trace).expect("loop ends");
        assert!(answer.contains(LOOP_STOP_PHRASE));
        assert_eq!(calls[0], "start");
        assert!(
            calls[1].contains("first-step-result"),
            "the jump must fold the prior answer into the next task: {calls:?}"
        );
        assert!(
            calls[1].contains("start"),
            "the jump must also carry the ORIGINAL task forward, not just the prior draft \
             (the backend closure is stateless across calls): {calls:?}"
        );
    }

    #[test]
    fn turn_node_places_the_loop_trace_ahead_of_the_final_answer() {
        let mut trace = Vec::new();
        log_transition(&mut trace, 0, LoopState::Categorize, "routed");
        log_transition(
            &mut trace,
            0,
            LoopState::DecideContinue,
            "end (stop phrase)",
        );
        let node = turn_node(0, "task text", trace, "final answer");
        assert_eq!(node.children.len(), 3, "{node:?}");
        assert_eq!(node.children[0].id, "loop-0-categorize");
        assert_eq!(node.children[1].id, "loop-0-decide-continue");
        assert_eq!(node.children[2].id, "turn-0-answer");
        assert!(node.children[2].label.contains("final answer"));
    }

    #[test]
    fn composed_prompt_carries_the_trees_markdown_verbatim_to_the_wire_seam() {
        // {WIRE} regression guard: CTX/PALACE/LOOP must still hand the backend
        // codec one plain string containing the tree's own Markdown, never a
        // reformatted summary -- this is what makes both `harness_wire` and
        // `harness_opencode` stay dumb transports.
        let tree = tree_from_turns(&[("earlier task".to_owned(), "earlier answer".to_owned())]);
        let expected_markdown = render_tree(&tree);
        let composed = compose_resumed_task(&tree, "new task");

        assert!(
            composed.contains(&expected_markdown),
            "composed prompt must contain the tree's rendered Markdown verbatim: {composed}"
        );
        assert!(composed.contains("- {turn-0}"), "{composed}");
    }
}
