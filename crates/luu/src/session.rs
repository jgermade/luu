//! What `chat` and `serve` must not each invent.
//!
//! Both drive the same loop, and both need the same prompt assembled the same
//! way. Two call sites building it by hand is exactly what the byte-identical
//! prefix commitment in `AGENTS.md` warns about, so there is one.

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::backend::{Backend, CompletionRequest, Message};
use agent_core::context::{
    ApproximateCounter, Budget, Context as AgentContext, Counter, Fragment, ModelCounter,
    TokenCounter,
};
use agent_core::protocol::{self, ServerMessage, TurnId};
use agent_core::record::{self, RecordLine};
use agent_core::sandbox::Sandbox;
use agent_core::task::{Plan, TaskId, plan_request};
use agent_core::tools::Tools;
use agent_core::trace::{Shared, TraceMessage};
use agent_core::turn::run_turn;
use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;
use tokio::sync::watch;

/// The fixed head of every prompt. Kept as one `&'static str` because the
/// prompt cache reuses it byte for byte — a formatting change here is a cache
/// miss on every call, and nothing fails to tell you.
pub const SYSTEM: &str = "You are Loude, a concise local coding agent.";

/// The sandbox and the tool set, resolved once and shared by `chat` and
/// `serve`. Here for the same reason the prompt assembly is: two call sites
/// resolving a policy differently is two sandboxes.
#[derive(Clone)]
pub struct Agency {
    pub tools: Arc<Tools>,
    pub sandbox: Arc<Sandbox>,
    pub max_steps: u32,
}

impl Agency {
    /// The tool definitions as they go into the cached prefix. Empty when there
    /// are no tools, so a run without them sends the same bytes it always did.
    pub fn definitions(&self) -> String {
        self.tools.definitions()
    }

    /// What `luu tools` prints and what a session says on startup.
    ///
    /// Every grant is listed, the implicit ones included: a sandbox whose real
    /// extent has to be inferred from the file that configured it is one nobody
    /// checks.
    pub fn describe(&self) -> String {
        let mut text = format!("sandbox — base {}\n", self.sandbox.base().display());
        for root in self.sandbox.roots() {
            text.push_str(&format!(
                "  {:<10} {}{}\n",
                root.access.as_str(),
                root.path.display(),
                match root.implicit {
                    true => "   (implicit: commands need their interpreter)",
                    false => "",
                }
            ));
        }
        text.push_str(&format!(
            "  commands   {}\n  network    {}\n  enforce    {}\n",
            match self.sandbox.commands().is_empty() {
                true => "(none)".to_string(),
                false => self.sandbox.commands().join(", "),
            },
            match self.sandbox.network() {
                true => "allowed",
                false => "denied",
            },
            self.sandbox.enforcement().as_str(),
        ));
        text.push_str(&format!(
            "  tools      {}\n",
            match self.tools.is_empty() {
                true => "(none)".to_string(),
                false => self.tools.names().collect::<Vec<_>>().join(", "),
            }
        ));
        text
    }
}

/// How much of the window is held back for the answer. A default, not a law:
/// too large wastes context, too small truncates answers, and the only way to
/// find out is to measure.
pub const DEFAULT_RESERVE: u32 = 512;

/// Builds the counter, and says out loud when it is not a real one.
///
/// An explicit `--tokenizer` that fails is an error: it was asked for. An
/// absent one degrades to the approximate counter with a warning, so a first
/// run works without hunting down a `tokenizer.json` — and every number it
/// produces is labelled all the way to the panel.
pub fn counter_for(
    model: &str,
    tokenizer: Option<&Path>,
) -> Result<(Arc<dyn TokenCounter>, Option<String>)> {
    match tokenizer {
        Some(path) => {
            let counter = ModelCounter::from_file(path, model)
                .with_context(|| format!("--tokenizer {}", path.display()))?;
            Ok((Arc::new(counter), None))
        }
        None => Ok((
            Arc::new(ApproximateCounter),
            Some(format!(
                "no tokenizer for {model}: counting approximately (chars/4). \
                 Pass --tokenizer <tokenizer.json> for real numbers."
            )),
        )),
    }
}

/// Asks the model for a plan, and reads one out of whatever it answered.
///
/// An ordinary user message on top of the unchanged prefix, so proposing a task
/// costs no prompt cache. The exchange is **not remembered**: the approved plan
/// enters the history, the request that produced it does not, or the objective
/// is in the prompt twice before any work has happened.
///
/// It does not stream. The client is shown the plan, not its generation — a
/// silent wait on a slow local model, and the first thing to change if it bites.
/// Returns the plan and the exact prompt that produced it, which is what the
/// trace channel needs in order to say what proposing cost.
///
/// `code_context` is whatever is pending for the next turn: a planning call
/// **sees** the fragments and does not consume them. The point of attaching a
/// file before proposing is that the plan is about that file — shown nothing, a
/// real 7B planned confidently against Python paths in a Rust workspace. The
/// turn that follows still gets them, and paying twice for one file is the
/// price of a plan made in view of it.
#[allow(clippy::too_many_arguments)]
pub async fn propose_plan(
    backend: &dyn Backend,
    model: &str,
    context: &mut AgentContext,
    budget: Budget,
    counter: &dyn TokenCounter,
    objective: &str,
    code_context: &[Fragment],
) -> (Plan, String) {
    let selection = context.select(&plan_request(objective), code_context, budget, counter);
    let sent = rendered(&selection.messages);
    let (events, mut inbox) = mpsc::channel(256);
    let drain = tokio::spawn(async move { while inbox.recv().await.is_some() {} });
    // Nothing cancels a planning call yet: it is one short generation, and a
    // cancel channel nobody holds is a sender dropped mid-turn.
    let (stop, cancel) = watch::channel(false);

    let outcome = run_turn(
        backend,
        CompletionRequest {
            model: model.to_string(),
            messages: selection.messages,
            context_limit: budget.limit,
        },
        events,
        cancel,
    )
    .await;
    drop(stop);
    let _ = drain.await;

    (Plan::from_reply(objective, &outcome.text), sent)
}

/// What the backend is about to receive, as one string, for the trace channel.
/// Not the wire format any backend uses — it is a rendering, and the panel that
/// diffs two of them only needs them to be rendered the same way twice.
pub fn rendered(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|m| format!("<|{:?}|>\n{}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// The previous *call's* rendered prompt, kept so the next one can be measured
/// against it.
///
/// Every call, not every turn: a planning call is a call, and it is what the
/// server's cache holds afterwards. Measuring the turn after a proposal against
/// the turn before it would report reuse against a prompt the cache no longer
/// has.
///
/// Here rather than in `agent-core` because the thing measured is a rendering,
/// and the rendering is this module's; here rather than in each caller because
/// `chat` and `serve` measuring against differently-assembled prompts is the
/// same failure the module exists to prevent.
#[derive(Default)]
pub struct PrefixTracker {
    previous: Option<String>,
}

impl PrefixTracker {
    /// Measures this turn's prompt against the one before it, and remembers it
    /// for the next. `None` on the first turn of a session: there is no
    /// previous prompt, so the quantity does not exist — and "0% reuse" would
    /// read as a cold cache rather than as the absence of one.
    pub fn measure(
        &mut self,
        turn: TurnId,
        prompt: &str,
        counter: &dyn TokenCounter,
    ) -> Option<TraceMessage> {
        let message = self
            .previous
            .as_deref()
            .map(|previous| TraceMessage::prefix_reuse(turn, previous, prompt, counter));
        self.previous = Some(prompt.to_string());
        message
    }

    /// The same, for a model call inside a turn — the tool round trip.
    ///
    /// Emitted from the second call of a turn onwards: the first is the turn's
    /// own prompt, measured before the call beside its budget, and measuring it
    /// twice would put it in the chain twice.
    pub fn measure_step(
        &mut self,
        turn: TurnId,
        step: u32,
        prompt: &str,
        counter: &dyn TokenCounter,
    ) -> TraceMessage {
        let shared = self
            .previous
            .as_deref()
            .map(|previous| Shared::measure(previous, prompt, counter));
        self.previous = Some(prompt.to_string());
        TraceMessage::StepCall {
            turn,
            step,
            text: prompt.to_string(),
            prompt_tokens: counter.count(prompt),
            shared,
        }
    }

    /// The same, for a planning call. It carries the prompt as well as the
    /// measurement: there is no `TurnStarted` to hang it off, and a number
    /// nobody can see the input of is not much of a reading.
    pub fn measure_plan(
        &mut self,
        task: TaskId,
        prompt: &str,
        counter: &dyn TokenCounter,
    ) -> TraceMessage {
        let shared = self
            .previous
            .as_deref()
            .map(|previous| Shared::measure(previous, prompt, counter));
        self.previous = Some(prompt.to_string());
        TraceMessage::PlanCall {
            task,
            text: prompt.to_string(),
            prompt_tokens: counter.count(prompt),
            shared,
        }
    }
}

/// One message on its way to clients, to the record, or both.
#[derive(Clone)]
pub enum Event {
    Protocol(ServerMessage),
    Trace(TraceMessage),
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Writes the JSON-lines record, on its own task so a slow disk never stalls a
/// turn.
pub struct Recorder {
    lines: mpsc::UnboundedSender<RecordLine>,
    started_at: u64,
}

impl Recorder {
    pub async fn create(
        path: &Path,
        backend: &str,
        model: &str,
        budget: Budget,
        counter: Counter,
        started_at: u64,
    ) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        let mut file = tokio::fs::File::create(path)
            .await
            .with_context(|| format!("creating {}", path.display()))?;

        let header = RecordLine::Header {
            format: record::FORMAT,
            protocol: protocol::VERSION,
            backend: backend.to_string(),
            model: model.to_string(),
            context_limit: budget.limit,
            counter: Some(counter),
            eviction: Some(budget.eviction),
            started_at,
        };
        file.write_all(format!("{}\n", serde_json::to_string(&header)?).as_bytes())
            .await?;

        let (lines, mut rx) = mpsc::unbounded_channel::<RecordLine>();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                let Ok(json) = serde_json::to_string(&line) else {
                    continue;
                };
                if file
                    .write_all(format!("{json}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
                // Flushed per line: a session worth replaying is usually one
                // that ended badly, and a buffered tail is the part you needed.
                let _ = file.flush().await;
            }
        });

        Ok(Self { lines, started_at })
    }

    pub fn write(&self, event: &Event) {
        let at_ms = now_ms().saturating_sub(self.started_at);
        let line = match event {
            Event::Protocol(message) => RecordLine::Protocol {
                at_ms,
                message: message.clone(),
            },
            Event::Trace(message) => RecordLine::Trace {
                at_ms,
                message: message.clone(),
            },
        };
        let _ = self.lines.send(line);
    }
}
