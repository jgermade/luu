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
                .json(&ChatRequest { model: &request.model, messages: &request.messages, stream: true })
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

    #[test]
    fn an_error_line_is_a_rejection() {
        let line = br#"{"error":"model 'nope' not found"}"#;
        assert!(matches!(parse_line(line), Err(BackendError::Rejected(_))));
    }
}
