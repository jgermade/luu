//! What earns its place in the prompt.
//!
//! Two commitments shape everything here, and both look like implementation
//! details from close up:
//!
//! - **Decide, then render.** The selection happens against a token budget and
//!   the rendering is a pure function of it, so every token sent can be
//!   attributed to a bucket. Rendering first and trimming the string afterwards
//!   loses that, and cuts wherever the limit happens to land.
//! - **The stable prefix stays byte-identical.** The system block is the part
//!   the prompt cache reuses. Nothing that changes per turn goes above it —
//!   selected code is fused into the *current* user message, as late as
//!   possible.
//!
//! See `RECORD/2026-08-27.context-manager.md` for how both were arrived at.

use serde::{Deserialize, Serialize};

use crate::backend::Message;
use crate::task::TaskId;
use crate::tools::ToolStep;
use crate::trace::Bucket;

/// Which counter produced a token count.
///
/// Carried with every stored count and every budget, because two runs measured
/// by different counters are not comparable — and nothing else would say so.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Counter {
    /// The model's own tokenizer, named by whatever identifies it to a reader.
    Model { id: String },
    /// A stand-in for when the tokenizer cannot be loaded. It exists so a
    /// missing file degrades instead of failing, and it is labelled everywhere
    /// it appears: it is not a measurement and must never be read as one.
    Approximate,
}

impl Counter {
    pub fn is_approximate(&self) -> bool {
        matches!(self, Self::Approximate)
    }
}

/// Counts tokens the way the loaded model would.
pub trait TokenCounter: Send + Sync {
    fn id(&self) -> Counter;
    fn count(&self, text: &str) -> u32;
}

/// Characters over four. Wrong by enough to matter, and useful only so that a
/// missing `tokenizer.json` is an annotation rather than a dead end.
pub struct ApproximateCounter;

impl TokenCounter for ApproximateCounter {
    fn id(&self) -> Counter {
        Counter::Approximate
    }

    fn count(&self, text: &str) -> u32 {
        text.chars().count().div_ceil(4) as u32
    }
}

#[derive(Debug, thiserror::Error)]
#[error("loading the tokenizer at {path}: {message}")]
pub struct TokenizerError {
    pub path: String,
    pub message: String,
}

/// The model's own tokenizer, from its `tokenizer.json`.
pub struct ModelCounter {
    tokenizer: tokenizers::Tokenizer,
    id: String,
}

impl ModelCounter {
    /// `id` is what a reader will see in a recording months later, so it should
    /// name the model rather than the file.
    pub fn from_file(
        path: &std::path::Path,
        id: impl Into<String>,
    ) -> Result<Self, TokenizerError> {
        let tokenizer = tokenizers::Tokenizer::from_file(path).map_err(|error| TokenizerError {
            path: path.display().to_string(),
            message: error.to_string(),
        })?;
        Ok(Self {
            tokenizer,
            id: id.into(),
        })
    }
}

impl TokenCounter for ModelCounter {
    fn id(&self) -> Counter {
        Counter::Model {
            id: self.id.clone(),
        }
    }

    fn count(&self, text: &str) -> u32 {
        // Special tokens are the chat template's business, and the template is
        // applied by the backend where we cannot see it. That gap is accepted
        // and reported rather than guessed at — see the record.
        match self.tokenizer.encode(text, false) {
            Ok(encoding) => encoding.len() as u32,
            // A tokenizer that loaded but cannot encode is not a case worth a
            // Result on every call site; the count degrades and the trace
            // still says which counter was in use.
            Err(_) => ApproximateCounter.count(text),
        }
    }
}

/// A piece of code selected for one turn.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Fragment {
    pub path: String,
    pub text: String,
}

/// What a turn is in the history.
///
/// A task's plan and its closing summary are turns like any other — that is
/// what keeps the strict user/assistant alternation the prompt shape depends
/// on, and what keeps them inside the budget rather than in a second, quieter
/// window nobody measures. The tag is here so the two can still be *counted*
/// apart: "the task scaffolding costs N tokens" is a number the strategy has to
/// justify, and without it that number hides inside `history`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum TurnKind {
    /// A prompt and what came back. Everything, before tasks existed — hence
    /// the default, which is what a turn recorded then deserializes as.
    #[default]
    Exchange,
    /// The approved plan, written once when the task started.
    Plan { task: TaskId },
    /// What a closed task left behind.
    Summary { task: TaskId },
}

impl TurnKind {
    pub fn task(self) -> Option<TaskId> {
        match self {
            Self::Exchange => None,
            Self::Plan { task } | Self::Summary { task } => Some(task),
        }
    }

    /// Which bucket its tokens are attributed to.
    fn bucket(self) -> &'static str {
        match self {
            Self::Exchange => "history",
            Self::Plan { .. } | Self::Summary { .. } => "tasks",
        }
    }
}

/// One exchange. The unit of everything the context manager does: eviction
/// drops a turn, compaction replaces one, relevance scores one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub prompt: String,
    pub answer: String,
    /// What the agent did before answering. Part of the turn, not of the
    /// history around it: a turn with tool calls is evicted whole, so the model
    /// never sees a result whose call has gone — and until the next turn is
    /// evicted it can see that it already read the file.
    #[serde(default)]
    pub steps: Vec<ToolStep>,
    /// Rendered fused into the user message, stored apart so that pruning can
    /// reach it later without parsing back what we already wrote.
    pub code_context: Vec<Fragment>,
    /// Counted once, when the turn closed. A closed turn does not change, and
    /// re-counting every turn on every turn is quadratic over a session.
    pub tokens: u32,
    /// Which counter produced `tokens`. Without it, swapping tokenizers
    /// mid-session sums two different units into one bar.
    pub counted_by: Counter,
    /// An exchange, or something a task wrote. Defaulted so a recording made
    /// before tasks existed still reads.
    #[serde(default)]
    pub kind: TurnKind,
}

/// How the history gives way when the next turn no longer fits.
///
/// Not a preference: the two rewrite the prompt at completely different rates,
/// and a prefix cache reuses the longest common prefix. See
/// `RECORD/2026-08-27.prefix-reuse-and-block-eviction.md`.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(tag = "policy", rename_all = "snake_case")]
pub enum Eviction {
    /// Drop exactly as many turns as it takes to fit. The baseline, and the
    /// thing to beat: once the window is full it drops one turn per turn, so
    /// the history block is rewritten from its front on every call and the
    /// reusable prefix collapses to the system block.
    Turn,
    /// When it no longer fits, drop past a low-water mark — a fraction of the
    /// history budget — instead of dropping the minimum. The cut is deeper and
    /// far less frequent: the history is rewritten once every N turns and stays
    /// byte-identical in between, which is what a prefix cache pays for.
    ///
    /// `low_water` is ours and invented, not inherited from anyone's tuned
    /// workload. It is a flag so that it can be measured.
    Block { low_water: f32 },
}

/// The window, what is held back from it, and how it gives way.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Budget {
    /// The model's context window. `None` means unknown — then there is no
    /// budget to spend, so nothing is selected and nothing is evicted.
    pub limit: Option<u32>,
    /// Room for the answer, set aside before any history is considered.
    pub reserve: u32,
    pub eviction: Eviction,
}

impl Budget {
    /// The CLI spells "unknown" as 0, because a flag has to have a default.
    pub fn new(limit: u32, reserve: u32, eviction: Eviction) -> Self {
        Self {
            limit: (limit > 0).then_some(limit),
            reserve,
            eviction,
        }
    }
}

/// What was decided, and the rendering of it. The two are produced together on
/// purpose: `buckets` describes `messages`, not an estimate of it.
#[derive(Debug, Clone)]
pub struct Selection {
    pub messages: Vec<Message>,
    /// In prompt order, with the reserve last — a stacked bar reads as the
    /// prompt reads.
    pub buckets: Vec<Bucket>,
    pub limit: Option<u32>,
    pub counter: Counter,
    /// Whole turns dropped to make room, oldest first.
    pub evicted: usize,
}

/// What a fold cost and what it bought.
///
/// Numbers, because the house rule is that a context strategy is measured. They
/// travel on the trace channel: the summary is what a client draws, and the
/// arithmetic behind it is debug data a stdio consumer should never carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fold {
    /// Turns replaced, the plan turn included.
    pub turns: usize,
    pub tokens_before: u32,
    pub tokens_after: u32,
}

/// The conversation, and the rule for turning it into a prompt.
#[derive(Debug, Clone)]
pub struct Context {
    system: String,
    /// The tool definitions, rendered once. Second half of the cached prefix
    /// and counted as its own bucket, because "the system block grew" is not an
    /// answer to why the window is full.
    tools: String,
    turns: Vec<Turn>,
    /// The oldest turn still in the window. It only ever moves forward.
    ///
    /// Without it, eviction is recomputed from scratch on every call and a turn
    /// that left the window comes back the moment a shorter prompt leaves room
    /// for it. Under [`Eviction::Turn`] that is a rare waste; under
    /// [`Eviction::Block`] it is fatal — the fill would walk straight back past
    /// the cut and move the front of the history every turn, which is the exact
    /// thing block eviction exists to stop.
    ///
    /// So the window a conversation has already lost is not a view that gets
    /// recomputed. It is something that happened.
    floor: usize,
}

impl Context {
    pub fn new(system: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            tools: String::new(),
            turns: Vec::new(),
            floor: 0,
        }
    }

    /// Adds the tool definitions to the prefix. Byte-stable by construction —
    /// see [`crate::tools::Tools::definitions`].
    pub fn with_tools(mut self, tools: impl Into<String>) -> Self {
        self.tools = tools.into();
        self
    }

    pub fn tools(&self) -> &str {
        &self.tools
    }

    /// The whole cached prefix, as one message. Assembled here and nowhere else
    /// so that two call sites cannot join it differently.
    fn system_message(&self) -> String {
        match self.tools.is_empty() {
            true => self.system.clone(),
            false => format!("{}\n\n{}", self.system, self.tools),
        }
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    pub fn system(&self) -> &str {
        &self.system
    }

    /// Closes a turn and counts it, once.
    pub fn push_turn(
        &mut self,
        prompt: impl Into<String>,
        answer: impl Into<String>,
        code_context: Vec<Fragment>,
        counter: &dyn TokenCounter,
    ) {
        self.push_turn_with_steps(prompt, answer, code_context, Vec::new(), counter);
    }

    /// The same, for a turn in which the agent used tools.
    pub fn push_turn_with_steps(
        &mut self,
        prompt: impl Into<String>,
        answer: impl Into<String>,
        code_context: Vec<Fragment>,
        steps: Vec<ToolStep>,
        counter: &dyn TokenCounter,
    ) {
        let prompt = prompt.into();
        let answer = answer.into();
        let tokens = counter.count(&user_text(&code_context, &prompt))
            + steps_tokens(&steps, counter)
            + counter.count(&answer);
        self.turns.push(Turn {
            prompt,
            answer,
            steps,
            code_context,
            tokens,
            counted_by: counter.id(),
            kind: TurnKind::Exchange,
        });
    }

    /// Writes an approved plan into the history, and says where it landed.
    ///
    /// The index is the task's span: everything from here to the end of the
    /// history is what the fold replaces at the close. Returned rather than
    /// looked up later, because a span recomputed from a search is a span that
    /// can be wrong.
    pub fn push_plan(
        &mut self,
        task: TaskId,
        plan: &crate::task::Plan,
        counter: &dyn TokenCounter,
    ) -> usize {
        self.push_written(
            TurnKind::Plan { task },
            plan.objective.clone(),
            plan.render(),
            counter,
        )
    }

    /// The one rewrite in a session: the turns from `from` become one summary.
    ///
    /// Deep and rare on purpose. Eviction drops the minimum and rewrites the
    /// front of the history every turn once the window is full; this cuts once,
    /// at a point the work chose, and the history is byte-identical in between.
    /// That is the shape a prompt cache pays for — see
    /// `RECORD/2026-08-27.prefix-reuse-and-block-eviction.md`.
    ///
    /// The floor moves back to keep the summary inside the window when the fold
    /// spans turns that had already left it. The **index** moves; the content
    /// does not come back — the folded turns are gone from the history, and
    /// what stands in their place is not one of them. Leaving the summary below
    /// the floor would spend the tokens to write it and send none of them.
    pub fn close_task(
        &mut self,
        task: TaskId,
        from: usize,
        objective: &str,
        summary: &str,
        counter: &dyn TokenCounter,
    ) -> Fold {
        let from = from.min(self.turns.len());
        let folded: Vec<Turn> = self.turns.drain(from..).collect();
        let tokens_before = folded
            .iter()
            .map(|turn| self.tokens_of(turn, counter))
            .sum();

        let at = self.push_written(
            TurnKind::Summary { task },
            objective.to_string(),
            summary.to_string(),
            counter,
        );
        self.floor = self.floor.min(at);

        Fold {
            turns: folded.len(),
            tokens_before,
            tokens_after: self.turns[at].tokens,
        }
    }

    /// A turn the agent wrote rather than exchanged: a plan or a summary. One
    /// user half and one assistant half, so nothing downstream has to know the
    /// difference.
    fn push_written(
        &mut self,
        kind: TurnKind,
        prompt: String,
        answer: String,
        counter: &dyn TokenCounter,
    ) -> usize {
        let tokens = counter.count(&prompt) + counter.count(&answer);
        self.turns.push(Turn {
            prompt,
            answer,
            steps: Vec::new(),
            code_context: Vec::new(),
            tokens,
            counted_by: counter.id(),
            kind,
        });
        self.turns.len() - 1
    }

    /// Chooses what fits, then renders it.
    ///
    /// History is kept from the newest backwards and always in whole turns, so
    /// the retained window can only ever start on a user message. Half a turn
    /// would leave an answer to a question nobody asked, and a window starting
    /// on an assistant message makes several chat templates continue instead of
    /// answering.
    ///
    /// Takes `&mut self` because evicting is not a reading of the history: what
    /// leaves the window stays out, so the decision has to outlive the call
    /// that made it. See [`Context::floor`].
    pub fn select(
        &mut self,
        prompt: &str,
        code_context: &[Fragment],
        budget: Budget,
        counter: &dyn TokenCounter,
    ) -> Selection {
        let system_tokens = counter.count(&self.system);
        let tools_tokens = counter.count(&self.tools);
        // Counted as the two buckets they will be plotted as. Tokenization is
        // not additive across a boundary, so this can differ by a token or two
        // from counting the concatenation; that is the same order as the
        // template overhead we already accept and report.
        let code_tokens = counter.count(&fragments_text(code_context));
        let prompt_tokens = counter.count(prompt);

        // Only what the window has not already lost is a candidate.
        let live = self.turns.len() - self.floor;
        if let Some(limit) = budget.limit {
            // The current turn and the reserve are not negotiable: nothing here
            // may trim the prompt the user just typed. If they alone exceed the
            // window, history is empty and the turn still goes — being over the
            // limit is the backend's error to report, not ours to hide by
            // cutting the question in half.
            let fixed = system_tokens + tools_tokens + code_tokens + prompt_tokens + budget.reserve;
            let available = limit.saturating_sub(fixed);

            if self.fill(available, counter) < live {
                // It no longer fits. How deep the cut goes is the whole
                // difference between the two policies.
                let target = match budget.eviction {
                    Eviction::Turn => available,
                    Eviction::Block { low_water } => {
                        (available as f32 * low_water.clamp(0.0, 1.0)) as u32
                    }
                };
                self.floor = self.turns.len() - self.fill(target, counter);
            }
        }

        let retained = &self.turns[self.floor..];
        // Split by what wrote it: an exchange the session had, or the plan and
        // summary a task left behind. One bar, two answers to "why is the
        // window full".
        let (history_tokens, task_tokens) =
            retained.iter().fold((0, 0), |(history, tasks), turn| {
                let tokens = self.tokens_of(turn, counter);
                match turn.kind.bucket() {
                    "tasks" => (history, tasks + tokens),
                    _ => (history + tokens, tasks),
                }
            });

        let mut messages = Vec::with_capacity(retained.len() * 2 + 2);
        messages.push(Message::system(self.system_message()));
        for turn in retained {
            messages.push(Message::user(user_text(&turn.code_context, &turn.prompt)));
            // Each step is a real exchange, so the alternation holds and no
            // chat template has to decide what two user messages in a row mean.
            for step in &turn.steps {
                messages.push(Message::assistant(step.text.clone()));
                messages.push(Message::user(step.result_text()));
            }
            messages.push(Message::assistant(turn.answer.clone()));
        }
        messages.push(Message::user(user_text(code_context, prompt)));

        let mut buckets = vec![
            Bucket::new("system", system_tokens),
            Bucket::new("tools", tools_tokens),
            // Ahead of `history`: a summary is the oldest thing in the window
            // it survives in, and the bar reads as the prompt reads.
            Bucket::new("tasks", task_tokens),
            Bucket::new("history", history_tokens),
            Bucket::new("code", code_tokens),
            Bucket::new("prompt", prompt_tokens),
        ];
        // Headroom only means something against a known window.
        if budget.limit.is_some() {
            buckets.push(Bucket::new("reserve", budget.reserve));
        }

        Selection {
            messages,
            buckets,
            limit: budget.limit,
            counter: counter.id(),
            evicted: self.floor,
        }
    }

    /// How many turns, newest first and never reaching past the floor, fit in
    /// `available`.
    fn fill(&self, available: u32, counter: &dyn TokenCounter) -> usize {
        let mut left = available;
        let mut keep = 0;
        for turn in self.turns[self.floor..].iter().rev() {
            let tokens = self.tokens_of(turn, counter);
            if tokens > left {
                break;
            }
            left -= tokens;
            keep += 1;
        }
        keep
    }

    /// The stored count, unless it was produced by a different counter — in
    /// which case it is not a count of the same thing and gets redone.
    fn tokens_of(&self, turn: &Turn, counter: &dyn TokenCounter) -> u32 {
        match turn.counted_by == counter.id() {
            true => turn.tokens,
            false => {
                counter.count(&user_text(&turn.code_context, &turn.prompt))
                    + steps_tokens(&turn.steps, counter)
                    + counter.count(&turn.answer)
            }
        }
    }
}

/// The tool exchanges of a turn, counted as the messages they render as.
fn steps_tokens(steps: &[ToolStep], counter: &dyn TokenCounter) -> u32 {
    steps
        .iter()
        .map(|step| counter.count(&step.text) + counter.count(&step.result_text()))
        .sum()
}

/// The fragments alone, as they are rendered inside a user message.
fn fragments_text(code_context: &[Fragment]) -> String {
    code_context
        .iter()
        .map(|fragment| format!("// {}\n{}\n\n", fragment.path, fragment.text))
        .collect()
}

/// The user half of a turn: its code, then what was asked.
///
/// Fused into one message rather than sent as two, because a standalone
/// context message leaves two `user` messages back to back and chat templates
/// disagree about what that means.
fn user_text(code_context: &[Fragment], prompt: &str) -> String {
    let mut text = fragments_text(code_context);
    text.push_str(prompt);
    text
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use super::*;
    use crate::backend::Role;

    /// One token per word, and it says how often it was asked. The second part
    /// is what makes "counted once" testable at all.
    #[derive(Default)]
    struct WordCounter {
        calls: AtomicUsize,
    }

    impl TokenCounter for WordCounter {
        fn id(&self) -> Counter {
            Counter::Model { id: "words".into() }
        }

        fn count(&self, text: &str) -> u32 {
            self.calls.fetch_add(1, Ordering::Relaxed);
            text.split_whitespace().count() as u32
        }
    }

    fn context_with(turns: usize, counter: &dyn TokenCounter) -> Context {
        let mut context = Context::new("system prompt here");
        for n in 0..turns {
            context.push_turn(
                format!("question number {n} padded out"),
                format!("answer number {n} padded out"),
                vec![],
                counter,
            );
        }
        context
    }

    #[test]
    fn an_unknown_window_selects_nothing_and_evicts_nothing() {
        let counter = WordCounter::default();
        let mut context = context_with(5, &counter);

        let selection = context.select(
            "now this",
            &[],
            Budget::new(0, 512, Eviction::Turn),
            &counter,
        );

        assert_eq!(selection.limit, None);
        assert_eq!(selection.evicted, 0);
        assert_eq!(selection.messages.len(), 5 * 2 + 2);
        // No window, no headroom to plot.
        assert!(!selection.buckets.iter().any(|b| b.name == "reserve"));
    }

    #[test]
    fn eviction_drops_whole_turns_and_never_orphans_an_answer() {
        let counter = WordCounter::default();
        let mut context = context_with(6, &counter);

        // Room for the fixed part and two turns, no more.
        let selection = context.select(
            "now this",
            &[],
            Budget::new(30, 4, Eviction::Turn),
            &counter,
        );

        assert!(selection.evicted > 0, "the point of the case");
        let roles: Vec<Role> = selection.messages.iter().map(|m| m.role).collect();
        assert_eq!(roles[0], Role::System);
        // Everything between the system block and the current turn alternates,
        // starting on a user message.
        for (index, role) in roles[1..].iter().enumerate() {
            let expected = match index % 2 {
                0 => Role::User,
                _ => Role::Assistant,
            };
            assert_eq!(*role, expected, "message {} of {roles:?}", index + 1);
        }
        assert_eq!(roles.last(), Some(&Role::User));
    }

    #[test]
    fn what_is_kept_is_the_newest_turns() {
        let counter = WordCounter::default();
        let mut context = context_with(6, &counter);

        let selection = context.select(
            "now this",
            &[],
            Budget::new(30, 4, Eviction::Turn),
            &counter,
        );

        let kept: Vec<&String> = selection
            .messages
            .iter()
            .filter(|m| m.role == Role::User)
            .map(|m| &m.content)
            .collect();
        assert!(!kept.iter().any(|text| text.contains("number 0")));
        assert!(kept.iter().any(|text| text.contains("number 5")));
    }

    #[test]
    fn the_system_block_is_byte_identical_whatever_the_history() {
        let counter = WordCounter::default();
        let mut empty = Context::new("system prompt here");
        let mut full = context_with(4, &counter);

        let first = empty.select(
            "hola",
            &[],
            Budget::new(8192, 512, Eviction::Turn),
            &counter,
        );
        let later = full.select(
            "hola",
            &[],
            Budget::new(8192, 512, Eviction::Turn),
            &counter,
        );

        assert_eq!(first.messages[0].content, later.messages[0].content);
        assert_eq!(first.messages[0].role, Role::System);
    }

    #[test]
    fn code_is_fused_into_the_current_user_message_never_the_system_block() {
        let counter = WordCounter::default();
        let mut context = context_with(2, &counter);
        let code = vec![Fragment {
            path: "src/lib.rs".into(),
            text: "fn main() {}".into(),
        }];

        let selection = context.select(
            "explain this",
            &code,
            Budget::new(8192, 512, Eviction::Turn),
            &counter,
        );

        assert!(!selection.messages[0].content.contains("src/lib.rs"));
        let last = selection.messages.last().unwrap();
        assert_eq!(last.role, Role::User);
        assert!(last.content.contains("src/lib.rs"));
        assert!(last.content.ends_with("explain this"));
    }

    #[test]
    fn a_closed_turn_is_counted_once_not_once_per_later_turn() {
        let counter = WordCounter::default();
        let mut context = context_with(10, &counter);
        let after_building = counter.calls.load(Ordering::Relaxed);

        context.select(
            "now this",
            &[],
            Budget::new(8192, 512, Eviction::Turn),
            &counter,
        );

        // System, the tool definitions, the fragments, the prompt. The ten
        // stored turns are not re-counted, which is the whole point of storing
        // their counts.
        assert_eq!(counter.calls.load(Ordering::Relaxed) - after_building, 4);
    }

    #[test]
    fn a_turn_counted_by_another_counter_is_recounted_rather_than_summed() {
        let words = WordCounter::default();
        let mut context = Context::new("system prompt here");
        context.push_turn("one two three", "four five", vec![], &ApproximateCounter);

        let selection = context.select(
            "now this",
            &[],
            Budget::new(8192, 0, Eviction::Turn),
            &words,
        );

        // The stored count came from chars/4; the bar is in words, so the turn
        // is re-counted rather than added in a foreign unit.
        let history = selection
            .buckets
            .iter()
            .find(|b| b.name == "history")
            .unwrap();
        assert_eq!(history.tokens, 5);
    }

    #[test]
    fn the_current_turn_survives_a_window_too_small_for_it() {
        let counter = WordCounter::default();
        let mut context = context_with(3, &counter);

        let selection = context.select(
            "a very long question indeed",
            &[],
            Budget::new(2, 8, Eviction::Turn),
            &counter,
        );

        assert_eq!(selection.evicted, 3);
        assert_eq!(selection.messages.len(), 2, "system and the prompt");
        assert_eq!(selection.messages[1].content, "a very long question indeed");
    }

    #[test]
    fn buckets_are_in_prompt_order_with_the_reserve_last() {
        let counter = WordCounter::default();
        let mut context = context_with(1, &counter);

        let selection = context.select(
            "hola",
            &[],
            Budget::new(8192, 512, Eviction::Turn),
            &counter,
        );

        let names: Vec<&str> = selection.buckets.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(
            names,
            [
                "system", "tools", "tasks", "history", "code", "prompt", "reserve"
            ]
        );
    }

    #[test]
    fn the_approximate_counter_says_so() {
        let mut context = Context::new("system");
        let selection = context.select(
            "hola",
            &[],
            Budget::new(8192, 512, Eviction::Turn),
            &ApproximateCounter,
        );
        assert!(selection.counter.is_approximate());
    }

    /// Turns of a known size, so a budget can be written in turns rather than
    /// in tokens nobody can check by eye.
    fn context_of_equal_turns(turns: usize, counter: &dyn TokenCounter) -> Context {
        let mut context = Context::new("sys");
        for n in 0..turns {
            // Four words each way: every turn costs exactly eight.
            context.push_turn(
                format!("q {n} aa bb"),
                format!("a {n} cc dd"),
                vec![],
                counter,
            );
        }
        context
    }

    #[test]
    fn a_turn_that_left_the_window_does_not_come_back_when_room_reappears() {
        let counter = WordCounter::default();
        let mut context = context_of_equal_turns(6, &counter);
        let budget = Budget::new(60, 0, Eviction::Turn);

        // A prompt big enough to force eviction, then a tiny one that would
        // leave room for what was just dropped.
        let long: String = std::iter::repeat_n("pad", 30).collect::<Vec<_>>().join(" ");
        let evicted = context.select(&long, &[], budget, &counter).evicted;
        assert!(
            evicted > 0,
            "the case only exists once something was dropped"
        );

        let after = context.select("hi", &[], budget, &counter);
        assert_eq!(
            after.evicted, evicted,
            "resurrecting a turn rewrites the history from its front, which is \
             the one thing the prefix cache cannot survive",
        );
    }

    #[test]
    fn a_fold_over_evicted_turns_still_puts_its_summary_in_the_window() {
        // The floor is monotone so that a turn which left the window cannot
        // come back. A summary is not one of those turns: it is new text,
        // written once, standing in for them. Left below the floor it would
        // cost tokens to produce and send none of them.
        let counter = WordCounter::default();
        let mut context = context_of_equal_turns(6, &counter);
        let budget = Budget::new(40, 0, Eviction::Turn);

        let evicted = context
            .select("pad pad pad pad", &[], budget, &counter)
            .evicted;
        assert!(
            evicted > 0,
            "the case only exists once something was dropped"
        );

        let fold = context.close_task(1, 0, "the objective", "the summary", &counter);
        assert_eq!(fold.turns, 6, "everything from the span is replaced");

        let selection = context.select("and now", &[], budget, &counter);
        assert_eq!(selection.evicted, 0, "the summary is inside the window");
        let sent = selection
            .messages
            .iter()
            .map(|message| message.content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(sent.contains("the summary"));
        assert!(
            !sent.contains("question number 0"),
            "the folded turns are gone from the history, not merely hidden",
        );
        let tasks = selection
            .buckets
            .iter()
            .find(|bucket| bucket.name == "tasks")
            .unwrap();
        assert!(
            tasks.tokens > 0,
            "and what the scaffolding costs is its own bucket",
        );
    }

    #[test]
    fn block_eviction_cuts_deeper_and_then_holds_still() {
        let counter = WordCounter::default();
        // Eight turns of eight tokens: 64 of history against a window of 40.
        let mut per_turn = context_of_equal_turns(8, &counter);
        let mut block = context_of_equal_turns(8, &counter);

        let window = 40;
        let turn = Budget::new(window, 0, Eviction::Turn);
        let blocks = Budget::new(window, 0, Eviction::Block { low_water: 0.5 });

        let first_per_turn = per_turn.select("q", &[], turn, &counter).evicted;
        let first_block = block.select("q", &[], blocks, &counter).evicted;
        assert!(
            first_block > first_per_turn,
            "the block policy pays for its stability up front: {first_block} vs {first_per_turn}",
        );

        // A closed turn is appended to each, and only the per-turn policy has
        // to move its front again to make room.
        per_turn.push_turn("q 8 aa bb", "a 8 cc dd", vec![], &counter);
        block.push_turn("q 8 aa bb", "a 8 cc dd", vec![], &counter);

        let next_per_turn = per_turn.select("q", &[], turn, &counter).evicted;
        let next_block = block.select("q", &[], blocks, &counter).evicted;
        assert!(
            next_per_turn > first_per_turn,
            "the baseline drops another turn, so the history is rewritten again",
        );
        assert_eq!(
            next_block, first_block,
            "the block policy still fits, so the history is byte-identical",
        );
    }

    #[test]
    fn a_history_that_still_fits_is_untouched_by_either_policy() {
        let counter = WordCounter::default();
        let mut context = context_of_equal_turns(3, &counter);

        let selection = context.select(
            "q",
            &[],
            Budget::new(8192, 0, Eviction::Block { low_water: 0.5 }),
            &counter,
        );

        assert_eq!(
            selection.evicted, 0,
            "the low-water mark is a floor, not a cap"
        );
        assert_eq!(selection.messages.len(), 3 * 2 + 2);
    }

    #[test]
    fn the_policy_is_part_of_what_a_run_was_measured_under() {
        let json = serde_json::to_value(Eviction::Block { low_water: 0.5 }).unwrap();
        assert_eq!(json["policy"], "block");
        assert_eq!(json["low_water"], 0.5);
        assert_eq!(
            serde_json::to_value(Eviction::Turn).unwrap()["policy"],
            "turn"
        );
    }
}

#[cfg(test)]
mod tool_turn_tests {
    use super::*;
    use crate::backend::Role;
    use crate::sandbox::{Applied, Verdict};
    use crate::tools::{ToolCall, ToolOutcome, ToolStep};

    fn step(name: &str, text: &str, output: &str) -> ToolStep {
        ToolStep {
            text: text.into(),
            call: ToolCall {
                name: name.into(),
                arguments: serde_json::json!({}),
            },
            outcome: ToolOutcome::ok(Verdict::allow("test", Applied::Process), output),
            duration_ms: 1,
        }
    }

    #[test]
    fn tool_definitions_are_in_the_prefix_and_counted_apart_from_the_system_text() {
        let mut context = Context::new("sys").with_tools("TOOLS");
        let selection = context.select(
            "hola",
            &[],
            Budget::new(8192, 512, Eviction::Turn),
            &ApproximateCounter,
        );

        assert_eq!(selection.messages[0].role, Role::System);
        assert!(selection.messages[0].content.contains("TOOLS"));
        let tools = selection
            .buckets
            .iter()
            .find(|b| b.name == "tools")
            .unwrap();
        assert!(
            tools.tokens > 0,
            "a bucket the definitions are hidden inside cannot explain a full window"
        );
    }

    #[test]
    fn a_turn_with_tool_calls_renders_as_alternating_messages() {
        let mut context = Context::new("sys");
        context.push_turn_with_steps(
            "read the file",
            "It defines main.",
            vec![],
            vec![step(
                "read_file",
                "let me look\n```tool\n{}\n```",
                "fn main() {}",
            )],
            &ApproximateCounter,
        );

        let selection = context.select(
            "and now?",
            &[],
            Budget::new(0, 0, Eviction::Turn),
            &ApproximateCounter,
        );
        let roles: Vec<Role> = selection.messages.iter().map(|m| m.role).collect();
        assert_eq!(
            roles,
            [
                Role::System,
                Role::User,
                Role::Assistant,
                Role::User,
                Role::Assistant,
                Role::User
            ],
            "the call and its result are a real exchange, so nothing has to \
             decide what two user messages in a row mean",
        );
        assert!(selection.messages[3].content.contains("fn main() {}"));
    }

    #[test]
    fn a_turn_leaves_the_window_with_its_tool_calls() {
        // Half a turn would leave a result whose call has gone, which reads as
        // output nobody asked for.
        let counter = ApproximateCounter;
        let mut context = Context::new("sys");
        for n in 0..4 {
            context.push_turn_with_steps(
                format!("question {n} padded out a little"),
                format!("answer {n} padded out a little"),
                vec![],
                vec![step("read_file", "calling", &"x".repeat(200))],
                &counter,
            );
        }

        let selection = context.select("now", &[], Budget::new(256, 32, Eviction::Turn), &counter);
        assert!(
            selection.evicted > 0,
            "the results are what filled the window"
        );
        for pair in selection.messages[1..].windows(2) {
            assert_ne!(
                pair[0].role, pair[1].role,
                "the alternation survives eviction"
            );
        }
    }

    #[test]
    fn the_steps_are_counted_into_the_turn_they_belong_to() {
        let counter = ApproximateCounter;
        let mut bare = Context::new("sys");
        bare.push_turn("q", "a", vec![], &counter);
        let mut with_steps = Context::new("sys");
        with_steps.push_turn_with_steps(
            "q",
            "a",
            vec![],
            vec![step("read_file", "calling", &"x".repeat(400))],
            &counter,
        );

        assert!(
            with_steps.turns()[0].tokens > bare.turns()[0].tokens,
            "a turn whose tool output is not in its count is a turn the budget cannot see"
        );
    }
}
