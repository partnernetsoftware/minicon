//! Bounded text/function-tool protocol adapters, owned by PRD 02.31.01.
//! No transport, credential lookup, tool executor or provider selection lives here.
//! Cross-protocol Chat -> Messages is explicitly non-thinking. Native Messages
//! preserves thinking/signature continuation; a route must honor the advertised
//! non-thinking requirement before dispatching a converted request.

use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
use std::time::{SystemTime, UNIX_EPOCH};

pub const MAX_EVENT_BYTES: usize = 1024 * 1024;
pub const MAX_STREAM_BYTES: usize = 32 * 1024 * 1024;
pub const MAX_REQUEST_BYTES: usize = 16 * 1024 * 1024;
pub const DEFAULT_OUTPUT_TOKENS: u64 = 4096;
const MAX_TOOLS: usize = 64;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Protocol {
    OpenAiChat,
    AnthropicMessages,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ProtocolError {
    TooLarge,
    InvalidJson,
    InvalidRequest,
    InvalidResponse,
    InvalidStream,
    TruncatedStream,
    Closed,
    UpstreamError,
    Unsupported(&'static str),
}

impl std::fmt::Display for ProtocolError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "model protocol: {self:?}")
    }
}
impl std::error::Error for ProtocolError {}

pub struct PreparedRequest {
    pub body: Vec<u8>,
    pub streaming: bool,
    pub include_usage: bool,
    pub requires_thinking_disabled: bool,
}

pub fn prepare_request(
    downstream: Protocol,
    upstream: Protocol,
    body: &[u8],
    model: &str,
) -> Result<PreparedRequest, ProtocolError> {
    model_id(model)?;
    let mut request = parse(body, MAX_REQUEST_BYTES)?;
    let source = as_object(&request, ProtocolError::InvalidRequest)?;
    validate_request(downstream, source)?;
    let streaming = optional_bool(source, "stream")?.unwrap_or(false);
    let include_usage = if downstream == Protocol::OpenAiChat {
        source
            .get("stream_options")
            .map(|options| {
                optional_bool(
                    as_object(options, ProtocolError::InvalidRequest)?,
                    "include_usage",
                )
            })
            .transpose()?
            .flatten()
            .unwrap_or(false)
    } else {
        false
    };
    let requires_thinking_disabled = downstream != upstream;
    if downstream != upstream {
        if (downstream, upstream) != (Protocol::OpenAiChat, Protocol::AnthropicMessages) {
            return Err(ProtocolError::Unsupported("request direction"));
        }
        request = chat_to_messages(source)?;
    }
    let object = request
        .as_object_mut()
        .ok_or(ProtocolError::InvalidRequest)?;
    object.insert("model".into(), model.into());
    let body = serde_json::to_vec(&request).map_err(|_| ProtocolError::InvalidJson)?;
    if body.len() > MAX_REQUEST_BYTES {
        return Err(ProtocolError::TooLarge);
    }
    Ok(PreparedRequest {
        body,
        streaming,
        include_usage,
        requires_thinking_disabled,
    })
}

fn validate_request(protocol: Protocol, request: &Map<String, Value>) -> Result<(), ProtocolError> {
    let allowed: &[&str] = match protocol {
        Protocol::OpenAiChat => &[
            "model",
            "messages",
            "tools",
            "tool_choice",
            "stream",
            "stream_options",
            "max_tokens",
            "max_completion_tokens",
            "temperature",
            "top_p",
            "stop",
            "n",
            "parallel_tool_calls",
            "user",
            "metadata",
            "thinking",
            "reasoning_effort",
        ],
        Protocol::AnthropicMessages => &[
            "model",
            "messages",
            "system",
            "max_tokens",
            "stream",
            "tools",
            "tool_choice",
            "stop_sequences",
            "temperature",
            "top_p",
            "top_k",
            "thinking",
            "output_config",
            "metadata",
        ],
    };
    fields(request, allowed)?;
    optional_bool(request, "stream")?;
    output_limit(request)?;
    if protocol == Protocol::AnthropicMessages && !request.contains_key("max_tokens") {
        return Err(ProtocolError::InvalidRequest);
    }
    if request
        .get("n")
        .is_some_and(|value| value.as_u64() != Some(1))
    {
        return Err(ProtocolError::Unsupported("multiple choices"));
    }
    if let Some(tools) = request.get("tools") {
        let tools = tools.as_array().ok_or(ProtocolError::InvalidRequest)?;
        if tools.len() > MAX_TOOLS {
            return Err(ProtocolError::TooLarge);
        }
        for tool in tools {
            let tool = as_object(tool, ProtocolError::InvalidRequest)?;
            match protocol {
                Protocol::OpenAiChat => {
                    fields(tool, &["type", "function"])?;
                    if tool.get("type").and_then(Value::as_str) != Some("function") {
                        return Err(ProtocolError::Unsupported("tool kind"));
                    }
                    let function = tool
                        .get("function")
                        .and_then(Value::as_object)
                        .ok_or(ProtocolError::InvalidRequest)?;
                    fields(function, &["name", "description", "parameters", "strict"])?;
                    required_str(function, "name", ProtocolError::InvalidRequest)?;
                    if !function.get("parameters").is_some_and(Value::is_object) {
                        return Err(ProtocolError::InvalidRequest);
                    }
                }
                Protocol::AnthropicMessages => {
                    fields(
                        tool,
                        &["name", "description", "input_schema", "cache_control"],
                    )?;
                    required_str(tool, "name", ProtocolError::InvalidRequest)?;
                    if !tool.get("input_schema").is_some_and(Value::is_object) {
                        return Err(ProtocolError::InvalidRequest);
                    }
                }
            }
        }
    }
    if let Some(system) = request.get("system") {
        content_blocks(system, true)?;
    }
    let messages = request
        .get("messages")
        .and_then(Value::as_array)
        .ok_or(ProtocolError::InvalidRequest)?;
    if messages.is_empty() || messages.len() > 4096 {
        return Err(ProtocolError::InvalidRequest);
    }
    for message in messages {
        let message = as_object(message, ProtocolError::InvalidRequest)?;
        match protocol {
            Protocol::OpenAiChat => {
                fields(
                    message,
                    &[
                        "role",
                        "content",
                        "tool_calls",
                        "tool_call_id",
                        "name",
                        "reasoning_content",
                        "refusal",
                    ],
                )?;
                let role = required_str(message, "role", ProtocolError::InvalidRequest)?;
                if !["system", "developer", "user", "assistant", "tool"].contains(&role) {
                    return Err(ProtocolError::Unsupported("message role"));
                }
                if let Some(content) = message.get("content")
                    && !content.is_null()
                {
                    content_blocks(content, true)?;
                }
                if let Some(calls) = message.get("tool_calls") {
                    chat_calls(calls)?;
                }
            }
            Protocol::AnthropicMessages => {
                fields(message, &["role", "content"])?;
                if !matches!(
                    message.get("role").and_then(Value::as_str),
                    Some("user" | "assistant")
                ) {
                    return Err(ProtocolError::Unsupported("message role"));
                }
                content_blocks(
                    message
                        .get("content")
                        .ok_or(ProtocolError::InvalidRequest)?,
                    false,
                )?;
            }
        }
    }
    Ok(())
}

fn content_blocks(content: &Value, only_text: bool) -> Result<(), ProtocolError> {
    if content.is_string() {
        return Ok(());
    }
    let blocks = content.as_array().ok_or(ProtocolError::InvalidRequest)?;
    for block in blocks {
        let block = as_object(block, ProtocolError::InvalidRequest)?;
        match required_str(block, "type", ProtocolError::InvalidRequest)? {
            "text" => {
                required_str_allow_empty(block, "text", ProtocolError::InvalidRequest)?;
            }
            "thinking" | "redacted_thinking" if !only_text => {}
            "tool_use" if !only_text => {
                required_str(block, "id", ProtocolError::InvalidRequest)?;
                required_str(block, "name", ProtocolError::InvalidRequest)?;
                if !block.get("input").is_some_and(Value::is_object) {
                    return Err(ProtocolError::InvalidRequest);
                }
            }
            "tool_result" if !only_text => {
                required_str(block, "tool_use_id", ProtocolError::InvalidRequest)?;
                if let Some(content) = block.get("content") {
                    content_blocks(content, true)?;
                }
            }
            _ => return Err(ProtocolError::Unsupported("content block")),
        }
    }
    Ok(())
}

fn chat_calls(value: &Value) -> Result<&Vec<Value>, ProtocolError> {
    let calls = value.as_array().ok_or(ProtocolError::InvalidRequest)?;
    if calls.len() > MAX_TOOLS {
        return Err(ProtocolError::TooLarge);
    }
    for call in calls {
        let call = as_object(call, ProtocolError::InvalidRequest)?;
        fields(call, &["id", "type", "function"])?;
        required_str(call, "id", ProtocolError::InvalidRequest)?;
        if call.get("type").and_then(Value::as_str) != Some("function") {
            return Err(ProtocolError::Unsupported("tool kind"));
        }
        let function = call
            .get("function")
            .and_then(Value::as_object)
            .ok_or(ProtocolError::InvalidRequest)?;
        fields(function, &["name", "arguments"])?;
        required_str(function, "name", ProtocolError::InvalidRequest)?;
        let arguments = required_str(function, "arguments", ProtocolError::InvalidRequest)?;
        if !parse(arguments.as_bytes(), MAX_EVENT_BYTES)?.is_object() {
            return Err(ProtocolError::InvalidRequest);
        }
    }
    Ok(calls)
}

fn chat_to_messages(source: &Map<String, Value>) -> Result<Value, ProtocolError> {
    for field in ["reasoning_effort", "thinking", "user", "metadata"] {
        if source.contains_key(field) {
            return Err(ProtocolError::Unsupported("cross-protocol request field"));
        }
    }
    if source.get("parallel_tool_calls") == Some(&Value::Bool(false)) {
        return Err(ProtocolError::Unsupported("parallel tool suppression"));
    }
    let mut target = Map::new();
    target.insert(
        "max_tokens".into(),
        output_limit(source)?
            .unwrap_or(DEFAULT_OUTPUT_TOKENS)
            .into(),
    );
    target.insert("thinking".into(), json!({"type":"disabled"}));
    for field in ["stream", "temperature", "top_p"] {
        if let Some(value) = source.get(field) {
            target.insert(field.into(), value.clone());
        }
    }
    if let Some(stop) = source.get("stop") {
        let stop = if stop.is_string() {
            json!([stop])
        } else {
            stop.clone()
        };
        if !stop
            .as_array()
            .is_some_and(|values| values.iter().all(Value::is_string))
        {
            return Err(ProtocolError::InvalidRequest);
        }
        target.insert("stop_sequences".into(), stop);
    }
    if let Some(options) = source.get("stream_options") {
        let options = as_object(options, ProtocolError::InvalidRequest)?;
        fields(options, &["include_usage"])?;
        optional_bool(options, "include_usage")?;
    }
    if let Some(tools) = source.get("tools").and_then(Value::as_array) {
        let mut mapped = Vec::new();
        for tool in tools {
            let function = tool["function"]
                .as_object()
                .ok_or(ProtocolError::InvalidRequest)?;
            if function.contains_key("strict") {
                return Err(ProtocolError::Unsupported("strict function schema"));
            }
            let mut output = json!({"name":function["name"],"input_schema":function["parameters"]});
            if let Some(description) = function.get("description") {
                output["description"] = description.clone();
            }
            mapped.push(output);
        }
        target.insert("tools".into(), mapped.into());
    }
    if let Some(choice) = source.get("tool_choice") {
        let mapped = match choice.as_str() {
            Some("auto") => json!({"type":"auto"}),
            Some("none") => json!({"type":"none"}),
            Some("required") => json!({"type":"any"}),
            Some(_) => return Err(ProtocolError::Unsupported("tool choice")),
            None => {
                let choice = as_object(choice, ProtocolError::InvalidRequest)?;
                fields(choice, &["type", "function"])?;
                if choice.get("type").and_then(Value::as_str) != Some("function") {
                    return Err(ProtocolError::Unsupported("tool choice"));
                }
                let function = choice
                    .get("function")
                    .and_then(Value::as_object)
                    .ok_or(ProtocolError::InvalidRequest)?;
                fields(function, &["name"])?;
                json!({"type":"tool","name":required_str(function,"name",ProtocolError::InvalidRequest)?})
            }
        };
        target.insert("tool_choice".into(), mapped);
    }
    let mut system = Vec::new();
    let mut messages: Vec<Value> = Vec::new();
    let mut outstanding = BTreeSet::new();
    let mut used = BTreeSet::new();
    for message in source["messages"]
        .as_array()
        .ok_or(ProtocolError::InvalidRequest)?
    {
        let message = message.as_object().ok_or(ProtocolError::InvalidRequest)?;
        if ["name", "reasoning_content", "refusal"]
            .iter()
            .any(|field| message.contains_key(*field))
        {
            return Err(ProtocolError::Unsupported("message continuation state"));
        }
        let role = required_str(message, "role", ProtocolError::InvalidRequest)?;
        let mut blocks = text_blocks(message.get("content"))?;
        match role {
            "system" if messages.is_empty() => {
                system.extend(blocks);
                continue;
            }
            "system" | "developer" => {
                return Err(ProtocolError::Unsupported(
                    "system position or developer role",
                ));
            }
            "assistant" => {
                if !outstanding.is_empty() {
                    return Err(ProtocolError::InvalidRequest);
                }
                if let Some(calls) = message.get("tool_calls") {
                    for call in chat_calls(calls)? {
                        let id = call["id"].as_str().ok_or(ProtocolError::InvalidRequest)?;
                        if !used.insert(id.to_owned()) {
                            return Err(ProtocolError::InvalidRequest);
                        }
                        outstanding.insert(id.to_owned());
                        let input = parse(
                            call["function"]["arguments"]
                                .as_str()
                                .ok_or(ProtocolError::InvalidRequest)?
                                .as_bytes(),
                            MAX_EVENT_BYTES,
                        )?;
                        blocks.push(json!({"type":"tool_use","id":id,"name":call["function"]["name"],"input":input}));
                    }
                }
            }
            "tool" => {
                let id = required_str(message, "tool_call_id", ProtocolError::InvalidRequest)?;
                if !outstanding.remove(id) {
                    return Err(ProtocolError::InvalidRequest);
                }
                blocks = vec![json!({"type":"tool_result","tool_use_id":id,"content":blocks})];
            }
            "user" => {
                if !outstanding.is_empty() {
                    return Err(ProtocolError::InvalidRequest);
                }
            }
            _ => return Err(ProtocolError::Unsupported("message role")),
        }
        let mapped_role = if role == "assistant" {
            "assistant"
        } else {
            "user"
        };
        if let Some(previous) = messages
            .last_mut()
            .filter(|previous| previous["role"] == mapped_role)
        {
            previous["content"]
                .as_array_mut()
                .ok_or(ProtocolError::InvalidRequest)?
                .extend(blocks);
        } else {
            messages.push(json!({"role":mapped_role,"content":blocks}));
        }
    }
    if !outstanding.is_empty() || messages.is_empty() {
        return Err(ProtocolError::InvalidRequest);
    }
    if !system.is_empty() {
        target.insert("system".into(), system.into());
    }
    target.insert("messages".into(), messages.into());
    Ok(target.into())
}

fn text_blocks(value: Option<&Value>) -> Result<Vec<Value>, ProtocolError> {
    match value {
        None | Some(Value::Null) => Ok(vec![]),
        Some(Value::String(text)) => Ok(vec![json!({"type":"text","text":text})]),
        Some(Value::Array(blocks)) => {
            for block in blocks {
                cross_fields(block, &["type", "text"], ProtocolError::InvalidRequest)?;
                if block["type"] != "text" || !block["text"].is_string() {
                    return Err(ProtocolError::Unsupported("text content"));
                }
            }
            Ok(blocks.clone())
        }
        _ => Err(ProtocolError::InvalidRequest),
    }
}

pub fn convert_response(
    downstream: Protocol,
    upstream: Protocol,
    body: &[u8],
    model: &str,
) -> Result<Vec<u8>, ProtocolError> {
    model_id(model)?;
    let value = parse(body, MAX_STREAM_BYTES)?;
    let object = as_object(&value, ProtocolError::InvalidResponse)?;
    if object.contains_key("error") {
        return Err(ProtocolError::UpstreamError);
    }
    if downstream == upstream {
        match upstream {
            Protocol::OpenAiChat => {
                let choices = value["choices"]
                    .as_array()
                    .ok_or(ProtocolError::InvalidResponse)?;
                if choices.len() != 1 {
                    return Err(ProtocolError::Unsupported("multiple choices"));
                }
                let message = &choices[0]["message"];
                if let Some(content) = message.get("content")
                    && !content.is_null()
                {
                    content_blocks(content, true)
                        .map_err(|_| ProtocolError::Unsupported("response content"))?;
                }
                if !choices[0]["finish_reason"].is_string() {
                    return Err(ProtocolError::InvalidResponse);
                }
            }
            Protocol::AnthropicMessages => {
                content_blocks(
                    value.get("content").ok_or(ProtocolError::InvalidResponse)?,
                    false,
                )
                .map_err(|_| ProtocolError::Unsupported("response content"))?;
                if !value["stop_reason"].is_string() {
                    return Err(ProtocolError::InvalidResponse);
                }
            }
        }
        return Ok(body.to_vec());
    }
    if (downstream, upstream) != (Protocol::OpenAiChat, Protocol::AnthropicMessages) {
        return Err(ProtocolError::Unsupported("response direction"));
    }
    let mut text = String::new();
    let mut tools = Vec::new();
    let mut ids = BTreeSet::new();
    for block in value["content"]
        .as_array()
        .ok_or(ProtocolError::InvalidResponse)?
    {
        match block["type"].as_str() {
            Some("text") => {
                cross_fields(block, &["type", "text"], ProtocolError::InvalidResponse)?;
                let part = block["text"]
                    .as_str()
                    .ok_or(ProtocolError::InvalidResponse)?;
                if !tools.is_empty() && !part.is_empty() {
                    return Err(ProtocolError::Unsupported("text after tool block"));
                }
                text.push_str(part);
            }
            Some("tool_use") => {
                cross_fields(
                    block,
                    &["type", "id", "name", "input"],
                    ProtocolError::InvalidResponse,
                )?;
                if tools.len() >= MAX_TOOLS {
                    return Err(ProtocolError::TooLarge);
                }
                let id = block["id"]
                    .as_str()
                    .filter(|id| !id.is_empty())
                    .ok_or(ProtocolError::InvalidResponse)?;
                let name = block["name"]
                    .as_str()
                    .filter(|name| !name.is_empty())
                    .ok_or(ProtocolError::InvalidResponse)?;
                if !ids.insert(id) || !block["input"].is_object() {
                    return Err(ProtocolError::InvalidResponse);
                }
                tools.push(json!({"id":id,"type":"function","function":{"name":name,"arguments":serde_json::to_string(&block["input"]).map_err(|_| ProtocolError::InvalidResponse)?}}));
            }
            _ => return Err(ProtocolError::Unsupported("thinking or response block")),
        }
    }
    let finish = finish_reason(
        value["stop_reason"]
            .as_str()
            .ok_or(ProtocolError::InvalidResponse)?,
    )?;
    if finish == "tool_calls" && tools.is_empty() {
        return Err(ProtocolError::InvalidResponse);
    }
    let mut message = json!({"role":"assistant","content":if text.is_empty() && !tools.is_empty() { Value::Null } else { text.into() }});
    if !tools.is_empty() {
        message["tool_calls"] = tools.into();
    }
    let id = required_str(object, "id", ProtocolError::InvalidResponse)?;
    let actual_model = required_str(object, "model", ProtocolError::InvalidResponse)?;
    let mut result = json!({"id":id,"object":"chat.completion","created":created(),"model":actual_model,"choices":[{"index":0,"message":message,"finish_reason":finish}]});
    let mut usage = Usage::default();
    usage.update(value.get("usage"))?;
    if let Some(mapped) = usage.chat()? {
        result["usage"] = mapped;
    }
    serde_json::to_vec(&result).map_err(|_| ProtocolError::InvalidResponse)
}

#[derive(Default)]
struct Usage {
    input: Option<u64>,
    output: Option<u64>,
    cache_read: Option<u64>,
    cache_create: Option<u64>,
}
impl Usage {
    fn update(&mut self, value: Option<&Value>) -> Result<(), ProtocolError> {
        let Some(value) = value else {
            return Ok(());
        };
        let object = as_object(value, ProtocolError::InvalidResponse)?;
        for (name, target) in [
            ("input_tokens", &mut self.input),
            ("output_tokens", &mut self.output),
            ("cache_read_input_tokens", &mut self.cache_read),
            ("cache_creation_input_tokens", &mut self.cache_create),
        ] {
            if let Some(value) = object.get(name) {
                *target = Some(value.as_u64().ok_or(ProtocolError::InvalidResponse)?);
            }
        }
        Ok(())
    }
    fn chat(&self) -> Result<Option<Value>, ProtocolError> {
        let (Some(input), Some(output)) = (self.input, self.output) else {
            return Ok(None);
        };
        let prompt = input
            .checked_add(self.cache_read.unwrap_or(0))
            .and_then(|value| value.checked_add(self.cache_create.unwrap_or(0)))
            .ok_or(ProtocolError::InvalidResponse)?;
        let total = prompt
            .checked_add(output)
            .ok_or(ProtocolError::InvalidResponse)?;
        let mut value =
            json!({"prompt_tokens":prompt,"completion_tokens":output,"total_tokens":total});
        if let Some(cached) = self.cache_read {
            value["prompt_tokens_details"] = json!({"cached_tokens":cached});
        }
        Ok(Some(value))
    }
}

struct Block {
    kind: BlockKind,
    closed: bool,
}
enum BlockKind {
    Text,
    Opaque,
    Tool {
        index: usize,
        arguments: String,
        initial_complete: bool,
    },
}

pub struct SseBridge {
    downstream: Protocol,
    upstream: Protocol,
    model: String,
    id: Option<String>,
    created: u64,
    line: Vec<u8>,
    data: Vec<u8>,
    event: Option<String>,
    frame_bytes: usize,
    input_bytes: usize,
    output_bytes: usize,
    skip_lf: bool,
    started: bool,
    stopped: bool,
    failed: bool,
    finish: Option<String>,
    blocks: BTreeMap<u64, Block>,
    tool_ids: BTreeSet<String>,
    tools: usize,
    usage: Usage,
    include_usage: bool,
}

impl SseBridge {
    pub fn new(
        downstream: Protocol,
        upstream: Protocol,
        model: &str,
    ) -> Result<Self, ProtocolError> {
        model_id(model)?;
        if downstream != upstream
            && (downstream, upstream) != (Protocol::OpenAiChat, Protocol::AnthropicMessages)
        {
            return Err(ProtocolError::Unsupported("stream direction"));
        }
        Ok(Self {
            downstream,
            upstream,
            model: model.into(),
            id: None,
            created: created(),
            line: vec![],
            data: vec![],
            event: None,
            frame_bytes: 0,
            input_bytes: 0,
            output_bytes: 0,
            skip_lf: false,
            started: false,
            stopped: false,
            failed: false,
            finish: None,
            blocks: BTreeMap::new(),
            tool_ids: BTreeSet::new(),
            tools: 0,
            usage: Usage::default(),
            include_usage: false,
        })
    }

    /// Only cross-protocol Chat output is shaped by this option. Native
    /// streams retain their upstream usage/event semantics unchanged.
    #[must_use]
    pub fn with_include_usage(mut self, include_usage: bool) -> Self {
        self.include_usage = include_usage;
        self
    }

    /// Returns complete encoded events as soon as their upstream frames arrive.
    /// Native output preserves event types and JSON semantics, including opaque
    /// fields. SSE whitespace, line endings and JSON key order are normalized.
    /// Only one incomplete SSE frame and bounded tool arguments are retained.
    pub fn push(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
        if self.failed {
            return Err(ProtocolError::Closed);
        }
        let result = self.push_inner(bytes);
        if result.is_err() {
            self.failed = true;
        }
        result
    }

    fn push_inner(&mut self, bytes: &[u8]) -> Result<Vec<Vec<u8>>, ProtocolError> {
        self.input_bytes = self
            .input_bytes
            .checked_add(bytes.len())
            .ok_or(ProtocolError::TooLarge)?;
        if self.input_bytes > MAX_STREAM_BYTES {
            return Err(ProtocolError::TooLarge);
        }
        let mut output = Vec::new();
        for byte in bytes {
            if *byte == b'\n' && self.skip_lf {
                self.skip_lf = false;
                continue;
            }
            self.skip_lf = false;
            self.frame_bytes += 1;
            if self.frame_bytes > MAX_EVENT_BYTES {
                return Err(ProtocolError::TooLarge);
            }
            match byte {
                b'\r' | b'\n' => {
                    self.skip_lf = *byte == b'\r';
                    if self.line.is_empty() {
                        let frames = self.dispatch()?;
                        for frame in frames {
                            self.emit(frame, &mut output)?;
                        }
                        self.frame_bytes = 0;
                        if self.failed {
                            // An in-band failure is already an output event.
                            // Do not lose it because the same transport chunk
                            // also carries [DONE] or another trailing frame.
                            return Ok(output);
                        }
                    } else {
                        let line = std::mem::take(&mut self.line);
                        let line =
                            std::str::from_utf8(&line).map_err(|_| ProtocolError::InvalidStream)?;
                        if line.starts_with(':') {
                            continue;
                        }
                        let (field, value) = line.split_once(':').unwrap_or((line, ""));
                        let value = value.strip_prefix(' ').unwrap_or(value);
                        match field {
                            "data" => {
                                self.data.extend_from_slice(value.as_bytes());
                                self.data.push(b'\n');
                            }
                            "event" => self.event = Some(value.to_owned()),
                            _ => {} // SSE ignores unknown fields; no model content is dropped.
                        }
                    }
                }
                _ => self.line.push(*byte),
            }
        }
        Ok(output)
    }

    /// EOF cannot manufacture a terminal event. An error event was already
    /// forwarded by push; its EOF remains a failure, never a successful [DONE].
    pub fn finish(&mut self) -> Result<Vec<Vec<u8>>, ProtocolError> {
        if self.failed {
            return Err(ProtocolError::UpstreamError);
        }
        if !self.line.is_empty() || !self.data.is_empty() || !self.stopped {
            self.failed = true;
            return Err(ProtocolError::TruncatedStream);
        }
        Ok(vec![])
    }

    fn emit(&mut self, frame: Vec<u8>, output: &mut Vec<Vec<u8>>) -> Result<(), ProtocolError> {
        if frame.len() > MAX_EVENT_BYTES {
            return Err(ProtocolError::TooLarge);
        }
        self.output_bytes = self
            .output_bytes
            .checked_add(frame.len())
            .ok_or(ProtocolError::TooLarge)?;
        if self.output_bytes > MAX_STREAM_BYTES {
            return Err(ProtocolError::TooLarge);
        }
        output.push(frame);
        Ok(())
    }

    fn dispatch(&mut self) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let mut data = std::mem::take(&mut self.data);
        let event = self.event.take();
        if data.is_empty() {
            return Ok(vec![]);
        }
        data.pop();
        if self.stopped || self.failed {
            return Err(ProtocolError::Closed);
        }
        if self.upstream == Protocol::OpenAiChat && data == b"[DONE]" {
            if self.finish.is_none() {
                return Err(ProtocolError::TruncatedStream);
            }
            self.stopped = true;
            return Ok(vec![b"data: [DONE]\n\n".to_vec()]);
        }
        let value = parse(&data, MAX_EVENT_BYTES).map_err(|_| ProtocolError::InvalidStream)?;
        if value.get("error").is_some()
            || event.as_deref() == Some("error")
            || value["type"] == "error"
        {
            self.failed = true;
            let kind = value["error"]["type"]
                .as_str()
                .filter(|kind| {
                    kind.len() < 128
                        && kind
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
                })
                .unwrap_or("upstream_error");
            let error = match self.downstream {
                Protocol::OpenAiChat => {
                    json!({"error":{"type":kind,"message":"Upstream model stream failed"}})
                }
                Protocol::AnthropicMessages => {
                    json!({"type":"error","error":{"type":kind,"message":"Upstream model stream failed"}})
                }
            };
            return Ok(vec![sse(
                if self.downstream == Protocol::AnthropicMessages {
                    Some("error")
                } else {
                    None
                },
                &error,
            )?]);
        }
        match self.upstream {
            Protocol::OpenAiChat => {
                let choices = value["choices"]
                    .as_array()
                    .ok_or(ProtocolError::InvalidStream)?;
                if choices.len() > 1 {
                    return Err(ProtocolError::Unsupported("multiple choices"));
                }
                for choice in choices {
                    if choice["index"].as_u64() != Some(0) || !choice["delta"].is_object() {
                        return Err(ProtocolError::InvalidStream);
                    }
                    if let Some(reason) = choice["finish_reason"].as_str() {
                        self.finish = Some(reason.into());
                    }
                }
                self.started = true;
                Ok(vec![sse(None, &value)?])
            }
            Protocol::AnthropicMessages => self.messages_event(event.as_deref(), &value),
        }
    }

    fn messages_event(
        &mut self,
        event: Option<&str>,
        value: &Value,
    ) -> Result<Vec<Vec<u8>>, ProtocolError> {
        let kind = value["type"].as_str().ok_or(ProtocolError::InvalidStream)?;
        if event.is_some_and(|event| event != kind) {
            return Err(ProtocolError::InvalidStream);
        }
        let native = self.downstream == Protocol::AnthropicMessages;
        let mut output = Vec::new();
        match kind {
            "ping" => {}
            "message_start" => {
                if self.started {
                    return Err(ProtocolError::InvalidStream);
                }
                let message = value["message"]
                    .as_object()
                    .ok_or(ProtocolError::InvalidStream)?;
                self.id = Some(required_str(message, "id", ProtocolError::InvalidStream)?.into());
                self.model = required_str(message, "model", ProtocolError::InvalidStream)?.into();
                if !message
                    .get("content")
                    .and_then(Value::as_array)
                    .is_some_and(Vec::is_empty)
                {
                    return Err(ProtocolError::InvalidStream);
                }
                self.usage.update(message.get("usage"))?;
                self.started = true;
                if !native {
                    output.push(self.chunk(
                        json!({"role":"assistant","content":""}),
                        None,
                        None,
                    )?);
                }
            }
            "content_block_start" => {
                self.require_started()?;
                let index = index(value)?;
                if self.blocks.contains_key(&index) || self.blocks.len() >= 256 {
                    return Err(ProtocolError::InvalidStream);
                }
                let block = &value["content_block"];
                if !native {
                    match block["type"].as_str() {
                        Some("text") => {
                            cross_fields(block, &["type", "text"], ProtocolError::InvalidStream)?
                        }
                        Some("tool_use") => cross_fields(
                            block,
                            &["type", "id", "name", "input"],
                            ProtocolError::InvalidStream,
                        )?,
                        _ => {}
                    }
                }
                let block_kind = match block["type"].as_str() {
                    Some("text") => {
                        let text = block["text"].as_str().ok_or(ProtocolError::InvalidStream)?;
                        if !native && self.tools > 0 && !text.is_empty() {
                            return Err(ProtocolError::Unsupported("text after tool block"));
                        }
                        if !native && !text.is_empty() {
                            output.push(self.chunk(json!({"content":text}), None, None)?);
                        }
                        BlockKind::Text
                    }
                    Some("tool_use") => {
                        if self.tools >= MAX_TOOLS {
                            return Err(ProtocolError::TooLarge);
                        }
                        let id = block["id"]
                            .as_str()
                            .filter(|id| !id.is_empty())
                            .ok_or(ProtocolError::InvalidStream)?;
                        let name = block["name"]
                            .as_str()
                            .filter(|name| !name.is_empty())
                            .ok_or(ProtocolError::InvalidStream)?;
                        if !self.tool_ids.insert(id.into()) || !block["input"].is_object() {
                            return Err(ProtocolError::InvalidStream);
                        }
                        let initial_complete = !block["input"]
                            .as_object()
                            .ok_or(ProtocolError::InvalidStream)?
                            .is_empty();
                        let arguments = if initial_complete {
                            serde_json::to_string(&block["input"])
                                .map_err(|_| ProtocolError::InvalidStream)?
                        } else {
                            String::new()
                        };
                        let tool = self.tools;
                        self.tools += 1;
                        if !native {
                            output.push(self.chunk(json!({"tool_calls":[{"index":tool,"id":id,"type":"function","function":{"name":name,"arguments":arguments}}]}),None,None)?);
                        }
                        BlockKind::Tool {
                            index: tool,
                            arguments,
                            initial_complete,
                        }
                    }
                    Some("thinking" | "redacted_thinking") if native => BlockKind::Opaque,
                    _ => return Err(ProtocolError::Unsupported("thinking or stream block")),
                };
                self.blocks.insert(
                    index,
                    Block {
                        kind: block_kind,
                        closed: false,
                    },
                );
            }
            "content_block_delta" => {
                self.require_started()?;
                let index = index(value)?;
                let block = self
                    .blocks
                    .get_mut(&index)
                    .filter(|block| !block.closed)
                    .ok_or(ProtocolError::InvalidStream)?;
                let delta = &value["delta"];
                if !native {
                    match delta["type"].as_str() {
                        Some("text_delta") => {
                            cross_fields(delta, &["type", "text"], ProtocolError::InvalidStream)?
                        }
                        Some("input_json_delta") => cross_fields(
                            delta,
                            &["type", "partial_json"],
                            ProtocolError::InvalidStream,
                        )?,
                        _ => {}
                    }
                }
                let mapped = match (&mut block.kind, delta["type"].as_str()) {
                    (BlockKind::Text, Some("text_delta")) => {
                        let text = delta["text"].as_str().ok_or(ProtocolError::InvalidStream)?;
                        if !native && self.tools > 0 && !text.is_empty() {
                            return Err(ProtocolError::Unsupported("text after tool block"));
                        }
                        Some(json!({"content":text}))
                    }
                    (
                        BlockKind::Tool {
                            index,
                            arguments,
                            initial_complete,
                        },
                        Some("input_json_delta"),
                    ) => {
                        let part = delta["partial_json"]
                            .as_str()
                            .ok_or(ProtocolError::InvalidStream)?;
                        if *initial_complete
                            || arguments.len().saturating_add(part.len()) > MAX_EVENT_BYTES
                        {
                            return Err(ProtocolError::InvalidStream);
                        }
                        arguments.push_str(part);
                        Some(json!({"tool_calls":[{"index":index,"function":{"arguments":part}}]}))
                    }
                    (BlockKind::Opaque, Some("thinking_delta" | "signature_delta")) if native => {
                        None
                    }
                    _ => return Err(ProtocolError::Unsupported("stream delta")),
                };
                if !native && let Some(delta) = mapped {
                    output.push(self.chunk(delta, None, None)?);
                }
            }
            "content_block_stop" => {
                self.require_started()?;
                let index = index(value)?;
                let block = self
                    .blocks
                    .get_mut(&index)
                    .filter(|block| !block.closed)
                    .ok_or(ProtocolError::InvalidStream)?;
                let empty_tool = if let BlockKind::Tool {
                    index, arguments, ..
                } = &mut block.kind
                {
                    if arguments.is_empty() {
                        *arguments = "{}".into();
                        Some(*index)
                    } else {
                        if !parse(arguments.as_bytes(), MAX_EVENT_BYTES)?.is_object() {
                            return Err(ProtocolError::InvalidStream);
                        }
                        None
                    }
                } else {
                    None
                };
                block.closed = true;
                if !native && let Some(index) = empty_tool {
                    output.push(self.chunk(
                        json!({"tool_calls":[{"index":index,"function":{"arguments":"{}"}}]}),
                        None,
                        None,
                    )?);
                }
            }
            "message_delta" => {
                self.require_started()?;
                if let Some(reason) = value["delta"]["stop_reason"].as_str() {
                    let mapped = if native {
                        reason
                    } else {
                        finish_reason(reason)?
                    };
                    if self
                        .finish
                        .as_deref()
                        .is_some_and(|previous| previous != mapped)
                    {
                        return Err(ProtocolError::InvalidStream);
                    }
                    self.finish = Some(mapped.into());
                }
                self.usage.update(value.get("usage"))?;
            }
            "message_stop" => {
                self.require_started()?;
                if self.finish.is_none() || self.blocks.values().any(|block| !block.closed) {
                    return Err(ProtocolError::TruncatedStream);
                }
                if self.finish.as_deref() == Some("tool_calls") && self.tools == 0 {
                    return Err(ProtocolError::InvalidStream);
                }
                if !native {
                    output.push(self.chunk(json!({}), self.finish.as_deref(), None)?);
                    if self.include_usage
                        && let Some(usage) = self.usage.chat()?
                    {
                        let id = self.id.as_ref().ok_or(ProtocolError::InvalidStream)?;
                        output.push(sse(
                            None,
                            &json!({
                                "id":id,"object":"chat.completion.chunk",
                                "created":self.created,"model":self.model,
                                "choices":[],"usage":usage
                            }),
                        )?);
                    }
                    output.push(b"data: [DONE]\n\n".to_vec());
                }
                self.stopped = true;
            }
            _ => return Err(ProtocolError::Unsupported("stream event")),
        }
        if native {
            output.push(sse(Some(kind), value)?);
        }
        Ok(output)
    }

    fn require_started(&self) -> Result<(), ProtocolError> {
        if !self.started || self.stopped {
            return Err(ProtocolError::InvalidStream);
        }
        Ok(())
    }

    fn chunk(
        &self,
        delta: Value,
        finish: Option<&str>,
        usage: Option<Value>,
    ) -> Result<Vec<u8>, ProtocolError> {
        let id = self.id.as_ref().ok_or(ProtocolError::InvalidStream)?;
        let mut value = json!({"id":id,"object":"chat.completion.chunk","created":self.created,"model":self.model,"choices":[{"index":0,"delta":delta,"finish_reason":finish}]});
        if let Some(usage) = usage {
            value["usage"] = usage;
        }
        sse(None, &value)
    }
}

fn sse(event: Option<&str>, value: &Value) -> Result<Vec<u8>, ProtocolError> {
    let json = serde_json::to_string(value).map_err(|_| ProtocolError::InvalidJson)?;
    Ok(match event {
        Some(event) => format!("event: {event}\ndata: {json}\n\n"),
        None => format!("data: {json}\n\n"),
    }
    .into_bytes())
}

fn index(value: &Value) -> Result<u64, ProtocolError> {
    value["index"]
        .as_u64()
        .filter(|index| *index < 256)
        .ok_or(ProtocolError::InvalidStream)
}
fn finish_reason(reason: &str) -> Result<&str, ProtocolError> {
    match reason {
        "end_turn" | "stop_sequence" => Ok("stop"),
        "max_tokens" => Ok("length"),
        "tool_use" => Ok("tool_calls"),
        _ => Err(ProtocolError::Unsupported("stop reason")),
    }
}
fn created() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |value| value.as_secs())
}
fn model_id(model: &str) -> Result<(), ProtocolError> {
    if model.is_empty() || model.len() > 256 || model.chars().any(char::is_control) {
        Err(ProtocolError::InvalidRequest)
    } else {
        Ok(())
    }
}
fn parse(body: &[u8], limit: usize) -> Result<Value, ProtocolError> {
    if body.len() > limit {
        return Err(ProtocolError::TooLarge);
    }
    serde_json::from_slice(body).map_err(|_| ProtocolError::InvalidJson)
}
fn as_object(value: &Value, error: ProtocolError) -> Result<&Map<String, Value>, ProtocolError> {
    value.as_object().ok_or(error)
}
fn fields(object: &Map<String, Value>, allowed: &[&str]) -> Result<(), ProtocolError> {
    if object
        .keys()
        .any(|field| !allowed.contains(&field.as_str()))
    {
        Err(ProtocolError::Unsupported("field"))
    } else {
        Ok(())
    }
}

/// Cross conversion cannot discard citations, tool caller state or another
/// nonempty extension. Null/empty placeholders carry no state; native paths
/// do not use this projection guard and preserve the original payload.
fn cross_fields(
    value: &Value,
    allowed: &[&str],
    error: ProtocolError,
) -> Result<(), ProtocolError> {
    let object = as_object(value, error)?;
    for (key, value) in object {
        if allowed.contains(&key.as_str()) {
            continue;
        }
        let empty = match value {
            Value::Null => true,
            Value::String(value) => value.is_empty(),
            Value::Array(value) => value.is_empty(),
            Value::Object(value) => value.is_empty(),
            _ => false,
        };
        if !empty {
            return Err(ProtocolError::Unsupported("cross-protocol content field"));
        }
    }
    Ok(())
}
fn required_str<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    error: ProtocolError,
) -> Result<&'a str, ProtocolError> {
    object
        .get(field)
        .and_then(Value::as_str)
        .filter(|value| !value.is_empty())
        .ok_or(error)
}
fn required_str_allow_empty<'a>(
    object: &'a Map<String, Value>,
    field: &str,
    error: ProtocolError,
) -> Result<&'a str, ProtocolError> {
    object.get(field).and_then(Value::as_str).ok_or(error)
}
fn optional_bool(object: &Map<String, Value>, field: &str) -> Result<Option<bool>, ProtocolError> {
    object
        .get(field)
        .map(|value| value.as_bool().ok_or(ProtocolError::InvalidRequest))
        .transpose()
}
fn output_limit(object: &Map<String, Value>) -> Result<Option<u64>, ProtocolError> {
    if object.contains_key("max_tokens") && object.contains_key("max_completion_tokens") {
        return Err(ProtocolError::InvalidRequest);
    }
    object
        .get("max_tokens")
        .or_else(|| object.get("max_completion_tokens"))
        .map(|value| {
            value
                .as_u64()
                .filter(|value| *value > 0)
                .ok_or(ProtocolError::InvalidRequest)
        })
        .transpose()
}

#[cfg(test)]
mod tests {
    use super::*;
    const CHAT: Protocol = Protocol::OpenAiChat;
    const MESSAGES: Protocol = Protocol::AnthropicMessages;
    const MODEL: &str = "synthetic-model";

    fn start() -> Vec<u8> {
        event(
            "message_start",
            json!({"type":"message_start","message":{"id":"msg_synthetic","model":MODEL,"role":"assistant","content":[],"usage":{"input_tokens":4,"output_tokens":0,"cache_read_input_tokens":2}}}),
        )
    }
    fn event(kind: &str, value: Value) -> Vec<u8> {
        sse(Some(kind), &value).unwrap()
    }
    fn end(reason: &str) -> Vec<u8> {
        [event("message_delta",json!({"type":"message_delta","delta":{"stop_reason":reason},"usage":{"output_tokens":3}})),event("message_stop",json!({"type":"message_stop"}))].concat()
    }
    fn text_stream() -> Vec<u8> {
        [start(),event("content_block_start",json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})),
        "event: content_block_delta\r\ndata: {\"type\":\"content_block_delta\",\r\ndata: \"index\":0,\"delta\":{\"type\":\"text_delta\",\"text\":\"你🙂\"}}\r\n\r\n".as_bytes().to_vec(),
        event("content_block_stop",json!({"type":"content_block_stop","index":0})),end("end_turn")].concat()
    }
    fn json_frames(frames: &[Vec<u8>]) -> Vec<Value> {
        frames
            .iter()
            .filter_map(|frame| {
                let line = std::str::from_utf8(frame)
                    .unwrap()
                    .lines()
                    .find(|line| line.starts_with("data: "))
                    .unwrap();
                let data = line.strip_prefix("data: ").unwrap();
                if data == "[DONE]" {
                    None
                } else {
                    Some(serde_json::from_str(data).unwrap())
                }
            })
            .collect()
    }

    // Independent fixture decoder: do not use the bridge or its encoder to
    // decide whether reencoded native SSE preserved event/JSON semantics.
    fn semantic_sse_frames(bytes: &[u8]) -> Vec<(Option<String>, Value)> {
        let normalized = std::str::from_utf8(bytes)
            .unwrap()
            .replace("\r\n", "\n")
            .replace('\r', "\n");
        normalized
            .split("\n\n")
            .filter_map(|frame| {
                let mut event = None;
                let mut data = Vec::new();
                for line in frame.lines() {
                    let (field, value) = line.split_once(':').unwrap_or((line, ""));
                    let value = value.strip_prefix(' ').unwrap_or(value);
                    match field {
                        "event" => event = Some(value.to_owned()),
                        "data" => data.push(value),
                        _ => {}
                    }
                }
                if data.is_empty() {
                    None
                } else {
                    Some((event, serde_json::from_str(&data.join("\n")).unwrap()))
                }
            })
            .collect()
    }

    #[test]
    fn native_messages_preserves_thinking_signature_and_request_semantics() {
        let body = json!({"model":"client-alias","max_tokens":1024,"thinking":{"type":"enabled"},"messages":[{"role":"assistant","content":[{"type":"thinking","thinking":"synthetic state","signature":"synthetic signature"},{"type":"text","text":"prior"}]},{"role":"user","content":"next"}]}).to_string();
        let prepared = prepare_request(MESSAGES, MESSAGES, body.as_bytes(), MODEL).unwrap();
        assert!(!prepared.requires_thinking_disabled);
        assert_eq!(
            serde_json::from_slice::<Value>(&prepared.body).unwrap()["max_tokens"],
            1024
        );
        let mut expected: Value = serde_json::from_str(&body).unwrap();
        expected["model"] = MODEL.into();
        assert_eq!(
            serde_json::from_slice::<Value>(&prepared.body).unwrap(),
            expected
        );
    }

    #[test]
    fn chat_tool_result_round_trip_keeps_ids_arguments_and_non_thinking_contract() {
        let body = json!({"messages":[{"role":"system","content":"bounded worker"},{"role":"user","content":"read it"},{"role":"assistant","content":null,"tool_calls":[{"id":"call_a","type":"function","function":{"name":"file","arguments":"{\"path\":\"a.txt\"}"}},{"id":"call_b","type":"function","function":{"name":"exec","arguments":"{\"program\":\"echo\"}"}}]},{"role":"tool","tool_call_id":"call_b","content":"exec result"},{"role":"tool","tool_call_id":"call_a","content":"file result"}],"tools":[{"type":"function","function":{"name":"file","parameters":{"type":"object"}}}],"stream":true,"stream_options":{"include_usage":true}}).to_string();
        let prepared = prepare_request(CHAT, MESSAGES, body.as_bytes(), MODEL).unwrap();
        assert!(prepared.streaming);
        assert!(prepared.requires_thinking_disabled);
        assert_eq!(
            serde_json::from_slice::<Value>(&prepared.body).unwrap()["max_tokens"],
            DEFAULT_OUTPUT_TOKENS
        );
        let mapped: Value = serde_json::from_slice(&prepared.body).unwrap();
        assert_eq!(mapped["thinking"], json!({"type":"disabled"}));
        assert_eq!(mapped["model"], MODEL);
        assert_eq!(mapped["messages"][1]["content"][0]["id"], "call_a");
        assert_eq!(
            mapped["messages"][1]["content"][0]["input"],
            json!({"path":"a.txt"})
        );
        assert_eq!(mapped["messages"][2]["content"][0]["tool_use_id"], "call_b");
        assert_eq!(mapped["messages"][2]["content"][1]["tool_use_id"], "call_a");
        assert_eq!(mapped["tools"][0]["input_schema"], json!({"type":"object"}));
    }

    #[test]
    fn unsupported_request_semantics_fail_before_dispatch_instead_of_disappearing() {
        for body in [
            json!({"messages":[{"role":"user","content":"x"}],"reasoning_effort":"high"}),
            json!({"messages":[{"role":"assistant","content":"x","reasoning_content":"must survive"}]}),
            json!({"messages":[{"role":"user","content":[{"type":"image_url","image_url":{"url":"https://example.invalid/image"}}]}]}),
            json!({"messages":[{"role":"user","content":"x"}],"response_format":{"type":"json_schema"}}),
            json!({"messages":[{"role":"user","content":"x"}],"parallel_tool_calls":false}),
            json!({"messages":[{"role":"user","content":"x"}],"n":2}),
            json!({"messages":[{"role":"user","content":"x"}],"max_tokens":10,"max_completion_tokens":20}),
            json!({"messages":[{"role":"tool","tool_call_id":"unknown","content":"result"}]}),
        ] {
            assert!(
                prepare_request(CHAT, MESSAGES, &serde_json::to_vec(&body).unwrap(), MODEL)
                    .is_err(),
                "accepted incompatible request"
            );
        }
        assert!(
            prepare_request(
                MESSAGES,
                CHAT,
                br#"{"max_tokens":10,"messages":[{"role":"user","content":"x"}]}"#,
                MODEL
            )
            .is_err()
        );
        assert!(
            prepare_request(
                CHAT,
                CHAT,
                br#"{"messages":[{"role":"user","content":"x"}]}"#,
                ""
            )
            .is_err()
        );
    }

    #[test]
    fn nonstream_messages_maps_tools_usage_and_finish_without_fabricated_counts() {
        let source = json!({"id":"msg_synthetic","model":MODEL,"type":"message","role":"assistant","content":[{"type":"text","text":"call"},{"type":"tool_use","id":"tool_synthetic","name":"file","input":{"path":"你.txt"}}],"stop_reason":"tool_use","usage":{"input_tokens":4,"cache_read_input_tokens":2,"cache_creation_input_tokens":1,"output_tokens":3}});
        let converted: Value = serde_json::from_slice(
            &convert_response(CHAT, MESSAGES, &serde_json::to_vec(&source).unwrap(), MODEL)
                .unwrap(),
        )
        .unwrap();
        assert_eq!(converted["choices"][0]["finish_reason"], "tool_calls");
        let call = &converted["choices"][0]["message"]["tool_calls"][0];
        assert_eq!(call["id"], "tool_synthetic");
        assert_eq!(
            serde_json::from_str::<Value>(call["function"]["arguments"].as_str().unwrap()).unwrap(),
            json!({"path":"你.txt"})
        );
        assert_eq!(converted["usage"]["prompt_tokens"], 7);
        assert_eq!(converted["usage"]["total_tokens"], 10);
        let mut missing_usage = source;
        missing_usage.as_object_mut().unwrap().remove("usage");
        let converted: Value = serde_json::from_slice(
            &convert_response(
                CHAT,
                MESSAGES,
                &serde_json::to_vec(&missing_usage).unwrap(),
                MODEL,
            )
            .unwrap(),
        )
        .unwrap();
        assert!(converted.get("usage").is_none());
    }

    #[test]
    fn thinking_continuation_is_preserved_natively_and_refused_on_cross_route() {
        let source = json!({"id":"msg_synthetic","model":MODEL,"content":[{"type":"thinking","thinking":"opaque thought","signature":"opaque signature"},{"type":"text","text":"answer"}],"stop_reason":"end_turn"});
        let encoded = serde_json::to_vec(&source).unwrap();
        assert_eq!(
            convert_response(MESSAGES, MESSAGES, &encoded, MODEL).unwrap(),
            encoded
        );
        assert_eq!(
            convert_response(CHAT, MESSAGES, &encoded, MODEL).unwrap_err(),
            ProtocolError::Unsupported("thinking or response block")
        );
    }

    #[test]
    fn every_byte_split_and_multiline_crlf_preserves_real_utf8_streaming() {
        let source = text_stream();
        let mut expected_bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        expected_bridge.created = 7;
        let expected = expected_bridge.push(&source).unwrap();
        expected_bridge.finish().unwrap();
        let frames = json_frames(&expected);
        assert!(
            frames
                .iter()
                .any(|frame| frame["choices"][0]["delta"]["content"] == "你🙂")
        );
        assert_eq!(
            frames.last().unwrap()["choices"][0]["finish_reason"],
            "stop"
        );
        assert!(frames.iter().all(|frame| frame.get("usage").is_none()));
        assert_eq!(expected.last().unwrap(), b"data: [DONE]\n\n");
        for split in 0..=source.len() {
            let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
            bridge.created = 7;
            let mut output = bridge.push(&source[..split]).unwrap();
            output.extend(bridge.push(&source[split..]).unwrap());
            bridge.finish().unwrap();
            assert_eq!(output, expected, "SSE byte split {split}");
        }
        let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        bridge.created = 7;
        let mut output = vec![];
        for byte in source {
            output.extend(bridge.push(&[byte]).unwrap());
        }
        bridge.finish().unwrap();
        assert_eq!(output, expected);
    }

    #[test]
    fn text_is_emitted_before_the_upstream_stream_finishes() {
        let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        let initial = [start(),event("content_block_start",json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"early"}}))].concat();
        let output = bridge.push(&initial).unwrap();
        assert!(
            json_frames(&output)
                .iter()
                .any(|frame| frame["choices"][0]["delta"]["content"] == "early")
        );
        assert!(!output.iter().any(|frame| frame == b"data: [DONE]\n\n"));
        assert_eq!(bridge.finish().unwrap_err(), ProtocolError::TruncatedStream);
    }

    #[test]
    fn multi_tool_stream_has_dense_indices_and_exact_incremental_arguments() {
        let source = [start(),
            event("content_block_start",json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"call_a","name":"file","input":{}}})),
            event("content_block_delta",json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"path\":"}})),
            event("content_block_delta",json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"你.txt\"}"}})),
            event("content_block_stop",json!({"type":"content_block_stop","index":2})),
            event("content_block_start",json!({"type":"content_block_start","index":5,"content_block":{"type":"tool_use","id":"call_b","name":"exec","input":{"program":"echo"}}})),
            event("content_block_stop",json!({"type":"content_block_stop","index":5})),end("tool_use")].concat();
        let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        let output = bridge.push(&source).unwrap();
        bridge.finish().unwrap();
        let mut arguments = [String::new(), String::new()];
        let mut ids = [String::new(), String::new()];
        for frame in json_frames(&output) {
            if let Some(calls) = frame["choices"][0]["delta"]["tool_calls"].as_array() {
                for call in calls {
                    let index = call["index"].as_u64().unwrap() as usize;
                    if let Some(id) = call["id"].as_str() {
                        ids[index] = id.into();
                    }
                    if let Some(part) = call["function"]["arguments"].as_str() {
                        arguments[index].push_str(part);
                    }
                }
            }
        }
        assert_eq!(ids, ["call_a", "call_b"]);
        assert_eq!(arguments[0], "{\"path\":\"你.txt\"}");
        assert_eq!(
            serde_json::from_str::<Value>(&arguments[1]).unwrap(),
            json!({"program":"echo"})
        );
        assert_eq!(
            json_frames(&output).last().unwrap()["choices"][0]["finish_reason"],
            "tool_calls"
        );
    }

    #[test]
    fn premature_done_missing_block_stop_and_bad_tool_json_cannot_succeed() {
        let mut chat = SseBridge::new(CHAT, CHAT, MODEL).unwrap();
        assert_eq!(
            chat.push(b"data: [DONE]\n\n").unwrap_err(),
            ProtocolError::TruncatedStream
        );
        let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        bridge.push(&start()).unwrap();
        assert_eq!(
            bridge
                .push(&event("message_stop", json!({"type":"message_stop"})))
                .unwrap_err(),
            ProtocolError::TruncatedStream
        );
        let source = [start(),event("content_block_start",json!({"type":"content_block_start","index":0,"content_block":{"type":"tool_use","id":"call","name":"file","input":{}}})),event("content_block_delta",json!({"type":"content_block_delta","index":0,"delta":{"type":"input_json_delta","partial_json":"{"}})),event("content_block_stop",json!({"type":"content_block_stop","index":0}))].concat();
        let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        assert!(bridge.push(&source).is_err());
        assert!(bridge.finish().is_err());
        let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        bridge.push(&start()).unwrap();
        bridge.push(&event("content_block_start",json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":"x"}}))).unwrap();
        assert!(bridge.push(&end("end_turn")).is_err());
    }

    #[test]
    fn in_band_error_is_forwarded_as_failure_without_a_synthetic_done() {
        let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        bridge.push(&start()).unwrap();
        let output = bridge.push(&event("error",json!({"type":"error","error":{"type":"overloaded_error","message":"synthetic-sensitive-provider-detail"}}))).unwrap();
        assert_eq!(json_frames(&output)[0]["error"]["type"], "overloaded_error");
        assert!(
            !String::from_utf8(output.concat())
                .unwrap()
                .contains("synthetic-sensitive-provider-detail")
        );
        assert_eq!(bridge.finish().unwrap_err(), ProtocolError::UpstreamError);
        assert_eq!(
            bridge.push(b"data: [DONE]\n\n").unwrap_err(),
            ProtocolError::Closed
        );
    }

    #[test]
    fn an_error_and_trailing_frames_in_one_chunk_keep_the_error_at_every_split() {
        for upstream in [CHAT, MESSAGES] {
            let prefix = if upstream == MESSAGES {
                start()
            } else {
                sse(None, &json!({"id":"chat_synthetic","model":MODEL,"choices":[{"index":0,"delta":{"content":"before error"},"finish_reason":null}]})).unwrap()
            };
            let failure = sse(None, &json!({"error":{"type":"overloaded_error","message":"synthetic-sensitive-detail"}})).unwrap();
            let source = [prefix, failure, b"data: [DONE]\n\n".to_vec()].concat();
            let mut baseline = SseBridge::new(CHAT, upstream, MODEL).unwrap();
            baseline.created = 7;
            let expected = baseline.push(&source).unwrap();
            assert_eq!(
                json_frames(&expected).last().unwrap()["error"]["type"],
                "overloaded_error"
            );
            assert!(!expected.iter().any(|event| event == b"data: [DONE]\n\n"));
            assert_eq!(baseline.finish(), Err(ProtocolError::UpstreamError));
            for split in 0..=source.len() {
                let mut bridge = SseBridge::new(CHAT, upstream, MODEL).unwrap();
                bridge.created = 7;
                let mut actual = bridge.push(&source[..split]).unwrap();
                match bridge.push(&source[split..]) {
                    Ok(events) => actual.extend(events),
                    Err(ProtocolError::Closed) if bridge.failed => {}
                    other => panic!("unexpected error stream result: {other:?}"),
                }
                assert_eq!(actual, expected, "error frame split {split}");
                assert_eq!(bridge.finish(), Err(ProtocolError::UpstreamError));
            }
        }
    }

    #[test]
    fn cross_content_extensions_are_refused_while_native_payloads_are_preserved() {
        let blocks = [
            json!({"type":"text","text":"answer","citations":[{"type":"char_location","start_char_index":0,"end_char_index":6}]}),
            json!({"type":"tool_use","id":"tool_synthetic","name":"file","input":{},"caller":{"type":"direct"}}),
        ];
        for block in blocks {
            let response = json!({"id":"msg_synthetic","model":MODEL,"content":[block.clone()],"stop_reason":if block["type"] == "tool_use" {"tool_use"} else {"end_turn"}});
            let encoded = serde_json::to_vec(&response).unwrap();
            assert_eq!(
                convert_response(CHAT, MESSAGES, &encoded, MODEL),
                Err(ProtocolError::Unsupported("cross-protocol content field"))
            );
            assert_eq!(
                convert_response(MESSAGES, MESSAGES, &encoded, MODEL).unwrap(),
                encoded
            );

            let source = [
                start(),
                event(
                    "content_block_start",
                    json!({"type":"content_block_start","index":0,"content_block":block}),
                ),
            ]
            .concat();
            assert_eq!(
                SseBridge::new(CHAT, MESSAGES, MODEL).unwrap().push(&source),
                Err(ProtocolError::Unsupported("cross-protocol content field"))
            );
            assert_eq!(
                SseBridge::new(MESSAGES, MESSAGES, MODEL)
                    .unwrap()
                    .push(&source)
                    .unwrap()
                    .concat(),
                source
            );
        }
        let request = json!({"messages":[{"role":"user","content":[{"type":"text","text":"hello","cache_control":{"type":"ephemeral"}}]}]}).to_string();
        assert!(matches!(
            prepare_request(CHAT, MESSAGES, request.as_bytes(), MODEL),
            Err(ProtocolError::Unsupported("cross-protocol content field"))
        ));
        assert!(prepare_request(CHAT, CHAT, request.as_bytes(), MODEL).is_ok());
        let empty_extensions = json!({"id":"msg_synthetic","model":MODEL,"content":[{"type":"text","text":"answer","citations":[]}],"stop_reason":"end_turn"});
        assert!(
            convert_response(
                CHAT,
                MESSAGES,
                &serde_json::to_vec(&empty_extensions).unwrap(),
                MODEL
            )
            .is_ok()
        );

        let source = [start(), event("content_block_start",json!({"type":"content_block_start","index":0,"content_block":{"type":"text","text":""}})), event("content_block_delta",json!({"type":"content_block_delta","index":0,"delta":{"type":"text_delta","text":"answer","citations":[{"type":"citation"}]}}))].concat();
        assert_eq!(
            SseBridge::new(CHAT, MESSAGES, MODEL).unwrap().push(&source),
            Err(ProtocolError::Unsupported("cross-protocol content field"))
        );
        assert_eq!(
            SseBridge::new(MESSAGES, MESSAGES, MODEL)
                .unwrap()
                .push(&source)
                .unwrap()
                .concat(),
            source
        );
    }

    #[test]
    fn converted_chat_usage_is_opt_in_and_has_a_separate_empty_choices_chunk() {
        let source = text_stream();
        for flag in [None, Some(false), Some(true)] {
            let mut request = json!({"stream":true,"messages":[{"role":"user","content":"hello"}]});
            if let Some(flag) = flag {
                request["stream_options"] = json!({"include_usage":flag});
            }
            let prepared = prepare_request(
                CHAT,
                MESSAGES,
                &serde_json::to_vec(&request).unwrap(),
                MODEL,
            )
            .unwrap();
            assert_eq!(prepared.include_usage, flag.unwrap_or(false));
            let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL)
                .unwrap()
                .with_include_usage(prepared.include_usage);
            let events = bridge.push(&source).unwrap();
            bridge.finish().unwrap();
            assert_eq!(events.last().unwrap(), b"data: [DONE]\n\n");
            let frames = json_frames(&events);
            let usage: Vec<_> = frames
                .iter()
                .filter(|frame| frame.get("usage").is_some())
                .collect();
            if flag == Some(true) {
                assert_eq!(usage.len(), 1);
                assert_eq!(usage[0]["choices"], json!([]));
                assert_eq!(usage[0]["usage"]["total_tokens"], 9);
                assert_eq!(frames.last().unwrap(), usage[0]);
                assert_eq!(
                    frames[frames.len() - 2]["choices"][0]["finish_reason"],
                    "stop"
                );
            } else {
                assert!(usage.is_empty());
                assert_eq!(
                    frames.last().unwrap()["choices"][0]["finish_reason"],
                    "stop"
                );
            }
            assert!(
                frames
                    .iter()
                    .filter(|frame| !frame["choices"].as_array().unwrap().is_empty())
                    .all(|frame| frame.get("usage").is_none())
            );
        }
        let mut native = SseBridge::new(MESSAGES, MESSAGES, MODEL)
            .unwrap()
            .with_include_usage(true);
        assert_eq!(
            semantic_sse_frames(&native.push(&source).unwrap().concat()),
            semantic_sse_frames(&source)
        );
        native.finish().unwrap();
    }

    #[test]
    fn native_messages_stream_keeps_thinking_and_signature_deltas() {
        let source = [start(),event("content_block_start",json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":""}})),event("content_block_delta",json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"synthetic thought"}})),event("content_block_delta",json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"synthetic signature","provider_extension":{"opaque":"retained"}}})),event("content_block_stop",json!({"type":"content_block_stop","index":0})),end("end_turn")].concat();
        let mut native = SseBridge::new(MESSAGES, MESSAGES, MODEL).unwrap();
        let actual = semantic_sse_frames(&native.push(&source).unwrap().concat());
        assert_eq!(actual, semantic_sse_frames(&source));
        assert!(
            actual
                .iter()
                .any(|(_, value)| value["delta"]["thinking"] == "synthetic thought")
        );
        assert!(actual.iter().any(|(event, value)| event.as_deref()
            == Some("content_block_delta")
            && value["delta"]["signature"] == "synthetic signature"
            && value["delta"]["provider_extension"]["opaque"] == "retained"));
        native.finish().unwrap();
        let mut cross = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        assert!(cross.push(&source).is_err());
    }

    #[test]
    fn event_stream_and_output_bounds_are_independent() {
        let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        assert_eq!(
            bridge.push(&vec![b'x'; MAX_EVENT_BYTES + 1]).unwrap_err(),
            ProtocolError::TooLarge
        );
        let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        bridge.input_bytes = MAX_STREAM_BYTES;
        assert_eq!(
            bridge.push(b": ping\n\n").unwrap_err(),
            ProtocolError::TooLarge
        );
        let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        bridge.output_bytes = MAX_STREAM_BYTES;
        assert_eq!(bridge.push(&start()).unwrap_err(), ProtocolError::TooLarge);
        let mut bridge = SseBridge::new(CHAT, MESSAGES, MODEL).unwrap();
        assert_eq!(
            bridge.push(b"data: \xff\n\n").unwrap_err(),
            ProtocolError::InvalidStream
        );
    }

    #[test]
    fn native_chat_stream_requires_the_actual_terminal_marker() {
        let first = sse(None,&json!({"id":"chat_synthetic","model":MODEL,"choices":[{"index":0,"delta":{"role":"assistant","content":"early"},"finish_reason":null}]})).unwrap();
        let final_chunk = sse(None,&json!({"id":"chat_synthetic","model":MODEL,"choices":[{"index":0,"delta":{},"finish_reason":"stop"}]})).unwrap();
        let mut bridge = SseBridge::new(CHAT, CHAT, MODEL).unwrap();
        assert_eq!(bridge.push(&first).unwrap(), vec![first.clone()]);
        bridge.push(&final_chunk).unwrap();
        assert_eq!(bridge.finish().unwrap_err(), ProtocolError::TruncatedStream);
        let mut bridge = SseBridge::new(CHAT, CHAT, MODEL).unwrap();
        let source = [first, final_chunk, b"data: [DONE]\n\n".to_vec()].concat();
        assert_eq!(bridge.push(&source).unwrap().concat(), source);
        bridge.finish().unwrap();
    }
}
