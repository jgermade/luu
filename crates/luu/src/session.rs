//! What `chat` and `serve` must not each invent.
//!
//! Both drive the same loop, and both need the same prompt assembled the same
//! way. Two call sites building it by hand is exactly what the byte-identical
//! prefix commitment in `AGENTS.md` warns about, so there is one.

use std::path::Path;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::backend::Message;
use agent_core::context::{ApproximateCounter, Budget, Counter, ModelCounter, TokenCounter};
use agent_core::protocol::{self, ServerMessage, TurnId};
use agent_core::record::{self, RecordLine};
use agent_core::sandbox::Sandbox;
use agent_core::tools::Tools;
use agent_core::trace::TraceMessage;
use agent_core::worker::{Executor, Worker};
use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

/// The fixed head of every prompt. Kept as one `&'static str` because the
/// prompt cache reuses it byte for byte — a formatting change here is a cache
/// miss on every call, and nothing fails to tell you.
pub const SYSTEM: &str = "You are luu, a concise local coding agent.";

/// What the agent is asked before a task starts, fused into the current user
/// message and never into the system block.
///
/// That placement is the whole reason this is a `&str` here rather than a line
/// appended to [`SYSTEM`]: the prefix is what the cache reuses, and a planning
/// instruction living up there would be paid for on every call of every turn,
/// including the ones that never plan.
pub const PLANNING: &str = "\
Before anything runs, propose a plan for the work below. Reply with one fenced \
block and nothing after it:

```plan
{\"objective\": \"what this piece of work is\", \"tasks\": [\"what you will do\"], \
\"files\": [\"paths you will read\"], \"writes\": [\"paths you will change\"], \
\"commands\": [\"programs you will run\"]}
```

A path you will change goes in `writes`, and you may not change one that is \
only in `files`. Name only what this piece of work needs; leave a list empty if \
it needs none. \
The person will read this and approve it or refuse it before you run anything.

The work:
";

/// The sandbox and the tool set, resolved once and shared by `chat` and
/// `serve`. Here for the same reason the prompt assembly is: two call sites
/// resolving a policy differently is two sandboxes.
#[derive(Clone)]
pub struct Agency {
    pub tools: Arc<Tools>,
    pub sandbox: Arc<Sandbox>,
    pub max_steps: u32,
    /// Where a tool call actually runs, when that is not this process.
    ///
    /// `None` is `runtime = "host"`, which is every run this repository has
    /// made so far and stays the default. `Some` is a `luu worker` on the other
    /// end of a pipe — a plain child under `direct`, and the container's only
    /// process under a container runtime. See
    /// `RECORD/2026-09-02.the-worker-and-the-seam.completed.md`.
    ///
    /// The *definitions* never move: they are the second half of the cached
    /// prefix, and a prefix assembled inside the image is one that shifts every
    /// time the image is rebuilt.
    pub worker: Option<Arc<Worker>>,
}

impl Agency {
    /// The tool definitions as they go into the cached prefix. Empty when there
    /// are no tools, so a run without them sends the same bytes it always did.
    pub fn definitions(&self) -> String {
        self.tools.definitions()
    }

    /// Where tool calls go. The seam, resolved once: the loop asks this and
    /// never asks where it runs.
    pub fn executor(&self) -> &dyn Executor {
        match &self.worker {
            Some(worker) => worker.as_ref(),
            None => self.tools.as_ref(),
        }
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
        // Beside `enforce`, because it is the same question: what holds a
        // child. On macOS it is the only line here that is true.
        text.push_str(&format!(
            "  limits     {}\n",
            self.sandbox
                .limits()
                .describe()
                .unwrap_or_else(|| "(none: the clock alone)".to_string()),
        ));
        if let Some(worker) = self.worker.as_ref().and_then(|worker| worker.describe()) {
            text.push_str(&worker);
        }
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

/// The previous turn's rendered prompt, kept so the next one can be measured
/// against it.
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

/// The first line of every stream, wherever the stream is kept.
///
/// One function because there are now two places that start one — the recorder
/// writing a `.jsonl` and the store keeping a session's own — and a header
/// assembled twice is two claims about what a run was comparable with. See
/// `RECORD/2026-09-04.the-border-and-the-gate.completed.md`.
pub fn header(
    backend: &str,
    model: &str,
    budget: Budget,
    counter: Counter,
    started_at: u64,
) -> RecordLine {
    RecordLine::Header {
        format: record::FORMAT,
        protocol: protocol::VERSION,
        backend: backend.to_string(),
        model: model.to_string(),
        context_limit: budget.limit,
        counter: Some(counter),
        eviction: Some(budget.eviction),
        started_at,
    }
}

/// One event as the line that records it. Beside [`header`] and for the same
/// reason: the file and the store keep the same stream, so they had better turn
/// an event into a line the same way.
pub fn line(event: &Event, at_ms: u64) -> RecordLine {
    match event {
        Event::Protocol(message) => RecordLine::Protocol {
            at_ms,
            message: message.clone(),
        },
        Event::Trace(message) => RecordLine::Trace {
            at_ms,
            message: message.clone(),
        },
    }
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

        let header = header(backend, model, budget, counter, started_at);
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
        let _ = self.lines.send(line(event, at_ms));
    }
}
