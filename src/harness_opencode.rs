//! The opencode-go-compatible harness backend.
//!
//! H1 forbids assuming the DeepSeek adapter covers this backend, so this
//! module owns its own request body, its own reply parsing and its own
//! bounds. It reuses only the `Transport` seam from `harness_wire`, never
//! that module's DeepSeek codec.
//!
//! ## What is assumed about the wire shape, and what is still owed
//!
//! This repository documents no opencode-go wire format: `PRD_02_31` and
//! `plan/plan-v0.2.0.md` say only that such a server "typically" listens on
//! loopback over plain HTTP, and nothing about its JSON. Rather than invent
//! undocumented fields, this adapter implements the **OpenAI-compatible
//! `/v1/chat/completions` shape** that servers of this kind expose. Stated
//! plainly, the assumptions are:
//!
//! 1. The endpoint is `POST <base>/v1/chat/completions`, `application/json`,
//!    with an `Authorization: Bearer <key>` header.
//! 2. The request carries `model`, `messages`, `tools` (each
//!    `{"type":"function","function":{name,description,parameters}}`) and
//!    `stream: false`.
//! 3. A reply is `choices[0].message`, with either `content` text or a
//!    `tool_calls` array whose entries carry `id` and
//!    `function.{name,arguments}`, `arguments` being a JSON *string*.
//! 4. A refusal states itself in a top-level `error.message`.
//!
//! These are assumptions, not verified facts. Being the same family of shape
//! as DeepSeek's is why the *codec below is still written independently*: if
//! the assumption turns out wrong, only this file changes, and no claim of
//! "any OpenAI-compatible endpoint" is inherited from H4.
//!
//! **Evidence still owed:** one live round trip against a real running
//! opencode-go server — the request accepted, a real `tool_calls` reply
//! parsed, one file landing under the task root. The tests below prove the
//! adapter over a *real socket* against a fake endpoint, which covers framing,
//! headers, body fields, parsing and every bounded refusal, but it cannot
//! prove the remote server agrees with assumptions 1-4. Until that run exists
//! this backend is `BLOCKED`, not `[x]`.

// Reached today only by this file's tests: `harness.rs` is another worker's
// file and does not call the adapter yet. Same scoped, non-test allowance and
// same reason as `harness_wire` and the two tools carry -- faking a caller to
// make it reachable would be worse than saying so.
#![cfg_attr(not(test), allow(dead_code))]

use crate::harness_wire::Transport;

// ---------------------------------------------------------------------------
// H5 -- this backend's own bounds
// ---------------------------------------------------------------------------

/// Most model round trips one opencode-go task may take.
///
/// Eight, not DeepSeek's sixteen, and deliberately not inherited: this backend
/// is normally a *local* server, so a turn costs the user's own machine rather
/// than a metered remote call, and a local model that has not converged in
/// eight turns is looping, not thinking. Halving the ceiling makes that loop
/// stop in bounded time instead of burning the machine for twice as long.
pub const OPENCODE_MAX_TURNS: usize = 8;

/// Most tool calls one opencode-go task may dispatch, across all turns.
///
/// Separate from the turn bound because a single turn may ask for many calls,
/// so counting turns alone does not bound the work actually done. Thirty-two
/// is four per permitted turn: enough for a read-edit-verify cycle on several
/// files, short of a model that has started enumerating the tree.
pub const OPENCODE_MAX_TOOL_CALLS: usize = 32;

/// Largest reply body this adapter will parse, in bytes.
///
/// One MiB, below the transport's own 4 MiB read ceiling on purpose: a reply
/// becomes the next turn's prompt, so the adapter refuses a body it would
/// otherwise feed back into the conversation. It is set *above*
/// `FILE_TOOL_MAX_READ_BYTES` (256 KiB) because a legitimate reply can quote a
/// tool result it was just handed plus its own prose, and below the
/// transport's because the transport bounds a socket while this bounds a
/// prompt, and the prompt is the tighter concern.
pub const OPENCODE_MAX_REPLY_BYTES: usize = 1024 * 1024;

/// The model name sent when the caller names none.
///
/// A local server's model set is the user's own, so this is only a placeholder
/// the caller is expected to override; it is named here so there is one place
/// to correct.
pub const OPENCODE_DEFAULT_MODEL: &str = "opencode";

// ---------------------------------------------------------------------------
// H5 -- endpoint resolution
// ---------------------------------------------------------------------------

/// The environment variable naming this backend's base URL.
///
/// Spelled `MINICON_OPENCODE_*` to match `Backend::key_var`'s
/// `MINICON_OPENCODE_API_KEY` exactly -- same product prefix, same backend
/// word, so the two settings of one backend read as a pair and neither invents
/// a second spelling (no `OPENCODE_GO_`, no `MINICON_OPENCODEGO_`).
pub const OPENCODE_BASE_URL_VAR: &str = "MINICON_OPENCODE_BASE_URL";

/// Where an opencode-go-compatible server is assumed to listen when
/// `OPENCODE_BASE_URL_VAR` is unset: loopback, plain HTTP.
///
/// Loopback by decision, not by convenience. A default that could be remote
/// would send a task and a bearer token off the machine because a variable was
/// forgotten; this default cannot leave the host, and the transport has no TLS
/// to reach a remote endpoint with anyway.
pub const OPENCODE_DEFAULT_BASE_URL: &str = "http://127.0.0.1:4096";

/// The path appended to the base URL. See assumption 1 in the module header.
pub const OPENCODE_CHAT_PATH: &str = "/v1/chat/completions";

/// Resolves the chat URL from an already-read environment value.
///
/// Takes the value rather than reading the environment so the refusals below
/// are provable without mutating process-wide state. `None` and an empty
/// string both mean "unset", which is the documented local default; anything
/// present but unusable is a **bounded refusal naming what was wrong**, never
/// a panic and never a quiet fall back to the default (falling back would turn
/// a typo in a base URL into traffic aimed somewhere the user did not type).
pub fn opencode_chat_url_from(raw: Option<&str>) -> Result<String, String> {
    let base = match raw.map(str::trim) {
        None | Some("") => OPENCODE_DEFAULT_BASE_URL,
        Some(value) => value,
    };
    if !base.starts_with("http://") {
        return Err(format!(
            "harness: {OPENCODE_BASE_URL_VAR}={base:?} is refused: it must be an http:// base \
             URL. MiniCon has no TLS capability, so an https:// endpoint cannot be reached and \
             will not be downgraded to pretend otherwise; the default when the variable is \
             unset is {OPENCODE_DEFAULT_BASE_URL}"
        ));
    }
    // Strip the scheme first and only then the trailing slashes: doing it the
    // other way round turns the hostless "http://" into the non-empty "http:"
    // and lets it through.
    let authority = base
        .trim_start_matches("http://")
        .trim_end_matches('/')
        .trim();
    if authority.is_empty() || authority.starts_with('/') {
        return Err(format!(
            "harness: {OPENCODE_BASE_URL_VAR}={base:?} is refused: it names no host"
        ));
    }
    Ok(format!("http://{authority}{OPENCODE_CHAT_PATH}"))
}

/// Resolves the chat URL from the process environment.
pub fn opencode_chat_url() -> Result<String, String> {
    let raw = std::env::var(OPENCODE_BASE_URL_VAR).ok();
    opencode_chat_url_from(raw.as_deref())
}

// ---------------------------------------------------------------------------
// H5 -- this backend's own codec
// ---------------------------------------------------------------------------

/// One tool call this backend asked for, arguments still unparsed.
///
/// This module's own type, not `harness_wire`'s: sharing it would make the two
/// adapters one adapter the first time either backend's shape moved.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OpencodeToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// What one opencode-go reply amounted to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OpencodeReply {
    /// A final answer: the loop stops here.
    Text(String),
    /// Work to do before the model can answer.
    ToolCalls(Vec<OpencodeToolCall>),
}

/// The two tools, written as this adapter assumes this backend wants them.
///
/// Both are advertised always, including with an empty `--allow-cmd`: an empty
/// allow-list refuses every `exec` call inside `ExecTool::admit`, it does not
/// hide the tool, so a model cannot tell "not allowed yet" from "tool absent"
/// by probing.
fn opencode_tool_definitions() -> serde_json::Value {
    serde_json::json!([
        {
            "type": "function",
            "function": {
                "name": "file",
                "description": "Read or write one UTF-8 text file, confined to the task root. \
                                Paths are relative to that root; an absolute path, a `..` \
                                component or a symlink is refused, never rewritten.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "op": {
                            "type": "string",
                            "enum": ["read", "write"],
                            "description": "Which operation to perform."
                        },
                        "path": {
                            "type": "string",
                            "description": "Path relative to the task root."
                        },
                        "contents": {
                            "type": "string",
                            "description": "The full new contents, for op=write only."
                        }
                    },
                    "required": ["op", "path"]
                }
            }
        },
        {
            "type": "function",
            "function": {
                "name": "exec",
                "description": "Run one allow-listed command in the task root as an argv \
                                vector. There is no shell: `;`, `&&`, `|` and `$(...)` are \
                                ordinary argument bytes. A command outside the allow-list is \
                                refused.",
                "parameters": {
                    "type": "object",
                    "properties": {
                        "argv": {
                            "type": "array",
                            "items": { "type": "string" },
                            "description": "The program's executable basename followed by its \
                                            arguments."
                        }
                    },
                    "required": ["argv"]
                }
            }
        }
    ])
}

/// Serializes one request body for this backend.
///
/// `stream` is pinned false: one bounded task wants one reproducible round
/// trip, and a streamed body would need framing the transport deliberately
/// does not have. No sampling knobs are sent, so nothing about the server's
/// own defaults is assumed beyond assumption 2 in the module header.
pub fn opencode_request_body(model: &str, messages: &[serde_json::Value]) -> String {
    serde_json::json!({
        "model": model,
        "messages": messages,
        "tools": opencode_tool_definitions(),
        "stream": false,
    })
    .to_string()
}

/// Parses one reply body into a final answer or tool calls.
///
/// Every failure carries the underlying detail -- the serde message, the
/// server's own `error.message`, the offending tool name, the measured size --
/// rather than collapsing to one generic sentence, which is exactly what
/// the archived v0.1.x precision audit's item 70 faulted in a removed
/// implementation. (That audit file is not in this clone, so it is cited by
/// name rather than by path.)
pub fn opencode_parse_reply(body: &str) -> Result<OpencodeReply, String> {
    if body.len() > OPENCODE_MAX_REPLY_BYTES {
        return Err(format!(
            "harness: the opencode-go reply is {} bytes, past the bound of \
             {OPENCODE_MAX_REPLY_BYTES} (OPENCODE_MAX_REPLY_BYTES), and was not parsed",
            body.len()
        ));
    }
    let value: serde_json::Value = serde_json::from_str(body).map_err(|error| {
        format!(
            "harness: the opencode-go reply is not JSON: {error}; body began: {beginning:?}",
            beginning = reply_excerpt(body)
        )
    })?;
    // A server states a refusal in its own error object (assumption 4);
    // surface it verbatim rather than reporting a missing field.
    if let Some(message) = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
    {
        return Err(format!(
            "harness: the opencode-go endpoint refused the request: {message}"
        ));
    }
    let message = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| {
            format!(
                "harness: the opencode-go reply has no choices[0].message; body began: \
                 {beginning:?}",
                beginning = reply_excerpt(body)
            )
        })?;

    if let Some(calls) = message
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
    {
        let mut parsed = Vec::with_capacity(calls.len());
        for call in calls {
            let function = call.get("function").ok_or_else(|| {
                format!("harness: an opencode-go tool call has no function object: {call}")
            })?;
            let name = function
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| {
                    format!("harness: an opencode-go tool call has no function name: {function}")
                })?;
            parsed.push(OpencodeToolCall {
                // An id is how a tool result is matched to its call. A call
                // without one is unanswerable, so refuse rather than invent.
                id: call
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| format!("harness: opencode-go tool call {name:?} has no id"))?
                    .to_owned(),
                name: name.to_owned(),
                arguments: function
                    .get("arguments")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("{}")
                    .to_owned(),
            });
        }
        // A reply carrying both text and tool calls is treated as tool calls:
        // the text is the model narrating what it is about to do, and stopping
        // there would drop the work it asked for.
        if !parsed.is_empty() {
            return Ok(OpencodeReply::ToolCalls(parsed));
        }
    }
    let text = message
        .get("content")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| {
            format!(
                "harness: the opencode-go reply has neither tool calls nor text content: \
                 {message}"
            )
        })?;
    Ok(OpencodeReply::Text(text.to_owned()))
}

/// The leading bytes of a body, for an error message that has to stay bounded
/// even when the body is not.
fn reply_excerpt(body: &str) -> String {
    const EXCERPT: usize = 200;
    match body.char_indices().nth(EXCERPT) {
        Some((index, _)) => format!("{}...", &body[..index]),
        None => body.to_owned(),
    }
}

// ---------------------------------------------------------------------------
// H5 -- this backend's own bounded turn loop
// ---------------------------------------------------------------------------

/// The instruction the task is wrapped in, so the model knows the two tools'
/// bounds before it trips over them.
fn opencode_system_prompt(root: &std::path::Path) -> String {
    format!(
        "You are MiniCon's bounded harness agent. You have exactly two tools, `file` and \
         `exec`, and no others. Every path is relative to the task root {root}; paths outside \
         it are refused, not rewritten. `exec` runs one allow-listed command per call with no \
         shell. A refusal is information: read it, adjust, and continue. When the task is done, \
         answer with plain text and no tool call.",
        root = root.display()
    )
}

/// Runs one bounded task against an opencode-go-compatible endpoint.
///
/// Stops for exactly three reasons: a final text reply, a transport or codec
/// error, or one of this module's two bounds. A **tool refusal is none of
/// them** -- a path outside the root or an unlisted command goes back to the
/// model as an ordinary tool result so it can correct itself, and it neither
/// aborts the run nor buys another turn.
pub fn opencode_run_task(
    transport: &dyn Transport,
    url: &str,
    bearer: &str,
    model: &str,
    task: &str,
    file_tool: &crate::harness::FileTool,
    exec_tool: &crate::harness::ExecTool,
) -> Result<String, String> {
    let mut messages = vec![
        serde_json::json!({
            "role": "system",
            "content": opencode_system_prompt(file_tool.root_path()),
        }),
        serde_json::json!({ "role": "user", "content": task }),
    ];
    let mut tool_calls_spent = 0usize;

    for _turn in 0..OPENCODE_MAX_TURNS {
        let body = opencode_request_body(model, &messages);
        let reply = transport.post_json(url, bearer, &body)?;
        match opencode_parse_reply(&reply)? {
            OpencodeReply::Text(text) => return Ok(text),
            OpencodeReply::ToolCalls(calls) => {
                if tool_calls_spent + calls.len() > OPENCODE_MAX_TOOL_CALLS {
                    return Err(format!(
                        "harness: stopped after {tool_calls_spent} opencode-go tool calls: this \
                         turn's {} more would pass the bound of {OPENCODE_MAX_TOOL_CALLS} \
                         (OPENCODE_MAX_TOOL_CALLS); the task did not finish",
                        calls.len()
                    ));
                }
                // Echo the assistant turn back before its results: a result
                // whose call is absent from the history has nothing to answer.
                messages.push(serde_json::json!({
                    "role": "assistant",
                    "content": serde_json::Value::Null,
                    "tool_calls": calls
                        .iter()
                        .map(|call| serde_json::json!({
                            "id": call.id,
                            "type": "function",
                            "function": { "name": call.name, "arguments": call.arguments },
                        }))
                        .collect::<Vec<_>>(),
                }));
                for call in &calls {
                    tool_calls_spent += 1;
                    // A refusal is a result, not an early return: both arms
                    // hand the model a string.
                    let result = opencode_dispatch(file_tool, exec_tool, call)
                        .unwrap_or_else(|refusal| refusal);
                    messages.push(serde_json::json!({
                        "role": "tool",
                        "tool_call_id": call.id,
                        "content": result,
                    }));
                }
            }
        }
    }
    Err(format!(
        "harness: stopped after the bound of {OPENCODE_MAX_TURNS} opencode-go model turns \
         (OPENCODE_MAX_TURNS) without a final answer; the task did not finish"
    ))
}

/// Runs one tool call. `Err` is still something to tell the model, not a reason
/// to abandon the run -- see `opencode_run_task`.
fn opencode_dispatch(
    file_tool: &crate::harness::FileTool,
    exec_tool: &crate::harness::ExecTool,
    call: &OpencodeToolCall,
) -> Result<String, String> {
    let arguments: serde_json::Value = serde_json::from_str(&call.arguments).map_err(|error| {
        format!(
            "{}: refused: the arguments are not a JSON object: {error}",
            call.name
        )
    })?;
    match call.name.as_str() {
        "file" => {
            let path = arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "file: refused: no `path` argument".to_owned())?;
            match arguments.get("op").and_then(serde_json::Value::as_str) {
                Some("read") => file_tool.read(path),
                Some("write") => {
                    let contents = arguments
                        .get("contents")
                        .and_then(serde_json::Value::as_str)
                        .ok_or_else(|| {
                            "file: refused: op=write needs a `contents` argument".to_owned()
                        })?;
                    let written = file_tool.write(path, contents)?;
                    Ok(format!("file: wrote {written} bytes to {path}"))
                }
                other => Err(format!(
                    "file: refused: op must be \"read\" or \"write\", got {other:?}"
                )),
            }
        }
        "exec" => {
            let argv = arguments
                .get("argv")
                .and_then(serde_json::Value::as_array)
                .ok_or_else(|| "exec: refused: no `argv` array argument".to_owned())?
                .iter()
                .map(|item| {
                    item.as_str().map(str::to_owned).ok_or_else(|| {
                        "exec: refused: every argv entry must be a string".to_owned()
                    })
                })
                .collect::<Result<Vec<String>, String>>()?;
            let (program, args) = argv
                .split_first()
                .ok_or_else(|| "exec: refused: argv is empty".to_owned())?;
            let outcome = exec_tool.run(program, args)?;
            Ok(format!(
                "exec: exit={} timed_out={}\nstdout:\n{}\nstderr:\n{}",
                outcome
                    .exit_code
                    .map_or_else(|| "terminated".to_owned(), |code| code.to_string()),
                outcome.timed_out,
                outcome.stdout,
                outcome.stderr
            ))
        }
        other => Err(format!(
            "{other}: refused: there is no such tool; this harness has exactly `file` and `exec`"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{ExecTool, FileTool};
    use crate::harness_wire::NetworkHttp;
    use std::io::{Read as _, Write as _};

    /// A fresh empty directory under the system temp dir, unique per test.
    fn fixture(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "minicon-harness-opencode-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture root");
        std::fs::canonicalize(&root).expect("canonical fixture root")
    }

    /// What one fake-endpoint request looked like on the wire.
    struct Received {
        head: String,
        body: String,
    }

    /// A real loopback HTTP server: binds 127.0.0.1:0, serves `replies` in
    /// order, one connection each, and hands back every request it read.
    ///
    /// Deliberately a real `TcpListener` and not a scripted `Transport`: a
    /// hand-written fake proves the codec but not that the adapter's request
    /// actually leaves as a well-formed HTTP POST with the right path and
    /// header, which is the half a string test cannot reach. Loopback only,
    /// so no test touches the network.
    struct FakeEndpoint {
        url: String,
        received: std::sync::mpsc::Receiver<Received>,
        /// Set on drop so the server thread stops waiting for a connection
        /// that will never come. Without it, a test that panics before
        /// consuming every scripted reply leaves the thread parked in
        /// `accept()` and the join in `Drop` hangs the whole test binary --
        /// measured, not hypothetical: it wedged a break-probe run.
        stop: std::sync::Arc<std::sync::atomic::AtomicBool>,
        server: Option<std::thread::JoinHandle<()>>,
    }

    impl FakeEndpoint {
        /// `replies` are raw HTTP responses, written verbatim.
        fn start(replies: Vec<String>) -> Self {
            let listener =
                std::net::TcpListener::bind("127.0.0.1:0").expect("bind loopback listener");
            let port = listener.local_addr().expect("listener address").port();
            let (sender, received) = std::sync::mpsc::channel();
            // Non-blocking accept plus a stop flag: every wait in this thread
            // is bounded, so the thread always terminates on its own.
            listener
                .set_nonblocking(true)
                .expect("a non-blocking listener");
            let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
            let thread_stop = std::sync::Arc::clone(&stop);
            let server = std::thread::spawn(move || {
                for reply in replies {
                    let mut stream = loop {
                        match listener.accept() {
                            Ok((stream, _)) => break stream,
                            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                                if thread_stop.load(std::sync::atomic::Ordering::Relaxed) {
                                    return;
                                }
                                std::thread::sleep(std::time::Duration::from_millis(2));
                            }
                            Err(_) => return,
                        }
                    };
                    // The accepted socket inherits the listener's mode, and a
                    // blocking read with a deadline is what this needs.
                    if stream.set_nonblocking(false).is_err()
                        || stream
                            .set_read_timeout(Some(std::time::Duration::from_secs(10)))
                            .is_err()
                    {
                        return;
                    }
                    let mut raw = Vec::new();
                    let mut chunk = [0u8; 8192];
                    // Read exactly the head plus the Content-Length body, so
                    // the read ends without waiting on the client to close.
                    loop {
                        let Ok(read) = stream.read(&mut chunk) else {
                            return;
                        };
                        if read == 0 {
                            break;
                        }
                        raw.extend_from_slice(&chunk[..read]);
                        if let Some(split) = raw.windows(4).position(|window| window == b"\r\n\r\n")
                        {
                            let head = String::from_utf8_lossy(&raw[..split]).into_owned();
                            let length = head
                                .split("\r\n")
                                .filter_map(|line| line.split_once(':'))
                                .find(|(name, _)| name.eq_ignore_ascii_case("content-length"))
                                .and_then(|(_, value)| value.trim().parse::<usize>().ok())
                                .unwrap_or(0);
                            if raw.len() >= split + 4 + length {
                                break;
                            }
                        }
                    }
                    let split = raw
                        .windows(4)
                        .position(|window| window == b"\r\n\r\n")
                        .unwrap_or(raw.len().saturating_sub(4));
                    let _ = sender.send(Received {
                        head: String::from_utf8_lossy(&raw[..split]).into_owned(),
                        body: String::from_utf8_lossy(&raw[split.saturating_add(4)..]).into_owned(),
                    });
                    let _ = stream.write_all(reply.as_bytes());
                    let _ = stream.flush();
                }
            });
            Self {
                url: format!("http://127.0.0.1:{port}{OPENCODE_CHAT_PATH}"),
                received,
                stop,
                server: Some(server),
            }
        }

        fn next_request(&self) -> Received {
            self.received
                .recv_timeout(std::time::Duration::from_secs(10))
                .expect("the fake endpoint received a request")
        }
    }

    impl Drop for FakeEndpoint {
        fn drop(&mut self) {
            self.stop.store(true, std::sync::atomic::Ordering::Relaxed);
            if let Some(server) = self.server.take() {
                let _ = server.join();
            }
        }
    }

    /// Wraps `body` in a minimal HTTP/1.1 response with the given status.
    fn http_reply(status: u16, body: &str) -> String {
        let reason = if (200..300).contains(&status) {
            "OK"
        } else {
            "Error"
        };
        format!(
            "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        )
    }

    // -- endpoint resolution -------------------------------------------------

    #[test]
    fn opencode_endpoint_defaults_to_the_documented_loopback_server() {
        assert_eq!(
            opencode_chat_url_from(None).expect("the default resolves"),
            format!("{OPENCODE_DEFAULT_BASE_URL}{OPENCODE_CHAT_PATH}")
        );
        // An empty or whitespace value means "unset", not "broken".
        assert_eq!(
            opencode_chat_url_from(Some("   ")).expect("blank means unset"),
            format!("{OPENCODE_DEFAULT_BASE_URL}{OPENCODE_CHAT_PATH}")
        );
        // The default must stay on loopback: a remote default would ship the
        // task and the bearer token off the host on a forgotten variable.
        assert!(OPENCODE_DEFAULT_BASE_URL.starts_with("http://127.0.0.1"));
    }

    #[test]
    fn opencode_endpoint_reads_the_process_environment_through_the_same_rules() {
        // The env-reading wrapper must agree with the pure resolver for
        // whatever this process actually carries -- that is the only claim it
        // makes, and it is asserted without mutating process-wide state.
        let raw = std::env::var(OPENCODE_BASE_URL_VAR).ok();
        assert_eq!(opencode_chat_url(), opencode_chat_url_from(raw.as_deref()));
    }

    #[test]
    fn opencode_endpoint_honours_an_explicit_base_url_without_doubling_slashes() {
        assert_eq!(
            opencode_chat_url_from(Some("http://127.0.0.1:9911/")).expect("explicit base"),
            format!("http://127.0.0.1:9911{OPENCODE_CHAT_PATH}")
        );
    }

    #[test]
    fn opencode_endpoint_refuses_a_non_http_or_hostless_base_url() {
        let https = opencode_chat_url_from(Some("https://127.0.0.1:9911"))
            .expect_err("https is refused, not downgraded");
        assert!(https.contains("no TLS capability"), "{https}");
        assert!(https.contains(OPENCODE_BASE_URL_VAR), "{https}");
        // And it does not quietly become the default instead.
        assert!(!https.starts_with("http://"), "{https}");

        let hostless =
            opencode_chat_url_from(Some("http://")).expect_err("a hostless base is refused");
        assert!(hostless.contains("names no host"), "{hostless}");

        let nonsense =
            opencode_chat_url_from(Some("not a url")).expect_err("an unparseable base is refused");
        assert!(nonsense.contains("not a url"), "{nonsense}");
    }

    // -- real round trip over a real socket ----------------------------------

    #[test]
    fn opencode_round_trip_over_a_real_socket_sends_the_assumed_request_and_writes_the_file() {
        let root = fixture("round-trip");
        let file_tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("file tool");
        let exec_tool = ExecTool::new(file_tool.root_path(), &[]);

        let endpoint = FakeEndpoint::start(vec![
            http_reply(
                200,
                &serde_json::json!({
                    "choices": [{
                        "message": {
                            "role": "assistant",
                            "content": "writing it now",
                            "tool_calls": [{
                                "id": "call-1",
                                "type": "function",
                                "function": {
                                    "name": "file",
                                    "arguments": "{\"op\":\"write\",\"path\":\"note.txt\",\
                                                  \"contents\":\"from the model\"}"
                                }
                            }]
                        }
                    }]
                })
                .to_string(),
            ),
            http_reply(
                200,
                &serde_json::json!({
                    "choices": [{ "message": { "role": "assistant", "content": "done" } }]
                })
                .to_string(),
            ),
        ]);

        let answer = opencode_run_task(
            &NetworkHttp,
            &endpoint.url,
            "test-key",
            OPENCODE_DEFAULT_MODEL,
            "write note.txt",
            &file_tool,
            &exec_tool,
        )
        .expect("the bounded task finishes");
        assert_eq!(answer, "done");

        // What the server actually received on the first request.
        let first = endpoint.next_request();
        let request_line = first.head.split("\r\n").next().unwrap_or_default();
        assert_eq!(
            request_line,
            format!("POST {OPENCODE_CHAT_PATH} HTTP/1.1"),
            "method and path (assumption 1)"
        );
        assert!(
            first
                .head
                .split("\r\n")
                .any(|line| line.eq_ignore_ascii_case("Authorization: Bearer test-key")),
            "the bearer header reached the server: {:?}",
            first.head
        );
        let sent: serde_json::Value =
            serde_json::from_str(&first.body).expect("the request body is JSON");
        assert_eq!(sent["model"], OPENCODE_DEFAULT_MODEL);
        assert_eq!(sent["stream"], serde_json::Value::Bool(false));
        assert_eq!(sent["messages"][0]["role"], "system");
        assert_eq!(sent["messages"][1]["content"], "write note.txt");
        let advertised: Vec<&str> = sent["tools"]
            .as_array()
            .expect("tools is an array")
            .iter()
            .map(|tool| tool["function"]["name"].as_str().expect("a tool name"))
            .collect();
        assert_eq!(advertised, vec!["file", "exec"], "both tools advertised");

        // The second request must carry the assistant echo and the tool
        // result, matched by id -- that is what makes a second turn answerable.
        let second = endpoint.next_request();
        let resumed: serde_json::Value =
            serde_json::from_str(&second.body).expect("the second body is JSON");
        assert_eq!(resumed["messages"][2]["tool_calls"][0]["id"], "call-1");
        assert_eq!(resumed["messages"][3]["role"], "tool");
        assert_eq!(resumed["messages"][3]["tool_call_id"], "call-1");
        assert!(
            resumed["messages"][3]["content"]
                .as_str()
                .unwrap_or_default()
                .contains("wrote 14 bytes to note.txt"),
            "{:?}",
            resumed["messages"][3]["content"]
        );

        // And the real effect: the model's file landed under the task root.
        assert_eq!(
            std::fs::read_to_string(root.join("note.txt")).expect("the model's file exists"),
            "from the model"
        );
    }

    #[test]
    fn opencode_round_trip_carries_a_tool_refusal_back_to_the_model() {
        let root = fixture("refusal");
        let file_tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("file tool");
        let exec_tool = ExecTool::new(file_tool.root_path(), &[]);

        let endpoint = FakeEndpoint::start(vec![
            http_reply(
                200,
                &serde_json::json!({
                    "choices": [{ "message": { "tool_calls": [{
                        "id": "call-esc",
                        "function": {
                            "name": "file",
                            "arguments": "{\"op\":\"read\",\"path\":\"../outside.txt\"}"
                        }
                    }] } }]
                })
                .to_string(),
            ),
            http_reply(
                200,
                &serde_json::json!({
                    "choices": [{ "message": { "content": "understood" } }]
                })
                .to_string(),
            ),
        ]);

        let answer = opencode_run_task(
            &NetworkHttp,
            &endpoint.url,
            "k",
            "m",
            "read outside",
            &file_tool,
            &exec_tool,
        )
        .expect("a tool refusal does not abort the run");
        assert_eq!(answer, "understood");

        let _first = endpoint.next_request();
        let second = endpoint.next_request();
        let resumed: serde_json::Value =
            serde_json::from_str(&second.body).expect("the second body is JSON");
        let result = resumed["messages"][3]["content"]
            .as_str()
            .expect("the tool result is a string")
            .to_owned();
        assert!(result.contains("refused"), "{result}");
        assert!(
            result.contains(".."),
            "the refusal names the cause: {result}"
        );
    }

    // -- bounded failures, each carrying the underlying detail ---------------

    #[test]
    fn opencode_non_2xx_status_is_a_bounded_error_carrying_the_server_text() {
        let root = fixture("status");
        let file_tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("file tool");
        let exec_tool = ExecTool::new(file_tool.root_path(), &[]);
        let endpoint = FakeEndpoint::start(vec![http_reply(
            503,
            "{\"error\":{\"message\":\"model loader is warming up\"}}",
        )]);

        let error = opencode_run_task(
            &NetworkHttp,
            &endpoint.url,
            "k",
            "m",
            "anything",
            &file_tool,
            &exec_tool,
        )
        .expect_err("a 503 stops the run");
        assert!(error.contains("503"), "the status is named: {error}");
        assert!(
            error.contains("model loader is warming up"),
            "the server's own words survive: {error}"
        );
    }

    #[test]
    fn opencode_truncated_json_body_is_a_bounded_error_carrying_the_parse_detail() {
        let root = fixture("truncated");
        let file_tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("file tool");
        let exec_tool = ExecTool::new(file_tool.root_path(), &[]);
        let endpoint = FakeEndpoint::start(vec![http_reply(200, "{\"choices\": [{\"mess")]);

        let error = opencode_run_task(
            &NetworkHttp,
            &endpoint.url,
            "k",
            "m",
            "anything",
            &file_tool,
            &exec_tool,
        )
        .expect_err("a truncated body stops the run");
        assert!(error.contains("not JSON"), "{error}");
        // The serde detail and an excerpt of the body both survive: this is
        // the generic-string collapse the precision audit's item 70 faulted.
        assert!(error.contains("line 1 column"), "serde detail: {error}");
        assert!(error.contains("choices"), "body excerpt: {error}");
    }

    #[test]
    fn opencode_reply_past_the_ceiling_is_refused_naming_the_bound_and_the_size() {
        let oversized = format!(
            "{{\"filler\":\"{}\"}}",
            "x".repeat(OPENCODE_MAX_REPLY_BYTES + 1)
        );
        let error =
            opencode_parse_reply(&oversized).expect_err("a body past the ceiling is refused");
        assert!(
            error.contains(&OPENCODE_MAX_REPLY_BYTES.to_string()),
            "the bound is named: {error}"
        );
        assert!(
            error.contains(&oversized.len().to_string()),
            "the measured size is named: {error}"
        );
        assert!(
            error.contains("OPENCODE_MAX_REPLY_BYTES"),
            "the constant is named so it can be found: {error}"
        );
    }

    #[test]
    fn opencode_server_error_object_and_missing_fields_are_named_not_generic() {
        let refused = opencode_parse_reply("{\"error\":{\"message\":\"no such model: zz\"}}")
            .expect_err("an error object is a refusal");
        assert!(refused.contains("no such model: zz"), "{refused}");

        let shapeless =
            opencode_parse_reply("{\"choices\":[]}").expect_err("no message is a refusal");
        assert!(shapeless.contains("choices[0].message"), "{shapeless}");

        let idless = opencode_parse_reply(
            "{\"choices\":[{\"message\":{\"tool_calls\":[{\"function\":{\"name\":\"file\"}}]}}]}",
        )
        .expect_err("a tool call with no id is unanswerable");
        assert!(idless.contains("\"file\""), "the tool is named: {idless}");
        assert!(idless.contains("no id"), "{idless}");

        let contentless = opencode_parse_reply("{\"choices\":[{\"message\":{}}]}")
            .expect_err("neither text nor calls is a refusal");
        assert!(
            contentless.contains("neither tool calls nor text content"),
            "{contentless}"
        );
    }

    #[test]
    fn opencode_turn_bound_stops_a_looping_model_naming_its_own_constant() {
        let root = fixture("turn-bound");
        let file_tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("file tool");
        let exec_tool = ExecTool::new(file_tool.root_path(), &[]);
        // Exactly as many replies as the bound permits, each asking for a
        // tool call, so the loop can only end at the bound -- and the fake
        // endpoint is never left waiting for a connection that never comes.
        let looping = serde_json::json!({
            "choices": [{ "message": { "tool_calls": [{
                "id": "loop",
                "function": { "name": "file", "arguments": "{\"op\":\"read\",\"path\":\"x\"}" }
            }] } }]
        })
        .to_string();
        let endpoint = FakeEndpoint::start(
            (0..OPENCODE_MAX_TURNS)
                .map(|_| http_reply(200, &looping))
                .collect(),
        );

        let error = opencode_run_task(
            &NetworkHttp,
            &endpoint.url,
            "k",
            "m",
            "loop forever",
            &file_tool,
            &exec_tool,
        )
        .expect_err("the turn bound stops the run");
        assert!(
            error.contains(&OPENCODE_MAX_TURNS.to_string()),
            "the bound is named: {error}"
        );
        assert!(error.contains("OPENCODE_MAX_TURNS"), "{error}");
        // The bound is this module's own, not DeepSeek's: asserted as a
        // compile-time const block, since both are constants.
        const {
            assert!(OPENCODE_MAX_TURNS < crate::harness_wire::HARNESS_MAX_TURNS);
        }
    }

    #[test]
    fn opencode_tool_call_bound_stops_a_single_greedy_turn() {
        let root = fixture("call-bound");
        let file_tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("file tool");
        let exec_tool = ExecTool::new(file_tool.root_path(), &[]);
        let calls: Vec<serde_json::Value> = (0..OPENCODE_MAX_TOOL_CALLS + 1)
            .map(|index| {
                serde_json::json!({
                    "id": format!("call-{index}"),
                    "function": {
                        "name": "file",
                        "arguments": "{\"op\":\"read\",\"path\":\"x\"}"
                    }
                })
            })
            .collect();
        let greedy =
            serde_json::json!({ "choices": [{ "message": { "tool_calls": calls } }] }).to_string();
        let endpoint = FakeEndpoint::start(vec![http_reply(200, &greedy)]);

        let error = opencode_run_task(
            &NetworkHttp,
            &endpoint.url,
            "k",
            "m",
            "ask for everything at once",
            &file_tool,
            &exec_tool,
        )
        .expect_err("the tool-call bound stops the run");
        assert!(error.contains("OPENCODE_MAX_TOOL_CALLS"), "{error}");
        assert!(
            error.contains(&(OPENCODE_MAX_TOOL_CALLS + 1).to_string()),
            "the refused count is named: {error}"
        );
    }
}
