//! A backend that streams canned text, with a configurable delay between
//! tokens.
//!
//! Not a test fixture that leaked into the library: the walking skeleton has to
//! be runnable, and its behaviour observable, without a model on the machine.
//! It is also the only way to exercise the slow-generation path deterministically
//! — which is exactly where a UI that re-renders per token falls over.

use std::time::Duration;

use super::{Backend, BackendError, Chunk, ChunkStream, CompletionRequest, StopReason, Usage};

pub struct Mock {
    reply: String,
    delay: Duration,
    fail_with: Option<String>,
}

impl Mock {
    pub fn new(reply: impl Into<String>) -> Self {
        Self { reply: reply.into(), delay: Duration::from_millis(25), fail_with: None }
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Makes the backend fail partway, so the error path is reachable on demand.
    pub fn failing(mut self, message: impl Into<String>) -> Self {
        self.fail_with = Some(message.into());
        self
    }
}

impl Default for Mock {
    fn default() -> Self {
        Self::new(
            "This is the mock backend. It streams a fixed reply one word at a \
             time so the agent loop, the protocol and the debug UI can be run \
             without a model on the machine.",
        )
    }
}

impl Backend for Mock {
    fn name(&self) -> &str {
        "mock"
    }

    fn stream(&self, _request: CompletionRequest) -> ChunkStream<'_> {
        let words: Vec<String> =
            self.reply.split_inclusive(' ').map(str::to_string).collect();
        let delay = self.delay;
        let fail_with = self.fail_with.clone();

        Box::pin(async_stream::try_stream! {
            let completion_tokens = words.len() as u32;

            for (i, word) in words.into_iter().enumerate() {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                if let Some(message) = &fail_with
                    && i == completion_tokens as usize / 2
                {
                    Err(BackendError::Transport(message.clone()))?;
                    return;
                }
                yield Chunk::Text(word);
            }

            yield Chunk::Done {
                stop: StopReason::Stop,
                usage: Usage { prompt_tokens: 0, completion_tokens },
            };
        })
    }
}
