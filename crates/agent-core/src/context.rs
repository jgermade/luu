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

use std::collections::HashSet;

use serde::{Deserialize, Serialize};

use crate::backend::Message;
use crate::job::{ApprovedBy, ClosedBy, Job, JobId, JobState, Plan, Replaced, TaskId};
use crate::protocol::TurnId;
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
///
/// `path` is the spec as it was written — `src/lib.rs:12-40`, not a canonical
/// path — so it carries the span, and two fragments are the same fragment when
/// the path *and* the bytes match. The bytes are half of that on purpose: a
/// file edited between two turns yields the same spec and different text, and
/// `resolving-a-symbol` is the record of what happens when the second one is
/// mistaken for the first.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
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
    /// The job this turn was asked inside, if any.
    #[serde(default, alias = "task")]
    pub job: Option<JobId>,
    /// Counted once, when the turn closed. A closed turn does not change, and
    /// re-counting every turn on every turn is quadratic over a session.
    pub tokens: u32,
    /// Which counter produced `tokens`. Without it, swapping tokenizers
    /// mid-session sums two different units into one bar.
    pub counted_by: Counter,
}

impl Turn {
    /// Backwards compatibility accessor for callers that called `turn.task`.
    pub fn task(&self) -> Option<JobId> {
        self.job
    }
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

/// What a span already in the window costs the turn that selects it again.
///
/// Rule A of `RECORD/2026-09-06.what-leaves-the-history.completed.md`, and the
/// smaller half of it: the `history` bucket of a selecting run is ~90% quoted
/// code by turn 20, and 9.2% of the code tokens in the precision corpus are a
/// span some earlier turn already put in the window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Repeat {
    /// Send it again, in every turn that selected it. Everything recorded
    /// before this enum existed, and the default, because a run made under a
    /// rule that did not exist is not comparable to one made under it.
    Always,
    /// Render it once, in the **oldest** turn of the window that carries it.
    ///
    /// Oldest and not newest, and that is the whole decision. The alternative
    /// rewrites a turn that is already in the prefix every time a later turn
    /// repeats one of its spans, which is rule B's cost and rule B's
    /// measurement; this way only the newest message changes, which is the one
    /// that changes anyway.
    ///
    /// The owner is chosen inside the window being rendered, never over the
    /// whole session, so eviction cannot orphan a span: dropping the turn that
    /// owned one hands it to the next turn that carries it, in the same call.
    /// Choosing an owner that may already be below the floor is
    /// `the-fold-fix-verified`'s failure with a different mechanism, and the
    /// precision corpus offers it 22 chances.
    Once,
}

/// What an older turn's quoted code costs once the window stops fitting.
///
/// Rule B of `RECORD/2026-09-06.what-leaves-the-history.completed.md`, argued in
/// `RECORD/2026-09-08.prune-behind.completed.md`, and the larger half: rule A reaches
/// the spans a *later* turn selected again, and this reaches the ones nobody
/// did — which by turn 20 of the precision run is most of a history block that
/// is ~90% quoted code.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Prune {
    /// An old turn carries its spans in full until it is evicted whole.
    /// Everything recorded before this enum existed, and the default.
    Never,
    /// Under pressure, the oldest turns of the window give up their spans and
    /// keep their exchange: the code becomes the line that cites it, the
    /// question and the answer stay.
    ///
    /// A ratchet, like [`Context::floor`] and for the same reason — a decision
    /// the window took has to outlive the call that took it, or the prompt
    /// oscillates and no prefix survives two calls. It moves to the target the
    /// eviction policy has already computed, so depth against frequency stays
    /// one trade with one knob, and the floor only moves when pruning the whole
    /// window is not enough.
    Behind,
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
    /// What a span already visible in the window costs to select again.
    pub repeat: Repeat,
    /// What an old turn's spans cost once the window no longer fits.
    pub prune: Prune,
}

impl Budget {
    /// The CLI spells "unknown" as 0, because a flag has to have a default.
    pub fn new(limit: u32, reserve: u32, eviction: Eviction) -> Self {
        Self {
            limit: (limit > 0).then_some(limit),
            reserve,
            eviction,
            repeat: Repeat::Always,
            prune: Prune::Never,
        }
    }

    /// Off by default and turned on here, so that every recording made before
    /// it stays readable as the arm it was.
    pub fn repeating(self, repeat: Repeat) -> Self {
        Self { repeat, ..self }
    }

    /// Off by default, for the reason `repeating` is: this one rewrites the
    /// history block where it fires, so a recording made under it is not the
    /// same arm as one made without it.
    pub fn pruning(self, prune: Prune) -> Self {
        Self { prune, ..self }
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
    /// How many turns of the window have given up their spans, as an index into
    /// the session's turns — the prune line, and a ratchet like the floor.
    pub pruned: usize,
    /// What *this* selection dropped, when it dropped anything. `None` is a
    /// selection that cut nothing, which is every one in a session that never
    /// fills its window.
    pub eviction: Option<Evicted>,
    /// What this selection pruned, if the line moved. `None` on a selection
    /// that pruned nothing, which is every one made under [`Prune::Never`].
    pub pruning: Option<Pruned>,
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

/// What one selection took out of the prompt without taking the turn with it.
///
/// Beside [`Evicted`] and deliberately not the same thing: an evicted turn is
/// gone from the conversation, and a pruned one is still asked, still answered
/// and still in the transcript — what left is its code. That is why this goes
/// on the trace channel where the buckets are, and eviction goes on the
/// protocol: a client that cannot read this is not wrong about which turns the
/// session has.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Pruned {
    /// The turns that gave up spans, oldest first. Only the ones that had any:
    /// the line passes over turns with nothing to give, and naming them would
    /// report a saving that did not happen.
    pub turns: Vec<TurnId>,
    /// What the window saved by it — the difference of two windows, not the sum
    /// of the spans, because under [`Repeat::Once`] a span the pruned turn
    /// owned passes to a younger turn instead of leaving.
    pub tokens: u32,
    /// Which counter produced `tokens`.
    pub counter: Counter,
}

/// One unit of rendered history: a live turn, or a closed job folded to its
/// summary. Eviction works over these rather than over turns, so the two ways
/// history gives way — forgetting the oldest and folding the finished —
/// compose instead of cutting each other in half.
#[derive(Debug, Clone, Copy)]
enum Item {
    /// An index into `turns`.
    Turn(usize),
    Folded {
        job: JobId,
        /// Index of the job's first turn still above the floor.
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
    /// the map changes when the repository does. Empty unless asked for.
    map: String,
    turns: Vec<Turn>,
    /// The oldest turn still in the window. It only ever moves forward.
    floor: usize,
    /// The prune line: turns below it render their spans as citations. Moves
    /// forward only, and only under [`Prune::Behind`]. Never below the floor,
    /// because a turn nobody renders has nothing to give up.
    pruned: usize,
    /// The session's jobs, in order. Closed ones fold their turns at
    /// selection time; nothing here rewrites the history.
    jobs: Vec<Job>,
}

impl Context {
    pub fn new(system: impl Into<String>) -> Self {
        Self {
            system: system.into(),
            tools: String::new(),
            map: String::new(),
            turns: Vec::new(),
            floor: 0,
            pruned: 0,
            jobs: Vec::new(),
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

    pub fn floor(&self) -> usize {
        self.floor
    }

    /// The prune line. Turns below it are still in the window and still
    /// answered; what they no longer carry is their code.
    pub fn pruned(&self) -> usize {
        self.pruned
    }

    /// Reconstructs an active context from a stored session view — the inverse fold.
    ///
    /// Rebuilds turns, jobs, summaries and eviction floor from the view,
    /// counting tokens with the supplied counter so the resumed context
    /// budgets identically to the original session.
    pub fn from_view(
        view: &crate::api::SessionView,
        system: impl Into<String>,
        tools: impl Into<String>,
        map: impl Into<String>,
        counter: &dyn TokenCounter,
    ) -> Self {
        let mut jobs = Vec::with_capacity(view.jobs.len());
        for jv in &view.jobs {
            let summary = jv.summary.as_ref().map(|text| crate::job::Summary {
                text: text.clone(),
                tokens: counter.count(text),
                counted_by: counter.id(),
                // Carried rather than recomputed: what the fold replaced was
                // counted at the close, over turns this view may no longer
                // have. Recounting it here would answer a different question
                // and look like the same one.
                replaced: jv.replaced.clone(),
            });
            jobs.push(Job {
                id: jv.id,
                objective: jv.objective.clone(),
                plan: jv.plan.clone(),
                state: jv.state,
                summary,
                closed_by: jv.closed_by,
                approved_by: jv.approved_by.clone(),
            });
        }

        let mut turns = Vec::with_capacity(view.turns.len());
        for tv in &view.turns {
            let mut steps = Vec::with_capacity(tv.tools.len());
            for call_view in &tv.tools {
                let call = crate::tools::ToolCall {
                    name: call_view.name.clone(),
                    arguments: call_view.arguments.clone(),
                };
                let text = format!(
                    "```tool\n{}\n```",
                    serde_json::to_string(&call).unwrap_or_default()
                );
                let outcome = crate::tools::ToolOutcome {
                    verdict: call_view.verdict.clone().unwrap_or_else(|| {
                        crate::sandbox::Verdict::allow("resumed", crate::sandbox::Applied::Process)
                    }),
                    output: call_view.output.clone(),
                    error: call_view.error.clone(),
                    truncated: call_view.truncated,
                    command: call_view.command.clone(),
                };
                steps.push(ToolStep {
                    text,
                    call,
                    outcome,
                    duration_ms: call_view.duration_ms.unwrap_or(0),
                });
            }

            let prompt = tv.prompt.clone();
            let answer = tv.text.clone();
            let tokens =
                counter.count(&prompt) + steps_tokens(&steps, counter) + counter.count(&answer);

            turns.push(Turn {
                id: tv.turn,
                prompt,
                answer,
                steps,
                code_context: Vec::new(),
                job: tv.job,
                tokens,
                counted_by: counter.id(),
            });
        }

        let floor = view
            .turns
            .iter()
            .take_while(|t| t.evicted_by.is_some())
            .count();

        Self {
            system: system.into(),
            tools: tools.into(),
            map: map.into(),
            turns,
            floor,
            // Zero and not carried: the view has no fragments, so `from_view`
            // rebuilds every turn with an empty `code_context` and a resumed
            // session has nothing to prune. Restoring a line here would say a
            // turn had given up spans it is not carrying.
            pruned: 0,
            jobs,
        }
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
            job: self.live_job(),
            tokens,
            counted_by: counter.id(),
        });
    }

    /// Proposes a job. Nothing runs in this state — approval is a separate
    /// act, because that is the entire point of the boundary.
    pub fn propose_job(&mut self, objective: impl Into<String>, plan: Plan) -> JobId {
        let id = self.jobs.len() as JobId + 1;
        self.jobs.push(Job::new(id, objective, plan));
        id
    }

    pub fn propose_task(&mut self, objective: impl Into<String>, plan: Plan) -> TaskId {
        self.propose_job(objective, plan)
    }

    /// Adds to a proposed job's plan what a person put in at the gate, and
    /// answers with the plan as it now stands.
    ///
    /// The amendment arrives with the approval, so this happens while the job
    /// is still `Proposed`: the plan a job is approved with is the one it
    /// keeps, and the one its sandbox is built from.
    #[allow(clippy::too_many_arguments)]
    pub fn amend_plan(
        &mut self,
        id: JobId,
        files: &[String],
        writes: &[String],
        commands: &[String],
        closes_on: Option<&str>,
        network: Option<bool>,
        egress: Option<&[String]>,
    ) -> Option<Plan> {
        let job = self.job_mut(id)?;
        job.plan
            .amend(files, writes, commands, closes_on, network, egress);
        Some(job.plan.clone())
    }

    /// Approves it. Turns pushed from here on belong to it.
    ///
    /// Only a proposal can be approved. The lifecycle is a state machine and
    /// every one of these is a guard on it: without them `reopen_job` on a
    /// *rejected* job sets it approved, which reinstates a plan a person
    /// turned down and hands the next prompt a live job to run inside — the
    /// gate leaking through the message meant for unfolding a fold.
    pub fn approve_job(&mut self, id: JobId, by: ApprovedBy) -> bool {
        match self.job_mut(id) {
            Some(job) if job.state == JobState::Proposed => {
                job.approve(by);
                true
            }
            _ => false,
        }
    }

    pub fn approve_task(&mut self, id: TaskId, by: ApprovedBy) -> bool {
        self.approve_job(id, by)
    }

    /// Refuses a proposal. Nothing ran under it, so nothing folds; the job
    /// stays in the session as the record of what was turned down.
    pub fn reject_job(&mut self, id: JobId) -> bool {
        match self.job_mut(id) {
            Some(job) if job.state == JobState::Proposed => {
                job.reject();
                true
            }
            _ => false,
        }
    }

    pub fn reject_task(&mut self, id: TaskId) -> bool {
        self.reject_job(id)
    }

    /// Closes it because a person said so. The rung below
    /// [`Context::close_if_met`], and the only one there was until it existed.
    pub fn close_job(&mut self, id: JobId, counter: &dyn TokenCounter) -> Option<String> {
        self.close_job_by(id, counter, ClosedBy::User)
    }

    pub fn close_task(&mut self, id: TaskId, counter: &dyn TokenCounter) -> Option<String> {
        self.close_job(id, counter)
    }

    /// Closes the live job if its plan's `closes_on` has been met by its own
    /// steps, and returns what closed and what it folded to.
    ///
    /// Called after a turn is folded into the history, which is what makes the
    /// close see that turn's steps. Nothing happens for a job whose plan
    /// declares no closing condition, which is every job a model planned on
    /// its own — see `RECORD/2026-09-02.closing-on-an-exit-code.completed.md` for why
    /// the field arrives at the gate and never from the planning call.
    pub fn close_if_met(&mut self, counter: &dyn TokenCounter) -> Option<(JobId, String)> {
        let id = self.live_job()?;
        let mine = |turn: &&Turn| turn.job == Some(id);
        let steps: Vec<&ToolStep> = self
            .turns
            .iter()
            .filter(mine)
            .flat_map(|turn| turn.steps.iter())
            .collect();
        let job = self.jobs.iter().find(|job| job.id == id)?;
        if !job.plan.met_by(&steps) {
            return None;
        }
        let summary = self.close_job_by(id, counter, ClosedBy::ExitCode)?;
        Some((id, summary))
    }

    /// Closes it: from the next selection on, its turns render as one summary.
    ///
    /// The summary is written from the job's own tool steps and the fragments
    /// its turns were handed, so it is evidence rather than the model's account
    /// of itself. Returns the summary text, or `None` if there is no such job.
    pub fn close_job_by(
        &mut self,
        id: JobId,
        counter: &dyn TokenCounter,
        by: ClosedBy,
    ) -> Option<String> {
        let mine = |turn: &&Turn| turn.job == Some(id);
        let steps: Vec<&ToolStep> = self
            .turns
            .iter()
            .filter(mine)
            .flat_map(|turn| turn.steps.iter())
            .collect();
        // The field beside the one above, and the reason the fold lost answers
        // until now: what a job read was in hand at the close and was never
        // asked for. See `RECORD/2026-08-30.the-fold-probe-run.completed.md`.
        let shown: Vec<&Fragment> = self
            .turns
            .iter()
            .filter(mine)
            .flat_map(|turn| turn.code_context.iter())
            .collect();
        let turns = self.turns.iter().filter(mine).count();
        // What the fold is about to stop sending, counted here rather than
        // afterwards: the window moves, a resume can change the counter, and
        // eviction may already have taken some of the same turns, so this
        // number is only true at the close. `tokens_of` is what the `history`
        // bucket sums, which is the bar the saving will be read against. See
        // `RECORD/2026-09-08.what-a-fold-writes-down.completed.md`.
        let replaced = Replaced {
            turns: self.turns.iter().filter(mine).map(|turn| turn.id).collect(),
            tokens: self
                .turns
                .iter()
                .filter(mine)
                .map(|turn| self.tokens_of(turn, counter))
                .sum(),
        };
        let job = self.jobs.iter_mut().find(|job| job.id == id)?;
        // Only an open job folds. A proposal has no turns to fold and nothing
        // has been approved to summarise; closing one would take the gate off
        // the screen with its prompt still held, and the session would be stuck
        // with no way to answer a proposal nobody can see.
        if !job.is_open() {
            return None;
        }
        job.close(&steps, &shown, turns, counter, by, Some(replaced));
        job.summary.as_ref().map(|summary| summary.text.clone())
    }

    /// What the fold of a closed job replaced, for the message that announces
    /// it. Read back rather than returned by the close: the close's answer is
    /// the summary a person reads, and threading a second value through four
    /// call sites to save one lookup is how a signature grows.
    pub fn replaced_by(&self, id: JobId) -> Option<Replaced> {
        self.job(id)
            .and_then(|job| job.summary.as_ref())
            .and_then(|summary| summary.replaced.clone())
    }

    pub fn close_task_by(
        &mut self,
        id: TaskId,
        counter: &dyn TokenCounter,
        by: ClosedBy,
    ) -> Option<String> {
        self.close_job_by(id, counter, by)
    }

    /// Reopens it: the fold stops applying and its turns are sent verbatim
    /// again. Nothing is recovered, because nothing was deleted.
    pub fn reopen_job(&mut self, id: JobId) -> bool {
        match self.job_mut(id) {
            Some(job) if job.is_closed() => {
                job.reopen();
                true
            }
            _ => false,
        }
    }

    pub fn reopen_task(&mut self, id: TaskId) -> bool {
        self.reopen_job(id)
    }

    pub fn jobs(&self) -> &[Job] {
        &self.jobs
    }

    pub fn tasks(&self) -> &[Job] {
        &self.jobs
    }

    pub fn job(&self, id: JobId) -> Option<&Job> {
        self.jobs.iter().find(|job| job.id == id)
    }

    pub fn task(&self, id: TaskId) -> Option<&Job> {
        self.job(id)
    }

    fn job_mut(&mut self, id: JobId) -> Option<&mut Job> {
        self.jobs.iter_mut().find(|job| job.id == id)
    }

    /// The job turns are currently attributed to: the last one approved and
    /// not closed. One level, deliberately — see the jobs record.
    pub fn live_job(&self) -> Option<JobId> {
        self.jobs
            .iter()
            .rev()
            .find(|job| job.is_open())
            .map(|job| job.id)
    }

    pub fn live_task(&self) -> Option<TaskId> {
        self.live_job()
    }

    /// The history as it renders: live turns one by one, and each closed job
    /// as a single folded item.
    ///
    /// Built from the floor forward, so a job closed after part of it was
    /// evicted folds only from what the window still holds — except that the
    /// summary it folds to is the whole job's. That is deliberate and it is a
    /// cost: see `RECORD/2026-08-30.tasks-in-code.completed.md`.
    fn items(&self) -> Vec<Item> {
        self.items_from(self.floor)
    }

    /// The same, from a floor that is not (yet) the one in force — the
    /// eviction report needs the window as it stood before the cut, and that
    /// floor is gone by the time there is anything to report.
    fn items_from(&self, floor: usize) -> Vec<Item> {
        let mut items = Vec::new();
        let mut index = floor;
        while index < self.turns.len() {
            let folded = self.turns[index]
                .job
                .filter(|id| self.job(*id).is_some_and(Job::is_closed));
            match folded {
                Some(id) => {
                    let first = index;
                    while index < self.turns.len() && self.turns[index].job == Some(id) {
                        index += 1;
                    }
                    items.push(Item::Folded { job: id, first });
                }
                None => {
                    items.push(Item::Turn(index));
                    index += 1;
                }
            }
        }
        items
    }

    /// What an item costs in the prompt it renders into, under a prune line
    /// that may or may not be the one in force — deciding needs the cost of a
    /// line the context has not taken yet.
    ///
    /// A pruned turn is the stored count less the spans it will not send, plus
    /// the citations that stand in for them. Subtracted rather than recounted,
    /// which is rule A's rule for rule A's reason: a turn that gives up nothing
    /// costs exactly what it has always cost, and every recording made before
    /// this reads unchanged.
    fn item_tokens(&self, item: &Item, counter: &dyn TokenCounter, pruned: usize) -> u32 {
        match item {
            Item::Turn(index) => {
                let turn = &self.turns[*index];
                let stored = self.tokens_of(turn, counter);
                match *index < pruned {
                    false => stored,
                    true => {
                        let carried = turn.code_context.iter();
                        let full: u32 = carried
                            .clone()
                            .map(|fragment| fragment_tokens(fragment, counter))
                            .sum();
                        let cited: u32 = carried
                            .map(|fragment| pruned_fragment_tokens(fragment, counter))
                            .sum();
                        stored.saturating_sub(full) + cited
                    }
                }
            }
            Item::Folded { job, .. } => self
                .job(*job)
                .and_then(|job| {
                    job.summary.as_ref().map(|summary| {
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
        //
        // Under `Repeat::Once` this is the whole bucket and not what will be
        // sent: what the turn gives up to the history depends on where the
        // floor lands, and the floor is decided below using this number. The
        // fixed point is not worth solving, because the direction is the safe
        // one — the budget the cut is computed against is the same one it would
        // have had without the rule, so the cut is never deeper than the
        // default's and the prompt that goes out is never bigger than the one
        // it was sized for. The saving that pays is the history's, below.
        let code_tokens = counter.count(&fragments_text(code_context));
        let prompt_tokens = counter.count(prompt);

        let mut eviction = None;
        let mut pruning = None;
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

            // How deep the cut goes is the whole difference between the two
            // policies, and the prune line goes to the same place for the same
            // reason: a second constant would be that argument had again,
            // worse.
            let target = match budget.eviction {
                Eviction::Turn => available,
                Eviction::Block { low_water } => {
                    (available as f32 * low_water.clamp(0.0, 1.0)) as u32
                }
            };

            // Pruning first, because it is the cheaper half of the same trade:
            // an evicted turn loses its question and its answer with its code,
            // and a pruned one loses only the code. The floor moves after, and
            // only if giving up every span in the window still leaves it over.
            if budget.prune == Prune::Behind
                && self.fits_from(available, counter, budget.repeat, self.pruned) > self.floor
            {
                let before = self.pruned;
                let held = self.window_tokens(self.floor, counter, budget.repeat, before);
                self.pruned = self.prune_to(target, counter, budget.repeat);
                pruning = (self.pruned > before).then(|| Pruned {
                    turns: self.turns[before.max(self.floor)..self.pruned]
                        .iter()
                        .filter(|turn| !turn.code_context.is_empty())
                        .map(|turn| turn.id)
                        .collect(),
                    tokens: held.saturating_sub(self.window_tokens(
                        self.floor,
                        counter,
                        budget.repeat,
                        self.pruned,
                    )),
                    counter: counter.id(),
                });
            }

            if self.fits_from(available, counter, budget.repeat, self.pruned) > self.floor {
                // Taken before the floor moves: this is the window the cut
                // chooses from, and what it drops is unreachable afterwards —
                // which is the whole reason this has to be reported from in
                // here rather than reconstructed outside. Under the prune line
                // already in force, so that what pruning saved is not reported
                // as what the cut freed.
                let before = self.floor;
                let held = self.window_tokens(before, counter, budget.repeat, self.pruned);

                self.floor = self.fits_from(target, counter, budget.repeat, self.pruned);

                // What the cut freed, and not what the dropped turns were
                // counted at. Under `Repeat::Once` those are two different
                // numbers: a span the oldest turn owned passes to the next turn
                // that carries it rather than leaving with it, so summing the
                // items below the floor would report a saving the window did
                // not get. The difference of two windows is the saving.
                eviction = (self.floor > before).then(|| Evicted {
                    turns: self.turns[before..self.floor]
                        .iter()
                        .map(|turn| turn.id)
                        .collect(),
                    tokens: held.saturating_sub(self.window_tokens(
                        self.floor,
                        counter,
                        budget.repeat,
                        self.pruned,
                    )),
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
        // Oldest first, so the first turn that carries a span is the one that
        // renders it — and the turn being asked, which is rendered last, is the
        // only one that can lose a span to the history. That direction is what
        // keeps the prefix: the block above the newest message is byte-identical
        // to the one the previous call sent, exactly as it was before this rule.
        let mut shown: HashSet<&Fragment> = HashSet::new();
        for item in &items {
            let tokens = self.item_tokens(item, counter, self.pruned);
            match item {
                Item::Turn(index) if *index < self.pruned => {
                    // Below the prune line: the exchange stays, the code does
                    // not, and what stands where it stood is the line that
                    // cites it. `item_tokens` has already charged this turn for
                    // the citations, so there is nothing to subtract here — a
                    // pruned turn owns none of its spans and hands every one of
                    // them to the oldest turn above the line that carries it.
                    let turn = &self.turns[*index];
                    history_tokens += tokens;
                    messages.push(Message::user(pruned_user_text(
                        &turn.code_context,
                        &turn.prompt,
                        counter,
                    )));
                    for step in &turn.steps {
                        messages.push(Message::assistant(step.text.clone()));
                        messages.push(Message::user(step.result_text()));
                    }
                    messages.push(Message::assistant(turn.answer.clone()));
                }
                Item::Turn(index) => {
                    let turn = &self.turns[*index];
                    let (kept, dropped) =
                        split_shown(&turn.code_context, &mut shown, budget.repeat);
                    // The stored count, less what this render will not send.
                    // Subtracted rather than recounted so that a turn repeating
                    // nothing costs exactly what it has always cost, and every
                    // recording made before `Repeat` reads unchanged.
                    history_tokens += tokens.saturating_sub(fragments_tokens(&dropped, counter));
                    messages.push(Message::user(user_text(kept, &turn.prompt)));
                    // Each step is a real exchange, so the alternation holds and
                    // no chat template has to decide what two user messages in a
                    // row mean.
                    for step in &turn.steps {
                        messages.push(Message::assistant(step.text.clone()));
                        messages.push(Message::user(step.result_text()));
                    }
                    messages.push(Message::assistant(turn.answer.clone()));
                }
                Item::Folded { job, .. } => {
                    summary_tokens += tokens;
                    // One exchange, so the alternation a folded block sits in
                    // is the same one a turn would have left behind. The user
                    // half is the objective as it was approved: it is what was
                    // asked, and inventing a sentence for it would be prose in
                    // the one place this design keeps prose out of.
                    let Some(job) = self.job(*job) else {
                        continue;
                    };
                    let Some(summary) = &job.summary else {
                        continue;
                    };
                    messages.push(Message::user(job.objective.clone()));
                    messages.push(Message::assistant(summary.text.clone()));
                }
            }
        }
        // The turn being asked, last and youngest: what the history above it is
        // already showing, it does not show again. `code_tokens` above was
        // counted before the floor moved, so it is the whole bucket; this is
        // what of it is actually sent.
        let (kept, _) = split_shown(code_context, &mut shown, budget.repeat);
        let code_tokens = match kept.len() == code_context.len() {
            true => code_tokens,
            false => counter.count(&fragments_text(kept.iter().copied())),
        };
        messages.push(Message::user(user_text(kept, prompt)));

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
            pruned: self.pruned,
            eviction,
            pruning,
        }
    }

    /// The oldest turn that still fits, taking items newest first and never
    /// reaching past the floor. Returns an index into `turns`.
    ///
    /// Items, not turns: a folded task is kept or dropped whole, because half a
    /// summary is not a summary of half a task. So the floor only ever lands on
    /// an item boundary.
    ///
    /// Under [`Repeat::Once`] an item does not have a cost of its own: a turn
    /// added here becomes the **oldest** of the candidate window and takes
    /// ownership of its spans from whatever younger turn was rendering them, so
    /// what it adds is its own tokens less the copies it just made redundant.
    /// The two always come in pairs — a copy taken over is a copy this turn
    /// already carries — so the running total never goes down and the break is
    /// still a break.
    fn fits_from(
        &self,
        available: u32,
        counter: &dyn TokenCounter,
        repeat: Repeat,
        pruned: usize,
    ) -> usize {
        let available = i64::from(available);
        let mut spent: i64 = 0;
        let mut start = self.turns.len();
        // A fragment in here is one some turn in the candidate window is
        // already rendering. Folded turns never enter it: a fold sends its
        // summary and not its turns, so a span inside one is not on the screen
        // and cannot stand in for anything.
        let mut rendered: HashSet<&Fragment> = HashSet::new();
        for item in self.items().iter().rev() {
            let mut tokens = i64::from(self.item_tokens(item, counter, pruned));
            if let (Repeat::Once, Item::Turn(index)) = (repeat, item) {
                // A pruned turn is not rendering its spans, so it neither owns
                // one nor makes a younger copy redundant: `item_tokens` has
                // already charged it for citations and the copies above it
                // stay.
                if *index >= pruned {
                    for fragment in &self.turns[*index].code_context {
                        if !rendered.insert(fragment) {
                            tokens -= i64::from(fragment_tokens(fragment, counter));
                        }
                    }
                }
            }
            if spent + tokens > available {
                break;
            }
            spent += tokens;
            start = item.first();
        }
        start
    }

    /// What the window starting at `floor` costs, under the repeat rule in
    /// force.
    ///
    /// Forward, because ownership is oldest-first and a forward walk is where
    /// that is legible; [`Context::fits_from`] reads the same rule from the
    /// other end because it has to stop early. The two agree on the total and
    /// disagree about which turn is charged for a shared span, which is the
    /// difference between deciding and reporting.
    fn window_tokens(
        &self,
        floor: usize,
        counter: &dyn TokenCounter,
        repeat: Repeat,
        pruned: usize,
    ) -> u32 {
        let mut total: i64 = 0;
        let mut shown: HashSet<&Fragment> = HashSet::new();
        for item in &self.items_from(floor) {
            let mut tokens = i64::from(self.item_tokens(item, counter, pruned));
            if let (Repeat::Once, Item::Turn(index)) = (repeat, item)
                && *index >= pruned
            {
                for fragment in &self.turns[*index].code_context {
                    if !shown.insert(fragment) {
                        tokens -= i64::from(fragment_tokens(fragment, counter));
                    }
                }
            }
            total += tokens;
        }
        total.max(0) as u32
    }

    /// How far the prune line has to move to bring the window under `target`:
    /// the shallowest line that fits, or — where none does — the one that
    /// leaves the window smallest.
    ///
    /// Oldest first, one turn at a time, and the window recounted at each step
    /// rather than derived from what the turn carries. Deriving it would be
    /// wrong under [`Repeat::Once`]: the turn being pruned is the oldest one
    /// left, so it owns every span it carries, and what the window saves by
    /// pruning it is not the span but the span *less what a younger turn now
    /// has to render instead*.
    ///
    /// Which is why this takes a minimum rather than the first line it reaches.
    /// **Pruning is not monotone under rule A** — pruning the turn that owns a
    /// shared span hands the whole span to a younger turn and adds a citation
    /// where it stood, so the window can come out *bigger*. There the answer is
    /// to leave the line where it is: rule A has already taken that saving, and
    /// there is no second copy for rule B to find. See the 2026-09-08 addendum
    /// to `RECORD/2026-09-08.prune-behind.completed.md`.
    ///
    /// The loop runs on the call where the window overflowed and walks only the
    /// turns still in it.
    fn prune_to(&self, target: u32, counter: &dyn TokenCounter, repeat: Repeat) -> usize {
        // Never below the floor: a turn nobody renders has nothing to give up,
        // and starting here is what keeps the line moving forward as the floor
        // does.
        let start = self.pruned.max(self.floor);
        let mut best = start;
        let mut smallest = self.window_tokens(self.floor, counter, repeat, start);

        let mut line = start;
        while smallest > target && line < self.turns.len() {
            line += 1;
            let cost = self.window_tokens(self.floor, counter, repeat, line);
            if cost < smallest {
                best = line;
                smallest = cost;
            }
            // The shallowest line that fits, and not the deepest that would:
            // the line is a ratchet, so what it gives up now it gives up for
            // the rest of the session.
            if cost <= target {
                return line;
            }
        }
        best
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

/// One fragment, as it is rendered inside a user message.
fn fragment_text(fragment: &Fragment) -> String {
    format!("// {}\n{}\n\n", fragment.path, fragment.text)
}

/// What one fragment costs where it is rendered.
fn fragment_tokens(fragment: &Fragment, counter: &dyn TokenCounter) -> u32 {
    counter.count(&fragment_text(fragment))
}

fn fragments_tokens(fragments: &[&Fragment], counter: &dyn TokenCounter) -> u32 {
    fragments
        .iter()
        .map(|fragment| fragment_tokens(fragment, counter))
        .sum()
}

/// One fragment, as the line that stands in for it once its turn has been
/// pruned.
///
/// The path, the span and what it was costing — enough for the model to tell
/// that the file was read, so that reading it again is a decision rather than a
/// discovery, and not enough to be prose. A summary would be model text in a
/// region every later turn is built on, which is the shape the fold already
/// rejects.
///
/// Where the citation would cost more than the span it replaces — a two-line
/// fragment, say — the span stays: a citation that is not cheaper is not a
/// saving, and pretending otherwise would make the window's cost stop being
/// monotone in the prune line.
fn pruned_fragment_text(fragment: &Fragment, counter: &dyn TokenCounter) -> String {
    let citation = format!(
        "// {} — {} tokens, pruned\n\n",
        fragment.path,
        fragment_tokens(fragment, counter)
    );
    match counter.count(&citation) < fragment_tokens(fragment, counter) {
        true => citation,
        false => fragment_text(fragment),
    }
}

/// What one fragment costs where its turn has been pruned.
fn pruned_fragment_tokens(fragment: &Fragment, counter: &dyn TokenCounter) -> u32 {
    counter.count(&pruned_fragment_text(fragment, counter))
}

/// The user half of a pruned turn: its citations, then what was asked.
fn pruned_user_text(code_context: &[Fragment], prompt: &str, counter: &dyn TokenCounter) -> String {
    let mut text: String = code_context
        .iter()
        .map(|fragment| pruned_fragment_text(fragment, counter))
        .collect();
    text.push_str(prompt);
    text
}

/// What this turn renders, and what a turn already rendered in this same call
/// is showing for it.
///
/// `shown` carries across the whole render, which is the invariant the rule
/// rests on: a span is only ever skipped because something *else in this
/// prompt* has it. Under [`Repeat::Always`] nothing is recorded and nothing is
/// skipped, so a run made without the rule cannot be quietly changed by it.
fn split_shown<'a>(
    code_context: &'a [Fragment],
    shown: &mut HashSet<&'a Fragment>,
    repeat: Repeat,
) -> (Vec<&'a Fragment>, Vec<&'a Fragment>) {
    match repeat {
        Repeat::Always => (code_context.iter().collect(), Vec::new()),
        Repeat::Once => code_context
            .iter()
            .partition(|fragment| shown.insert(fragment)),
    }
}

/// The fragments alone, as they are rendered inside a user message.
fn fragments_text<'a>(code_context: impl IntoIterator<Item = &'a Fragment>) -> String {
    code_context.into_iter().map(fragment_text).collect()
}

/// The user half of a turn: its code, then what was asked.
///
/// Fused into one message rather than sent as two, because a standalone
/// context message leaves two `user` messages back to back and chat templates
/// disagree about what that means.
fn user_text<'a>(code_context: impl IntoIterator<Item = &'a Fragment>, prompt: &str) -> String {
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
        context.approve_task(task, ApprovedBy::Operator);
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

    /// The fold's own claim: which turns stopped being sent, and what they were
    /// worth in the prompt they left. Counted at the close because it is only
    /// true then — see
    /// `RECORD/2026-09-08.what-a-fold-writes-down.completed.md`.
    #[test]
    fn a_fold_writes_down_what_it_replaced() {
        let counter = WordCounter::default();
        let (context, task) = context_with_closed_task(0, &counter);

        let replaced = context.replaced_by(task).expect("the close counted it");
        assert_eq!(
            replaced.turns,
            vec![1, 2, 3],
            "named rather than counted: which turns stopped being sent",
        );

        let summary = context
            .job(task)
            .and_then(|job| job.summary.as_ref())
            .expect("a closed job has one");
        assert_eq!(
            summary.replaced.as_ref(),
            Some(&replaced),
            "the numbers live with the summary they are subtracted from",
        );
        assert!(
            replaced.tokens > summary.tokens,
            "three turns cost more than the line that stands in for them: \
             {} replaced by {}",
            replaced.tokens,
            summary.tokens,
        );

        // The same unit on both sides, which is the whole reason the count
        // happens at the close: `tokens_of` is what the `history` bucket sums.
        let by_hand: u32 = context
            .turns
            .iter()
            .filter(|turn| turn.job == Some(task))
            .map(|turn| context.tokens_of(turn, &counter))
            .sum();
        assert_eq!(replaced.tokens, by_hand);
    }

    /// A reopened job is being written again, so nothing has been saved and the
    /// numbers go where the summary goes.
    #[test]
    fn reopening_drops_what_the_fold_wrote_down() {
        let counter = WordCounter::default();
        let (mut context, task) = context_with_closed_task(0, &counter);
        assert!(context.replaced_by(task).is_some());

        context.reopen_task(task);

        assert!(context.replaced_by(task).is_none());
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
        context.approve_task(task, ApprovedBy::Operator);
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
        assert!(selection.messages[2].content.contains("[job closed]"));
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
        context.approve_task(task, ApprovedBy::Operator);
        context.push_turn(3, "inside the task", "c", vec![], &counter);

        let attributed: Vec<Option<JobId>> = context.turns().iter().map(|turn| turn.job).collect();
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

    // ---- Rule A: what a span already in the window costs to select again ----
    //
    // `RECORD/2026-09-06.what-leaves-the-history.completed.md`. The corpus these were
    // written against is `scripts/tasks/grounded.txt`, whose consecutive turns
    // select overlapping spans; these are the mechanism underneath it.

    fn fragment(path: &str, text: &str) -> Fragment {
        Fragment {
            path: path.into(),
            text: text.into(),
        }
    }

    /// A context whose turns each carry the fragments they were given.
    fn context_carrying(spans: &[&[Fragment]], counter: &dyn TokenCounter) -> Context {
        let mut context = Context::new("system prompt here");
        for (n, code) in spans.iter().enumerate() {
            context.push_turn(
                n as TurnId + 1,
                format!("question number {n} padded out"),
                format!("answer number {n} padded out"),
                code.to_vec(),
                counter,
            );
        }
        context
    }

    fn user_messages(selection: &Selection) -> Vec<&String> {
        selection
            .messages
            .iter()
            .filter(|m| m.role == Role::User)
            .map(|m| &m.content)
            .collect()
    }

    fn occurrences(selection: &Selection, needle: &str) -> usize {
        selection
            .messages
            .iter()
            .filter(|m| m.content.contains(needle))
            .count()
    }

    #[test]
    fn a_span_two_turns_carry_is_rendered_by_the_older_one() {
        let counter = WordCounter::default();
        let shared = fragment("src/lib.rs:1-4", "fn main () {}");
        let mut context = context_carrying(
            &[std::slice::from_ref(&shared), std::slice::from_ref(&shared)],
            &counter,
        );

        let selection = context.select(
            "now this",
            &[],
            Budget::new(8192, 0, Eviction::Turn).repeating(Repeat::Once),
            &counter,
        );

        assert_eq!(occurrences(&selection, "src/lib.rs:1-4"), 1);
        let users = user_messages(&selection);
        assert!(
            users[0].contains("src/lib.rs:1-4"),
            "the oldest turn keeps it, so the block above the newest message \
             does not move when a later turn selects it again: {users:?}",
        );
        assert!(!users[1].contains("src/lib.rs:1-4"));
    }

    #[test]
    fn the_default_sends_it_in_every_turn_that_selected_it() {
        let counter = WordCounter::default();
        let shared = fragment("src/lib.rs:1-4", "fn main () {}");
        let mut context = context_carrying(
            &[std::slice::from_ref(&shared), std::slice::from_ref(&shared)],
            &counter,
        );

        let selection = context.select(
            "now this",
            &[],
            Budget::new(8192, 0, Eviction::Turn),
            &counter,
        );

        assert_eq!(
            occurrences(&selection, "src/lib.rs:1-4"),
            2,
            "every recording on disk was made under this arm and must stay readable as it",
        );
    }

    #[test]
    fn the_turn_being_asked_is_the_one_that_gives_the_span_up() {
        let counter = WordCounter::default();
        let shared = fragment("src/lib.rs:1-4", "fn main () {}");
        let mut context = context_carrying(&[std::slice::from_ref(&shared)], &counter);

        let selection = context.select(
            "now this",
            std::slice::from_ref(&shared),
            Budget::new(8192, 0, Eviction::Turn).repeating(Repeat::Once),
            &counter,
        );

        assert_eq!(occurrences(&selection, "src/lib.rs:1-4"), 1);
        let last = selection.messages.last().unwrap();
        assert_eq!(
            last.content, "now this",
            "the youngest message is the one that changes anyway",
        );
        let code = selection.buckets.iter().find(|b| b.name == "code").unwrap();
        assert_eq!(code.tokens, 0, "the bucket has to say what was sent");
    }

    /// The failure this rule is one bad decision away from, and the reason the
    /// owner is chosen inside the window rather than over the session:
    /// `the-fold-fix-verified`, with a different mechanism.
    #[test]
    fn evicting_the_turn_that_owned_a_span_hands_it_to_the_next_one() {
        let counter = WordCounter::default();
        let shared = fragment("src/lib.rs:1-4", "fn main () {}");
        let mut context = context_carrying(&[std::slice::from_ref(&shared); 3], &counter);
        let repeat = Repeat::Once;

        // Wide enough for everything: turn 1 owns it.
        let first = context.select(
            "now this",
            &[],
            Budget::new(8192, 0, Eviction::Turn).repeating(repeat),
            &counter,
        );
        assert!(user_messages(&first)[0].contains("fn main () {}"));

        // Narrow enough to lose turn 1. The span must not go with it.
        let second = context.select(
            "now this",
            &[],
            Budget::new(26, 0, Eviction::Turn).repeating(repeat),
            &counter,
        );
        assert!(second.evicted > 0, "the point of the case");
        assert_eq!(
            occurrences(&second, "src/lib.rs:1-4"),
            1,
            "still exactly once — and now in a turn that is still here",
        );
        assert!(
            user_messages(&second)[0].contains("fn main () {}"),
            "a model reasoning over code it can no longer see is the failure \
             this whole rule is built around",
        );
    }

    #[test]
    fn a_span_whose_bytes_changed_is_not_the_span_it_replaces() {
        let counter = WordCounter::default();
        let mut context = context_carrying(
            &[&[fragment("src/lib.rs:1-4", "fn main () {}")][..]],
            &counter,
        );

        let selection = context.select(
            "now this",
            &[fragment("src/lib.rs:1-4", "fn main () { work () }")],
            Budget::new(8192, 0, Eviction::Turn).repeating(Repeat::Once),
            &counter,
        );

        assert_eq!(
            occurrences(&selection, "src/lib.rs:1-4"),
            2,
            "the same spec after an edit is a different claim — see \
             `resolving-a-symbol`, which is the live bug this would reintroduce",
        );
    }

    #[test]
    fn a_span_inside_a_folded_job_does_not_stand_in_for_one_on_the_screen() {
        let counter = WordCounter::default();
        let shared = fragment("src/lib.rs:1-4", "fn main () {}");
        let mut context = Context::new("system prompt here");
        let job = context.propose_job("read it", Plan::default());
        context.approve_job(job, ApprovedBy::Operator);
        context.push_turn(1, "look at this", "looked", vec![shared.clone()], &counter);
        context.close_job_by(job, &counter, ClosedBy::User);
        context.push_turn(2, "and again", "again", vec![shared.clone()], &counter);

        let selection = context.select(
            "now this",
            &[],
            Budget::new(8192, 0, Eviction::Turn).repeating(Repeat::Once),
            &counter,
        );

        assert!(
            user_messages(&selection)
                .iter()
                .any(|text| text.contains("fn main () {}")),
            "a folded job sends its summary, not its turns: nothing inside one \
             is on the screen to stand in for anything — {:?}",
            user_messages(&selection),
        );
    }

    /// The buckets are what the panel plots and what a recording stores. A rule
    /// that drops bytes from the prompt without moving the bar would make every
    /// number after it a fiction.
    #[test]
    fn the_buckets_describe_what_was_actually_sent() {
        let counter = WordCounter::default();
        let shared = fragment("src/lib.rs:1-4", "fn main () {}");
        let mut context = context_carrying(
            &[std::slice::from_ref(&shared), std::slice::from_ref(&shared)],
            &counter,
        );

        let with = context.select(
            "now this",
            std::slice::from_ref(&shared),
            Budget::new(8192, 0, Eviction::Turn).repeating(Repeat::Once),
            &counter,
        );
        let bucket = |selection: &Selection, name: &str| {
            selection
                .buckets
                .iter()
                .find(|b| b.name == name)
                .unwrap()
                .tokens
        };

        // The word counter is additive across a concatenation, so this is an
        // equality here and would be within a token or two under a real
        // tokenizer — the gap the `code`/`prompt` split already carries. What
        // it pins is the bookkeeping: bytes dropped from the prompt are dropped
        // from the bar.
        let rendered: u32 = with
            .messages
            .iter()
            .map(|m| counter.count(&m.content))
            .sum();
        let plotted = bucket(&with, "system")
            + bucket(&with, "tools")
            + bucket(&with, "map")
            + bucket(&with, "summaries")
            + bucket(&with, "history")
            + bucket(&with, "code")
            + bucket(&with, "prompt");
        assert_eq!(rendered, plotted, "{:?}", with.buckets);
    }

    /// The guarantee that makes the rule safe to turn on: the cut is computed
    /// against a history that costs less, so it is never deeper than the one
    /// the same budget would have made without it.
    #[test]
    fn the_rule_never_evicts_more_than_the_default_would() {
        let counter = WordCounter::default();
        let shared = fragment("src/lib.rs:1-4", "fn main () {}");
        let carried = [std::slice::from_ref(&shared); 4];
        let mut kept_more = 0;

        for limit in 20..90 {
            let budget = Budget::new(limit, 0, Eviction::Turn);
            let mut plain = context_carrying(&carried, &counter);
            let mut once = context_carrying(&carried, &counter);

            let plain = plain.select("now this", &[], budget, &counter);
            let once = once.select("now this", &[], budget.repeating(Repeat::Once), &counter);

            assert!(
                once.evicted <= plain.evicted,
                "at limit {limit}: {} evicted with the rule against {} without it",
                once.evicted,
                plain.evicted,
            );
            kept_more += usize::from(once.evicted < plain.evicted);
        }

        assert!(
            kept_more > 0,
            "never worse is only half of it — a range of windows where the rule \
             changes nothing is a range that proves nothing",
        );
    }

    /// Rule A's whole claim against rule B: the history block does not move, so
    /// a prefix cache reuses everything above the newest message.
    #[test]
    fn a_later_repeat_does_not_rewrite_the_block_above_it() {
        let counter = WordCounter::default();
        let shared = fragment("src/lib.rs:1-4", "fn main () {}");
        let mut context = context_carrying(&[std::slice::from_ref(&shared)], &counter);
        let budget = Budget::new(8192, 0, Eviction::Turn).repeating(Repeat::Once);

        let before = context.select("first", &[], budget, &counter);
        // The next turn selects the same span, and answers.
        context.push_turn(2, "second", "answered", vec![shared.clone()], &counter);
        let after = context.select("third", &[], budget, &counter);

        assert_eq!(
            before.messages[..before.messages.len() - 1],
            after.messages[..before.messages.len() - 1],
            "everything the previous call sent above its own prompt is byte-identical",
        );
    }

    /// Rule B, and the sentence the whole record is: the exchange survives the
    /// cut its quoted code does not.
    #[test]
    fn a_pruned_turn_keeps_its_exchange_and_gives_up_its_code() {
        let counter = WordCounter::default();
        let span = fragment(
            "src/lib.rs:1-9",
            "fn main () { let a = one two three four }",
        );
        let mut context = context_carrying(
            &[
                std::slice::from_ref(&span),
                std::slice::from_ref(&span),
                &[],
            ],
            &counter,
        );

        // Narrow enough that the window does not fit, wide enough that the
        // conversation does once the code is a citation — which is the whole
        // trade: nothing is evicted here.
        let budget = Budget::new(50, 0, Eviction::Turn).pruning(Prune::Behind);
        let selection = context.select("now this", &[], budget, &counter);

        assert!(selection.pruned > 0, "the point of the case");
        assert_eq!(selection.evicted, 0, "and nothing had to be dropped whole");
        let users = user_messages(&selection);
        assert!(
            users[0].contains("src/lib.rs:1-9") && !users[0].contains("fn main"),
            "the citation stands where the span stood: {users:?}",
        );
        assert!(
            users[0].contains("question number 0"),
            "and the question is still being asked: {users:?}",
        );
        assert!(
            selection
                .messages
                .iter()
                .any(|m| m.content.contains("answer number 0")),
            "with the answer it got",
        );
    }

    /// Pruning is the cheaper half of the same trade, so it goes first: a
    /// window that fits once the old code is a citation loses no turn at all.
    #[test]
    fn pruning_comes_before_eviction() {
        let counter = WordCounter::default();
        let span = fragment(
            "src/lib.rs:1-9",
            "fn main () { let a = one two three four }",
        );
        let carried: Vec<&[Fragment]> = vec![std::slice::from_ref(&span); 3];

        let plain = Budget::new(40, 0, Eviction::Turn);
        let mut without = context_carrying(&carried, &counter);
        let without = without.select("now this", &[], plain, &counter);

        let mut with = context_carrying(&carried, &counter);
        let with = with.select("now this", &[], plain.pruning(Prune::Behind), &counter);

        assert!(without.evicted > 0, "the arm this is measured against");
        assert!(
            with.evicted < without.evicted,
            "a turn that gave up its code did not have to be dropped: {} against {}",
            with.evicted,
            without.evicted,
        );
    }

    /// A ratchet, for the reason the floor is one: a line that moved back would
    /// rewrite the history block twice and leave no prefix to reuse.
    #[test]
    fn the_prune_line_only_moves_forward() {
        let counter = WordCounter::default();
        let span = fragment(
            "src/lib.rs:1-9",
            "fn main () { let a = one two three four }",
        );
        let carried: Vec<&[Fragment]> = vec![std::slice::from_ref(&span); 3];
        let mut context = context_carrying(&carried, &counter);

        let narrow = context.select(
            "now this",
            &[],
            Budget::new(40, 0, Eviction::Turn).pruning(Prune::Behind),
            &counter,
        );
        assert!(narrow.pruned > 0, "the point of the case");

        let wide = context.select(
            "now this",
            &[],
            Budget::new(8192, 0, Eviction::Turn).pruning(Prune::Behind),
            &counter,
        );
        assert_eq!(
            wide.pruned, narrow.pruned,
            "room appearing again does not put the code back",
        );
    }

    #[test]
    fn the_default_prunes_nothing() {
        let counter = WordCounter::default();
        let span = fragment(
            "src/lib.rs:1-9",
            "fn main () { let a = one two three four }",
        );
        let carried: Vec<&[Fragment]> = vec![std::slice::from_ref(&span); 3];
        let mut context = context_carrying(&carried, &counter);

        let selection = context.select(
            "now this",
            &[],
            Budget::new(40, 0, Eviction::Turn),
            &counter,
        );

        assert_eq!(
            selection.pruned, 0,
            "every recording on disk was made under this arm and must stay readable as it",
        );
        assert_eq!(occurrences(&selection, "pruned"), 0);
    }

    /// The guard that keeps the window's cost monotone in the prune line: a
    /// citation is only taken where it is cheaper than what it replaces.
    #[test]
    fn a_citation_that_costs_more_than_the_span_is_not_taken() {
        let counter = WordCounter::default();
        let tiny = fragment("src/lib.rs:1-1", "ok");
        let mut context = context_carrying(
            &[std::slice::from_ref(&tiny), std::slice::from_ref(&tiny)],
            &counter,
        );

        let selection = context.select(
            "now this",
            &[],
            Budget::new(20, 0, Eviction::Turn).pruning(Prune::Behind),
            &counter,
        );

        let users = user_messages(&selection);
        assert!(
            users[0].contains("ok"),
            "two words do not get cheaper by being cited: {users:?}",
        );
    }

    /// §4 of the record, and the part most likely to be got wrong quietly: a
    /// render that sends less than it counted evicts on tokens nobody sent.
    #[test]
    fn the_history_bucket_says_what_a_pruned_window_actually_sent() {
        let counter = WordCounter::default();
        let span = fragment(
            "src/lib.rs:1-9",
            "fn main () { let a = one two three four }",
        );
        let carried: Vec<&[Fragment]> = vec![std::slice::from_ref(&span); 3];
        let mut context = context_carrying(&carried, &counter);

        let selection = context.select(
            "now this",
            &[],
            Budget::new(40, 0, Eviction::Turn).pruning(Prune::Behind),
            &counter,
        );
        assert!(selection.pruned > 0, "the point of the case");

        // Everything but the system block and the turn being asked, which are
        // their own buckets.
        let history: u32 = selection.messages[1..selection.messages.len() - 1]
            .iter()
            .map(|m| counter.count(&m.content))
            .sum();
        let bucket = selection
            .buckets
            .iter()
            .find(|b| b.name == "history")
            .unwrap();
        assert_eq!(
            bucket.tokens, history,
            "the bar has to be a count of the prompt that went out",
        );
    }

    /// A and B are independent flags whose combination is defined rather than
    /// accidental — and the definition is not the one the record predicted.
    /// Where rule A has already reduced a span to one copy, pruning the turn
    /// that holds it hands the copy to a younger turn and adds a citation where
    /// it stood, which is bigger. So B declines, and the span is still rendered
    /// exactly once, by a turn that is still there.
    #[test]
    fn pruning_declines_where_rule_a_already_took_the_saving() {
        let counter = WordCounter::default();
        let span = fragment(
            "src/lib.rs:1-9",
            "fn main () { let a = one two three four }",
        );
        let carried: Vec<&[Fragment]> = vec![std::slice::from_ref(&span); 3];
        let mut context = context_carrying(&carried, &counter);

        let both = Budget::new(40, 0, Eviction::Turn)
            .repeating(Repeat::Once)
            .pruning(Prune::Behind);
        let selection = context.select("now this", &[], both, &counter);

        assert_eq!(
            selection.pruned, 0,
            "there is no second copy left for B to find",
        );
        let users = user_messages(&selection);
        let rendering = users.iter().filter(|user| user.contains("fn main")).count();
        assert_eq!(
            rendering, 1,
            "and the one copy is in a turn that is still here: {users:?}",
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

    /// A step that ran a command and came back with an exit code, which is the
    /// only kind `close_if_met` looks at.
    fn ran(command: &str, args: &[&str], exit_code: Option<i32>) -> ToolStep {
        let mut step = step("run_command", "running it", "");
        step.call.arguments = serde_json::json!({"command": command, "args": args});
        step.outcome.command = Some(crate::tools::CommandResult {
            exit_code,
            signal: None,
            stdout: String::new(),
            stderr: String::new(),
            duration_ms: 1,
        });
        step
    }

    /// An open task whose plan closes on `cargo test`, and one turn that ran
    /// the command with the exit code given.
    fn task_that_ran(exit_code: Option<i32>) -> (Context, TaskId) {
        let counter = ApproximateCounter;
        let mut context = Context::new("system");
        let task = context.propose_task(
            "make the tests pass",
            Plan {
                commands: vec!["cargo".into()],
                closes_on: Some("cargo test".into()),
                ..Plan::default()
            },
        );
        context.approve_task(task, ApprovedBy::Operator);
        context.push_turn_with_steps(
            1,
            "fix the failing test",
            "Fixed.",
            vec![],
            vec![ran("cargo", &["test"], exit_code)],
            &counter,
        );
        (context, task)
    }

    #[test]
    fn a_green_run_closes_the_task_and_says_which_authority_did() {
        let (mut context, task) = task_that_ran(Some(0));
        let closed = context.close_if_met(&ApproximateCounter);

        assert_eq!(closed.map(|(id, _)| id), Some(task));
        let task = context.task(task).expect("the task");
        assert!(task.is_closed());
        assert_eq!(
            task.closed_by,
            Some(ClosedBy::ExitCode),
            "a recording where both closes look the same cannot count either",
        );
    }

    #[test]
    fn a_red_run_leaves_it_open_and_leaves_the_person_the_authority() {
        let (mut context, task) = task_that_ran(Some(1));
        assert!(context.close_if_met(&ApproximateCounter).is_none());
        assert!(context.task(task).expect("the task").is_open());

        // And the rung below still works on it, which is the whole point of the
        // condition being opt-in: nothing was taken away from the person.
        assert!(context.close_task(task, &ApproximateCounter).is_some());
        assert_eq!(
            context.task(task).expect("the task").closed_by,
            Some(ClosedBy::User),
        );
    }

    #[test]
    fn another_tasks_green_run_is_not_this_ones_evidence() {
        let counter = ApproximateCounter;
        let (mut context, first) = task_that_ran(Some(0));
        context.close_if_met(&counter);

        let second = context.propose_task(
            "something else",
            Plan {
                commands: vec!["cargo".into()],
                closes_on: Some("cargo test".into()),
                ..Plan::default()
            },
        );
        context.approve_task(second, ApprovedBy::Operator);
        context.push_turn(2, "and now this", "Looking.", vec![], &counter);

        assert!(
            context.close_if_met(&counter).is_none(),
            "the first task's steps closed the first task and nothing else",
        );
        assert!(context.task(first).expect("the task").is_closed());
        assert!(context.task(second).expect("the task").is_open());
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
        assert_eq!(context.task(task).unwrap().state, JobState::Proposed);

        assert!(context.approve_task(task, ApprovedBy::Operator));
        assert!(
            !context.approve_task(task, ApprovedBy::Operator),
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
        assert_eq!(context.task(task).unwrap().state, JobState::Approved);
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
        assert_eq!(context.task(task).unwrap().state, JobState::Rejected);
        assert!(context.live_task().is_none(), "nothing is live");
    }
}
