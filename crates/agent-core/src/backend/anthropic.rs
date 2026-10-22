//! Any Anthropic-compatible server (`POST /v1/messages`, SSE stream).
//!
//! Anthropic's own API, and the ones that copied its shape — MiniMax,
//! DeepSeek, Moonshot, Z.ai, and the `/v1/messages` that `llama-server` and
//! Ollama grew. The URL is what their docs give for `ANTHROPIC_BASE_URL`, and
//! the path is appended here, as the SDKs do.
//!
//! **What the OpenAI backend's note says of the window holds here too**: there
//! is no field for it, and `max_tokens` caps the *output*. What does not hold is
//! leaving `max_tokens` unsent — this API refuses a request without one — so it
//! is the backend's own number, [`DEFAULT_MAX_TOKENS`] unless a profile says.
//!
//! **Thinking is a block here, not text**, and it goes back. A thinking block
//! streams up as `<think>…</think>`, which is what everything above the backend
//! already reads as reasoning; the block itself, signature and all, is kept
//! against the exact prefix it answered, and sent back as it came whenever that
//! prefix comes back unchanged — every step of a tool loop, which is
//! interleaved thinking, and every earlier turn luu has not edited. Any other
//! reply goes back as its text, the thinking cut out. See [`Replies`].
//!
//! See `RECORD/2026-10-22.an-anthropic-compatible-backend.completed.md`.

use std::collections::VecDeque;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Mutex;

use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Backend, BackendError, BackendFuture, Chunk, ChunkStream, CompletionRequest, Constraint,
    Message, ModelFacts, Role, StopReason, ToolSpec, Usage, WindowIs,
};
use crate::tools::{ToolCall, split_call, thinking_spans};

/// `llama-server`'s address, as the OpenAI backend's default is: a machine
/// this repository's own recipe starts, and not a host somebody else runs.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080";

/// The API version every compatible server was written against. `2026-10-21`
/// refuses `temperature` on every model, which would make a pinned run
/// impossible everywhere to make it right on Claude alone.
pub const VERSION: &str = "2023-06-01";

/// `max_tokens` when nothing says: required on this API, and a guess. It has
/// to hold the thinking as well as the answer; a server whose ceiling is lower
/// says so in its own 400 at the first turn, and `max-tokens` on the profile
/// is the answer to it.
pub const DEFAULT_MAX_TOKENS: u32 = 16_384;

/// How many replies [`Replies`] holds. A tool loop is a handful of steps; the
/// rest is earlier turns, which matter less the older they are.
const REMEMBERED: usize = 64;

/// The smallest budget the API takes for `{type: "enabled"}`.
const MIN_BUDGET: u32 = 1024;

/// What the request says about thinking. Absent on the profile is *nothing
/// sent* — the server's own default — which is not one of these.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thinking {
    /// `{type: "adaptive", display: "summarized"}`: the model decides how
    /// much, and the summary comes back as text the page can fold. Claude
    /// 4.6 and later.
    Adaptive,
    /// `{type: "disabled"}`. Refused by the models that always think.
    Off,
    /// `{type: "enabled", budget_tokens: N}`: Claude 4.5 and earlier, and the
    /// compatible servers that implement that shape.
    Budget(u32),
}

impl Thinking {
    fn body(self) -> Value {
        match self {
            Self::Adaptive => json!({"type": "adaptive", "display": "summarized"}),
            Self::Off => json!({"type": "disabled"}),
            Self::Budget(tokens) => json!({"type": "enabled", "budget_tokens": tokens}),
        }
    }

    /// The tokens this takes out of `max_tokens` before the answer starts,
    /// when it says.
    pub fn budget(self) -> Option<u32> {
        match self {
            Self::Budget(tokens) => Some(tokens),
            _ => None,
        }
    }
}

/// `"adaptive"`, `"off"`, or a number — what a person writes in `config.toml`.
impl serde::Serialize for Thinking {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::Adaptive => serializer.serialize_str("adaptive"),
            Self::Off => serializer.serialize_str("off"),
            Self::Budget(tokens) => serializer.serialize_u32(*tokens),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Thinking {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct Visitor;
        impl serde::de::Visitor<'_> for Visitor {
            type Value = Thinking;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                write!(
                    f,
                    "\"adaptive\", \"off\", or a budget of at least {MIN_BUDGET} tokens"
                )
            }
            fn visit_str<E: serde::de::Error>(self, value: &str) -> Result<Thinking, E> {
                match value {
                    "adaptive" => Ok(Thinking::Adaptive),
                    "off" => Ok(Thinking::Off),
                    _ => Err(E::invalid_value(serde::de::Unexpected::Str(value), &self)),
                }
            }
            fn visit_u64<E: serde::de::Error>(self, value: u64) -> Result<Thinking, E> {
                match u32::try_from(value) {
                    Ok(tokens) if tokens >= MIN_BUDGET => Ok(Thinking::Budget(tokens)),
                    _ => Err(E::invalid_value(
                        serde::de::Unexpected::Unsigned(value),
                        &self,
                    )),
                }
            }
            fn visit_i64<E: serde::de::Error>(self, value: i64) -> Result<Thinking, E> {
                match u64::try_from(value) {
                    Ok(value) => self.visit_u64(value),
                    Err(_) => Err(E::invalid_value(
                        serde::de::Unexpected::Signed(value),
                        &self,
                    )),
                }
            }
        }
        deserializer.deserialize_any(Visitor)
    }
}

/// `output_config.effort`. Absent is the server's default — `medium` on
/// Claude Opus 5.5, `high` on most of the rest, which is why it is a key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Effort {
    Low,
    Medium,
    High,
    Xhigh,
    Max,
}

pub struct Anthropic {
    base_url: String,
    /// `anthropic@<host>`, for the reason `openai@<host>` is: a record header
    /// that cannot tell two destinations apart cannot say two runs compare.
    name: String,
    api_key: Option<String>,
    max_tokens: u32,
    thinking: Option<Thinking>,
    effort: Option<Effort>,
    http: reqwest::Client,
    replies: Replies,
}

impl Anthropic {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let name = format!("anthropic@{}", super::openai::host_of(&base_url));
        Self {
            base_url,
            name,
            api_key: None,
            max_tokens: DEFAULT_MAX_TOKENS,
            thinking: None,
            effort: None,
            http: reqwest::Client::new(),
            replies: Replies::default(),
        }
    }

    /// Sent as `x-api-key`. Absent sends no key header at all, which is what a
    /// local `llama-server` wants.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    pub fn with_max_tokens(mut self, max_tokens: u32) -> Self {
        self.max_tokens = max_tokens;
        self
    }

    pub fn with_thinking(mut self, thinking: Option<Thinking>) -> Self {
        self.thinking = thinking;
        self
    }

    pub fn with_effort(mut self, effort: Option<Effort>) -> Self {
        self.effort = effort;
        self
    }

    /// The OpenAI backend's caveat, in this API's words: the window is
    /// budgeted against and there is nowhere to send it.
    pub fn window_caveat(context_limit: Option<u32>) -> Option<String> {
        let limit = context_limit.filter(|limit| *limit > 0)?;
        Some(format!(
            "--context-limit {limit} is what this run budgets against, and the Messages API \
             has no field to send it in: the window is the model's, or what the server was \
             started with. The prompt_tokens each turn reports is what actually arrived."
        ))
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1/{path}", self.base_url)
    }

    fn get(&self, url: &str) -> reqwest::RequestBuilder {
        let mut request = self
            .http
            .get(url)
            .timeout(super::LIST_TIMEOUT)
            .header("anthropic-version", VERSION);
        if let Some(key) = &self.api_key {
            request = request.header("x-api-key", key);
        }
        request
    }

    /// `GET /v1/models`, every page at once. `None` is a server that does not
    /// list — a `404` — which is a fact about it rather than a failure.
    async fn listing(&self) -> Result<Option<Vec<Value>>, BackendError> {
        let url = self.url("models?limit=1000");
        let response = self
            .get(&url)
            .send()
            .await
            .map_err(|error| BackendError::transport(&error))?;
        if response.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(None);
        }
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(BackendError::Rejected(format!(
                "{status} from {url}: {body}"
            )));
        }
        let listed: Value = response
            .json()
            .await
            .map_err(|error| BackendError::Malformed(error.to_string()))?;
        Ok(Some(
            listed
                .get("data")
                .and_then(Value::as_array)
                .cloned()
                .unwrap_or_default(),
        ))
    }
}

impl Default for Anthropic {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }
}

// ---- what is kept, and what goes back --------------------------------------

/// The replies this backend streamed that carried thinking, as the blocks the
/// server sent, each under the hash of **the prefix it answered and its own
/// text**: the model, the tools, every message before it.
///
/// That key is the server's rule computed on this side. A thinking block's
/// signature binds the conversation before it, and a block whose prefix
/// changed is refused; so a block is sent back only when the prefix it was
/// produced under comes back byte for byte, and otherwise the reply goes back
/// as text. A change before one reply is a change before every reply after
/// it, so what stops being replayed is always a leading run — the one removal
/// the server allows. Inside a turn the prefix never changes (the loop appends
/// to a copy of the selection), which is what keeps interleaved thinking
/// working on every step; across turns it holds until luu evicts, folds or
/// prunes, which is the context manager doing its job.
///
/// The process's own: a resumed session replays nothing from before it, and
/// the store keeps text, never a signature.
#[derive(Default)]
struct Replies {
    held: Mutex<VecDeque<(u64, Vec<Value>)>>,
}

impl Replies {
    fn recall(&self, key: u64) -> Option<Vec<Value>> {
        let held = self
            .held
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        held.iter()
            .rev()
            .find(|(k, _)| *k == key)
            .map(|(_, blocks)| blocks.clone())
    }

    fn keep(&self, key: u64, blocks: Vec<Value>) {
        let mut held = self
            .held
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        held.retain(|(k, _)| *k != key);
        held.push_back((key, blocks));
        while held.len() > REMEMBERED {
            // Oldest first, so what is forgotten is a leading run too.
            held.pop_front();
        }
    }
}

/// The hash a reply is kept under, fed message by message so every prefix of
/// the history has its own state.
#[derive(Clone)]
struct Prefix(DefaultHasher);

impl Prefix {
    fn new(model: &str, tools: &[ToolSpec]) -> Self {
        let mut hasher = DefaultHasher::new();
        model.hash(&mut hasher);
        for tool in tools {
            tool.name.hash(&mut hasher);
            tool.description.hash(&mut hasher);
            tool.parameters.to_string().hash(&mut hasher);
        }
        Self(hasher)
    }

    fn feed(&mut self, message: &Message) {
        let role: u8 = match message.role {
            Role::System => 0,
            Role::User => 1,
            Role::Assistant => 2,
        };
        role.hash(&mut self.0);
        message.content.hash(&mut self.0);
    }

    /// The key of a reply with this text, answering this prefix.
    fn reply(&self, text: &str) -> u64 {
        let mut hasher = self.0.clone();
        u8::MAX.hash(&mut hasher);
        text.hash(&mut hasher);
        hasher.finish()
    }
}

/// `text` without its reasoning, for a reply that goes back as text: a model
/// shown its own `<think>` as something it said learns to say it.
fn without_thinking(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut at = 0;
    for span in thinking_spans(text) {
        out.push_str(&text[at..span.start]);
        at = span.end;
    }
    out.push_str(&text[at..]);
    out.trim().to_string()
}

fn text_block(text: &str) -> Value {
    json!({"type": "text", "text": text})
}

fn tool_use(id: &str, call: &ToolCall) -> Value {
    // `input` is an object on this API; arguments that never parsed into one
    // are sent empty, and the tool's own refusal is what the model reads next.
    let input = match &call.arguments {
        Value::Object(_) => call.arguments.clone(),
        _ => json!({}),
    };
    json!({"type": "tool_use", "id": id, "name": call.name, "input": input})
}

fn kind(block: &Value) -> &str {
    block.get("type").and_then(Value::as_str).unwrap_or("")
}

/// The history as this API spells it: the system text on its own, and the
/// messages as blocks, with consecutive messages of one role merged — a
/// `tool_result` and the next prompt are one user message here.
///
/// Under `native`, an assistant message holding a call followed by its
/// `[name]` result becomes a `tool_use` and a `tool_result`, by the pairing
/// [`super::native`] does. A reply [`Replies`] holds under this exact prefix
/// goes back as the blocks it came as.
struct Rendered {
    system: Option<String>,
    messages: Vec<Value>,
}

fn render(
    history: &[Message],
    native: bool,
    mut prefix: Prefix,
    recall: &dyn Fn(u64) -> Option<Vec<Value>>,
) -> Rendered {
    let mut system: Vec<&str> = Vec::new();
    let mut turns: Vec<(&'static str, Vec<Value>)> = Vec::new();
    let mut push = |role: &'static str, blocks: Vec<Value>| {
        if blocks.is_empty() {
            return;
        }
        match turns.last_mut() {
            Some((last, held)) if *last == role => held.extend(blocks),
            _ => turns.push((role, blocks)),
        }
    };
    let mut calls = 0;
    // Once a held reply is not sent back as it came, nothing after it may
    // be: the server allows dropping a leading run and nothing else.
    let mut replaying = true;
    let mut i = 0;
    while i < history.len() {
        let message = &history[i];
        match message.role {
            Role::System => {
                system.push(&message.content);
                prefix.feed(message);
                i += 1;
            }
            Role::User => {
                let text = message.content.trim();
                push(
                    "user",
                    match text.is_empty() {
                        true => Vec::new(),
                        false => vec![text_block(&message.content)],
                    },
                );
                prefix.feed(message);
                i += 1;
            }
            Role::Assistant => {
                let held = replaying
                    .then(|| recall(prefix.reply(&message.content)))
                    .flatten();
                let paired = native
                    .then(|| split_call(&message.content))
                    .flatten()
                    .filter(|(_, call)| {
                        history.get(i + 1).is_some_and(|next| {
                            next.role == Role::User
                                && next.content.starts_with(&format!("[{}]", call.name))
                        })
                    });
                let mut sent = held.as_ref().map(|blocks| {
                    // One `tool_use` with its result after it, or none: a
                    // `tool_use` the next message does not answer is refused.
                    let mut answered = None;
                    let blocks: Vec<Value> = blocks
                        .iter()
                        .filter(|block| match kind(block) {
                            "tool_use" => {
                                let wanted = paired.as_ref().is_some_and(|(_, call)| {
                                    block.get("name").and_then(Value::as_str)
                                        == Some(call.name.as_str())
                                });
                                let take = wanted && answered.is_none();
                                if take {
                                    answered =
                                        block.get("id").and_then(Value::as_str).map(str::to_string);
                                }
                                take
                            }
                            _ => true,
                        })
                        .cloned()
                        .collect();
                    (blocks, answered)
                });
                // A held reply left with nothing but its thinking is not the
                // reply the server sent. One whose call the model wrote as
                // text goes back as it came, and its result as text after it.
                if let Some((blocks, _)) = &sent
                    && !blocks
                        .iter()
                        .any(|b| matches!(kind(b), "text" | "tool_use"))
                {
                    sent = None;
                }
                if held.is_some() && sent.is_none() {
                    replaying = false;
                }
                let answered = match (sent, &paired) {
                    (Some((blocks, answered)), _) => {
                        push("assistant", blocks);
                        answered
                    }
                    (None, Some((prose, call))) => {
                        calls += 1;
                        let id = format!("luu_{calls}");
                        let prose = without_thinking(prose);
                        let mut blocks = Vec::new();
                        if !prose.is_empty() {
                            blocks.push(text_block(&prose));
                        }
                        blocks.push(tool_use(&id, call));
                        push("assistant", blocks);
                        Some(id)
                    }
                    (None, None) => {
                        let text = without_thinking(&message.content);
                        push(
                            "assistant",
                            match text.is_empty() {
                                true => Vec::new(),
                                false => vec![text_block(&text)],
                            },
                        );
                        None
                    }
                };
                prefix.feed(message);
                i += 1;
                if let (Some(id), Some(result)) = (answered, history.get(i)) {
                    push(
                        "user",
                        vec![json!({
                            "type": "tool_result",
                            "tool_use_id": id,
                            "content": result.content,
                        })],
                    );
                    prefix.feed(result);
                    i += 1;
                }
            }
        }
    }
    let mut messages: Vec<Value> = turns
        .into_iter()
        .map(|(role, content)| json!({"role": role, "content": content}))
        .collect();
    // The second breakpoint: the history so far, which the next call of this
    // turn — and the next turn, until something is evicted — starts with.
    if let Some(last) = messages
        .last_mut()
        .and_then(|message| message.get_mut("content"))
        .and_then(Value::as_array_mut)
        .and_then(|content| content.last_mut())
        .and_then(Value::as_object_mut)
    {
        last.insert("cache_control".into(), json!({"type": "ephemeral"}));
    }
    Rendered {
        system: (!system.is_empty()).then(|| system.join("\n\n")),
        messages,
    }
}

#[derive(serde::Serialize)]
struct Body<'a> {
    model: &'a str,
    /// Required: the API refuses a request without it.
    max_tokens: u32,
    /// One block, marked for the prompt cache. The tools render before the
    /// system text, so this one breakpoint covers both — the stable prefix
    /// this project keeps byte-identical on purpose, cached only if marked.
    #[serde(skip_serializing_if = "Option::is_none")]
    system: Option<Vec<Value>>,
    messages: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<Vec<Value>>,
    /// One call per reply: the loop runs one per step, and a replayed
    /// `tool_use` without its `tool_result` is refused.
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<Value>,
    stream: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    thinking: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    output_config: Option<Value>,
}

fn tools(specs: &[ToolSpec]) -> Vec<Value> {
    specs
        .iter()
        .map(|spec| {
            json!({
                "name": spec.name,
                "description": spec.description,
                "input_schema": spec.parameters,
            })
        })
        .collect()
}

// ---- what comes back -------------------------------------------------------

#[derive(Deserialize, Default, Debug, Clone, Copy, PartialEq, Eq)]
struct ApiUsage {
    #[serde(default)]
    input_tokens: Option<u32>,
    #[serde(default)]
    cache_creation_input_tokens: Option<u32>,
    #[serde(default)]
    cache_read_input_tokens: Option<u32>,
    #[serde(default)]
    output_tokens: Option<u32>,
}

impl ApiUsage {
    /// A later report over an earlier one, field by field: `message_delta`
    /// repeats what it knows and may say nothing of the input.
    fn over(self, earlier: Self) -> Self {
        Self {
            input_tokens: self.input_tokens.or(earlier.input_tokens),
            cache_creation_input_tokens: self
                .cache_creation_input_tokens
                .or(earlier.cache_creation_input_tokens),
            cache_read_input_tokens: self
                .cache_read_input_tokens
                .or(earlier.cache_read_input_tokens),
            output_tokens: self.output_tokens.or(earlier.output_tokens),
        }
    }

    /// The prompt is all three input counts: `input_tokens` alone leaves out
    /// what the cache served, and would report a cached prompt as a short one.
    fn usage(self) -> Option<Usage> {
        let input = [
            self.input_tokens,
            self.cache_creation_input_tokens,
            self.cache_read_input_tokens,
        ];
        if input.iter().all(Option::is_none) && self.output_tokens.is_none() {
            return None;
        }
        Some(Usage {
            prompt_tokens: input.iter().flatten().sum(),
            completion_tokens: self.output_tokens.unwrap_or(0),
        })
    }
}

/// What one SSE payload stands for. The `event:` line before it says the
/// same as the payload's own `type`, so only the payload is read.
#[derive(Debug, PartialEq)]
enum Parsed {
    Start(ApiUsage),
    Block {
        index: usize,
        block: Value,
    },
    Text {
        index: usize,
        text: String,
    },
    Thinking {
        index: usize,
        text: String,
    },
    Signature {
        index: usize,
        signature: String,
    },
    Json {
        index: usize,
        partial: String,
    },
    BlockStop(usize),
    Delta {
        stop: Option<String>,
        usage: ApiUsage,
    },
    Stop,
    /// `ping`, a citation, or a type added after this was written.
    Nothing,
}

fn parse_event(payload: &[u8]) -> Result<Parsed, BackendError> {
    let event: Value =
        serde_json::from_slice(payload).map_err(|e| BackendError::Malformed(e.to_string()))?;
    let index = || {
        event
            .get("index")
            .and_then(Value::as_u64)
            .map(|index| index as usize)
            .unwrap_or(0)
    };
    let usage = |value: Option<&Value>| {
        value
            .and_then(|usage| serde_json::from_value::<ApiUsage>(usage.clone()).ok())
            .unwrap_or_default()
    };
    let string = |value: Option<&Value>| value.and_then(Value::as_str).unwrap_or("").to_string();
    Ok(match kind(&event) {
        "error" => {
            let error = event.get("error");
            return Err(BackendError::Rejected(format!(
                "{}: {}",
                string(error.and_then(|e| e.get("type"))),
                string(error.and_then(|e| e.get("message"))),
            )));
        }
        "message_start" => Parsed::Start(usage(event.pointer("/message/usage"))),
        "content_block_start" => Parsed::Block {
            index: index(),
            block: event.get("content_block").cloned().unwrap_or(Value::Null),
        },
        "content_block_delta" => {
            let delta = event.get("delta");
            match delta.map(kind).unwrap_or("") {
                "text_delta" => Parsed::Text {
                    index: index(),
                    text: string(delta.and_then(|d| d.get("text"))),
                },
                "thinking_delta" => Parsed::Thinking {
                    index: index(),
                    text: string(delta.and_then(|d| d.get("thinking"))),
                },
                "signature_delta" => Parsed::Signature {
                    index: index(),
                    signature: string(delta.and_then(|d| d.get("signature"))),
                },
                "input_json_delta" => Parsed::Json {
                    index: index(),
                    partial: string(delta.and_then(|d| d.get("partial_json"))),
                },
                _ => Parsed::Nothing,
            }
        }
        "content_block_stop" => Parsed::BlockStop(index()),
        "message_delta" => Parsed::Delta {
            stop: event
                .pointer("/delta/stop_reason")
                .and_then(Value::as_str)
                .map(str::to_string),
            usage: usage(event.get("usage")),
        },
        "message_stop" => Parsed::Stop,
        _ => Parsed::Nothing,
    })
}

fn stop_reason(reason: &str) -> StopReason {
    match reason {
        // A reply that ends in a call ended the way a fenced call does: the
        // call is what the loop reads next.
        "end_turn" | "stop_sequence" | "tool_use" => StopReason::Stop,
        "max_tokens" | "model_context_window_exceeded" => StopReason::Length,
        // `refusal`, `pause_turn`, or something added since. Reported as
        // itself rather than guessed at.
        _ => StopReason::Other,
    }
}

/// One content block as it streams in, by its index.
#[derive(Debug)]
enum Block {
    Text(String),
    Thinking {
        text: String,
        signature: String,
        /// Whether `<think>` went up: a block with no text streams nothing.
        opened: bool,
    },
    /// Reasoning the server encrypted. Never shown; sent back as it came.
    Redacted(Value),
    ToolUse {
        id: String,
        name: String,
        json: String,
    },
    /// A server tool's block, or a type added since. Not luu's to send back.
    Other,
}

impl Block {
    fn start(block: &Value) -> Self {
        let string = |key: &str| {
            block
                .get(key)
                .and_then(Value::as_str)
                .unwrap_or("")
                .to_string()
        };
        match kind(block) {
            "text" => Self::Text(string("text")),
            "thinking" => Self::Thinking {
                text: string("thinking"),
                signature: string("signature"),
                opened: false,
            },
            "redacted_thinking" => Self::Redacted(block.clone()),
            "tool_use" => Self::ToolUse {
                id: string("id"),
                name: string("name"),
                json: String::new(),
            },
            _ => Self::Other,
        }
    }

    /// The block as the server would want it back.
    fn sent(&self) -> Option<Value> {
        match self {
            Self::Text(text) if !text.is_empty() => Some(text_block(text)),
            Self::Text(_) | Self::Other => None,
            Self::Thinking {
                text, signature, ..
            } => Some(json!({"type": "thinking", "thinking": text, "signature": signature})),
            Self::Redacted(block) => Some(block.clone()),
            Self::ToolUse { id, name, json } => Some(tool_use(
                id,
                &ToolCall {
                    name: name.clone(),
                    arguments: arguments(json),
                },
            )),
        }
    }
}

fn arguments(json: &str) -> Value {
    match json.trim() {
        "" => json!({}),
        raw => serde_json::from_str(raw).unwrap_or_else(|_| Value::String(raw.to_string())),
    }
}

impl Backend for Anthropic {
    fn name(&self) -> &str {
        &self.name
    }

    fn models(&self) -> BackendFuture<'_, Vec<String>> {
        Box::pin(async move {
            Ok(self
                .listing()
                .await?
                .unwrap_or_default()
                .iter()
                .filter_map(|model| model.get("id").and_then(Value::as_str))
                .map(str::to_string)
                .collect())
        })
    }

    /// Claude's listing says, per model, `max_input_tokens` — the most a
    /// request may carry, which luu budgets under — and the capability tree.
    /// Its presence is read as *a Claude model*, and every one of those takes
    /// `tools`: the field is the API's, not a capability a model may lack. A
    /// server that lists without either says nothing of it.
    fn facts<'a>(
        &'a self,
        models: &'a [String],
    ) -> BackendFuture<'a, std::collections::BTreeMap<String, ModelFacts>> {
        Box::pin(async move {
            let mut said = std::collections::BTreeMap::new();
            for entry in self.listing().await?.unwrap_or_default() {
                let Some(id) = entry.get("id").and_then(Value::as_str) else {
                    continue;
                };
                if !models.iter().any(|model| model == id) {
                    continue;
                }
                let window = entry
                    .get("max_input_tokens")
                    .and_then(Value::as_u64)
                    .filter(|n| *n > 0)
                    .and_then(|n| u32::try_from(n).ok());
                let facts = ModelFacts {
                    window,
                    window_is: window.map(|_| WindowIs::Limit),
                    tools: entry.get("capabilities").map(|_| true),
                };
                if facts != ModelFacts::default() {
                    said.insert(id.to_string(), facts);
                }
            }
            Ok(said)
        })
    }

    /// Claude's structured outputs are `output_config.format`; a compatible
    /// server may not know the field. There is no field for a grammar.
    fn constrain_caveat(&self, constraint: &Constraint) -> Option<String> {
        Some(match constraint {
            Constraint::Grammar(_) => format!(
                "a grammar has no field on the Messages API, so none is sent to {}: the reply \
                 is unconstrained.",
                self.base_url
            ),
            Constraint::Schema(_) => format!(
                "sending the schema as `output_config.format` to {}: Claude honours it, and a \
                 compatible server may ignore it without saying so.",
                self.base_url
            ),
        })
    }

    fn sampling_caveat(&self, temperature: Option<f32>, seed: Option<u32>) -> Option<String> {
        let mut said = Vec::new();
        if let Some(seed) = seed {
            said.push(format!(
                "--seed {seed} is not sent: the Messages API has no field for a seed, so this \
                 run is not pinned to one"
            ));
        }
        if let Some(temperature) = temperature {
            said.push(format!(
                "temperature {temperature} is sent: Claude 4.7 and later refuse any temperature \
                 with a 400, and other servers decide for themselves"
            ));
        }
        (!said.is_empty()).then(|| said.join("; "))
    }

    fn stream(&self, request: CompletionRequest) -> ChunkStream<'_> {
        let url = self.url("messages");
        Box::pin(async_stream::try_stream! {
            let native = !request.tools.is_empty();
            let prefix = Prefix::new(&request.model, &request.tools);
            let rendered = render(&request.messages, native, prefix.clone(), &|key| {
                self.replies.recall(key)
            });
            let mut output_config = serde_json::Map::new();
            if let Some(effort) = self.effort {
                output_config.insert("effort".into(), json!(effort));
            }
            if let Some(Constraint::Schema(schema)) = &request.constraint {
                output_config.insert(
                    "format".into(),
                    json!({"type": "json_schema", "schema": schema}),
                );
            }
            let body = Body {
                model: &request.model,
                max_tokens: self.max_tokens,
                system: rendered.system.map(|text| {
                    vec![json!({
                        "type": "text",
                        "text": text,
                        "cache_control": {"type": "ephemeral"},
                    })]
                }),
                messages: rendered.messages,
                tools: native.then(|| tools(&request.tools)),
                tool_choice: native
                    .then(|| json!({"type": "auto", "disable_parallel_tool_use": true})),
                stream: true,
                temperature: request.temperature,
                thinking: self.thinking.map(Thinking::body),
                output_config: (!output_config.is_empty()).then_some(Value::Object(output_config)),
            };

            let mut post = self
                .http
                .post(&url)
                .header("anthropic-version", VERSION)
                .json(&body);
            if let Some(key) = &self.api_key {
                post = post.header("x-api-key", key);
            }
            // Claude before 4.6 thinks between calls only behind this; later
            // models do it with adaptive thinking, and ignore the header.
            if native && self.thinking.is_some_and(|t| t.budget().is_some()) {
                post = post.header("anthropic-beta", "interleaved-thinking-2025-05-14");
            }

            let response = post
                .send()
                .await
                .map_err(|e| BackendError::transport(&e))?;
            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                Err(BackendError::Rejected(format!("{status}: {body}")))?;
                return;
            }

            let mut body = response.bytes_stream();
            let mut buf: Vec<u8> = Vec::new();
            let mut blocks: Vec<Block> = Vec::new();
            let mut usage = ApiUsage::default();
            let mut stop: Option<StopReason> = None;
            let mut done = false;
            // Everything that went up, which is the text the loop will put in
            // the history and so the text this reply is kept under.
            let mut said = String::new();
            // A thinking block closed and nothing has followed it yet: the
            // blank line after `</think>` goes in with whatever does.
            let mut after_thinking = false;

            while let Some(bytes) = body.next().await {
                let bytes = bytes.map_err(|e| BackendError::transport(&e))?;
                buf.extend_from_slice(&bytes);
                while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=nl).collect();
                    let line = line.strip_suffix(b"\n").unwrap_or(&line);
                    let line = line.strip_suffix(b"\r").unwrap_or(line);
                    let Some(payload) = super::openai::payload_of(line) else { continue };
                    let mut up = String::new();
                    match parse_event(payload)? {
                        Parsed::Start(reported) => usage = reported.over(usage),
                        Parsed::Block { index, block } => {
                            if blocks.len() <= index {
                                blocks.resize_with(index + 1, || Block::Other);
                            }
                            blocks[index] = Block::start(&block);
                            if let Block::Text(text) = &blocks[index]
                                && !text.is_empty()
                            {
                                if std::mem::take(&mut after_thinking) {
                                    up.push_str("\n\n");
                                }
                                up.push_str(text);
                            }
                        }
                        Parsed::Text { index, text } => {
                            if let Some(Block::Text(held)) = blocks.get_mut(index) {
                                held.push_str(&text);
                            }
                            if !text.is_empty() && std::mem::take(&mut after_thinking) {
                                up.push_str("\n\n");
                            }
                            up.push_str(&text);
                        }
                        Parsed::Thinking { index, text } => {
                            if let Some(Block::Thinking { text: held, opened, .. }) =
                                blocks.get_mut(index)
                                && !text.is_empty()
                            {
                                if !*opened {
                                    *opened = true;
                                    if std::mem::take(&mut after_thinking) {
                                        up.push_str("\n\n");
                                    }
                                    up.push_str("<think>");
                                }
                                held.push_str(&text);
                                up.push_str(&text);
                            }
                        }
                        Parsed::Signature { index, signature } => {
                            if let Some(Block::Thinking { signature: held, .. }) =
                                blocks.get_mut(index)
                            {
                                held.push_str(&signature);
                            }
                        }
                        Parsed::Json { index, partial } => {
                            if let Some(Block::ToolUse { json, .. }) = blocks.get_mut(index) {
                                json.push_str(&partial);
                            }
                        }
                        Parsed::BlockStop(index) => {
                            if let Some(Block::Thinking { opened: true, .. }) = blocks.get(index) {
                                up.push_str("</think>");
                                after_thinking = true;
                            }
                        }
                        Parsed::Delta { stop: reason, usage: reported } => {
                            usage = reported.over(usage);
                            if let Some(reason) = reason {
                                stop = Some(stop_reason(&reason));
                            }
                        }
                        Parsed::Stop => done = true,
                        Parsed::Nothing => {}
                    }
                    if !up.is_empty() {
                        said.push_str(&up);
                        yield Chunk::Text(up);
                    }
                }
            }

            // Cut off: no stop reason and no `message_stop`. Saying it stopped
            // would invent the one fact the caller reads to know it is whole.
            let stop = match (stop, done) {
                (Some(stop), _) => stop,
                (None, true) => StopReason::Stop,
                (None, false) => {
                    Err(BackendError::Transport(
                        "the stream ended without a stop_reason and without message_stop".into(),
                    ))?;
                    return;
                }
            };
            // The calls come up as the fence everything above reads, as the
            // OpenAI backend's do — see `super::native`.
            for block in &blocks {
                if let Block::ToolUse { name, json, .. } = block
                    && !name.is_empty()
                {
                    let call = ToolCall { name: name.clone(), arguments: arguments(json) };
                    let fence = super::native::fence(&call, !said.is_empty());
                    said.push_str(&fence);
                    yield Chunk::Text(fence);
                }
            }
            if blocks
                .iter()
                .any(|block| matches!(block, Block::Thinking { .. } | Block::Redacted(_)))
            {
                let mut answered = prefix;
                for message in &request.messages {
                    answered.feed(message);
                }
                self.replies.keep(
                    answered.reply(&said),
                    blocks.iter().filter_map(Block::sent).collect(),
                );
            }
            yield Chunk::Done { stop, usage: usage.usage() };
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prefix() -> Prefix {
        Prefix::new("claude-opus-5-5", &[])
    }

    fn nothing(_: u64) -> Option<Vec<Value>> {
        None
    }

    #[test]
    fn thinking_is_written_three_ways_and_refuses_the_rest() {
        let read = |text: &str| serde_json::from_str::<Thinking>(text);
        assert_eq!(read("\"adaptive\"").unwrap(), Thinking::Adaptive);
        assert_eq!(read("\"off\"").unwrap(), Thinking::Off);
        assert_eq!(read("4096").unwrap(), Thinking::Budget(4096));
        // Below the API's own minimum, and a word it does not know.
        assert!(read("512").is_err());
        assert!(read("\"enabled\"").is_err());
        assert_eq!(
            serde_json::to_string(&Thinking::Budget(2048)).unwrap(),
            "2048"
        );
        assert_eq!(
            Thinking::Adaptive.body(),
            json!({"type": "adaptive", "display": "summarized"})
        );
    }

    #[test]
    fn the_system_text_is_its_own_and_the_last_block_is_cached() {
        let history = [Message::system("you are luu"), Message::user("hola")];
        let rendered = render(&history, false, prefix(), &nothing);
        assert_eq!(rendered.system.as_deref(), Some("you are luu"));
        assert_eq!(
            Value::Array(rendered.messages),
            json!([{"role": "user", "content": [
                {"type": "text", "text": "hola", "cache_control": {"type": "ephemeral"}}
            ]}])
        );
    }

    /// A reply nobody holds goes back as what it said: its reasoning is cut,
    /// because sent as text it reads as something the model said.
    #[test]
    fn a_reply_sent_back_as_text_leaves_its_thinking_out() {
        let history = [
            Message::user("hola"),
            Message::assistant("<think>they greet</think>\n\nHola."),
            Message::user("¿qué tal?"),
        ];
        let rendered = render(&history, false, prefix(), &nothing);
        assert_eq!(rendered.messages[1]["content"][0]["text"], "Hola.");
    }

    #[test]
    fn a_step_becomes_a_tool_use_and_a_tool_result_merged_with_the_next_prompt() {
        let history = [
            Message::user("read it"),
            Message::assistant(
                "Sure.\n\n```tool\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"a\"}}\n```",
            ),
            Message::user("[read_file] ok\n# luu"),
            Message::user("and now?"),
        ];
        let rendered = render(&history, true, prefix(), &nothing);
        let roles: Vec<&str> = rendered
            .messages
            .iter()
            .map(|m| m["role"].as_str().unwrap())
            .collect();
        assert_eq!(roles, ["user", "assistant", "user"]);
        let assistant = &rendered.messages[1]["content"];
        assert_eq!(assistant[0], json!({"type": "text", "text": "Sure."}));
        assert_eq!(
            assistant[1],
            json!({"type": "tool_use", "id": "luu_1", "name": "read_file", "input": {"path": "a"}})
        );
        let user = &rendered.messages[2]["content"];
        assert_eq!(user[0]["type"], "tool_result");
        assert_eq!(user[0]["tool_use_id"], "luu_1");
        assert_eq!(user[0]["content"], "[read_file] ok\n# luu");
        assert_eq!(user[1]["text"], "and now?");
    }

    /// The replay rule: held blocks go back exactly when the prefix they
    /// answered comes back unchanged, with the server's own `tool_use` id.
    #[test]
    fn a_held_reply_goes_back_signed_only_under_its_own_prefix() {
        let call = "```tool\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"a\"}}\n```";
        let reply = format!("<think>read it first</think>\n\n{call}");
        let blocks = vec![
            json!({"type": "thinking", "thinking": "read it first", "signature": "sig"}),
            json!({"type": "tool_use", "id": "toolu_9", "name": "read_file", "input": {"path": "a"}}),
        ];
        let history = [
            Message::system("sys"),
            Message::user("read a"),
            Message::assistant(reply.clone()),
            Message::user("[read_file] ok"),
        ];
        let mut answered = prefix();
        for message in &history[..2] {
            answered.feed(message);
        }
        let key = answered.reply(&reply);
        let held = |k: u64| (k == key).then(|| blocks.clone());

        let rendered = render(&history, true, prefix(), &held);
        assert_eq!(
            rendered.messages[1]["content"],
            Value::Array(blocks.clone())
        );
        assert_eq!(rendered.messages[2]["content"][0]["tool_use_id"], "toolu_9");

        // One byte different before it — an edited system text — and the
        // reply goes back as text, with luu's own id.
        let mut edited = history.clone();
        edited[0] = Message::system("sys!");
        let rendered = render(&edited, true, prefix(), &held);
        let content = &rendered.messages[1]["content"];
        assert_eq!(content[0]["type"], "tool_use");
        assert_eq!(content[0]["id"], "luu_1");
    }

    /// A call the model wrote as text rather than as a `tool_use`: the reply
    /// goes back as the server sent it, and the result follows as text, since
    /// there is no `tool_use` for a `tool_result` to answer.
    #[test]
    fn a_held_reply_whose_call_was_text_goes_back_as_it_came() {
        let reply = "<think>hm</think>\n\n```json\n{\"name\":\"list_dir\",\"arguments\":{}}\n```";
        let blocks = vec![
            json!({"type": "thinking", "thinking": "hm", "signature": "sig"}),
            json!({"type": "text", "text": "```json\n{\"name\":\"list_dir\",\"arguments\":{}}\n```"}),
        ];
        let history = [
            Message::user("list it"),
            Message::assistant(reply),
            Message::user("[list_dir] ok\na.rs"),
        ];
        let mut answered = prefix();
        answered.feed(&history[0]);
        let key = answered.reply(reply);
        let held = |k: u64| (k == key).then(|| blocks.clone());
        let rendered = render(&history, true, prefix(), &held);
        assert_eq!(
            rendered.messages[1]["content"],
            Value::Array(blocks.clone())
        );
        assert_eq!(rendered.messages[2]["content"][0]["type"], "text");
        assert_eq!(
            rendered.messages[2]["content"][0]["text"],
            "[list_dir] ok\na.rs"
        );
    }

    #[test]
    fn the_stream_events_are_read() {
        let start = br#"{"type":"message_start","message":{"usage":{"input_tokens":3,"cache_read_input_tokens":20,"cache_creation_input_tokens":0,"output_tokens":1}}}"#;
        let Parsed::Start(usage) = parse_event(start).unwrap() else {
            panic!("a start")
        };
        let delta = br#"{"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":9}}"#;
        let Parsed::Delta { stop, usage: later } = parse_event(delta).unwrap() else {
            panic!("a delta")
        };
        assert_eq!(stop.as_deref(), Some("tool_use"));
        // The prompt is every input count, cached ones included.
        assert_eq!(
            later.over(usage).usage(),
            Some(Usage {
                prompt_tokens: 23,
                completion_tokens: 9
            })
        );
        assert_eq!(
            parse_event(br#"{"type":"content_block_delta","index":1,"delta":{"type":"thinking_delta","thinking":"hm"}}"#).unwrap(),
            Parsed::Thinking { index: 1, text: "hm".into() }
        );
        assert_eq!(parse_event(br#"{"type":"ping"}"#).unwrap(), Parsed::Nothing);
        assert!(matches!(
            parse_event(br#"{"type":"error","error":{"type":"overloaded_error","message":"Overloaded"}}"#),
            Err(BackendError::Rejected(message)) if message.contains("Overloaded")
        ));
    }

    #[test]
    fn the_stop_reasons_map_and_the_unknown_ones_say_so() {
        assert_eq!(stop_reason("end_turn"), StopReason::Stop);
        assert_eq!(stop_reason("tool_use"), StopReason::Stop);
        assert_eq!(stop_reason("max_tokens"), StopReason::Length);
        assert_eq!(stop_reason("refusal"), StopReason::Other);
        assert_eq!(stop_reason("pause_turn"), StopReason::Other);
    }

    #[test]
    fn the_name_says_which_endpoint() {
        assert_eq!(
            Anthropic::new("https://api.minimax.io/anthropic/").name(),
            "anthropic@api.minimax.io"
        );
    }

    #[test]
    fn a_seed_has_nowhere_to_go_and_says_so() {
        let backend = Anthropic::default();
        assert_eq!(backend.sampling_caveat(None, None), None);
        let said = backend.sampling_caveat(None, Some(42)).unwrap();
        assert!(said.contains("--seed 42 is not sent"), "{said}");
    }
}
