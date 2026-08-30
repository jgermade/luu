//! A task: the unit of work, of approval, and of forgetting.
//!
//! A session is a sequence of these rather than a mode the agent is in. One
//! boundary does three jobs — it is the cut point compaction takes, the scope
//! permission is granted at, and the grouping the transcript shows — which is
//! the whole argument for it. See
//! `RECORD/2026-08-27.tasks-instead-of-modes.md` for the argument and
//! `RECORD/2026-08-30.tasks-in-code.md` for what this file does and does not do.
//!
//! **Closing is an event, not a mutation.** Nothing here deletes a turn: a
//! closed task changes how its turns *render* (see [`crate::context`]), so
//! reopening is folding differently rather than undoing a deletion.

use serde::{Deserialize, Serialize};

use crate::context::{Counter, TokenCounter};
use crate::sandbox::{Access, Sandbox};
use crate::tools::{ToolCall, ToolStep};

/// Tasks are numbered per session, in order, starting at 1.
pub type TaskId = u64;

/// How many distinct actions a summary lists before it stops listing them.
///
/// The summary enters the write-once region every later turn is built on, so
/// its size is not free and must not grow with the length of the task. A cap
/// is not a strategy — it is what stops one long task quietly becoming the
/// prompt.
const MAX_EVIDENCE_LINES: usize = 12;

/// What the agent proposes to do, and what the user approves.
///
/// Not prose: the files and commands are the part that is checked against the
/// sandbox, so they have to be a list rather than a paragraph someone would
/// have to parse back.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Plan {
    #[serde(default)]
    pub steps: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
    #[serde(default)]
    pub commands: Vec<String>,
}

impl Plan {
    /// What the plan asks for that the sandbox does not grant, one line each.
    ///
    /// The design wants the approved plan to *be* the policy for its task, with
    /// the policy file as its floor. This is the half of that which cannot be
    /// wrong: a plan naming a file nobody may touch or a command nobody may run
    /// is refused before anything runs, rather than discovered as a denial four
    /// turns in. Narrowing the sandbox *down* to the plan waits for a human who
    /// can add the file the plan forgot.
    ///
    /// Reachability only: a plan says which files it will *touch*, and this
    /// asks whether they can be read. A file the task will write into a
    /// read-only root passes here and is denied at the call, which is the one
    /// case this check was meant to catch and does not. Splitting the plan into
    /// what it reads and what it writes is the fix, and it waits for the same
    /// gate — a plan is only worth that precision once a human writes it.
    pub fn unmet(&self, sandbox: &Sandbox) -> Vec<String> {
        let mut unmet = Vec::new();
        for file in &self.files {
            let check = sandbox.check_path(std::path::Path::new(file), Access::Read);
            if !check.verdict.allowed {
                unmet.push(format!("file {file}: {}", check.verdict.rule));
            }
        }
        for command in &self.commands {
            if !sandbox.commands().iter().any(|allowed| allowed == command) {
                unmet.push(format!(
                    "command {command}: not in the sandbox's allowed commands"
                ));
            }
        }
        unmet
    }

    /// One line per part it has, for a human reading a run go by.
    pub fn describe(&self) -> String {
        let mut text = String::new();
        for (part, items) in [
            ("steps", &self.steps),
            ("files", &self.files),
            ("commands", &self.commands),
        ] {
            if !items.is_empty() {
                text.push_str(&format!("  {part:<9}{}\n", items.join(" · ")));
            }
        }
        text
    }
}

/// Where a task is in its life. `Closed` is the only one that changes how the
/// history renders.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskState {
    /// Proposed and not yet approved. Nothing runs in this state.
    Proposed,
    /// Approved. Its turns are the live ones, sent verbatim.
    Approved,
    /// Closed: its turns are folded to [`Task::summary`] from here on.
    Closed,
}

/// What a closed task leaves behind, counted once at the close.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Summary {
    pub text: String,
    pub tokens: u32,
    /// Which counter produced `tokens` — same reason [`crate::context::Turn`]
    /// carries one: two counters summed into one bar is not a measurement.
    pub counted_by: Counter,
}

/// One piece of work: proposed, approved, run, closed.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: TaskId,
    /// What the user asked for, as they asked for it.
    pub objective: String,
    pub plan: Plan,
    pub state: TaskState,
    /// Present from the close onwards. Dropped on a reopen, because a reopened
    /// task is being written again and the old account of it is not an account
    /// of what it will have been.
    pub summary: Option<Summary>,
}

impl Task {
    pub fn new(id: TaskId, objective: impl Into<String>, plan: Plan) -> Self {
        Self {
            id,
            objective: objective.into(),
            plan,
            state: TaskState::Proposed,
            summary: None,
        }
    }

    pub fn approve(&mut self) {
        self.state = TaskState::Approved;
    }

    /// Approved and not closed: the one task turns are attributed to.
    pub fn is_open(&self) -> bool {
        self.state == TaskState::Approved
    }

    pub fn is_closed(&self) -> bool {
        self.state == TaskState::Closed
    }

    /// Closes the task and writes its summary, once.
    ///
    /// `steps` is everything the task's turns did, in order, and `turns` is how
    /// many turns it took. Both are facts about what happened; nothing the model
    /// said about what happened is used.
    pub fn close(&mut self, steps: &[&ToolStep], turns: usize, counter: &dyn TokenCounter) {
        let text = summary_text(&self.objective, &self.plan, steps, turns);
        self.summary = Some(Summary {
            tokens: counter.count(&text),
            counted_by: counter.id(),
            text,
        });
        self.state = TaskState::Closed;
    }

    /// Reopens it: the fold stops applying and the turns are sent verbatim
    /// again. Not an undo — nothing was deleted — which is the whole reason
    /// closing was made an event.
    pub fn reopen(&mut self) {
        self.state = TaskState::Approved;
        self.summary = None;
    }
}

/// The deterministic summary: the approved plan, then what the tools actually
/// reported. Read `RECORD/2026-08-30.tasks-in-code.md` before adding prose to
/// it — it lands in the write-once region, where being wrong is unrecoverable
/// for the rest of the session.
fn summary_text(objective: &str, plan: &Plan, steps: &[&ToolStep], turns: usize) -> String {
    let mut text = format!("[task closed] {objective}\n");
    if !plan.steps.is_empty() {
        text.push_str("approved plan:\n");
        for step in &plan.steps {
            text.push_str(&format!("  - {step}\n"));
        }
    }

    let evidence = evidence(steps);
    match evidence.is_empty() {
        // Said out loud rather than left off: "this task ran no tools" is a
        // fact about it, and a summary that simply stops reads like one that
        // was cut short.
        true => text.push_str(&format!(
            "no tools ran; {turns} turn(s) folded, their text not kept\n"
        )),
        false => {
            text.push_str(&format!("evidence, from {turns} turn(s):\n"));
            for line in &evidence {
                text.push_str(&format!("  {line}\n"));
            }
        }
    }
    text
}

/// The distinct actions a task took, in order, each with what it reported.
///
/// Identical lines collapse rather than repeat: a loop that read the same file
/// eight times is one fact, and eight copies of it in the prefix is eight
/// copies of it paid for on every later call.
fn evidence(steps: &[&ToolStep]) -> Vec<String> {
    let mut lines: Vec<(String, usize)> = Vec::new();
    for step in steps {
        let status = step.outcome.error.as_deref().unwrap_or("ok");
        let line = match target(&step.call) {
            Some(target) => format!("{} {target} — {status}", step.call.name),
            None => format!("{} — {status}", step.call.name),
        };
        match lines.iter_mut().find(|(seen, _)| *seen == line) {
            Some((_, count)) => *count += 1,
            None => lines.push((line, 1)),
        }
    }

    let over = lines.len().saturating_sub(MAX_EVIDENCE_LINES);
    let mut rendered: Vec<String> = lines
        .into_iter()
        .take(MAX_EVIDENCE_LINES)
        .map(|(line, count)| match count {
            1 => line,
            n => format!("{line} (x{n})"),
        })
        .collect();
    if over > 0 {
        rendered.push(format!("... and {over} more"));
    }
    rendered
}

/// What a call acted on, in as few tokens as say it: the path, or the command
/// line. Arguments a reader cannot act on are left out — the summary is
/// evidence, not a replay.
fn target(call: &ToolCall) -> Option<String> {
    if let Some(path) = call.arguments.get("path").and_then(|v| v.as_str()) {
        return Some(path.to_string());
    }
    let command = call.arguments.get("command")?.as_str()?;
    let args = call
        .arguments
        .get("args")
        .and_then(|v| v.as_array())
        .map(|args| {
            args.iter()
                .filter_map(|arg| arg.as_str())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default();
    Some(match args.is_empty() {
        true => command.to_string(),
        false => format!("{command} {args}"),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ApproximateCounter;
    use crate::sandbox::{PathRule, SandboxPolicy, Verdict};
    use crate::tools::ToolOutcome;

    fn step(name: &str, arguments: serde_json::Value, error: Option<&str>) -> ToolStep {
        ToolStep {
            text: String::new(),
            call: ToolCall {
                name: name.into(),
                arguments,
            },
            outcome: ToolOutcome {
                verdict: Verdict::deny("test"),
                output: String::new(),
                error: error.map(str::to_string),
                truncated: false,
            },
            duration_ms: 0,
        }
    }

    #[test]
    fn the_summary_is_the_plan_and_the_evidence_and_nothing_the_model_said() {
        let mut task = Task::new(
            2,
            "add a --dry-run flag",
            Plan {
                steps: vec!["read the CLI".into(), "add the flag".into()],
                files: vec!["crates/luu/src/lib.rs".into()],
                commands: vec!["cargo".into()],
            },
        );
        task.approve();
        let steps = [
            step("read_file", serde_json::json!({"path": "src/lib.rs"}), None),
            step(
                "run_command",
                serde_json::json!({"command": "cargo", "args": ["test"]}),
                Some("cargo exited with 1"),
            ),
        ];
        task.close(&steps.iter().collect::<Vec<_>>(), 3, &ApproximateCounter);

        let text = &task.summary.as_ref().unwrap().text;
        assert!(text.contains("add a --dry-run flag"));
        assert!(text.contains("- read the CLI"));
        assert!(text.contains("read_file src/lib.rs — ok"));
        assert!(
            text.contains("run_command cargo test — cargo exited with 1"),
            "a non-zero exit is the evidence, not a detail: {text}"
        );
        assert!(task.is_closed());
    }

    #[test]
    fn repeated_identical_actions_collapse_rather_than_repeat() {
        let mut task = Task::new(1, "look around", Plan::default());
        let steps: Vec<ToolStep> = (0..8)
            .map(|_| step("list_dir", serde_json::json!({"path": "."}), None))
            .collect();
        task.close(&steps.iter().collect::<Vec<_>>(), 8, &ApproximateCounter);

        let text = &task.summary.as_ref().unwrap().text;
        assert!(text.contains("list_dir . — ok (x8)"), "{text}");
        assert_eq!(
            text.lines().filter(|l| l.contains("list_dir")).count(),
            1,
            "eight copies in the prefix is eight copies paid for every later call",
        );
    }

    #[test]
    fn a_task_that_ran_no_tools_says_so_rather_than_stopping() {
        let mut task = Task::new(1, "explain the design", Plan::default());
        task.close(&[], 4, &ApproximateCounter);
        let text = &task.summary.as_ref().unwrap().text;
        assert!(text.contains("no tools ran"), "{text}");
        assert!(text.contains("4 turn(s) folded"), "{text}");
    }

    #[test]
    fn reopening_drops_the_summary_without_recovering_anything() {
        let mut task = Task::new(1, "x", Plan::default());
        task.close(&[], 1, &ApproximateCounter);
        task.reopen();
        assert!(task.is_open());
        assert!(
            task.summary.is_none(),
            "the fold stops applying; the turns were never deleted to recover",
        );
    }

    #[test]
    fn a_plan_asking_for_more_than_the_sandbox_grants_is_refused_by_name() {
        let base = std::env::current_dir().unwrap();
        let sandbox = Sandbox::new(
            &SandboxPolicy {
                paths: vec![PathRule::new(".", Access::Read)],
                commands: vec!["cargo".into()],
                ..SandboxPolicy::default()
            },
            &base,
        )
        .unwrap();

        let plan = Plan {
            steps: vec![],
            files: vec!["Cargo.toml".into(), "/etc/passwd".into()],
            commands: vec!["cargo".into(), "curl".into()],
        };
        let unmet = plan.unmet(&sandbox);

        assert_eq!(unmet.len(), 2, "{unmet:?}");
        assert!(unmet[0].contains("/etc/passwd"));
        assert!(unmet[1].contains("curl"));
    }
}
