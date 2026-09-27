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
    // Refuse a missing credential first. Finding it here means no tool ever runs
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
        match request.backend {
            Backend::DeepSeek => crate::harness_wire::run_task(
                &crate::harness_wire::NetworkHttp,
                crate::harness_wire::DEEPSEEK_CHAT_URL,
                &key,
                crate::harness_wire::DEEPSEEK_MODEL,
                task,
                &file_tool,
                &exec_tool,
            ),
            Backend::OpencodeGo => {
                let url = crate::harness_opencode::opencode_chat_url()?;
                let model = crate::harness_opencode::opencode_model();
                crate::harness_opencode::opencode_run_task(
                    &crate::harness_wire::NetworkHttp,
                    &url,
                    &key,
                    &model,
                    task,
                    &file_tool,
                    &exec_tool,
                )
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
        new_session,
        continue_session,
    })
}

fn harness_usage() -> String {
    "usage: minicon harness --root PATH --task TEXT [--backend deepseek|opencode-go] \
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

/// How long a killed child is given to actually leave after
/// `terminate_and_wait`, on top of `EXEC_TOOL_TIMEOUT` itself.
const EXEC_TOOL_KILL_GRACE: std::time::Duration = std::time::Duration::from_secs(5);

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
    /// Uses `agenterm_platform::contained_process::ContainedHeadlessCommand`
    /// (the crate's `contained-process-spawn` feature, enabled in
    /// `Cargo.toml`), not `std::process::Command`. This keeps the same
    /// argv-vector, no-shell guarantee H3's invariant is about and adds the
    /// resource containment the design of record actually asked for: a
    /// Windows kill-on-close Job Object or a Unix process group owns the
    /// child's whole descendant tree, so a runaway grandchild cannot outlive
    /// this call's own timeout kill.
    fn spawn_contained(&self, program: &str, args: &[String]) -> Result<ExecOutcome, String> {
        use agenterm_platform::contained_process::{
            ContainedHeadlessCommand, ContainedProcessLimits,
        };
        use std::io::Read as _;

        let mut command = ContainedHeadlessCommand::new(program);
        command
            .args(args.iter().cloned())
            .current_dir(&self.root)
            .capture_output()
            .limits(ContainedProcessLimits {
                cpu_seconds: Some(EXEC_TOOL_TIMEOUT.as_secs()),
                // agenterm_platform's macOS contained_process refuses any
                // `memory_bytes` limit outright (RLIMIT_AS cannot be set
                // below the process-wide dyld mapping there), so asking for
                // one made every spawn fail on macOS. Other limits are still
                // enforced there.
                #[cfg(target_os = "macos")]
                memory_bytes: None,
                #[cfg(not(target_os = "macos"))]
                memory_bytes: Some(EXEC_TOOL_MEMORY_BYTES),
                file_size_bytes: Some(EXEC_TOOL_FILE_SIZE_BYTES),
                open_files: Some(EXEC_TOOL_OPEN_FILES),
                active_processes: Some(EXEC_TOOL_ACTIVE_PROCESSES),
            });

        let mut child = command
            .spawn()
            .map_err(|error| format!("exec: {program:?} could not be started: {error}"))?;

        // Drain both captured streams on their own threads. Polling for the
        // exit while a full pipe blocks the child would deadlock the
        // timeout this bound exists to provide.
        let drain = |stream: Option<agenterm_platform::contained_process::ContainedChildOutput>| {
            std::thread::spawn(move || {
                let mut buffer = Vec::new();
                if let Some(mut stream) = stream {
                    let _ = stream.read_to_end(&mut buffer);
                }
                buffer
            })
        };
        let stdout = drain(child.take_stdout());
        let stderr = drain(child.take_stderr());

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
                let _ = child.terminate_and_wait(EXEC_TOOL_KILL_GRACE);
                break None;
            }
            std::thread::sleep(std::time::Duration::from_millis(5));
        };

        let collect = |handle: std::thread::JoinHandle<Vec<u8>>| {
            String::from_utf8_lossy(&handle.join().unwrap_or_default()).into_owned()
        };
        Ok(ExecOutcome {
            exit_code: status.and_then(|status| status.conventional_code().map(|code| code as i32)),
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
