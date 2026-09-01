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
use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

/// The fixed head of every prompt. Kept as one `&'static str` because the
/// prompt cache reuses it byte for byte — a formatting change here is a cache
/// miss on every call, and nothing fails to tell you.
pub const SYSTEM: &str = "You are Loude, a concise local coding agent.";

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
{\"objective\": \"what this piece of work is\", \"steps\": [\"what you will do\"], \
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

/// Names, by [`TurnId`], what [`agent_core::context::Selection::evicted`] only
/// counts.
///
/// `Context` tracks eviction as a floor over its own turn indices and knows
/// nothing of `TurnId` — it is `agent-core`'s and deliberately not the
/// context manager's, the same separation that keeps trace messages taking a
/// `turn` from their caller rather than tracking one themselves. So this
/// keeps the one thing `Context` does not: which id produced the turn at each
/// index, pushed in the same call as [`agent_core::context::Context::push_turn_with_steps`]
/// so the two can never drift apart.
#[derive(Default)]
pub struct EvictionTombstones {
    turn_ids: Vec<TurnId>,
    /// How much of `Selection::evicted` has already been reported. Eviction
    /// only grows, so a rising count is turned into the slice of ids newly
    /// past it and nothing has to be diffed against the floor itself.
    reported: usize,
}

impl EvictionTombstones {
    /// Records which id a just-pushed turn was, in the same order
    /// `Context::turns()` grows in.
    pub fn pushed(&mut self, turn: TurnId) {
        self.turn_ids.push(turn);
    }

    /// The ids newly behind the floor after a `select`, if any left the
    /// window this call.
    pub fn mark(&mut self, evicted: usize) -> Option<Vec<TurnId>> {
        if evicted <= self.reported {
            return None;
        }
        let forgotten = self.turn_ids[self.reported..evicted].to_vec();
        self.reported = evicted;
        Some(forgotten)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nothing_evicted_yet_marks_nothing() {
        let mut tombstones = EvictionTombstones::default();
        tombstones.pushed(1);
        tombstones.pushed(2);

        assert_eq!(tombstones.mark(0), None, "the floor has not moved");
    }

    #[test]
    fn a_rising_floor_names_the_ids_it_passed() {
        let mut tombstones = EvictionTombstones::default();
        for turn in 1..=5 {
            tombstones.pushed(turn);
        }

        assert_eq!(
            tombstones.mark(2),
            Some(vec![1, 2]),
            "the first two turns pushed, by the id they were given",
        );
        // The same floor reported again — the common case, most turns evict
        // nothing new — must not repeat what was already named.
        assert_eq!(tombstones.mark(2), None);

        assert_eq!(
            tombstones.mark(4),
            Some(vec![3, 4]),
            "only the ids newly behind the floor, not the ones already reported",
        );
    }

    #[test]
    fn ids_need_not_be_consecutive() {
        // A turn that produced nothing is never pushed, so the id sequence a
        // real session hands in can skip numbers.
        let mut tombstones = EvictionTombstones::default();
        tombstones.pushed(1);
        tombstones.pushed(3);
        tombstones.pushed(4);

        assert_eq!(tombstones.mark(1), Some(vec![1]));
        assert_eq!(tombstones.mark(3), Some(vec![3, 4]));
    }
}
