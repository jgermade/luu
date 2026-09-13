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
    /// One per call, in order; the last one repeats once they run out — unless
    /// [`Self::cycling`], where the list is a ring instead. A single reply is
    /// the ordinary case and a list is what makes the tool loop runnable
    /// without a model — a scripted call, then the answer to its result.
    replies: std::sync::Mutex<std::collections::VecDeque<String>>,
    /// Rotate the list instead of holding the last reply, so `2 × n` replies
    /// are `n` turns of *call, then the answer to its result*, repeated.
    ///
    /// The mock is handed a prompt and never a turn, so nothing here knows
    /// where one begins: the phase holds because a turn costs
    /// `1 + (the calls it made)` model calls and a scripted run makes no
    /// planning call. A turn that calls one tool fewer than the list expects
    /// shifts every later turn by one, and it is the *run* that notices — a
    /// turn with no step in its recording. See
    /// `RECORD/2026-09-13.a-mock-that-calls-a-tool.completed.md`.
    cycling: bool,
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
            cycling: false,
            delay: Duration::from_millis(25),
            fail_with: None,
        }
    }

    /// The list as a ring: what makes a corpus where every turn calls a tool,
    /// which is what measuring a rule over tool results needs and what
    /// `--mock-reply` cannot produce.
    pub fn cycling(mut self) -> Self {
        self.cycling = true;
        self
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

    /// The one name it answers to, so a picker over a mock profile is not empty.
    fn models(&self) -> super::BackendFuture<'_, Vec<String>> {
        Box::pin(async { Ok(vec!["mock".to_string()]) })
    }

    fn stream(&self, _request: CompletionRequest) -> ChunkStream<'_> {
        let reply = {
            let mut replies = self.replies.lock().expect("no panic holds this lock");
            match (self.cycling, replies.len() > 1) {
                (true, _) => {
                    let reply = replies.pop_front().unwrap_or_default();
                    replies.push_back(reply.clone());
                    reply
                }
                (false, true) => replies.pop_front().unwrap_or_default(),
                (false, false) => replies.front().cloned().unwrap_or_default(),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::backend::Backend;
    use futures_util::StreamExt;

    async fn say(backend: &Mock, calls: usize) -> Vec<String> {
        let mut said = Vec::new();
        for _ in 0..calls {
            let mut stream = backend.stream(CompletionRequest {
                model: "mock".into(),
                messages: Vec::new(),
                context_limit: None,
                temperature: None,
                seed: None,
            });
            let mut text = String::new();
            while let Some(chunk) = stream.next().await {
                if let Ok(Chunk::Text(word)) = chunk {
                    text.push_str(&word);
                }
            }
            said.push(text);
        }
        said
    }

    #[tokio::test]
    async fn the_last_reply_repeats_by_default() {
        let backend = Mock::replies(vec!["uno".into(), "dos".into()]).delay(Duration::ZERO);
        assert_eq!(say(&backend, 4).await, ["uno", "dos", "dos", "dos"]);
    }

    /// The whole of the instrument: two replies are one turn's worth of calls,
    /// and turn 11 is answered exactly as turn 1 was.
    #[tokio::test]
    async fn cycling_makes_the_list_a_ring() {
        let backend = Mock::replies(vec!["call".into(), "answer".into()])
            .cycling()
            .delay(Duration::ZERO);
        assert_eq!(
            say(&backend, 6).await,
            ["call", "answer", "call", "answer", "call", "answer"]
        );
    }

    /// A ring of one is the constant reply, not an empty one: the guard that
    /// stops `--mock-script` with a single block from answering nothing.
    #[tokio::test]
    async fn a_ring_of_one_repeats_it() {
        let backend = Mock::replies(vec!["solo".into()])
            .cycling()
            .delay(Duration::ZERO);
        assert_eq!(say(&backend, 3).await, ["solo", "solo", "solo"]);
    }
}
