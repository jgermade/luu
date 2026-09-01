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
//! See `RECORD/2026-08-27.context-manager.completed.md` for how both were arrived at.

use serde::{Deserialize, Serialize};

use crate::backend::Message;
use crate::protocol::TurnId;
use crate::task::{Plan, Task, TaskId, TaskState};
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

/// One exchange. The unit of everything the context manager does: eviction
/// drops a turn, compaction replaces one, relevance scores one.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Turn {
    /// The turn number the session handed out, so that a turn dropped from the
    /// window can be *named* rather than counted.
    ///
    /// Assigned by the caller rather than by the position in `turns`, because
    /// the two come apart: a turn that produced nothing is never pushed, and
    /// from then on the nth turn of a session is not the nth element here. An
    /// index would be wrong silently, in the one file whose whole purpose is
    /// to be trusted months later.
    pub id: TurnId,
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
    /// The task this turn was asked inside, if any. The whole coupling between
    /// a task and the history: the context manager never needs to know what a
    /// task was *for*, only which turns belong to one that has closed.
    #[serde(default)]
    pub task: Option<TaskId>,
    /// Counted once, when the turn closed. A closed turn does not change, and
    /// re-counting every turn on every turn is quadratic over a session.
    pub tokens: u32,
    /// Which counter produced `tokens`. Without it, swapping tokenizers
    /// mid-session sums two different units into one bar.
    pub counted_by: Counter,
}

/// How the history gives way when the next turn no longer fits.
///
/// Not a preference: the two rewrite the prompt at completely different rates,
/// and a prefix cache reuses the longest common prefix. See
/// `RECORD/2026-08-27.prefix-reuse-and-block-eviction.completed.md`.
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
    /// How many whole turns the window has lost since the session started —
    /// the floor, as a number.
    pub evicted: usize,
    /// What *this* selection dropped, when it dropped anything. `None` is a
    /// selection that cut nothing, which is every one in a session that never
    /// fills its window.
    pub eviction: Option<Evicted>,
}

/// What one selection dropped from the window — and it stays dropped: the
/// floor only ever moves forward.
///
/// Carried out of [`Context::select`] rather than logged inside it, because the
/// context manager does not know which turn is about to be sent. The caller
/// does, and it is the one that puts this on the wire.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Evicted {
    /// The turns that left, oldest first. Named rather than counted: a reader
    /// months later cannot recover *which* without re-implementing the floor,
    /// and a transcript cannot mark them.
    pub turns: Vec<TurnId>,
    /// What they were worth in the prompt they are no longer in.
    pub tokens: u32,
    /// Which counter produced `tokens`. Every count in this system says who
    /// produced it, because two runs measured differently are not comparable
    /// and nothing else would say so.
    pub counter: Counter,
    /// Which cut this was. The whole difference between the policies is how
    /// deep they go, so a tombstone that did not name the policy would leave
    /// the reader doing the subtraction it exists to spare them.
    pub policy: Eviction,
}

/// One unit of rendered history: a live turn, or a closed task folded to its
/// summary. Eviction works over these rather than over turns, so the two ways
/// history gives way — forgetting the oldest and folding the finished —
/// compose instead of cutting each other in half.
#[derive(Debug, Clone, Copy)]
enum Item {
    /// An index into `turns`.
    Turn(usize),
    Folded {
        task: TaskId,
        /// Index of the task's first turn still above the floor.
        first: usize,
    },
}

impl Item {
    /// The index of the oldest turn this item covers.
    fn first(&self) -> usize {
        match self {
            Self::Turn(index) => *index,
            Self::Folded { first, .. } => *first,
        }
    }
}

/// The conversation, and the rule for turning it into a prompt.
#[derive(Debug, Clone)]
pub struct Context {
    system: String,
    /// The tool definitions, rendered once. Second half of the cached prefix
    /// and counted as its own bucket, because "the system block grew" is not an
    /// answer to why the window is full.
    tools: String,
    /// The repository outline, rendered once. Last of the cached prefix, which
    /// is where blocks are ordered by how often they are *rewritten*: the
    /// system block is a constant, the tools change when the tool set does, and
    /// the map changes when the repository does. Empty unless asked for — a map
    /// that arrived switched on would change every number in every recording
    /// made so far. See `crate::repo_map`.
    map: String,
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
    /// The session's tasks, in order. Closed ones fold their turns at
    /// selection time; nothing here rewrites the history.
    tasks: Vec<Task>,
}

impl Context {
    pub fn new(system: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            tools: String::new(),
            map: String::new(),
            turns: Vec::new(),
            floor: 0,
            tasks: Vec::new(),
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

    /// Adds the repository outline to the prefix, under the tool definitions.
    pub fn with_map(mut self, map: impl Into<String>) -> Self {
        self.map = map.into();
        self
    }

    pub fn map(&self) -> &str {
        &self.map
    }

    /// The whole cached prefix, as one message. Assembled here and nowhere else
    /// so that two call sites cannot join it differently.
    fn system_message(&self) -> String {
        let mut text = self.system.clone();
        for block in [&self.tools, &self.map] {
            if !block.is_empty() {
                text.push_str("\n\n");
                text.push_str(block);
            }
        }
        text
    }

    pub fn turns(&self) -> &[Turn] {
        &self.turns
    }

    pub fn system(&self) -> &str {
        &self.system
    }

    /// Closes a turn and counts it, once. `id` is the number the session gave
    /// the turn — see [`Turn::id`].
    pub fn push_turn(
        &mut self,
        id: TurnId,
        prompt: impl Into<String>,
        answer: impl Into<String>,
        code_context: Vec<Fragment>,
        counter: &dyn TokenCounter,
    ) {
        self.push_turn_with_steps(id, prompt, answer, code_context, Vec::new(), counter);
    }

    /// The same, for a turn in which the agent used tools.
    pub fn push_turn_with_steps(
        &mut self,
        id: TurnId,
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
            id,
            prompt,
            answer,
            steps,
            code_context,
            task: self.live_task(),
            tokens,
            counted_by: counter.id(),
        });
    }

    /// Proposes a task. Nothing runs in this state — approval is a separate
    /// act, because that is the entire point of the boundary.
    pub fn propose_task(&mut self, objective: impl Into<String>, plan: Plan) -> TaskId {
        let id = self.tasks.len() as TaskId + 1;
        self.tasks.push(Task::new(id, objective, plan));
        id
    }

    /// Adds to a proposed task's plan what a person put in at the gate, and
    /// answers with the plan as it now stands.
    ///
    /// The amendment arrives with the approval, so this happens while the task
    /// is still `Proposed`: the plan a task is approved with is the one it
    /// keeps, and the one its sandbox is built from.
    pub fn amend_plan(
        &mut self,
        id: TaskId,
        files: &[String],
        writes: &[String],
        commands: &[String],
    ) -> Option<Plan> {
        let task = self.task_mut(id)?;
        task.plan.amend(files, writes, commands);
        Some(task.plan.clone())
    }

    /// Approves it. Turns pushed from here on belong to it.
    ///
    /// Only a proposal can be approved. The lifecycle is a state machine and
    /// every one of these is a guard on it: without them `reopen_task` on a
    /// *rejected* task sets it approved, which reinstates a plan a person
    /// turned down and hands the next prompt a live task to run inside — the
    /// gate leaking through the message meant for unfolding a fold.
    pub fn approve_task(&mut self, id: TaskId) -> bool {
        match self.task_mut(id) {
            Some(task) if task.state == TaskState::Proposed => {
                task.approve();
                true
            }
            _ => false,
        }
    }

    /// Refuses a proposal. Nothing ran under it, so nothing folds; the task
    /// stays in the session as the record of what was turned down.
    pub fn reject_task(&mut self, id: TaskId) -> bool {
        match self.task_mut(id) {
            Some(task) if task.state == TaskState::Proposed => {
                task.reject();
                true
            }
            _ => false,
        }
    }

    /// Closes it: from the next selection on, its turns render as one summary.
    ///
    /// The summary is written from the task's own tool steps and the fragments
    /// its turns were handed, so it is evidence rather than the model's account
    /// of itself. Returns the summary text, or `None` if there is no such task.
    pub fn close_task(&mut self, id: TaskId, counter: &dyn TokenCounter) -> Option<String> {
        let mine = |turn: &&Turn| turn.task == Some(id);
        let steps: Vec<&ToolStep> = self
            .turns
            .iter()
            .filter(mine)
            .flat_map(|turn| turn.steps.iter())
            .collect();
        // The field beside the one above, and the reason the fold lost answers
        // until now: what a task read was in hand at the close and was never
        // asked for. See `RECORD/2026-08-30.the-fold-probe-run.completed.md`.
        let shown: Vec<&Fragment> = self
            .turns
            .iter()
            .filter(mine)
            .flat_map(|turn| turn.code_context.iter())
            .collect();
        let turns = self.turns.iter().filter(mine).count();
        let task = self.tasks.iter_mut().find(|task| task.id == id)?;
        // Only an open task folds. A proposal has no turns to fold and nothing
        // has been approved to summarise; closing one would take the gate off
        // the screen with its prompt still held, and the session would be stuck
        // with no way to answer a proposal nobody can see.
        if !task.is_open() {
            return None;
        }
        task.close(&steps, &shown, turns, counter);
        task.summary.as_ref().map(|summary| summary.text.clone())
    }

    /// Reopens it: the fold stops applying and its turns are sent verbatim
    /// again. Nothing is recovered, because nothing was deleted.
    pub fn reopen_task(&mut self, id: TaskId) -> bool {
        match self.task_mut(id) {
            Some(task) if task.is_closed() => {
                task.reopen();
                true
            }
            _ => false,
        }
    }

    pub fn tasks(&self) -> &[Task] {
        &self.tasks
    }

    pub fn task(&self, id: TaskId) -> Option<&Task> {
        self.tasks.iter().find(|task| task.id == id)
    }

    fn task_mut(&mut self, id: TaskId) -> Option<&mut Task> {
        self.tasks.iter_mut().find(|task| task.id == id)
    }

    /// The task turns are currently attributed to: the last one approved and
    /// not closed. One level, deliberately — see the tasks record.
    pub fn live_task(&self) -> Option<TaskId> {
        self.tasks
            .iter()
            .rev()
            .find(|task| task.is_open())
            .map(|task| task.id)
    }

    /// The history as it renders: live turns one by one, and each closed task
    /// as a single folded item.
    ///
    /// Built from the floor forward, so a task closed after part of it was
    /// evicted folds only from what the window still holds — except that the
    /// summary it folds to is the whole task's. That is deliberate and it is a
    /// cost: see `RECORD/2026-08-30.tasks-in-code.completed.md`.
    fn items(&self) -> Vec<Item> {
        let mut items = Vec::new();
        let mut index = self.floor;
        while index < self.turns.len() {
            let folded = self.turns[index]
                .task
                .filter(|id| self.task(*id).is_some_and(Task::is_closed));
            match folded {
                Some(id) => {
                    let first = index;
                    while index < self.turns.len() && self.turns[index].task == Some(id) {
                        index += 1;
                    }
                    items.push(Item::Folded { task: id, first });
                }
                None => {
                    items.push(Item::Turn(index));
                    index += 1;
                }
            }
        }
        items
    }

    /// What an item costs in the prompt it renders into.
    fn item_tokens(&self, item: &Item, counter: &dyn TokenCounter) -> u32 {
        match item {
            Item::Turn(index) => self.tokens_of(&self.turns[*index], counter),
            Item::Folded { task, .. } => self
                .task(*task)
                .and_then(|task| {
                    task.summary.as_ref().map(|summary| {
                        // Stored counts are stale under a changed counter, the
                        // same way a turn's are, and are redone rather than
                        // summed into a different unit.
                        match summary.counted_by == counter.id() {
                            true => summary.tokens,
                            false => counter.count(&summary.text),
                        }
                    })
                })
                .unwrap_or(0),
        }
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
        let map_tokens = counter.count(&self.map);
        // Counted as the two buckets they will be plotted as. Tokenization is
        // not additive across a boundary, so this can differ by a token or two
        // from counting the concatenation; that is the same order as the
        // template overhead we already accept and report.
        let code_tokens = counter.count(&fragments_text(code_context));
        let prompt_tokens = counter.count(prompt);

        let mut eviction = None;
        if let Some(limit) = budget.limit {
            // The current turn and the reserve are not negotiable: nothing here
            // may trim the prompt the user just typed. If they alone exceed the
            // window, history is empty and the turn still goes — being over the
            // limit is the backend's error to report, not ours to hide by
            // cutting the question in half.
            let fixed = system_tokens
                + tools_tokens
                + map_tokens
                + code_tokens
                + prompt_tokens
                + budget.reserve;
            let available = limit.saturating_sub(fixed);

            if self.fits_from(available, counter) > self.floor {
                // Taken before the floor moves: these are the items the cut
                // chooses from, and the ones it drops are unreachable
                // afterwards — which is the whole reason this has to be
                // reported from in here rather than reconstructed outside.
                let before = self.floor;
                let items = self.items();

                // It no longer fits. How deep the cut goes is the whole
                // difference between the two policies.
                let target = match budget.eviction {
                    Eviction::Turn => available,
                    Eviction::Block { low_water } => {
                        (available as f32 * low_water.clamp(0.0, 1.0)) as u32
                    }
                };
                self.floor = self.fits_from(target, counter);

                // The floor only ever lands on an item boundary, so summing the
                // items below it is exact rather than a share of one.
                eviction = (self.floor > before).then(|| Evicted {
                    turns: self.turns[before..self.floor]
                        .iter()
                        .map(|turn| turn.id)
                        .collect(),
                    tokens: items
                        .iter()
                        .filter(|item| item.first() < self.floor)
                        .map(|item| self.item_tokens(item, counter))
                        .sum(),
                    counter: counter.id(),
                    policy: budget.eviction,
                });
            }
        }

        // Decided, then rendered — and the fold is part of the decision: a
        // closed task is one item worth its summary, so eviction and compaction
        // compose instead of arguing over the same turns.
        let items = self.items();
        let mut history_tokens = 0;
        let mut summary_tokens = 0;
        let mut messages = Vec::with_capacity(items.len() * 2 + 2);
        messages.push(Message::system(self.system_message()));
        for item in &items {
            let tokens = self.item_tokens(item, counter);
            match item {
                Item::Turn(index) => {
                    history_tokens += tokens;
                    let turn = &self.turns[*index];
                    messages.push(Message::user(user_text(&turn.code_context, &turn.prompt)));
                    // Each step is a real exchange, so the alternation holds and
                    // no chat template has to decide what two user messages in a
                    // row mean.
                    for step in &turn.steps {
                        messages.push(Message::assistant(step.text.clone()));
                        messages.push(Message::user(step.result_text()));
                    }
                    messages.push(Message::assistant(turn.answer.clone()));
                }
                Item::Folded { task, .. } => {
                    summary_tokens += tokens;
                    // One exchange, so the alternation a folded block sits in
                    // is the same one a turn would have left behind. The user
                    // half is the objective as it was approved: it is what was
                    // asked, and inventing a sentence for it would be prose in
                    // the one place this design keeps prose out of.
                    let Some(task) = self.task(*task) else {
                        continue;
                    };
                    let Some(summary) = &task.summary else {
                        continue;
                    };
                    messages.push(Message::user(task.objective.clone()));
                    messages.push(Message::assistant(summary.text.clone()));
                }
            }
        }
        messages.push(Message::user(user_text(code_context, prompt)));

        let mut buckets = vec![
            Bucket::new("system", system_tokens),
            Bucket::new("tools", tools_tokens),
            // Beside the tools rather than inside them: both are prefix, and
            // "the prefix grew" is not an answer to why the window is full.
            Bucket::new("map", map_tokens),
            // Beside `history` rather than inside it: the fold is the one thing
            // the panel exists to watch, and a single bar cannot show a block
            // being replaced by a line.
            Bucket::new("summaries", summary_tokens),
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
            eviction,
        }
    }

    /// The oldest turn that still fits, taking items newest first and never
    /// reaching past the floor. Returns an index into `turns`.
    ///
    /// Items, not turns: a folded task is kept or dropped whole, because half a
    /// summary is not a summary of half a task. So the floor only ever lands on
    /// an item boundary.
    fn fits_from(&self, available: u32, counter: &dyn TokenCounter) -> usize {
        let mut left = available;
        let mut start = self.turns.len();
        for item in self.items().iter().rev() {
            let tokens = self.item_tokens(item, counter);
            if tokens > left {
                break;
            }
            left -= tokens;
            start = item.first();
        }
        start
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
                n as TurnId + 1,
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

        // System, the tool definitions, the map, the fragments, the prompt.
        // The ten stored turns are not re-counted, which is the whole point of
        // storing their counts.
        assert_eq!(counter.calls.load(Ordering::Relaxed) - after_building, 5);
    }

    #[test]
    fn a_turn_counted_by_another_counter_is_recounted_rather_than_summed() {
        let words = WordCounter::default();
        let mut context = Context::new("system prompt here");
        context.push_turn(1, "one two three", "four five", vec![], &ApproximateCounter);

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
    fn the_map_is_prefix_and_stays_byte_identical_as_the_history_grows() {
        // The placement argument is worthless if the map wobbles: it is above
        // the history precisely so that a prompt cache can keep it.
        let counter = WordCounter::default();
        let mut context = Context::new("system prompt here")
            .with_tools("# Tools\nread_file")
            .with_map("# Repository map\nsrc/lib.rs\n     1  pub fn main");
        let budget = Budget::new(8192, 512, Eviction::Turn);

        let first = context.select("one", &[], budget, &counter);
        context.push_turn(1, "one", "an answer", vec![], &counter);
        let second = context.select("two", &[], budget, &counter);

        assert_eq!(
            first.messages[0].content, second.messages[0].content,
            "the whole prefix is one message and it does not move",
        );
        assert!(
            first.messages[0].content.ends_with("     1  pub fn main"),
            "system, then tools, then the map — most stable first: {}",
            first.messages[0].content,
        );
        let map = |selection: &Selection| {
            selection
                .buckets
                .iter()
                .find(|bucket| bucket.name == "map")
                .expect("the map bucket")
                .tokens
        };
        assert!(map(&first) > 0);
        assert_eq!(
            map(&first),
            map(&second),
            "and it costs the same every turn"
        );
    }

    #[test]
    fn a_map_is_counted_against_the_window_rather_than_added_beside_it() {
        // It is in the prompt, so it is in the budget: history has to give way
        // for it like anything else. A map that was free would be a map that
        // silently ate the answer.
        let counter = WordCounter::default();
        let mut bare = context_of_equal_turns(6, &counter);
        let mut mapped = context_of_equal_turns(6, &counter);
        mapped = mapped.with_map("aa bb cc dd ee ff gg hh ii jj kk ll mm nn oo pp");
        let system = counter.count(bare.system());
        let budget = Budget::new(system + 1 + 8 * 5, 0, Eviction::Turn);

        let without = bare.select("q", &[], budget, &counter);
        let with = mapped.select("q", &[], budget, &counter);

        assert!(
            with.evicted > without.evicted,
            "the map cost history: {} evicted with it, {} without",
            with.evicted,
            without.evicted,
        );
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
                "system",
                "tools",
                "map",
                "summaries",
                "history",
                "code",
                "prompt",
                "reserve"
            ]
        );
    }

    /// A task with turns in it, closed or not, for the fold cases below.
    fn context_with_closed_task(live: usize, counter: &dyn TokenCounter) -> (Context, TaskId) {
        let mut context = Context::new("system prompt here");
        let task = context.propose_task("explain the context manager", Plan::default());
        context.approve_task(task);
        for n in 0..3 {
            context.push_turn(
                n as TurnId + 1,
                format!("question number {n} padded out"),
                format!("answer number {n} padded out"),
                vec![],
                counter,
            );
        }
        context.close_task(task, counter);
        for n in 0..live {
            context.push_turn(
                n as TurnId + 4,
                format!("later question {n} padded out"),
                format!("later answer {n} padded out"),
                vec![],
                counter,
            );
        }
        (context, task)
    }

    #[test]
    fn the_fold_keeps_what_the_task_was_shown() {
        // The probe's turns 17 and 18: a task grounded by a fragment, closed,
        // and asked about afterwards. Before this, the summary said "no tools
        // ran" and the file was gone. See
        // `RECORD/2026-08-30.the-fold-probe-run.completed.md`.
        let counter = WordCounter::default();
        let mut context = Context::new("system prompt here");
        let task = context.propose_task("work out what the policy grants", Plan::default());
        context.approve_task(task);
        context.push_turn(
            1,
            "which programs does this policy allow?",
            "cargo, rustc, git, rg, ls",
            vec![Fragment {
                path: "luu.toml:1-3".into(),
                text: "[sandbox]\ncommands = [\"cargo\", \"rg\"]\n".into(),
            }],
            &counter,
        );
        let summary = context.close_task(task, &counter).unwrap();

        assert!(summary.contains("luu.toml:1-3"), "{summary}");
        assert!(
            summary.contains("commands = [\"cargo\", \"rg\"]"),
            "the fragment was in hand at the close: {summary}",
        );

        let selection = context.select(
            "which programs does the policy allow?",
            &[],
            Budget::new(0, 0, Eviction::Turn),
            &counter,
        );
        assert!(
            selection.messages[2].content.contains("cargo"),
            "what the folded task read is still in the prompt: {:?}",
            selection.messages[2].content,
        );
    }

    #[test]
    fn a_closed_task_folds_its_turns_into_one_exchange() {
        let counter = WordCounter::default();
        let (mut context, _) = context_with_closed_task(0, &counter);

        let selection =
            context.select("now this", &[], Budget::new(0, 0, Eviction::Turn), &counter);

        assert_eq!(
            selection.messages.len(),
            1 + 2 + 1,
            "system, the folded pair, and the current prompt",
        );
        assert_eq!(selection.messages[1].role, Role::User);
        assert_eq!(
            selection.messages[1].content, "explain the context manager",
            "the user half of a fold is the objective as approved, not a sentence about it",
        );
        assert!(selection.messages[2].content.contains("[task closed]"));
        assert_eq!(
            context.turns().len(),
            3,
            "closing is an event, not a mutation: the turns are still there",
        );

        let bucket = |name: &str| {
            selection
                .buckets
                .iter()
                .find(|b| b.name == name)
                .unwrap()
                .tokens
        };
        assert!(bucket("summaries") > 0);
        assert_eq!(bucket("history"), 0, "nothing is left unfolded");
    }

    #[test]
    fn reopening_sends_the_turns_verbatim_again() {
        let counter = WordCounter::default();
        let (mut context, task) = context_with_closed_task(0, &counter);
        let folded = context.select("now this", &[], Budget::new(0, 0, Eviction::Turn), &counter);

        assert!(context.reopen_task(task));
        let reopened = context.select("now this", &[], Budget::new(0, 0, Eviction::Turn), &counter);

        assert_eq!(folded.messages.len(), 4);
        assert_eq!(
            reopened.messages.len(),
            3 * 2 + 2,
            "the fold stopped applying; nothing had to be recovered",
        );
    }

    #[test]
    fn a_folded_task_is_kept_or_dropped_whole_at_every_window_size() {
        // The fold and eviction meet here: a folded block is one item, so the
        // floor can land before it or after it and never inside it.
        for limit in (20..240).step_by(4) {
            let counter = WordCounter::default();
            let (mut context, _) = context_with_closed_task(3, &counter);
            let selection = context.select(
                "now this",
                &[],
                Budget::new(limit, 0, Eviction::Turn),
                &counter,
            );
            assert!(
                selection.evicted == 0 || selection.evicted >= 3,
                "limit {limit} cut inside the folded task ({} turns dropped): \
                 half a summary is not a summary of half a task",
                selection.evicted,
            );
        }
    }

    #[test]
    fn turns_are_attributed_to_the_task_that_was_open_when_they_were_pushed() {
        let counter = WordCounter::default();
        let mut context = Context::new("system");
        context.push_turn(1, "before any task", "a", vec![], &counter);
        let task = context.propose_task("do the thing", Plan::default());
        context.push_turn(2, "proposed, not approved", "b", vec![], &counter);
        context.approve_task(task);
        context.push_turn(3, "inside the task", "c", vec![], &counter);

        let attributed: Vec<Option<TaskId>> =
            context.turns().iter().map(|turn| turn.task).collect();
        assert_eq!(
            attributed,
            vec![None, None, Some(task)],
            "nothing runs inside a proposal, so nothing is attributed to one",
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
                n as TurnId + 1,
                format!("q {n} aa bb"),
                format!("a {n} cc dd"),
                vec![],
                counter,
            );
        }
        context
    }

    #[test]
    fn a_selection_that_cuts_says_which_turns_it_dropped() {
        // Eight tokens a turn, so the arithmetic is checkable by eye: room for
        // three turns and a fourth one asked.
        let counter = WordCounter::default();
        let mut context = context_of_equal_turns(4, &counter);
        let system = counter.count(context.system());

        let selection = context.select(
            "q",
            &[],
            Budget::new(system + 1 + 8 * 3, 0, Eviction::Turn),
            &counter,
        );

        let evicted = selection.eviction.expect("the window could not hold four");
        assert_eq!(
            evicted.turns,
            vec![1],
            "the per-turn policy drops the minimum, and names it",
        );
        assert_eq!(evicted.tokens, 8, "what the dropped turn was worth");
        assert_eq!(evicted.counter, counter.id(), "a count says who counted it");
        assert_eq!(evicted.policy, Eviction::Turn);
    }

    #[test]
    fn a_selection_that_fits_drops_nothing_and_says_so() {
        let counter = WordCounter::default();
        let mut context = context_of_equal_turns(3, &counter);
        let system = counter.count(context.system());

        let selection = context.select(
            "q",
            &[],
            Budget::new(system + 1 + 8 * 4, 0, Eviction::Turn),
            &counter,
        );

        assert!(
            selection.eviction.is_none(),
            "a tombstone for a cut that did not happen is a lie a panel would plot",
        );
    }

    #[test]
    fn the_cut_names_session_turns_rather_than_positions_in_the_history() {
        // A turn that produced nothing is never pushed — both call sites skip
        // it — so the third turn of a session can be the second one here. An
        // index would name the wrong turn, quietly, in the one file whose
        // purpose is to be trusted months later.
        let counter = WordCounter::default();
        let mut context = Context::new("sys");
        context.push_turn(1, "q 1 aa bb", "a 1 cc dd", vec![], &counter);
        // 2 is missing: it was cancelled before it produced anything.
        context.push_turn(3, "q 3 aa bb", "a 3 cc dd", vec![], &counter);
        context.push_turn(4, "q 4 aa bb", "a 4 cc dd", vec![], &counter);
        let system = counter.count(context.system());

        let selection = context.select(
            "q",
            &[],
            Budget::new(system + 1 + 8 * 2, 0, Eviction::Turn),
            &counter,
        );

        assert_eq!(
            selection.eviction.expect("a cut").turns,
            vec![1],
            "the number the session handed out, not the index",
        );
    }

    #[test]
    fn the_block_policy_reports_the_deeper_cut_it_made() {
        let counter = WordCounter::default();
        let mut context = context_of_equal_turns(6, &counter);
        let system = counter.count(context.system());
        let policy = Eviction::Block { low_water: 0.5 };

        let selection = context.select(
            "q",
            &[],
            Budget::new(system + 1 + 8 * 5, 0, policy),
            &counter,
        );

        let evicted = selection.eviction.expect("the window could not hold six");
        assert_eq!(
            evicted.policy, policy,
            "which cut this was, not just that one happened"
        );
        assert!(
            evicted.turns.len() > 1,
            "block eviction cuts past what it needs: {:?}",
            evicted.turns,
        );
        assert_eq!(
            evicted.tokens as usize,
            evicted.turns.len() * 8,
            "the tokens are the turns that left, not a share of them",
        );
    }

    #[test]
    fn a_folded_task_leaving_the_window_names_the_turns_it_covered() {
        // The two ways history stops being sent, composed: the task is already
        // folded to its summary when the summary itself falls out.
        let counter = WordCounter::default();
        let (mut context, _) = context_with_closed_task(3, &counter);

        let mut cut = None;
        // Tightened one token at a time, so the first cut is caught wherever
        // the summary happens to land rather than at a limit picked by hand.
        for limit in (1..=200).rev() {
            let selection =
                context.select("q", &[], Budget::new(limit, 0, Eviction::Turn), &counter);
            if let Some(evicted) = selection.eviction {
                cut = Some(evicted);
                break;
            }
        }

        let evicted = cut.expect("some limit is too small for the whole history");
        assert_eq!(
            evicted.turns,
            vec![1, 2, 3],
            "a folded task is kept or dropped whole, and its turns are named",
        );
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
        per_turn.push_turn(9, "q 8 aa bb", "a 8 cc dd", vec![], &counter);
        block.push_turn(9, "q 8 aa bb", "a 8 cc dd", vec![], &counter);

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
            1,
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
                n as TurnId + 1,
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
        bare.push_turn(1, "q", "a", vec![], &counter);
        let mut with_steps = Context::new("sys");
        with_steps.push_turn_with_steps(
            1,
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

    /// The lifecycle is a state machine, and the messages that drive it arrive
    /// from a socket anyone can open. Every transition that is not legal has to
    /// be refused rather than performed — found by driving the real page:
    /// closing a *proposed* task took the gate off the screen with its prompt
    /// still held, and the session had no way back.
    #[test]
    fn only_the_legal_task_transitions_happen() {
        let counter = ApproximateCounter;
        let mut context = Context::new("system");
        let task = context.propose_task("add a flag", Plan::default());

        assert!(
            context.close_task(task, &counter).is_none(),
            "a proposal has nothing to fold: nothing was approved and no turn ran",
        );
        assert!(!context.reopen_task(task), "it was never closed");
        assert_eq!(context.task(task).unwrap().state, TaskState::Proposed);

        assert!(context.approve_task(task));
        assert!(
            !context.approve_task(task),
            "approving twice is not a state"
        );
        assert!(
            !context.reject_task(task),
            "an approved task is past refusing"
        );

        assert!(context.close_task(task, &counter).is_some());
        assert!(
            context.close_task(task, &counter).is_none(),
            "closing a closed task would rewrite the summary the model already has",
        );
        assert!(context.reopen_task(task));
        assert_eq!(context.task(task).unwrap().state, TaskState::Approved);
    }

    /// The sharp one: a refused plan must not come back through the message
    /// that exists for unfolding a fold. Reopening a rejected task used to set
    /// it approved, which makes it the live task — and the next prompt then
    /// runs inside a plan a person turned down, with no gate in front of it.
    #[test]
    fn a_rejected_plan_cannot_be_reopened_into_a_live_task() {
        let mut context = Context::new("system");
        let task = context.propose_task("delete everything", Plan::default());
        assert!(context.reject_task(task));

        assert!(!context.reopen_task(task));
        assert_eq!(context.task(task).unwrap().state, TaskState::Rejected);
        assert!(context.live_task().is_none(), "nothing is live");
    }
}
