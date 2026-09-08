//! Inference backends.
//!
//! One trait, so the choice of backend is confined. The first implementation
//! talks to Ollama over HTTP; binding `llama-cpp-rs` directly is the eventual
//! answer for KV-cache control, deferred until there is something to measure
//! (`RECORD/2026-08-26.walking-skeleton.completed.md`).

use std::pin::Pin;

use futures_util::Stream;
use serde::{Deserialize, Serialize};

pub mod mock;
pub mod ollama;
pub mod openai;

/// A message in the conversation handed to the model.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
        Self {
            role: Role::System,
            content: content.into(),
        }
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: Role::User,
            content: content.into(),
        }
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: Role::Assistant,
            content: content.into(),
        }
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
    /// The window the caller budgeted against, when it knows one.
    ///
    /// It has to be *sent*, not merely respected: Ollama's own default is a
    /// couple of thousand tokens and it silently truncates the prompt to it, so
    /// a run that budgets 32k against a server serving 4k measures a prompt the
    /// model never saw — and every bucket, every reuse figure and every usage
    /// count in that run is a reading of something else. `None` is the CLI's
    /// "unknown window", where there was nothing to budget and nothing to send.
    pub context_limit: Option<u32>,
    /// Pinned sampling, so two runs meant to be compared differ only by what
    /// they're testing. `None` leaves it to the server's own default — the
    /// same "unknown, so nothing sent" rule `context_limit` follows, for the
    /// same reason: a made-up default would be a second thing the two runs
    /// could differ by without either one saying so.
    pub temperature: Option<f32>,
    pub seed: Option<u32>,
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
    Done {
        stop: StopReason,
        /// `None` is **not reported**, which is a different fact from zero and
        /// has to stay different: this is the number the budget panel plots
        /// against our own count, and a zero would read as "the server saw an
        /// empty prompt". Ollama always reports counts; an OpenAI-compatible
        /// server reports them only when asked (`stream_options.include_usage`)
        /// and some do not report them at all. See
        /// `RECORD/2026-09-01.an-openai-compatible-backend.completed.md`.
        usage: Option<Usage>,
    },
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

pub type ChunkStream<'a> = Pin<Box<dyn Stream<Item = Result<Chunk, BackendError>> + Send + 'a>>;

/// Object-safe on purpose: the turn loop holds a `dyn Backend`, so swapping
/// Ollama for an FFI binding never reaches the loop.
/// How long a model listing may take. Not the generation timeout: nothing is
/// being generated, and the caller is a page waiting on a request.
pub const LIST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// A future a `dyn Backend` can hand back. Boxed for the same reason
/// [`ChunkStream`] is: the turn loop holds a `dyn Backend`.
pub type BackendFuture<'a, T> =
    Pin<Box<dyn std::future::Future<Output = Result<T, BackendError>> + Send + 'a>>;

pub trait Backend: Send + Sync {
    fn name(&self) -> &str;
    fn stream(&self, request: CompletionRequest) -> ChunkStream<'_>;

    /// The models this destination will answer for, when it can say.
    ///
    /// The default is **empty, not an error**: "this destination does not offer
    /// a list" is a fact about a backend, and a caller that has to tell it apart
    /// from a failure gets that from the `Err` arm. Nothing in a turn calls
    /// this — it exists so a person choosing a model is choosing from what is
    /// actually pulled rather than typing one from memory.
    fn models(&self) -> BackendFuture<'_, Vec<String>> {
        Box::pin(async { Ok(Vec::new()) })
    }
}
