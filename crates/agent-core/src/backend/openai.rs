//! Any OpenAI-compatible server (`POST /chat/completions`, SSE stream).
//!
//! One implementation, and `llama-server`, vLLM, LM Studio, Ollama's own `/v1`
//! and the hosted endpoints all arrive — which is why this is roadmap item 1:
//! five of six machines in
//! [`ROADMAP/2026-09-01/machines.md`](../../../../ROADMAP/2026-09-01/machines.md)
//! serve through one of those rather than through Ollama.
//!
//! **The rule AGENTS.md prints in bold does not apply here, and assuming it did
//! would be the bug.** For Ollama the window has to be *sent*, as
//! `options.num_ctx`, or the server silently truncates to its own default. This
//! API has no field for the window at all: `max_tokens` caps the *output*, and
//! on `llama-server`, vLLM and LM Studio the window is what the server was
//! started with (`llama-server -c 8192`). So `context_limit` is not sent — not
//! because it does not matter, but because there is nowhere to put it that
//! would not be a different number in a different field.
//!
//! The check therefore moves to the response: `usage.prompt_tokens` comes back
//! on every call and is already compared against our own count per turn. Which
//! is why [`stream_options`] is not optional here — see [`Body`].
//!
//! See `RECORD/2026-09-01.an-openai-compatible-backend.completed.md`.

use futures_util::StreamExt;
use serde::Deserialize;

use super::{
    Backend, BackendError, Chunk, ChunkStream, CompletionRequest, Message, StopReason, Usage,
};

/// `llama-server`'s default. Not a claim that it is the likeliest server, just
/// the one this repository's own recipe would reach for first.
pub const DEFAULT_BASE_URL: &str = "http://127.0.0.1:8080/v1";

pub struct OpenAi {
    base_url: String,
    /// `openai@<host>`, because the record header's whole job is that two runs
    /// are comparable — and "openai" alone cannot tell a 7B on a laptop from a
    /// hosted frontier model.
    name: String,
    api_key: Option<String>,
    http: reqwest::Client,
}

impl OpenAi {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        let name = format!("openai@{}", host_of(&base_url));
        Self {
            base_url,
            name,
            api_key: None,
            http: reqwest::Client::new(),
        }
    }

    /// The bearer token, when the endpoint wants one.
    ///
    /// Absent means **no header at all**, which is what a local `llama-server`
    /// wants: an empty bearer is worse than nothing, since a server that checks
    /// keys would then reject with a message about the wrong thing.
    pub fn with_api_key(mut self, key: impl Into<String>) -> Self {
        self.api_key = Some(key.into());
        self
    }

    /// What a run should be told once, before it measures anything.
    ///
    /// `None` when there is nothing to warn about. This is the sentence that
    /// keeps the untransferable rule visible: the caller budgeted against a
    /// window this API gives it no way to send, so the two can disagree and the
    /// only place that shows is `usage.prompt_tokens` afterwards.
    pub fn window_caveat(context_limit: Option<u32>) -> Option<String> {
        let limit = context_limit.filter(|limit| *limit > 0)?;
        Some(format!(
            "the window is the server's: --context-limit {limit} is what this run \
             budgets against and there is no field on this API to send it in. Start the \
             server with at least {limit} (llama-server -c {limit}, vLLM \
             --max-model-len {limit}); the prompt_tokens each turn reports is what \
             actually arrived."
        ))
    }
}

impl Default for OpenAi {
    fn default() -> Self {
        Self::new(DEFAULT_BASE_URL)
    }
}

/// `host:port` out of a base URL, for the backend's name. Falls back to the
/// whole string rather than to something invented: a name nobody can trace back
/// to an endpoint is the failure this exists to avoid.
fn host_of(base_url: &str) -> String {
    base_url
        .split_once("://")
        .map(|(_, rest)| rest)
        .unwrap_or(base_url)
        .split('/')
        .next()
        .unwrap_or(base_url)
        .to_string()
}

#[derive(serde::Serialize)]
struct Body<'a> {
    model: &'a str,
    messages: &'a [Message],
    stream: bool,
    /// **Always sent.** Without it these servers stream no usage at all, and a
    /// backend that then reported zero would be claiming the server saw an
    /// empty prompt — the number the budget panel plots against our own count.
    stream_options: StreamOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u32>,
    // No `max_tokens`, and no window: see the module note. The output cap that
    // *would* belong here is `--reserve`, which `CompletionRequest` does not
    // carry — one change with one argument, not a field smuggled in beside this.
}

#[derive(serde::Serialize)]
struct StreamOptions {
    include_usage: bool,
}

/// Only the fields we act on, so a server-side addition never breaks the parse.
#[derive(Deserialize)]
struct Event {
    #[serde(default)]
    choices: Vec<Choice>,
    /// Arrives on its own final event, after the last choice, when
    /// `include_usage` was asked for.
    #[serde(default)]
    usage: Option<ApiUsage>,
    #[serde(default)]
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: Delta,
    #[serde(default)]
    finish_reason: Option<String>,
}

#[derive(Deserialize, Default)]
struct Delta {
    #[serde(default)]
    content: Option<String>,
}

#[derive(Deserialize)]
struct ApiUsage {
    #[serde(default)]
    prompt_tokens: u32,
    #[serde(default)]
    completion_tokens: u32,
}

/// The shape OpenAI documents, and what `llama-server` sends mid-stream when a
/// request goes wrong after the headers are already out.
#[derive(Deserialize)]
struct ApiError {
    #[serde(default)]
    message: Option<String>,
}

/// What one SSE `data:` payload stands for.
///
/// Three outcomes and not two: a payload can carry text, or the stop reason, or
/// the usage that arrives *after* it on its own event — so a parse cannot
/// return a single `Chunk` and be done.
#[derive(Debug, PartialEq, Eq)]
enum Parsed {
    Text(String),
    Stop(StopReason),
    Usage(Usage),
    /// A keep-alive, an empty delta, or the role-only first event.
    Nothing,
}

fn parse_event(payload: &[u8]) -> Result<Parsed, BackendError> {
    let event: Event =
        serde_json::from_slice(payload).map_err(|e| BackendError::Malformed(e.to_string()))?;

    if let Some(error) = event.error {
        return Err(BackendError::Rejected(
            error.message.unwrap_or_else(|| "no message".into()),
        ));
    }

    if let Some(choice) = event.choices.first() {
        if let Some(reason) = &choice.finish_reason {
            return Ok(Parsed::Stop(match reason.as_str() {
                "stop" => StopReason::Stop,
                "length" => StopReason::Length,
                // `tool_calls`, `content_filter`, or something this server
                // invented. Reported as itself rather than guessed at.
                _ => StopReason::Other,
            }));
        }
        if let Some(text) = &choice.delta.content
            && !text.is_empty()
        {
            return Ok(Parsed::Text(text.clone()));
        }
    }

    // The usage event carries an empty `choices`, so it is checked after them
    // and not instead: a server that sends both on one event still streams its
    // text.
    if let Some(usage) = event.usage {
        return Ok(Parsed::Usage(Usage {
            prompt_tokens: usage.prompt_tokens,
            completion_tokens: usage.completion_tokens,
        }));
    }

    Ok(Parsed::Nothing)
}

/// The `data:` payload of one SSE line, or `None` for a line that is not one —
/// a blank separator, or a `:` comment some servers send as a heartbeat.
fn payload_of(line: &[u8]) -> Option<&[u8]> {
    let text = line.strip_prefix(b"data:")?;
    let payload = match text.first() {
        Some(b' ') => &text[1..],
        _ => text,
    };
    (!payload.is_empty()).then_some(payload)
}

impl Backend for OpenAi {
    fn name(&self) -> &str {
        &self.name
    }

    fn stream(&self, request: CompletionRequest) -> ChunkStream<'_> {
        let url = format!("{}/chat/completions", self.base_url);
        let http = self.http.clone();
        let api_key = self.api_key.clone();

        Box::pin(async_stream::try_stream! {
            let mut post = http.post(&url).json(&Body {
                model: &request.model,
                messages: &request.messages,
                stream: true,
                stream_options: StreamOptions { include_usage: true },
                temperature: request.temperature,
                seed: request.seed,
            });
            if let Some(key) = &api_key {
                post = post.bearer_auth(key);
            }

            let response = post
                .send()
                .await
                .map_err(|e| BackendError::Transport(e.to_string()))?;

            if !response.status().is_success() {
                let status = response.status();
                let body = response.text().await.unwrap_or_default();
                Err(BackendError::Rejected(format!("{status}: {body}")))?;
                return;
            }

            let mut body = response.bytes_stream();
            let mut buf: Vec<u8> = Vec::new();
            // Held rather than yielded on sight: the stop reason arrives on one
            // event and the usage on the next, and `Chunk::Done` carries both.
            // Yielding at the stop would report every run as "not reported".
            let mut stop: Option<StopReason> = None;
            let mut usage: Option<Usage> = None;
            let mut done = false;

            while let Some(bytes) = body.next().await {
                let bytes = bytes.map_err(|e| BackendError::Transport(e.to_string()))?;
                buf.extend_from_slice(&bytes);

                // A chunk boundary lands anywhere, so only whole lines are parsed.
                while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=nl).collect();
                    let line = match line.strip_suffix(b"\n") {
                        Some(line) => line,
                        None => &line,
                    };
                    let line = match line.strip_suffix(b"\r") {
                        Some(line) => line,
                        None => line,
                    };
                    let Some(payload) = payload_of(line) else { continue };
                    if payload == b"[DONE]" {
                        done = true;
                        continue;
                    }
                    match parse_event(payload)? {
                        Parsed::Text(text) => yield Chunk::Text(text),
                        Parsed::Stop(reason) => stop = Some(reason),
                        Parsed::Usage(reported) => usage = Some(reported),
                        Parsed::Nothing => {}
                    }
                }
            }

            // A stream that ended without `[DONE]` and without a stop reason was
            // cut off, and saying it stopped normally would be inventing the one
            // fact the caller reads to decide whether the answer is complete.
            let stop = match (stop, done) {
                (Some(stop), _) => stop,
                (None, true) => StopReason::Stop,
                (None, false) => {
                    Err(BackendError::Transport(
                        "the stream ended without a finish_reason and without [DONE]".into(),
                    ))?;
                    return;
                }
            };
            yield Chunk::Done { stop, usage };
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_text_delta_is_text_and_an_empty_one_is_nothing() {
        let event = br#"{"choices":[{"delta":{"content":"Hola"}}]}"#;
        assert_eq!(parse_event(event).unwrap(), Parsed::Text("Hola".into()));

        // The first event of every stream: a role and no content.
        let role = br#"{"choices":[{"delta":{"role":"assistant"}}]}"#;
        assert_eq!(parse_event(role).unwrap(), Parsed::Nothing);
    }

    #[test]
    fn the_finish_reason_becomes_a_stop_reason_and_the_unknown_ones_say_so() {
        let stop = br#"{"choices":[{"delta":{},"finish_reason":"stop"}]}"#;
        assert_eq!(parse_event(stop).unwrap(), Parsed::Stop(StopReason::Stop));

        let length = br#"{"choices":[{"delta":{},"finish_reason":"length"}]}"#;
        assert_eq!(
            parse_event(length).unwrap(),
            Parsed::Stop(StopReason::Length)
        );

        // Not guessed at: a `tool_calls` finish is a different thing from a
        // model that stopped, and reading it as `Stop` would hide it.
        let tools = br#"{"choices":[{"delta":{},"finish_reason":"tool_calls"}]}"#;
        assert_eq!(parse_event(tools).unwrap(), Parsed::Stop(StopReason::Other));
    }

    #[test]
    fn the_usage_event_carries_no_choices_and_is_still_read() {
        let event = br#"{"choices":[],"usage":{"prompt_tokens":26,"completion_tokens":7}}"#;
        assert_eq!(
            parse_event(event).unwrap(),
            Parsed::Usage(Usage {
                prompt_tokens: 26,
                completion_tokens: 7,
            })
        );
    }

    #[test]
    fn an_error_object_mid_stream_is_a_rejection() {
        let event = br#"{"error":{"message":"model not found","type":"invalid_request_error"}}"#;
        assert!(matches!(
            parse_event(event),
            Err(BackendError::Rejected(message)) if message.contains("model not found")
        ));
    }

    #[test]
    fn only_data_lines_carry_a_payload() {
        assert_eq!(payload_of(b"data: {\"a\":1}"), Some(&b"{\"a\":1}"[..]));
        // Some servers omit the space; the spec allows it either way.
        assert_eq!(payload_of(b"data:{\"a\":1}"), Some(&b"{\"a\":1}"[..]));
        // A heartbeat comment and a blank separator are not events.
        assert_eq!(payload_of(b": ping"), None);
        assert_eq!(payload_of(b""), None);
        assert_eq!(payload_of(b"event: message"), None);
        assert_eq!(payload_of(b"data:"), None);
    }

    /// The window is not on the request, and that is the decision rather than
    /// an omission: this API has no field for it.
    #[test]
    fn the_body_carries_sampling_and_asks_for_usage_and_sends_no_window() {
        let messages = [Message::user("hola")];
        let body = serde_json::to_value(Body {
            model: "qwen2.5-coder-7b",
            messages: &messages,
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
            temperature: Some(0.0),
            seed: Some(42),
        })
        .unwrap();

        assert_eq!(body["stream_options"]["include_usage"], true);
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["seed"], 42);
        assert!(body.get("max_tokens").is_none(), "{body}");
        assert!(body.get("num_ctx").is_none(), "{body}");
        assert!(body.get("context_limit").is_none(), "{body}");
    }

    #[test]
    fn unpinned_sampling_is_left_to_the_server() {
        let messages = [Message::user("hola")];
        let body = serde_json::to_value(Body {
            model: "qwen2.5-coder-7b",
            messages: &messages,
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
            temperature: None,
            seed: None,
        })
        .unwrap();
        assert!(body.get("temperature").is_none(), "{body}");
        assert!(body.get("seed").is_none(), "{body}");
    }

    /// Two runs against different endpoints must not both be called "openai" in
    /// a record header whose whole job is that runs are comparable.
    #[test]
    fn the_name_says_which_endpoint() {
        assert_eq!(
            OpenAi::new("http://127.0.0.1:8080/v1").name(),
            "openai@127.0.0.1:8080"
        );
        assert_eq!(
            OpenAi::new("https://api.example.com/v1").name(),
            "openai@api.example.com"
        );
    }

    #[test]
    fn the_caveat_names_the_window_it_cannot_send() {
        let caveat = OpenAi::window_caveat(Some(8192)).expect("a budgeted run says something");
        assert!(caveat.contains("8192"), "{caveat}");
        assert!(caveat.contains("llama-server -c 8192"), "{caveat}");
        // Nothing to warn about when nothing was budgeted.
        assert_eq!(OpenAi::window_caveat(None), None);
        assert_eq!(OpenAi::window_caveat(Some(0)), None);
    }
}
