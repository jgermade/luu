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

/// One exchange. The unit of everything the context manager does: eviction
/// drops a turn, compaction replaces one, relevance scores one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    pub prompt: String,
    pub answer: String,
    /// Rendered fused into the user message, stored apart so that pruning can
    /// reach it later without parsing back what we already wrote.
    pub code_context: Vec<Fragment>,
    /// Counted once, when the turn closed. A closed turn does not change, and
    /// re-counting every turn on every turn is quadratic over a session.
    pub tokens: u32,
    /// Which counter produced `tokens`. Without it, swapping tokenizers
    /// mid-session sums two different units into one bar.
    pub counted_by: Counter,
}

/// The window, and what is held back from it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Budget {
    /// The model's context window. `None` means unknown — then there is no
    /// budget to spend, so nothing is selected and nothing is evicted.
    pub limit: Option<u32>,
    /// Room for the answer, set aside before any history is considered.
    pub reserve: u32,
}

impl Budget {
    /// The CLI spells "unknown" as 0, because a flag has to have a default.
    pub fn new(limit: u32, reserve: u32) -> Self {
        Self {
            limit: (limit > 0).then_some(limit),
            reserve,
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

/// The conversation, and the rule for turning it into a prompt.
#[derive(Debug, Clone)]
pub struct Context {
    system: String,
    turns: Vec<Turn>,
}

impl Context {
    pub fn new(system: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            turns: Vec::new(),
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
        let prompt = prompt.into();
        let answer = answer.into();
        let tokens = counter.count(&user_text(&code_context, &prompt)) + counter.count(&answer);
        self.turns.push(Turn {
            prompt,
            answer,
            code_context,
            tokens,
            counted_by: counter.id(),
        });
    }

    /// Chooses what fits, then renders it.
    ///
    /// History is kept from the newest backwards and always in whole turns, so
    /// the retained window can only ever start on a user message. Half a turn
    /// would leave an answer to a question nobody asked, and a window starting
    /// on an assistant message makes several chat templates continue instead of
    /// answering.
    pub fn select(
        &self,
        prompt: &str,
        code_context: &[Fragment],
        budget: Budget,
        counter: &dyn TokenCounter,
    ) -> Selection {
        let system_tokens = counter.count(&self.system);
        // Counted as the two buckets they will be plotted as. Tokenization is
        // not additive across a boundary, so this can differ by a token or two
        // from counting the concatenation; that is the same order as the
        // template overhead we already accept and report.
        let code_tokens = counter.count(&fragments_text(code_context));
        let prompt_tokens = counter.count(prompt);

        let keep = match budget.limit {
            None => self.turns.len(),
            Some(limit) => {
                // The current turn and the reserve are not negotiable: nothing
                // here may trim the prompt the user just typed. If they alone
                // exceed the window, history is empty and the turn still goes —
                // being over the limit is the backend's error to report, not
                // ours to hide by cutting the question in half.
                let fixed = system_tokens + code_tokens + prompt_tokens + budget.reserve;
                let mut available = limit.saturating_sub(fixed);
                let mut keep = 0;
                for turn in self.turns.iter().rev() {
                    let tokens = self.tokens_of(turn, counter);
                    if tokens > available {
                        break;
                    }
                    available -= tokens;
                    keep += 1;
                }
                keep
            }
        };

        let retained = &self.turns[self.turns.len() - keep..];
        let history_tokens: u32 = retained
            .iter()
            .map(|turn| self.tokens_of(turn, counter))
            .sum();

        let mut messages = Vec::with_capacity(retained.len() * 2 + 2);
        messages.push(Message::system(self.system.clone()));
        for turn in retained {
            messages.push(Message::user(user_text(&turn.code_context, &turn.prompt)));
            messages.push(Message::assistant(turn.answer.clone()));
        }
        messages.push(Message::user(user_text(code_context, prompt)));

        let mut buckets = vec![
            Bucket::new("system", system_tokens),
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
            evicted: self.turns.len() - keep,
        }
    }

    /// The stored count, unless it was produced by a different counter — in
    /// which case it is not a count of the same thing and gets redone.
    fn tokens_of(&self, turn: &Turn, counter: &dyn TokenCounter) -> u32 {
        match turn.counted_by == counter.id() {
            true => turn.tokens,
            false => {
                counter.count(&user_text(&turn.code_context, &turn.prompt))
                    + counter.count(&turn.answer)
            }
        }
    }
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
        let context = context_with(5, &counter);

        let selection = context.select("now this", &[], Budget::new(0, 512), &counter);

        assert_eq!(selection.limit, None);
        assert_eq!(selection.evicted, 0);
        assert_eq!(selection.messages.len(), 5 * 2 + 2);
        // No window, no headroom to plot.
        assert!(!selection.buckets.iter().any(|b| b.name == "reserve"));
    }

    #[test]
    fn eviction_drops_whole_turns_and_never_orphans_an_answer() {
        let counter = WordCounter::default();
        let context = context_with(6, &counter);

        // Room for the fixed part and two turns, no more.
        let selection = context.select("now this", &[], Budget::new(30, 4), &counter);

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
        let context = context_with(6, &counter);

        let selection = context.select("now this", &[], Budget::new(30, 4), &counter);

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
        let empty = Context::new("system prompt here");
        let full = context_with(4, &counter);

        let first = empty.select("hola", &[], Budget::new(8192, 512), &counter);
        let later = full.select("hola", &[], Budget::new(8192, 512), &counter);

        assert_eq!(first.messages[0].content, later.messages[0].content);
        assert_eq!(first.messages[0].role, Role::System);
    }

    #[test]
    fn code_is_fused_into_the_current_user_message_never_the_system_block() {
        let counter = WordCounter::default();
        let context = context_with(2, &counter);
        let code = vec![Fragment {
            path: "src/lib.rs".into(),
            text: "fn main() {}".into(),
        }];

        let selection = context.select("explain this", &code, Budget::new(8192, 512), &counter);

        assert!(!selection.messages[0].content.contains("src/lib.rs"));
        let last = selection.messages.last().unwrap();
        assert_eq!(last.role, Role::User);
        assert!(last.content.contains("src/lib.rs"));
        assert!(last.content.ends_with("explain this"));
    }

    #[test]
    fn a_closed_turn_is_counted_once_not_once_per_later_turn() {
        let counter = WordCounter::default();
        let context = context_with(10, &counter);
        let after_building = counter.calls.load(Ordering::Relaxed);

        context.select("now this", &[], Budget::new(8192, 512), &counter);

        // System, the fragments, the prompt. The ten stored turns are not
        // re-counted, which is the whole point of storing their counts.
        assert_eq!(counter.calls.load(Ordering::Relaxed) - after_building, 3);
    }

    #[test]
    fn a_turn_counted_by_another_counter_is_recounted_rather_than_summed() {
        let words = WordCounter::default();
        let mut context = Context::new("system prompt here");
        context.push_turn("one two three", "four five", vec![], &ApproximateCounter);

        let selection = context.select("now this", &[], Budget::new(8192, 0), &words);

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
        let context = context_with(3, &counter);

        let selection = context.select(
            "a very long question indeed",
            &[],
            Budget::new(2, 8),
            &counter,
        );

        assert_eq!(selection.evicted, 3);
        assert_eq!(selection.messages.len(), 2, "system and the prompt");
        assert_eq!(selection.messages[1].content, "a very long question indeed");
    }

    #[test]
    fn buckets_are_in_prompt_order_with_the_reserve_last() {
        let counter = WordCounter::default();
        let context = context_with(1, &counter);

        let selection = context.select("hola", &[], Budget::new(8192, 512), &counter);

        let names: Vec<&str> = selection.buckets.iter().map(|b| b.name.as_str()).collect();
        assert_eq!(names, ["system", "history", "code", "prompt", "reserve"]);
    }

    #[test]
    fn the_approximate_counter_says_so() {
        let context = Context::new("system");
        let selection = context.select("hola", &[], Budget::new(8192, 512), &ApproximateCounter);
        assert!(selection.counter.is_approximate());
    }
}
