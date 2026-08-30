//! Ollama over HTTP (`POST /api/chat`, NDJSON stream).

use futures_util::StreamExt;
use serde::Deserialize;

use super::{
    Backend, BackendError, Chunk, ChunkStream, CompletionRequest, Message, StopReason, Usage,
};

pub struct Ollama {
    base_url: String,
    http: reqwest::Client,
}

impl Ollama {
    pub fn new(base_url: impl Into<String>) -> Self {
        let base_url = base_url.into().trim_end_matches('/').to_string();
        Self {
            base_url,
            http: reqwest::Client::new(),
        }
    }
}

impl Default for Ollama {
    fn default() -> Self {
        Self::new("http://127.0.0.1:11434")
    }
}

#[derive(serde::Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [Message],
    stream: bool,
    /// Omitted entirely when the window is unknown, so the server keeps its own
    /// default rather than being told a number we made up.
    #[serde(skip_serializing_if = "Option::is_none")]
    options: Option<Options>,
}

/// Only what we have a reason to set. Every field is optional and the struct
/// itself is omitted from the request entirely when all three are — the
/// server keeps its own default rather than being told one we made up.
#[derive(serde::Serialize, Default)]
struct Options {
    #[serde(skip_serializing_if = "Option::is_none")]
    num_ctx: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    temperature: Option<f32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    seed: Option<u32>,
}

impl Options {
    fn from_request(request: &CompletionRequest) -> Option<Self> {
        if request.context_limit.is_none()
            && request.temperature.is_none()
            && request.seed.is_none()
        {
            return None;
        }
        Some(Self {
            num_ctx: request.context_limit,
            temperature: request.temperature,
            seed: request.seed,
        })
    }
}

/// Only the fields we act on. Ollama sends more; ignoring the rest means a
/// server-side addition never breaks the parse.
#[derive(Deserialize)]
struct ChatChunk {
    #[serde(default)]
    message: Option<ChunkMessage>,
    #[serde(default)]
    done: bool,
    #[serde(default)]
    done_reason: Option<String>,
    #[serde(default)]
    prompt_eval_count: Option<u32>,
    #[serde(default)]
    eval_count: Option<u32>,
    #[serde(default)]
    error: Option<String>,
}

#[derive(Deserialize)]
struct ChunkMessage {
    #[serde(default)]
    content: String,
}

/// Parses one NDJSON line into the chunk it stands for, or `None` for a line
/// that carries no text (Ollama sends empty deltas).
fn parse_line(line: &[u8]) -> Result<Option<Chunk>, BackendError> {
    let parsed: ChatChunk =
        serde_json::from_slice(line).map_err(|e| BackendError::Malformed(e.to_string()))?;

    if let Some(err) = parsed.error {
        return Err(BackendError::Rejected(err));
    }

    if parsed.done {
        return Ok(Some(Chunk::Done {
            stop: match parsed.done_reason.as_deref() {
                Some("stop") | None => StopReason::Stop,
                Some("length") => StopReason::Length,
                Some(_) => StopReason::Other,
            },
            usage: Usage {
                prompt_tokens: parsed.prompt_eval_count.unwrap_or(0),
                completion_tokens: parsed.eval_count.unwrap_or(0),
            },
        }));
    }

    let text = parsed.message.map(|m| m.content).unwrap_or_default();
    Ok((!text.is_empty()).then_some(Chunk::Text(text)))
}

impl Backend for Ollama {
    fn name(&self) -> &str {
        "ollama"
    }

    fn stream(&self, request: CompletionRequest) -> ChunkStream<'_> {
        let url = format!("{}/api/chat", self.base_url);
        let http = self.http.clone();

        Box::pin(async_stream::try_stream! {
            let response = http
                .post(&url)
                .json(&ChatRequest {
                    model: &request.model,
                    messages: &request.messages,
                    stream: true,
                    options: Options::from_request(&request),
                })
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

            while let Some(bytes) = body.next().await {
                let bytes = bytes.map_err(|e| BackendError::Transport(e.to_string()))?;
                buf.extend_from_slice(&bytes);

                // A chunk boundary lands anywhere, so only whole lines are parsed.
                while let Some(nl) = buf.iter().position(|b| *b == b'\n') {
                    let line: Vec<u8> = buf.drain(..=nl).collect();
                    let line = &line[..line.len() - 1];
                    if line.iter().all(u8::is_ascii_whitespace) {
                        continue;
                    }
                    if let Some(chunk) = parse_line(line)? {
                        yield chunk;
                    }
                }
            }

            // A last line without its newline: the stream ending is the delimiter.
            // A let chain would read better, but this body is macro-expanded
            // and loses its edition, so `&& let` is rejected here.
            let tail = match buf.iter().all(u8::is_ascii_whitespace) {
                true => None,
                false => parse_line(&buf)?,
            };
            if let Some(chunk) = tail {
                yield chunk;
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn text_delta_becomes_a_text_chunk() {
        let line = br#"{"message":{"role":"assistant","content":"Hola"},"done":false}"#;
        assert!(matches!(parse_line(line), Ok(Some(Chunk::Text(t))) if t == "Hola"));
    }

    #[test]
    fn an_empty_delta_yields_nothing() {
        let line = br#"{"message":{"role":"assistant","content":""},"done":false}"#;
        assert!(matches!(parse_line(line), Ok(None)));
    }

    #[test]
    fn the_done_line_carries_the_usage() {
        let line = br#"{"done":true,"done_reason":"stop","prompt_eval_count":26,"eval_count":7}"#;
        let Ok(Some(Chunk::Done { stop, usage })) = parse_line(line) else {
            panic!("expected a Done chunk");
        };
        assert_eq!(stop, StopReason::Stop);
        assert_eq!(usage.prompt_tokens, 26);
        assert_eq!(usage.completion_tokens, 7);
    }

    /// The window we budgeted against has to reach the server, or the run
    /// measures a prompt the model never saw: Ollama truncates to its own
    /// `num_ctx` and says nothing.
    #[test]
    fn a_known_window_is_sent_as_num_ctx() {
        let messages = [Message::user("hola")];
        let body = serde_json::to_value(ChatRequest {
            model: "qwen2.5-coder:7b",
            messages: &messages,
            stream: true,
            options: Some(Options {
                num_ctx: Some(8192),
                ..Default::default()
            }),
        })
        .unwrap();
        assert_eq!(body["options"]["num_ctx"], 8192);
        assert!(body["options"].get("temperature").is_none());
        assert!(body["options"].get("seed").is_none());
    }

    /// Pinned so two runs meant to be compared differ only by what they're
    /// testing, not by where the sampler happened to wander.
    #[test]
    fn pinned_sampling_is_sent_alongside_the_window() {
        let request = CompletionRequest {
            model: "qwen2.5-coder:7b".into(),
            messages: vec![Message::user("hola")],
            context_limit: Some(8192),
            temperature: Some(0.0),
            seed: Some(42),
        };
        let options = Options::from_request(&request).unwrap();
        let body = serde_json::to_value(options).unwrap();
        assert_eq!(body["num_ctx"], 8192);
        assert_eq!(body["temperature"], 0.0);
        assert_eq!(body["seed"], 42);
    }

    /// Sampling with no known window still has to reach the server: the two
    /// are independent knobs and neither implies the other.
    #[test]
    fn sampling_alone_still_sends_options() {
        let request = CompletionRequest {
            model: "qwen2.5-coder:7b".into(),
            messages: vec![Message::user("hola")],
            context_limit: None,
            temperature: Some(0.0),
            seed: Some(42),
        };
        assert!(Options::from_request(&request).is_some());
    }

    #[test]
    fn an_unknown_window_sends_no_options_at_all() {
        // Not `num_ctx: 0`, and not a default we invented: the server keeps its
        // own, and the recording says the window was unknown.
        let messages = [Message::user("hola")];
        let body = serde_json::to_value(ChatRequest {
            model: "qwen2.5-coder:7b",
            messages: &messages,
            stream: true,
            options: None,
        })
        .unwrap();
        assert!(body.get("options").is_none(), "{body}");
    }

    #[test]
    fn an_error_line_is_a_rejection() {
        let line = br#"{"error":"model 'nope' not found"}"#;
        assert!(matches!(parse_line(line), Err(BackendError::Rejected(_))));
    }
}
