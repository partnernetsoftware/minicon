//! The `harness` backend wire layer: how one bounded task reaches a model and
//! how that model's tool calls reach `harness`'s two tools.
//!
//! Design of record: `prd/PRD_02_31_v0_2_horizon.md` ("harness -- detail"),
//! sequencing in `plan/plan-v0.2.0.md` (H4, H5).
//!
//! Split from `harness.rs` on purpose. The two tools' bounds (H2, H3) are the
//! product's security surface and are complete; everything here is the
//! transport and the turn loop, which is where a backend's own wire quirks
//! live. Keeping them in separate files keeps a wire-format fix from touching
//! a bound.

// Every item below is reached only by the H5 adapter in `harness.rs`, which is
// another worker's file and does not call it yet, plus by this file's own
// tests. Same scoped, non-test allowance the two tools carry, for the same
// reason: faking a caller to make it reachable would be worse than saying so.
#![cfg_attr(not(test), allow(dead_code))]

/// One HTTP round trip, so the turn loop can be tested without a network.
///
/// Deliberately this small: `harness` posts one JSON body and reads one JSON
/// body back. Anything richer would be a general HTTP client, which MiniCon
/// does not have and does not want.
pub trait Transport {
    /// POSTs `body` as `application/json` and returns the response body.
    ///
    /// A non-2xx status is an `Err` carrying the status and as much of the
    /// body as the endpoint sent, since that is where an API states why it
    /// refused (a bad key, a rejected model name).
    fn post_json(&self, url: &str, bearer: &str, body: &str) -> Result<String, String>;
}

// ---------------------------------------------------------------------------
// H4 -- the transport
// ---------------------------------------------------------------------------

/// Largest response body `PlainHttp` will accumulate. A response body becomes
/// a model reply that becomes a prompt, so the bound belongs to the read, for
/// the same reason `FILE_TOOL_MAX_READ_BYTES` does.
pub const PLAIN_HTTP_MAX_RESPONSE_BYTES: usize = 4 * 1024 * 1024;

/// How long one round trip may take before the socket is abandoned.
pub const PLAIN_HTTP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(120);

/// An ordinary HTTP/1.1 POST over `std::net::TcpStream`, and nothing more.
///
/// This is not a placeholder: an opencode-go-compatible endpoint normally
/// listens on loopback over plain HTTP, which is exactly what this reaches.
/// What it cannot reach is anything `https://`, because MiniCon has no TLS
/// capability anywhere -- neither in this crate nor in `agenterm-platform`,
/// whose network features are DNS, interfaces and routes only. An `https://`
/// URL is therefore **refused by name** rather than silently downgraded to
/// port 80 or wrapped in something that only looks encrypted.
#[derive(Clone, Copy, Debug, Default)]
pub struct PlainHttp;

impl Transport for PlainHttp {
    fn post_json(&self, url: &str, bearer: &str, body: &str) -> Result<String, String> {
        use std::io::{Read as _, Write as _};

        let (authority, path) = split_plain_http_url(url)?;
        let host = authority
            .rsplit_once(':')
            .map_or(authority, |(host, _)| host)
            .to_owned();
        let address = if authority.contains(':') {
            authority.to_owned()
        } else {
            format!("{authority}:80")
        };

        let mut stream = std::net::TcpStream::connect(&address)
            .map_err(|error| format!("harness: cannot connect to {address}: {error}"))?;
        stream
            .set_read_timeout(Some(PLAIN_HTTP_TIMEOUT))
            .and_then(|()| stream.set_write_timeout(Some(PLAIN_HTTP_TIMEOUT)))
            .map_err(|error| format!("harness: cannot bound the socket's timeouts: {error}"))?;

        // `Connection: close` on purpose: with no keep-alive there is no
        // second response framing to get wrong, and end-of-stream is a valid
        // end of body even if the endpoint omits Content-Length.
        let request = format!(
            "POST {path} HTTP/1.1\r\nHost: {host}\r\nAuthorization: Bearer {bearer}\r\n\
             Content-Type: application/json\r\nAccept: application/json\r\n\
             Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
            body.len()
        );
        stream
            .write_all(request.as_bytes())
            .and_then(|()| stream.flush())
            .map_err(|error| {
                format!("harness: writing the request to {address} failed: {error}")
            })?;

        let mut raw = Vec::new();
        let mut chunk = [0u8; 8192];
        loop {
            let read = stream.read(&mut chunk).map_err(|error| {
                format!("harness: reading the reply from {address} failed: {error}")
            })?;
            if read == 0 {
                break;
            }
            raw.extend_from_slice(&chunk[..read]);
            if raw.len() > PLAIN_HTTP_MAX_RESPONSE_BYTES {
                return Err(format!(
                    "harness: the reply from {address} exceeds {PLAIN_HTTP_MAX_RESPONSE_BYTES} \
                     bytes and was abandoned"
                ));
            }
        }
        parse_plain_http_response(&raw)
    }
}

/// Splits a plain-HTTP URL into its authority and its request target.
///
/// Separate from the socket work so the `https://` refusal is testable without
/// a network, which is the only way it can be tested at all.
fn split_plain_http_url(url: &str) -> Result<(&str, &str), String> {
    if url.starts_with("https://") {
        return Err(format!(
            "harness: {url:?} is refused: MiniCon has no TLS capability yet, in this crate or in \
             agenterm-platform, so an https:// endpoint cannot be reached and will not be \
             downgraded to plain HTTP to pretend otherwise; tracked as the H4 transport item in \
             plan/plan-v0.2.0.md. A plain http:// endpoint (an opencode-go-compatible server on \
             loopback) works today."
        ));
    }
    let rest = url.strip_prefix("http://").ok_or_else(|| {
        format!("harness: {url:?} is refused: the transport speaks only http:// URLs")
    })?;
    let (authority, path) = match rest.find('/') {
        Some(index) => (&rest[..index], &rest[index..]),
        None => (rest, "/"),
    };
    if authority.is_empty() {
        return Err(format!("harness: {url:?} is refused: it names no host"));
    }
    Ok((authority, path))
}

/// Turns raw response bytes into the body, or into an error naming the status.
fn parse_plain_http_response(raw: &[u8]) -> Result<String, String> {
    let split = raw
        .windows(4)
        .position(|window| window == b"\r\n\r\n")
        .ok_or_else(|| "harness: the reply has no complete HTTP header block".to_owned())?;
    let head = String::from_utf8_lossy(&raw[..split]).into_owned();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().unwrap_or_default();
    let status = status_line
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse::<u16>().ok())
        .ok_or_else(|| format!("harness: the reply has no HTTP status: {status_line:?}"))?;

    let mut body = raw[split + 4..].to_vec();
    // Honour Content-Length when it is present and shorter than what arrived;
    // otherwise end-of-stream is the end of the body (Connection: close).
    for line in lines {
        if let Some(value) = line
            .split_once(':')
            .filter(|(name, _)| name.eq_ignore_ascii_case("content-length"))
            .and_then(|(_, value)| value.trim().parse::<usize>().ok())
        {
            if value < body.len() {
                body.truncate(value);
            } else if value > body.len() {
                return Err(format!(
                    "harness: the reply promised {value} body bytes but the connection ended \
                     after {}",
                    body.len()
                ));
            }
            break;
        }
    }
    let body = String::from_utf8_lossy(&body).into_owned();
    if (200..300).contains(&status) {
        Ok(body)
    } else {
        Err(format!(
            "harness: the endpoint answered HTTP {status}: {body}"
        ))
    }
}

// ---------------------------------------------------------------------------
// H4 -- the DeepSeek chat-completions codec
// ---------------------------------------------------------------------------

/// DeepSeek's chat-completions endpoint and default model.
///
/// Named here and nowhere else so there is one place to correct. Note the
/// `https://`: reaching it needs the TLS capability MiniCon does not have yet,
/// which `PlainHttp` refuses by name rather than faking.
pub const DEEPSEEK_CHAT_URL: &str = "https://api.deepseek.com/chat/completions";
pub const DEEPSEEK_MODEL: &str = "deepseek-chat";

/// One tool call the model asked for, with its arguments still unparsed.
///
/// The arguments arrive as a JSON string inside a JSON object, so they are
/// carried verbatim and parsed at dispatch: a malformed argument object is then
/// one tool's bounded failure, reportable back to the model, rather than a
/// parse error that kills the whole reply.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WireToolCall {
    pub id: String,
    pub name: String,
    pub arguments: String,
}

/// What one model reply amounted to.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ModelReply {
    /// A final answer: the loop stops here.
    Text(String),
    /// Work to do before the model can answer.
    ToolCalls(Vec<WireToolCall>),
}

/// The two tools, as this backend wants tool definitions written.
///
/// Both are advertised **always**, including when `--allow-cmd` is empty. An
/// empty allow-list makes every `exec` call refused (`ExecTool::admit`); it
/// does not hide the tool, so a model cannot tell "not allowed yet" from "tool
/// absent" by probing. This shape is written for DeepSeek's documented
/// chat-completions request and is not claimed to describe any other backend:
/// the second backend gets its own codec, never one inferred from this one.
fn deepseek_tool_definitions() -> serde_json::Value {
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

/// Serializes one request body: the model, the conversation so far, and both
/// tool definitions.
fn deepseek_request_body(model: &str, messages: &[serde_json::Value]) -> String {
    serde_json::json!({
        "model": model,
        "messages": messages,
        "tools": deepseek_tool_definitions(),
        // No sampling knobs and no streaming: one bounded task wants one
        // reproducible round trip, and a streamed body would need a framing
        // parser this transport deliberately does not have.
        "stream": false,
    })
    .to_string()
}

/// Parses one response body into either a final text answer or tool calls.
///
/// A reply carrying both is treated as tool calls: the text is the model
/// narrating what it is about to do, not an answer, and stopping there would
/// drop the work it asked for.
fn deepseek_parse_reply(body: &str) -> Result<ModelReply, String> {
    let value: serde_json::Value = serde_json::from_str(body)
        .map_err(|error| format!("harness: the reply is not JSON: {error}"))?;
    // An API states a refusal in its own error object; surface that verbatim
    // rather than reporting a missing field.
    if let Some(message) = value
        .get("error")
        .and_then(|error| error.get("message"))
        .and_then(serde_json::Value::as_str)
    {
        return Err(format!(
            "harness: the backend refused the request: {message}"
        ));
    }
    let message = value
        .get("choices")
        .and_then(|choices| choices.get(0))
        .and_then(|choice| choice.get("message"))
        .ok_or_else(|| "harness: the reply has no choices[0].message".to_owned())?;

    if let Some(calls) = message
        .get("tool_calls")
        .and_then(serde_json::Value::as_array)
    {
        let mut parsed = Vec::with_capacity(calls.len());
        for call in calls {
            let function = call
                .get("function")
                .ok_or_else(|| "harness: a tool call has no function object".to_owned())?;
            let name = function
                .get("name")
                .and_then(serde_json::Value::as_str)
                .ok_or_else(|| "harness: a tool call has no function name".to_owned())?;
            parsed.push(WireToolCall {
                // An id is how a tool result is matched to its call. A reply
                // without one is unanswerable, so refuse rather than invent.
                id: call
                    .get("id")
                    .and_then(serde_json::Value::as_str)
                    .ok_or_else(|| format!("harness: tool call {name:?} has no id"))?
                    .to_owned(),
                name: name.to_owned(),
                arguments: function
                    .get("arguments")
                    .and_then(serde_json::Value::as_str)
                    .unwrap_or("{}")
                    .to_owned(),
            });
        }
        if !parsed.is_empty() {
            return Ok(ModelReply::ToolCalls(parsed));
        }
    }
    let text = message
        .get("content")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "harness: the reply has neither tool calls nor text content".to_owned())?;
    Ok(ModelReply::Text(text.to_owned()))
}

// ---------------------------------------------------------------------------
// H4 -- the bounded turn loop
// ---------------------------------------------------------------------------

/// Most model round trips one task may take.
pub const HARNESS_MAX_TURNS: usize = 16;

/// Most tool calls one task may dispatch, across all turns.
///
/// Separate from the turn bound because one turn can ask for many calls, so a
/// turn count alone does not bound the work done.
pub const HARNESS_MAX_TOOL_CALLS: usize = 64;

/// The instruction the task is wrapped in, so the model knows what the two
/// tools' bounds are before it trips over them.
fn harness_system_prompt(root: &std::path::Path) -> String {
    format!(
        "You are MiniCon's bounded harness agent. You have exactly two tools, `file` and \
         `exec`, and no others. Every path is relative to the task root {root}; paths outside \
         it are refused, not rewritten. `exec` runs one allow-listed command per call with no \
         shell. A refusal is information: read it, adjust, and continue. When the task is done, \
         answer with plain text and no tool call.",
        root = root.display()
    )
}

/// Runs one bounded task to a final assistant message.
///
/// The loop stops for exactly three reasons: a final text reply (success), a
/// transport or codec error, or one of the two bounds above. A **tool refusal
/// is none of them** -- a path outside the root or an unlisted command comes
/// back to the model as an ordinary tool result so it can correct itself, and
/// it neither aborts the run nor buys another turn.
pub fn run_task(
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
            "content": harness_system_prompt(file_tool.root_path()),
        }),
        serde_json::json!({ "role": "user", "content": task }),
    ];
    let mut tool_calls_spent = 0usize;

    for _turn in 0..HARNESS_MAX_TURNS {
        let body = deepseek_request_body(model, &messages);
        let reply = transport.post_json(url, bearer, &body)?;
        match deepseek_parse_reply(&reply)? {
            ModelReply::Text(text) => return Ok(text),
            ModelReply::ToolCalls(calls) => {
                if tool_calls_spent + calls.len() > HARNESS_MAX_TOOL_CALLS {
                    return Err(format!(
                        "harness: stopped after {tool_calls_spent} tool calls: this turn's {} \
                         more would pass the bound of {HARNESS_MAX_TOOL_CALLS} \
                         (HARNESS_MAX_TOOL_CALLS); the task did not finish",
                        calls.len()
                    ));
                }
                // Echo the assistant turn back before its results: the wire
                // format matches each result to the call it answers by id, and
                // a result whose call is absent from the history is rejected.
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
                    // Both arms feed the model a string either way: the point
                    // of `unwrap_or_else` here is that a refusal is a result,
                    // not an early return.
                    let result = dispatch_tool_call(file_tool, exec_tool, call)
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
        "harness: stopped after the bound of {HARNESS_MAX_TURNS} model turns \
         (HARNESS_MAX_TURNS) without a final answer; the task did not finish"
    ))
}

/// Runs one tool call. `Err` is still something to tell the model, not a
/// reason to abandon the run -- see `run_task`.
fn dispatch_tool_call(
    file_tool: &crate::harness::FileTool,
    exec_tool: &crate::harness::ExecTool,
    call: &WireToolCall,
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
        // Refused, and named: the model invented a tool, and telling it so is
        // how it stops trying.
        other => Err(format!(
            "{other}: refused: there is no such tool; this harness has exactly `file` and `exec`"
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::harness::{ExecTool, FileTool};

    /// A fresh empty directory under the system temp dir, unique per test.
    fn fixture(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!(
            "minicon-harness-wire-{tag}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).expect("fixture root");
        std::fs::canonicalize(&root).expect("canonical fixture root")
    }

    /// A `Transport` that answers from a script and keeps every request body it
    /// was handed, so the loop and the codec are provable without a network.
    struct ScriptedTransport {
        replies: std::cell::RefCell<std::collections::VecDeque<String>>,
        sent: std::cell::RefCell<Vec<String>>,
    }

    impl ScriptedTransport {
        fn new(replies: &[&str]) -> Self {
            Self {
                replies: std::cell::RefCell::new(
                    replies.iter().map(|reply| (*reply).to_owned()).collect(),
                ),
                sent: std::cell::RefCell::new(Vec::new()),
            }
        }

        fn sent(&self) -> Vec<String> {
            self.sent.borrow().clone()
        }
    }

    impl Transport for ScriptedTransport {
        fn post_json(&self, _url: &str, _bearer: &str, body: &str) -> Result<String, String> {
            self.sent.borrow_mut().push(body.to_owned());
            self.replies
                .borrow_mut()
                .pop_front()
                .ok_or_else(|| "scripted transport: the script ran out of replies".to_owned())
        }
    }

    /// One tool-call reply, as the wire format spells it.
    fn tool_call_reply(id: &str, name: &str, arguments: serde_json::Value) -> String {
        serde_json::json!({
            "choices": [{
                "message": {
                    "role": "assistant",
                    "content": serde_json::Value::Null,
                    "tool_calls": [{
                        "id": id,
                        "type": "function",
                        "function": { "name": name, "arguments": arguments.to_string() },
                    }],
                },
            }],
        })
        .to_string()
    }

    fn text_reply(text: &str) -> String {
        serde_json::json!({ "choices": [{ "message": { "role": "assistant", "content": text } }] })
            .to_string()
    }

    fn tools(root: &std::path::Path, allow: &[&str]) -> (FileTool, ExecTool) {
        let file_tool = FileTool::new(root.to_str().expect("utf-8 root")).expect("root exists");
        let allow = allow
            .iter()
            .map(|name| (*name).to_owned())
            .collect::<Vec<_>>();
        let exec_tool = ExecTool::new(file_tool.root_path(), &allow);
        (file_tool, exec_tool)
    }

    #[test]
    fn loop_lands_a_file_write_and_feeds_the_result_back() {
        let root = fixture("write");
        let (file_tool, exec_tool) = tools(&root, &[]);
        let transport = ScriptedTransport::new(&[
            &tool_call_reply(
                "call-1",
                "file",
                serde_json::json!({ "op": "write", "path": "notes/todo.txt", "contents": "one\n" }),
            ),
            &text_reply("wrote the note"),
        ]);

        let answer = run_task(
            &transport,
            "http://localhost:1/chat",
            "key",
            DEEPSEEK_MODEL,
            "write a note",
            &file_tool,
            &exec_tool,
        )
        .expect("the task finishes");
        assert_eq!(answer, "wrote the note");

        // The bytes actually landed inside the root.
        assert_eq!(
            std::fs::read_to_string(root.join("notes/todo.txt")).expect("written file"),
            "one\n"
        );
        // ...and the result was fed back before the second round trip.
        let second = &transport.sent()[1];
        assert!(second.contains("call-1"), "{second}");
        assert!(
            second.contains("wrote 4 bytes to notes/todo.txt"),
            "{second}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_refused_path_is_reported_to_the_model_and_the_run_continues() {
        let root = fixture("refusal");
        let (file_tool, exec_tool) = tools(&root, &[]);
        let transport = ScriptedTransport::new(&[
            &tool_call_reply(
                "call-1",
                "file",
                serde_json::json!({ "op": "read", "path": "../escape.txt" }),
            ),
            &tool_call_reply(
                "call-2",
                "file",
                serde_json::json!({ "op": "write", "path": "inside.txt", "contents": "ok\n" }),
            ),
            &text_reply("corrected and done"),
        ]);

        let answer = run_task(
            &transport,
            "http://localhost:1/chat",
            "key",
            DEEPSEEK_MODEL,
            "read outside",
            &file_tool,
            &exec_tool,
        )
        .expect("a refusal must not abort the run");
        assert_eq!(answer, "corrected and done");

        // The refusal reached the model as a tool result, naming the reason.
        let second = &transport.sent()[1];
        assert!(second.contains("call-1"), "{second}");
        assert!(
            second.contains("refused") && second.contains(".."),
            "{second}"
        );
        // And the run really did keep going: three round trips, and the
        // correction landed.
        assert_eq!(transport.sent().len(), 3);
        assert_eq!(
            std::fs::read_to_string(root.join("inside.txt")).expect("corrected write"),
            "ok\n"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_unlisted_command_is_reported_to_the_model_and_the_run_continues() {
        let root = fixture("exec-refusal");
        let (file_tool, exec_tool) = tools(&root, &[]);
        let transport = ScriptedTransport::new(&[
            &tool_call_reply("call-1", "exec", serde_json::json!({ "argv": ["ls"] })),
            &text_reply("cannot run commands, stopping"),
        ]);

        let answer = run_task(
            &transport,
            "http://localhost:1/chat",
            "key",
            DEEPSEEK_MODEL,
            "list files",
            &file_tool,
            &exec_tool,
        )
        .expect("an exec refusal must not abort the run");
        assert_eq!(answer, "cannot run commands, stopping");
        let second = &transport.sent()[1];
        assert!(
            second.contains("refused") && second.contains("allow-list"),
            "{second}"
        );
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_turn_bound_is_a_bounded_error_not_an_endless_loop() {
        let root = fixture("turn-bound");
        let (file_tool, exec_tool) = tools(&root, &[]);
        // One more scripted reply than the bound allows, all of them asking for
        // work, so only the bound can stop this.
        let reply = tool_call_reply(
            "call-1",
            "file",
            serde_json::json!({ "op": "write", "path": "loop.txt", "contents": "again\n" }),
        );
        let replies = vec![reply.as_str(); HARNESS_MAX_TURNS + 1];
        let transport = ScriptedTransport::new(&replies);

        let error = run_task(
            &transport,
            "http://localhost:1/chat",
            "key",
            DEEPSEEK_MODEL,
            "loop forever",
            &file_tool,
            &exec_tool,
        )
        .expect_err("the bound must stop the run");
        assert!(
            error.contains("HARNESS_MAX_TURNS") && error.contains("did not finish"),
            "{error}"
        );
        // Bounded means bounded: exactly the allowed number of round trips.
        assert_eq!(transport.sent().len(), HARNESS_MAX_TURNS);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn both_tools_are_advertised_even_with_an_empty_allow_list() {
        let root = fixture("advertise");
        let (file_tool, exec_tool) = tools(&root, &[]);
        let transport = ScriptedTransport::new(&[&text_reply("nothing to do")]);
        run_task(
            &transport,
            "http://localhost:1/chat",
            "key",
            DEEPSEEK_MODEL,
            "do nothing",
            &file_tool,
            &exec_tool,
        )
        .expect("the task finishes");

        let body: serde_json::Value =
            serde_json::from_str(&transport.sent()[0]).expect("the request body is JSON");
        let names = body["tools"]
            .as_array()
            .expect("tools array")
            .iter()
            .map(|tool| tool["function"]["name"].as_str().expect("a tool name"))
            .collect::<Vec<_>>();
        assert_eq!(names, vec!["file", "exec"]);
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn plain_http_refuses_https_naming_the_missing_tls_capability() {
        let error = PlainHttp
            .post_json("https://api.deepseek.com/chat/completions", "key", "{}")
            .expect_err("https must be refused, not attempted");
        assert!(error.contains("no TLS capability"), "{error}");
        assert!(error.contains("H4"), "{error}");
        // The constant that needs it is the one named in the refusal's reason.
        assert!(DEEPSEEK_CHAT_URL.starts_with("https://"));
    }

    #[test]
    fn plain_http_splits_a_url_and_reports_a_non_2xx_status_with_its_body() {
        assert_eq!(
            split_plain_http_url("http://localhost:4096/v1/chat").expect("a plain url"),
            ("localhost:4096", "/v1/chat")
        );
        assert_eq!(
            split_plain_http_url("http://localhost:4096").expect("no path"),
            ("localhost:4096", "/")
        );
        let error = parse_plain_http_response(
            b"HTTP/1.1 401 Unauthorized\r\nContent-Length: 15\r\n\r\n{\"error\":true}\n",
        )
        .expect_err("a non-2xx status is an error");
        assert!(
            error.contains("HTTP 401") && error.contains("\"error\":true"),
            "{error}"
        );
        assert_eq!(
            parse_plain_http_response(b"HTTP/1.1 200 OK\r\nContent-Length: 2\r\n\r\n{}trailing")
                .expect("a 2xx body"),
            "{}"
        );
    }
}
