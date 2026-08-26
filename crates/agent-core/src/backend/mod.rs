//! Inference backends.
//!
//! One trait, so the choice of backend is confined. The first implementation
//! talks to Ollama over HTTP; binding `llama-cpp-rs` directly is the eventual
//! answer for KV-cache control, deferred until there is something to measure
//! (`RECORD/2026-08-26.walking-skeleton.md`).

use std::pin::Pin;

use futures_util::Stream;
use serde::{Deserialize, Serialize};

pub mod mock;
pub mod ollama;

/// A message in the conversation handed to the model.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: Role,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Role {
    System,
    User,
    Assistant,
}

impl Message {
    pub fn system(content: impl Into<String>) -> Self {
        Self { role: Role::System, content: content.into() }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self { role: Role::User, content: content.into() }
    }
}

/// What the core asks a backend for. Deliberately not a prompt string: the
/// stable prefix (system text, and later the tool definitions) has to stay
/// byte-identical across calls for the prompt cache to be worth anything, so
/// the backend assembles it the same way every time rather than each caller
/// formatting its own.
#[derive(Debug, Clone)]
pub struct CompletionRequest {
    pub model: String,
    pub messages: Vec<Message>,
}

/// Token counts, as the backend reports them. Not our own tokenizer's opinion —
/// that arrives with the context manager, and the two disagreeing is a finding,
/// not a bug to paper over.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Usage {
    pub prompt_tokens: u32,
    pub completion_tokens: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StopReason {
    /// The model stopped on its own.
    Stop,
    /// The backend hit a length limit.
    Length,
    /// Something else, reported verbatim by the backend.
    Other,
}

/// What a backend yields while generating.
#[derive(Debug, Clone)]
pub enum Chunk {
    Text(String),
    Done { stop: StopReason, usage: Usage },
}

#[derive(Debug, thiserror::Error)]
pub enum BackendError {
    #[error("transport: {0}")]
    Transport(String),
    #[error("backend rejected the request: {0}")]
    Rejected(String),
    #[error("malformed response: {0}")]
    Malformed(String),
}

pub type ChunkStream<'a> =
    Pin<Box<dyn Stream<Item = Result<Chunk, BackendError>> + Send + 'a>>;

/// Object-safe on purpose: the turn loop holds a `dyn Backend`, so swapping
/// Ollama for an FFI binding never reaches the loop.
pub trait Backend: Send + Sync {
    fn name(&self) -> &str;
    fn stream(&self, request: CompletionRequest) -> ChunkStream<'_>;
}
