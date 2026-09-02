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
    /// One per call, in order; the last one repeats once they run out. A single
    /// reply is the ordinary case and a list is what makes the tool loop
    /// runnable without a model — a scripted call, then the answer to its
    /// result.
    replies: std::sync::Mutex<std::collections::VecDeque<String>>,
    delay: Duration,
    fail_with: Option<String>,
}

impl Mock {
    pub fn new(reply: impl Into<String>) -> Self {
        Self::replies(vec![reply.into()])
    }

    pub fn replies(replies: Vec<String>) -> Self {
        Self {
            replies: std::sync::Mutex::new(replies.into()),
            delay: Duration::from_millis(25),
            fail_with: None,
        }
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
        let reply = {
            let mut replies = self.replies.lock().expect("no panic holds this lock");
            match replies.len() > 1 {
                true => replies.pop_front().unwrap_or_default(),
                false => replies.front().cloned().unwrap_or_default(),
            }
        };
        // Word by word, because what this backend exists to exercise is a
        // stream arriving in pieces.
        let words: Vec<String> = reply.split_inclusive(' ').map(str::to_string).collect();
        let delay = self.delay;
        let fail_with = self.fail_with.clone();

        Box::pin(async_stream::try_stream! {
            let completion_tokens = words.len() as u32;

            for (i, word) in words.into_iter().enumerate() {
                if !delay.is_zero() {
                    tokio::time::sleep(delay).await;
                }
                // Same macro-expansion limit as in `ollama.rs`: no let chains here.
                let failing = fail_with.as_ref().filter(|_| i == completion_tokens as usize / 2);
                if let Some(message) = failing {
                    Err(BackendError::Transport(message.clone()))?;
                    return;
                }
                yield Chunk::Text(word);
            }

            yield Chunk::Done {
                stop: StopReason::Stop,
                // `Some`, and the prompt count is honestly zero: the mock never
                // read a prompt. That is not the same as a backend that read
                // one and did not say how big it was, which is `None`.
                usage: Some(Usage { prompt_tokens: 0, completion_tokens }),
            };
        })
    }
}
