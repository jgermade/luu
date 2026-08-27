//! A session is a sequence of tasks, not a mode the agent is in.
//!
//! ```text
//! the user asks for something
//!   → the agent proposes a plan: steps, files it will touch, commands it runs
//!   → CONFIRMATION                  ← nothing runs before this
//!   → loop: act · check
//!   → close: the task is summarised; its turns stop being sent verbatim
//! ```
//!
//! A task owns no turns. It is bookkeeping — a plan, a state, and where in the
//! history it began — and everything it produces enters the history as an
//! ordinary [`crate::context::Turn`], tagged with a [`crate::context::TurnKind`].
//! A second history beside the real one disagrees with it the first time
//! something is evicted.
//!
//! See `RECORD/2026-08-27.tasks-instead-of-modes.md` for why the boundary
//! exists and `RECORD/2026-08-27.tasks-in-the-core.md` for this shape.

use serde::{Deserialize, Serialize};

use crate::context::{Context, Fold, TokenCounter};
use crate::protocol::TurnId;
use crate::tools::{ToolStep, fenced};

/// Tasks are numbered per session, in order, starting at 1.
pub type TaskId = u64;

/// What the agent proposes and the user approves. Write-once: a plan is never
/// edited, because it is the thing that was agreed to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    /// Verbatim, as the user typed it. Everything else in here is the model's.
    pub objective: String,
    pub steps: Vec<String>,
    /// Files the plan says it will touch, and programs it says it will run.
    ///
    /// **Declared, not enforced.** The approved plan is meant to become the
    /// task's `SandboxPolicy`, and cannot yet: `Sandbox::new` needs every
    /// granted path to exist (Landlock takes a descriptor per root) and the
    /// most ordinary claim a plan makes is that it will create a file. Until
    /// that is answered these are what the confirmation shows, and the session
    /// policy is what holds. See the record.
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
    /// False when the model did not answer in the block it was asked for and
    /// its own prose became the steps. Carried so a client can say so: the user
    /// is about to approve this, and how much of it is structured is part of
    /// what they are approving.
    #[serde(default)]
    pub parsed: bool,
}

impl Plan {
    /// Reads a plan out of what the model answered.
    ///
    /// The fenced form first — same parse as a tool call, so the mock backend
    /// scripts it and the lifecycle runs end to end without a model. When that
    /// fails, **the model's own lines become the steps**: a plan with our
    /// sentences in it would be a confirmation of something nobody proposed.
    pub fn from_reply(objective: impl Into<String>, text: &str) -> Self {
        #[derive(Deserialize)]
        struct Proposed {
            #[serde(default)]
            steps: Vec<String>,
            #[serde(default)]
            paths: Vec<String>,
            #[serde(default)]
            commands: Vec<String>,
        }

        let objective = objective.into();
        let proposed = fenced(text, "```plan")
            .and_then(|body| serde_json::from_str::<Proposed>(body).ok())
            .filter(|plan| !plan.steps.is_empty());

        match proposed {
            Some(plan) => Self {
                objective,
                steps: plan.steps,
                paths: plan.paths,
                commands: plan.commands,
                parsed: true,
            },
            None => Self {
                objective,
                steps: prose_steps(text),
                paths: Vec::new(),
                commands: Vec::new(),
                parsed: false,
            },
        }
    }

    /// How the approved plan reads in the history, as the answer half of its
    /// turn. The model proposed it and the user approved it, so it is the
    /// assistant's line — and the alternation the prompt shape depends on holds
    /// without a message type nothing else has.
    pub fn render(&self) -> String {
        let mut text = String::from("plan:");
        for (index, step) in self.steps.iter().enumerate() {
            text.push_str(&format!("\n  {}. {step}", index + 1));
        }
        if !self.paths.is_empty() {
            text.push_str(&format!("\nfiles: {}", self.paths.join(", ")));
        }
        if !self.commands.is_empty() {
            text.push_str(&format!("\ncommands: {}", self.commands.join(", ")));
        }
        text
    }
}

/// The lines of an answer that was not a plan block, as steps.
///
/// Bullets and numbering are stripped so the rendering is ours; the words are
/// the model's.
fn prose_steps(text: &str) -> Vec<String> {
    text.lines()
        .map(|line| {
            line.trim()
                .trim_start_matches(['-', '*', '#', '>'])
                .trim_start_matches(|c: char| c.is_ascii_digit())
                .trim_start_matches(['.', ')'])
                .trim()
        })
        .filter(|line| !line.is_empty() && !line.starts_with("```"))
        .map(str::to_string)
        .collect()
}

/// The user message that asks for a plan.
///
/// An ordinary user message on top of the unchanged prefix, so proposing a task
/// costs no prompt cache. Short on purpose: a 7B pays for every token of it,
/// once per task.
pub fn plan_request(objective: &str) -> String {
    format!(
        "Propose a plan for the objective below before doing any of it. \
         Answer with one ```plan block and nothing else:\n\n\
         ```plan\n\
         {{\"steps\":[\"what you will do, in order\"],\
         \"paths\":[\"files you will touch\"],\
         \"commands\":[\"programs you will run\"]}}\n\
         ```\n\n\
         OBJECTIVE: {objective}"
    )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Proposed and waiting. **Nothing runs in this state** — that is the whole
    /// gate, and it is one comparison rather than a mode anyone selects.
    Proposed,
    Approved,
    Closed,
    /// The user read the plan and said no. Kept rather than deleted: a plan
    /// that was refused is the most useful thing in the transcript.
    Discarded,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    pub plan: Plan,
    pub state: TaskState,
    /// Where this task's history begins — the index of its plan turn.
    ///
    /// Here rather than in a second structure beside the context, because two
    /// records of the same span disagree the first time one of them is wrong.
    /// Meaningless until the task is approved, which is when the plan is
    /// written and the span starts existing.
    pub history_from: usize,
    /// The turns run inside it, in order.
    #[serde(default)]
    pub turns: Vec<TurnId>,
    pub summary: Option<String>,
}

/// What the steps of a task actually did, gathered from the history rather than
/// from the model's account of it.
///
/// The distinction is the whole reason this type exists: a summary written from
/// the model's own narrative inherits its hallucinations, at twice the price.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Evidence {
    /// The exchanges the task ran. Its plan is not one of them: the plan is
    /// what was agreed to, not work that happened.
    pub turns: usize,
    /// Paths a tool wrote to, first mention first.
    pub touched: Vec<String>,
    pub read: Vec<String>,
    pub commands: Vec<CommandRun>,
    /// Calls the sandbox refused. In the summary because a task that was
    /// refused three times and one that was not are different tasks, and the
    /// difference is what stops a later turn trying the same thing again.
    pub denied: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CommandRun {
    pub command: String,
    /// `ok`, or what the tool said went wrong — the exit line, verbatim.
    pub result: String,
}

impl Evidence {
    /// Reads the evidence off the turns themselves.
    pub fn gather<'a>(turns: impl IntoIterator<Item = &'a crate::context::Turn>) -> Self {
        let mut evidence = Self::default();
        for turn in turns {
            if turn.kind == crate::context::TurnKind::Exchange {
                evidence.turns += 1;
            }
            for step in &turn.steps {
                evidence.add(step);
            }
        }
        evidence
    }

    fn add(&mut self, step: &ToolStep) {
        if !step.outcome.verdict.allowed {
            self.denied += 1;
            return;
        }
        let argument = |key: &str| {
            step.call.arguments[key]
                .as_str()
                .map(str::to_string)
                .unwrap_or_default()
        };
        let result = match &step.outcome.error {
            Some(error) => error.clone(),
            None => "ok".to_string(),
        };
        match step.call.name.as_str() {
            "write_file" | "edit_file" => push_once(&mut self.touched, argument("path")),
            "read_file" | "list_dir" => push_once(&mut self.read, argument("path")),
            "run_command" => {
                let mut command = argument("command");
                let arguments = step.call.arguments["args"]
                    .as_array()
                    .map(|args| {
                        args.iter()
                            .filter_map(|a| a.as_str())
                            .collect::<Vec<_>>()
                            .join(" ")
                    })
                    .unwrap_or_default();
                if !arguments.is_empty() {
                    command.push(' ');
                    command.push_str(&arguments);
                }
                self.commands.push(CommandRun { command, result });
            }
            _ => {}
        }
    }
}

fn push_once(list: &mut Vec<String>, value: String) {
    if !value.is_empty() && !list.contains(&value) {
        list.push(value);
    }
}

/// What a closed task leaves behind.
///
/// Derived, not generated: no model call, so closing cannot fail because
/// generation did, and the same session summarises the same way twice. Model
/// prose on top is a later, measured addition — this text lands in the
/// write-once region every later turn is built on.
pub fn summarize(task: &Task, evidence: &Evidence) -> String {
    let mut text = format!("[task {}] {} — closed", task.id, task.plan.objective);
    if !task.plan.steps.is_empty() {
        text.push_str("\nplan:");
        for (index, step) in task.plan.steps.iter().enumerate() {
            text.push_str(&format!("\n  {}. {step}", index + 1));
        }
    }
    if !evidence.touched.is_empty() {
        text.push_str(&format!("\ntouched: {}", evidence.touched.join(", ")));
    }
    if !evidence.read.is_empty() {
        text.push_str(&format!("\nread: {}", evidence.read.join(", ")));
    }
    for (index, run) in evidence.commands.iter().enumerate() {
        let label = match index {
            0 => "\ncommands: ",
            _ => "\n          ",
        };
        text.push_str(&format!("{label}{} → {}", run.command, run.result));
    }
    if evidence.denied > 0 {
        text.push_str(&format!("\ndenied: {} call(s)", evidence.denied));
    }
    text.push_str(&format!("\nfolded {} turn(s)", evidence.turns));
    text
}

/// A task, closed: what it left in the history and what the fold cost.
#[derive(Debug, Clone)]
pub struct Closed {
    pub summary: String,
    pub fold: Fold,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TaskError {
    #[error("no task {0}")]
    NoSuchTask(TaskId),
    #[error("task {id} is {state}, not {wanted}")]
    WrongState {
        id: TaskId,
        state: &'static str,
        wanted: &'static str,
    },
}

impl TaskState {
    fn as_str(self) -> &'static str {
        match self {
            Self::Proposed => "waiting for approval",
            Self::Approved => "running",
            Self::Closed => "closed",
            Self::Discarded => "discarded",
        }
    }
}

/// The session's tasks and the state machine over them.
///
/// In `agent-core` and not in the CLI for the same reason the prompt assembly
/// is: two call sites deciding when a prompt may run is two gates.
#[derive(Debug, Default)]
pub struct Tasks {
    next: TaskId,
    all: Vec<Task>,
}

impl Tasks {
    /// Proposes one. It runs nothing: the state is [`TaskState::Proposed`]
    /// until somebody approves it.
    pub fn propose(&mut self, plan: Plan) -> TaskId {
        self.next += 1;
        self.all.push(Task {
            id: self.next,
            plan,
            state: TaskState::Proposed,
            history_from: 0,
            turns: Vec::new(),
            summary: None,
        });
        self.next
    }

    pub fn all(&self) -> &[Task] {
        &self.all
    }

    pub fn get(&self, id: TaskId) -> Option<&Task> {
        self.all.iter().find(|task| task.id == id)
    }

    fn get_mut(&mut self, id: TaskId) -> Result<&mut Task, TaskError> {
        self.all
            .iter_mut()
            .find(|task| task.id == id)
            .ok_or(TaskError::NoSuchTask(id))
    }

    /// The one waiting for approval, if any.
    pub fn pending(&self) -> Option<&Task> {
        self.all
            .iter()
            .find(|task| task.state == TaskState::Proposed)
    }

    pub fn active(&self) -> Option<&Task> {
        self.all
            .iter()
            .find(|task| task.state == TaskState::Approved)
    }

    /// May a prompt run right now? `Err` is the refusal, in the words a client
    /// shows: a gate that silently swallows the message is one nobody trusts
    /// twice.
    pub fn gate(&self) -> Result<(), String> {
        match self.pending() {
            Some(task) => Err(format!(
                "task {} is waiting to be approved — approve or discard it first",
                task.id
            )),
            None => Ok(()),
        }
    }

    /// May a task be proposed right now? One at a time: a session is a
    /// *sequence* of tasks, and two open at once is the matrix the whole plan
    /// exists to avoid.
    pub fn may_propose(&self) -> Result<(), String> {
        if let Some(task) = self.pending() {
            return Err(format!(
                "task {} is already waiting to be approved",
                task.id
            ));
        }
        if let Some(task) = self.active() {
            return Err(format!("task {} is still open — close it first", task.id));
        }
        Ok(())
    }

    /// Approves a plan and writes it into the history, which is where the
    /// task's span starts.
    pub fn approve(
        &mut self,
        id: TaskId,
        context: &mut Context,
        counter: &dyn TokenCounter,
    ) -> Result<(), TaskError> {
        let task = self.get_mut(id)?;
        if task.state != TaskState::Proposed {
            return Err(TaskError::WrongState {
                id,
                state: task.state.as_str(),
                wanted: "waiting for approval",
            });
        }
        task.history_from = context.push_plan(id, &task.plan, counter);
        task.state = TaskState::Approved;
        Ok(())
    }

    pub fn discard(&mut self, id: TaskId) -> Result<(), TaskError> {
        let task = self.get_mut(id)?;
        if task.state != TaskState::Proposed {
            return Err(TaskError::WrongState {
                id,
                state: task.state.as_str(),
                wanted: "waiting for approval",
            });
        }
        task.state = TaskState::Discarded;
        Ok(())
    }

    /// Notes that a turn ran inside the open task, if one is open.
    pub fn record_turn(&mut self, turn: TurnId) {
        if let Some(task) = self
            .all
            .iter_mut()
            .find(|task| task.state == TaskState::Approved)
        {
            task.turns.push(turn);
        }
    }

    /// Closes it: gathers the evidence, writes the summary, folds the history.
    ///
    /// The three happen here rather than at each caller because a summary
    /// written from one span and folded over another is a lie nobody would
    /// catch.
    pub fn close(
        &mut self,
        id: TaskId,
        context: &mut Context,
        counter: &dyn TokenCounter,
    ) -> Result<Closed, TaskError> {
        let task = self.get_mut(id)?;
        if task.state != TaskState::Approved {
            return Err(TaskError::WrongState {
                id,
                state: task.state.as_str(),
                wanted: "running",
            });
        }

        let from = task.history_from;
        let evidence = Evidence::gather(context.turns().iter().skip(from));
        let summary = summarize(task, &evidence);
        let fold = context.close_task(id, from, &task.plan.objective, &summary, counter);

        task.state = TaskState::Closed;
        task.summary = Some(summary.clone());
        Ok(Closed { summary, fold })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::{ApproximateCounter, Fragment};
    use crate::sandbox::{Applied, Verdict};
    use crate::tools::{ToolCall, ToolOutcome};

    fn step(name: &str, arguments: serde_json::Value, outcome: ToolOutcome) -> ToolStep {
        ToolStep {
            text: String::new(),
            call: ToolCall {
                name: name.into(),
                arguments,
            },
            outcome,
            duration_ms: 1,
        }
    }

    fn allowed() -> Verdict {
        Verdict::allow("./ (read-write)", Applied::Process)
    }

    #[test]
    fn a_fenced_plan_parses_into_its_parts() {
        let plan = Plan::from_reply(
            "rename the counter",
            "Sure.\n\n```plan\n{\"steps\":[\"read context.rs\",\"rename it\"],\
             \"paths\":[\"src/context.rs\"],\"commands\":[\"cargo\"]}\n```\n",
        );
        assert!(plan.parsed);
        assert_eq!(plan.steps, ["read context.rs", "rename it"]);
        assert_eq!(plan.paths, ["src/context.rs"]);
        assert_eq!(plan.objective, "rename the counter", "kept verbatim");
    }

    #[test]
    fn a_plan_that_is_not_a_block_keeps_the_models_own_words() {
        // The alternative is inventing steps on its behalf and asking someone
        // to approve them.
        let plan = Plan::from_reply("do the thing", "1. read the file\n- then edit it\n");
        assert!(!plan.parsed);
        assert_eq!(plan.steps, ["read the file", "then edit it"]);
        assert!(
            plan.paths.is_empty(),
            "nothing is claimed that was not said"
        );
    }

    #[test]
    fn evidence_comes_from_the_steps_and_not_from_what_the_model_said() {
        let mut context = Context::new("sys");
        let counter = ApproximateCounter;
        context.push_turn_with_steps(
            "do it",
            "I rewrote everything and all tests pass.", // the narrative
            Vec::<Fragment>::new(),
            vec![
                step(
                    "write_file",
                    serde_json::json!({"path": "src/lib.rs"}),
                    ToolOutcome::ok(allowed(), ""),
                ),
                step(
                    "run_command",
                    serde_json::json!({"command": "cargo", "args": ["test"]}),
                    ToolOutcome::failed(allowed(), "cargo exited with 101"),
                ),
                step(
                    "read_file",
                    serde_json::json!({"path": "/etc/hostname"}),
                    ToolOutcome::denied(Verdict::deny("no rule grants read")),
                ),
            ],
            &counter,
        );

        let evidence = Evidence::gather(context.turns());
        assert_eq!(evidence.touched, ["src/lib.rs"]);
        assert_eq!(evidence.commands[0].command, "cargo test");
        assert_eq!(evidence.commands[0].result, "cargo exited with 101");
        assert_eq!(evidence.denied, 1);

        let task = Task {
            id: 1,
            plan: Plan::from_reply("do it", "```plan\n{\"steps\":[\"edit lib.rs\"]}\n```"),
            state: TaskState::Approved,
            history_from: 0,
            turns: vec![1],
            summary: None,
        };
        let summary = summarize(&task, &evidence);
        assert!(
            summary.contains("cargo test → cargo exited with 101"),
            "{summary}"
        );
        assert!(summary.contains("denied: 1"), "{summary}");
        assert!(
            !summary.contains("all tests pass"),
            "the model's account of its own work is not evidence: {summary}",
        );
        assert_eq!(
            summary,
            summarize(&task, &evidence),
            "and it is deterministic"
        );
    }

    #[test]
    fn nothing_runs_while_a_plan_is_waiting() {
        let mut tasks = Tasks::default();
        let mut context = Context::new("sys");
        let counter = ApproximateCounter;

        let id = tasks.propose(Plan::from_reply("x", "```plan\n{\"steps\":[\"a\"]}\n```"));
        assert!(tasks.gate().is_err(), "the gate is the whole point");
        assert!(tasks.may_propose().is_err(), "one task at a time");

        tasks.approve(id, &mut context, &counter).unwrap();
        assert!(tasks.gate().is_ok());
        assert_eq!(tasks.active().unwrap().id, id);
        assert_eq!(
            tasks.approve(id, &mut context, &counter),
            Err(TaskError::WrongState {
                id,
                state: "running",
                wanted: "waiting for approval"
            }),
            "approving twice would write the plan into the history twice",
        );
    }

    #[test]
    fn a_discarded_plan_is_kept_and_runs_nothing() {
        let mut tasks = Tasks::default();
        let id = tasks.propose(Plan::from_reply("x", "steps"));
        tasks.discard(id).unwrap();
        assert_eq!(tasks.get(id).unwrap().state, TaskState::Discarded);
        assert!(
            tasks.gate().is_ok(),
            "a refused plan does not block the session"
        );
        assert!(tasks.active().is_none());
    }

    #[test]
    fn closing_folds_the_span_the_task_opened() {
        let mut tasks = Tasks::default();
        let mut context = Context::new("sys");
        let counter = ApproximateCounter;

        context.push_turn(
            "before the task",
            "unrelated",
            Vec::<Fragment>::new(),
            &counter,
        );
        let id = tasks.propose(Plan::from_reply(
            "rename it",
            "```plan\n{\"steps\":[\"edit\"]}\n```",
        ));
        tasks.approve(id, &mut context, &counter).unwrap();
        for turn in 1..=3 {
            // Long enough to be worth folding. A fold of three toy turns costs
            // more than it saves, which is a real property of the strategy and
            // the reason the numbers travel with it.
            context.push_turn(
                format!("step {turn}"),
                "x".repeat(400),
                Vec::<Fragment>::new(),
                &counter,
            );
            tasks.record_turn(turn);
        }

        let closed = tasks.close(id, &mut context, &counter).unwrap();
        assert_eq!(closed.fold.turns, 4, "the plan turn folds with the work");
        assert!(
            closed.fold.tokens_after < closed.fold.tokens_before,
            "{} → {}",
            closed.fold.tokens_before,
            closed.fold.tokens_after,
        );
        assert_eq!(
            context.turns().len(),
            2,
            "the turn before the task is untouched; the task is one summary",
        );
        assert_eq!(context.turns()[1].answer, closed.summary);
        assert_eq!(tasks.get(id).unwrap().turns, [1, 2, 3]);
        assert!(
            tasks.close(id, &mut context, &counter).is_err(),
            "closing is once"
        );
    }
}
