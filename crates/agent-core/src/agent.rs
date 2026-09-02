//! The loop: prompt, model, tool, model, answer.
//!
//! ```text
//! prompt ─► model ─► text with a tool call in it?
//!                      │ no  → the turn's answer
//!                      │ yes → parse → check the policy → execute → append → model
//! ```
//!
//! [`run_turn`] is the model half and this is the loop around it, rather than
//! one function doing both: cancellation, streaming and the backend's stop
//! reasons are already right there, and a second implementation of them would
//! be a second set of bugs.
//!
//! Every step is a real message pair — `assistant(call)` then `user(result)` —
//! so the strict alternation the prompt shape depends on holds all the way
//! through, and the turn that gets stored can be replayed exactly.
//!
//! See `RECORD/2026-08-27.tools-and-sandbox.completed.md`.

use tokio::sync::{mpsc, watch};

use crate::backend::{Backend, CompletionRequest, Message, Usage};
use crate::sandbox::Sandbox;
use crate::tools::{ToolStep, Tools, parse_call};
use crate::turn::{EndReason, TurnEvent, TurnOutcome, run_turn};

/// How many tool calls one turn may make before it has to answer.
///
/// A default, not a law. Too low and the agent cannot finish an investigation;
/// too high and a model that has decided to `list_dir` forever costs a session.
pub const DEFAULT_MAX_STEPS: u32 = 8;

/// What a turn with tools produced.
#[derive(Debug, Clone)]
pub struct AgentOutcome {
    /// The final assistant text — what the model said once it stopped calling
    /// tools. Partial on a cancel or a failure.
    pub text: String,
    /// What it did on the way, in order, ready to be stored with the turn.
    pub steps: Vec<ToolStep>,
    pub reason: EndReason,
    /// Summed over the model calls this turn made. The prompt was sent once per
    /// step, so this double-counts the shared prefix on purpose: it is what the
    /// backend actually processed, and it is the cost the turn paid.
    pub usage: Option<Usage>,
    pub error: Option<String>,
}

impl AgentOutcome {
    fn from_final(
        text: String,
        steps: Vec<ToolStep>,
        last: TurnOutcome,
        usage: Option<Usage>,
    ) -> Self {
        Self {
            text,
            steps,
            reason: last.reason,
            usage,
            error: last.error,
        }
    }
}

/// Runs one turn, letting the model use tools until it answers.
///
/// `request.messages` is the prompt as the context manager assembled it; the
/// steps are appended to a copy, so the caller's selection is not rewritten
/// underneath it.
pub async fn run_agent_turn(
    backend: &dyn Backend,
    request: CompletionRequest,
    tools: &Tools,
    sandbox: &Sandbox,
    max_steps: u32,
    events: mpsc::Sender<TurnEvent>,
    cancel: watch::Receiver<bool>,
) -> AgentOutcome {
    let CompletionRequest {
        model,
        mut messages,
        context_limit,
        temperature,
        seed,
    } = request;
    let mut steps: Vec<ToolStep> = Vec::new();
    let mut usage: Option<Usage> = None;

    for step in 1..=max_steps.max(1) {
        // Tokens are forwarded as they arrive; the intermediate `Ended` is not,
        // because a turn ends once and a client that saw three would draw three.
        let (inner, mut inbox) = mpsc::channel::<TurnEvent>(256);
        let forward = {
            let events = events.clone();
            tokio::spawn(async move {
                while let Some(event) = inbox.recv().await {
                    if let TurnEvent::Token(_) = event
                        && events.send(event).await.is_err()
                    {
                        break;
                    }
                }
            })
        };

        // Before the call, and including the first: the loop announces every
        // call it makes, and what to do with that is the caller's business.
        let _ = events
            .send(TurnEvent::ModelCall {
                step,
                messages: messages.clone(),
            })
            .await;

        let outcome = run_turn(
            backend,
            CompletionRequest {
                model: model.clone(),
                messages: messages.clone(),
                // Every call of the turn budgets against the same window, so
                // every call has to be told the same one.
                context_limit,
                temperature,
                seed,
            },
            inner,
            cancel.clone(),
        )
        .await;
        let _ = forward.await;
        usage = sum(usage, outcome.usage);

        // A failed or cancelled step ends the turn where it is. Feeding a tool
        // call parsed out of a half-generated answer back into the loop would
        // execute something nobody finished asking for.
        if outcome.error.is_some() || outcome.reason == EndReason::Cancelled {
            let _ = events
                .send(match &outcome.error {
                    Some(error) => TurnEvent::Failed(error.clone()),
                    None => TurnEvent::Ended {
                        reason: EndReason::Cancelled,
                        usage: None,
                    },
                })
                .await;
            return AgentOutcome::from_final(outcome.text.clone(), steps, outcome, usage);
        }

        let Some(call) = parse_call(&outcome.text) else {
            let _ = events
                .send(TurnEvent::Ended {
                    reason: outcome.reason,
                    usage,
                })
                .await;
            return AgentOutcome::from_final(outcome.text.clone(), steps, outcome, usage);
        };

        // Emitted before the sandbox is consulted, so a denial reads as a call
        // that was refused rather than as nothing having happened.
        let _ = events
            .send(TurnEvent::ToolCall {
                step,
                call: call.clone(),
            })
            .await;

        let started = std::time::Instant::now();
        let result = tools.call(&call, sandbox).await;
        let taken = ToolStep {
            text: outcome.text.clone(),
            call,
            outcome: result,
            duration_ms: started.elapsed().as_millis() as u64,
        };

        let _ = events
            .send(TurnEvent::ToolResult {
                step,
                outcome: Box::new(taken.clone()),
            })
            .await;

        messages.push(Message::assistant(taken.text.clone()));
        messages.push(Message::user(taken.result_text()));
        steps.push(taken);
    }

    // The budget is spent and the model is still working. Saying `stop` here
    // would present an investigation cut short as a conclusion.
    let text = steps
        .last()
        .map(|step| step.text.clone())
        .unwrap_or_default();
    let _ = events
        .send(TurnEvent::Ended {
            reason: EndReason::ToolLimit,
            usage,
        })
        .await;
    AgentOutcome {
        text,
        steps,
        reason: EndReason::ToolLimit,
        usage,
        error: None,
    }
}

/// Adds a step's counts to the turn's. Unknown plus known is known: a backend
/// that reported nothing on one step did not make the others unmeasured.
fn sum(total: Option<Usage>, step: Option<Usage>) -> Option<Usage> {
    match (total, step) {
        (Some(total), Some(step)) => Some(Usage {
            prompt_tokens: total.prompt_tokens + step.prompt_tokens,
            completion_tokens: total.completion_tokens + step.completion_tokens,
        }),
        (total, step) => total.or(step),
    }
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;
    use crate::backend::{Chunk, ChunkStream, StopReason};
    use crate::sandbox::{Access, Applied, PathRule, SandboxPolicy};

    /// A backend that says one scripted thing per call, so a tool loop can be
    /// written down as the conversation it is meant to have.
    struct Scripted {
        replies: std::sync::Mutex<std::collections::VecDeque<String>>,
    }

    impl Scripted {
        fn new(replies: &[&str]) -> Self {
            Self {
                replies: std::sync::Mutex::new(
                    replies.iter().map(|text| (*text).to_string()).collect(),
                ),
            }
        }
    }

    impl Backend for Scripted {
        fn name(&self) -> &str {
            "scripted"
        }

        fn stream(&self, _request: CompletionRequest) -> ChunkStream<'_> {
            let text = self
                .replies
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or_else(|| "done".to_string());
            Box::pin(futures_util::stream::iter(vec![
                Ok(Chunk::Text(text)),
                Ok(Chunk::Done {
                    stop: StopReason::Stop,
                    usage: Some(Usage {
                        prompt_tokens: 10,
                        completion_tokens: 2,
                    }),
                }),
            ]))
        }
    }

    struct Fixture {
        root: PathBuf,
        sandbox: Sandbox,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "luu-agent-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("notes.txt"), "the answer is 42\n").unwrap();
            let root = root.canonicalize().unwrap();
            let sandbox = Sandbox::new(
                &SandboxPolicy {
                    paths: vec![PathRule::new(".", Access::ReadWrite)],
                    ..SandboxPolicy::default()
                },
                &root,
            )
            .unwrap();
            Self { root, sandbox }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    async fn drive(
        fixture: &Fixture,
        replies: &[&str],
        max_steps: u32,
    ) -> (Vec<TurnEvent>, AgentOutcome) {
        let backend = Scripted::new(replies);
        let tools = Tools::standard();
        let (tx, mut rx) = mpsc::channel(256);
        let drain = tokio::spawn(async move {
            let mut seen = Vec::new();
            while let Some(event) = rx.recv().await {
                seen.push(event);
            }
            seen
        });
        let (_stop, cancel) = watch::channel(false);
        let outcome = run_agent_turn(
            &backend,
            CompletionRequest {
                model: "scripted".into(),
                messages: vec![Message::user("what is in notes.txt?")],
                context_limit: None,
                temperature: None,
                seed: None,
            },
            &tools,
            &fixture.sandbox,
            max_steps,
            tx,
            cancel,
        )
        .await;
        (drain.await.unwrap(), outcome)
    }

    #[tokio::test]
    async fn every_model_call_is_announced_with_what_it_sends() {
        // A tooled turn is two calls, and the counts the backend reports on
        // `Ended` are summed over both. Before this event only the first was
        // measurable, so on any turn with a tool our count and the backend's
        // were not counts of the same thing — 1 590 against 3 552 on the run
        // that found it. See `RECORD/2026-08-27.the-m4-pro-run.completed.md`.
        let fixture = Fixture::new("modelcalls");
        let (events, _) = drive(
            &fixture,
            &[
                "Let me look.\n```tool\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"notes.txt\"}}\n```",
                "It says the answer is 42.",
            ],
            DEFAULT_MAX_STEPS,
        )
        .await;

        let calls: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                TurnEvent::ModelCall { step, messages } => Some((*step, messages)),
                _ => None,
            })
            .collect();
        assert_eq!(
            calls.len(),
            2,
            "one call per round trip, the first included"
        );
        assert_eq!(calls[0].0, 1);
        assert_eq!(calls[1].0, 2);
        assert_eq!(
            calls[1].1.len(),
            calls[0].1.len() + 2,
            "the second call carries the assistant's call and the tool's result",
        );
    }

    #[tokio::test]
    async fn a_tool_call_is_executed_and_its_result_comes_back_to_the_model() {
        let fixture = Fixture::new("readloop");
        let (events, outcome) = drive(
            &fixture,
            &[
                "Let me look.\n```tool\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"notes.txt\"}}\n```",
                "It says the answer is 42.",
            ],
            DEFAULT_MAX_STEPS,
        )
        .await;

        assert_eq!(outcome.text, "It says the answer is 42.");
        assert_eq!(outcome.steps.len(), 1);
        assert!(outcome.steps[0].outcome.output.contains("42"));
        assert_eq!(outcome.reason, EndReason::Stop);

        assert_eq!(
            events
                .iter()
                .filter(|event| matches!(event, TurnEvent::Ended { .. }))
                .count(),
            1,
            "a turn ends once, however many model calls it took",
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TurnEvent::ToolCall { .. }))
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TurnEvent::ToolResult { .. }))
        );
    }

    #[tokio::test]
    async fn a_denied_call_is_reported_to_the_model_rather_than_ending_the_turn() {
        let fixture = Fixture::new("denied");
        let (_, outcome) = drive(
            &fixture,
            &[
                "```tool\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"/etc/passwd\"}}\n```",
                "I cannot read that.",
            ],
            DEFAULT_MAX_STEPS,
        )
        .await;

        assert_eq!(outcome.steps.len(), 1);
        assert!(!outcome.steps[0].outcome.verdict.allowed);
        assert!(
            outcome.steps[0].result_text().contains("denied"),
            "the model is told, and can try something else"
        );
        assert_eq!(outcome.text, "I cannot read that.");
    }

    #[tokio::test]
    async fn a_plain_answer_makes_no_calls() {
        let fixture = Fixture::new("plain");
        let (events, outcome) = drive(&fixture, &["It is a text file."], DEFAULT_MAX_STEPS).await;
        assert!(outcome.steps.is_empty());
        assert!(
            !events
                .iter()
                .any(|e| matches!(e, TurnEvent::ToolCall { .. }))
        );
    }

    #[tokio::test]
    async fn a_model_that_never_answers_stops_at_the_limit_and_says_so() {
        let fixture = Fixture::new("limit");
        let looping = "```tool\n{\"name\":\"list_dir\",\"arguments\":{}}\n```";
        let (_, outcome) = drive(&fixture, &[looping, looping, looping, looping], 2).await;

        assert_eq!(outcome.steps.len(), 2);
        assert_eq!(
            outcome.reason,
            EndReason::ToolLimit,
            "`stop` would present an investigation cut short as a conclusion",
        );
    }

    #[tokio::test]
    async fn usage_is_summed_over_the_calls_the_turn_actually_made() {
        let fixture = Fixture::new("usage");
        let (_, outcome) = drive(
            &fixture,
            &[
                "```tool\n{\"name\":\"list_dir\",\"arguments\":{}}\n```",
                "There is one file.",
            ],
            DEFAULT_MAX_STEPS,
        )
        .await;
        // Two model calls at 10 prompt tokens each: the prefix was sent twice
        // and paid for twice, which is the number worth reporting.
        assert_eq!(outcome.usage.unwrap().prompt_tokens, 20);
    }

    #[tokio::test]
    async fn the_verdict_travels_with_the_result() {
        let fixture = Fixture::new("verdict");
        let (events, _) = drive(
            &fixture,
            &[
                "```tool\n{\"name\":\"list_dir\",\"arguments\":{}}\n```",
                "done",
            ],
            DEFAULT_MAX_STEPS,
        )
        .await;

        let result = events
            .iter()
            .find_map(|event| match event {
                TurnEvent::ToolResult { outcome, .. } => Some(outcome),
                _ => None,
            })
            .unwrap();
        assert!(result.outcome.verdict.allowed);
        assert_eq!(
            result.outcome.verdict.enforced_by,
            Applied::Process,
            "an in-process tool is held by an in-process check, and says so",
        );
        assert!(!result.outcome.verdict.rule.is_empty());
    }

    #[tokio::test]
    async fn the_steps_stay_in_the_conversation_the_model_sees() {
        // The point of storing them: on the second step the model is looking at
        // its own call and the result, not at a prompt where neither happened.
        let fixture = Fixture::new("history");
        let (_, outcome) = drive(
            &fixture,
            &[
                "```tool\n{\"name\":\"read_file\",\"arguments\":{\"path\":\"notes.txt\"}}\n```",
                "42.",
            ],
            DEFAULT_MAX_STEPS,
        )
        .await;
        assert!(outcome.steps[0].text.contains("read_file"));
        assert!(outcome.steps[0].result_text().starts_with("[read_file] ok"));
    }
}
