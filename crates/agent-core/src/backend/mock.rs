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
    /// Answer a request for a plan with a plan block instead of the canned
    /// reply. Only on the default mock: a scripted reply is what the caller
    /// asked for, and second-guessing it would make `--mock-reply` mean
    /// "usually".
    plans: bool,
}

/// What the mock proposes when it is asked for a plan.
///
/// Short and structured on purpose. A plan the parser rejects falls back to the
/// model's prose, which is a bigger plan and a bigger summary — so a mock that
/// never answers in the block makes the task scaffolding look more expensive
/// than it is, and that number is one the context strategy is judged by.
const PLAN: &str = "```plan\n\
    {\"steps\":[\"read what is there\",\"work out what it says\",\"answer\"],\
    \"paths\":[],\"commands\":[]}\n\
    ```";

impl Mock {
    pub fn new(reply: impl Into<String>) -> Self {
        Self::replies(vec![reply.into()])
    }

    pub fn replies(replies: Vec<String>) -> Self {
        Self {
            replies: std::sync::Mutex::new(replies.into()),
            delay: Duration::from_millis(25),
            fail_with: None,
            plans: false,
        }
    }

    pub fn delay(mut self, delay: Duration) -> Self {
        self.delay = delay;
        self
    }

    /// Answer a planning call with [`PLAN`]. On by default and off whenever
    /// replies were scripted.
    pub fn answering_plan_requests(mut self) -> Self {
        self.plans = true;
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
        .answering_plan_requests()
    }
}

impl Backend for Mock {
    fn name(&self) -> &str {
        "mock"
    }

    fn stream(&self, request: CompletionRequest) -> ChunkStream<'_> {
        // Reading the request is the one thing this backend does with it, and
        // it is the same trick `--mock-reply` performs by hand: a planning call
        // is recognisable because the message asks for the block by name.
        if self.plans && asks_for_a_plan(&request) {
            return stream_words(PLAN.to_string(), self.delay, None);
        }
        let reply = {
            let mut replies = self.replies.lock().expect("no panic holds this lock");
            match replies.len() > 1 {
                true => replies.pop_front().unwrap_or_default(),
                false => replies.front().cloned().unwrap_or_default(),
            }
        };
        stream_words(reply, self.delay, self.fail_with.clone())
    }
}

/// The last user message asks for a plan when it names the block it wants back.
fn asks_for_a_plan(request: &CompletionRequest) -> bool {
    request
        .messages
        .last()
        .is_some_and(|message| message.content.contains("```plan"))
}

fn stream_words(reply: String, delay: Duration, fail_with: Option<String>) -> ChunkStream<'static> {
    // Word by word, because what this backend exists to exercise is a stream
    // arriving in pieces.
    let words: Vec<String> = reply.split_inclusive(' ').map(str::to_string).collect();

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
            usage: Usage { prompt_tokens: 0, completion_tokens },
        };
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::task::{Plan, plan_request};
    use futures_util::StreamExt;

    async fn answer(backend: &Mock, prompt: &str) -> String {
        let mut text = String::new();
        let mut stream = backend.stream(CompletionRequest {
            model: "mock".into(),
            messages: vec![super::super::Message::user(prompt)],
            context_limit: None,
        });
        while let Some(Ok(chunk)) = stream.next().await {
            if let Chunk::Text(delta) = chunk {
                text.push_str(&delta);
            }
        }
        text
    }

    #[tokio::test]
    async fn a_planning_call_gets_a_plan_that_parses() {
        // Otherwise every plan in a mock run is prose taken as steps, which
        // makes the task scaffolding look more expensive than it is — and that
        // is a number the context strategy is judged by.
        let plan = Plan::from_reply(
            "find out what it does",
            &answer(
                &Mock::default().delay(Duration::ZERO),
                &plan_request("find out what it does"),
            )
            .await,
        );
        assert!(plan.parsed, "the mock's plan has to survive the parser");
        assert_eq!(plan.steps.len(), 3);
    }

    #[tokio::test]
    async fn an_ordinary_prompt_still_gets_the_canned_reply() {
        let text = answer(
            &Mock::default().delay(Duration::ZERO),
            "what is in AGENTS.md?",
        )
        .await;
        assert!(text.starts_with("This is the mock backend"));
    }

    #[tokio::test]
    async fn a_scripted_reply_wins_over_the_plan_answer() {
        // `--mock-reply` is what the caller asked for. Second-guessing it would
        // make the flag mean "usually".
        let backend = Mock::replies(vec!["scripted".into()]).delay(Duration::ZERO);
        assert_eq!(
            answer(&backend, &plan_request("anything")).await,
            "scripted"
        );
    }
}
