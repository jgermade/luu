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

use crate::context::{Counter, Fragment, TokenCounter};
use crate::sandbox::{Access, Authority, Sandbox, SandboxError, SandboxPolicy};
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

/// How many tokens of quoted source one summary may carry.
///
/// The same argument as [`MAX_EVIDENCE_LINES`], applied to the other half of
/// the summary: a quote is paid for on every call from the close to the end of
/// the session, so what it costs must be a function of this number and not of
/// how much the task was handed. The value is a starting point and wants the
/// grounded probe to choose it — see
/// `RECORD/2026-08-30.what-a-summary-should-carry.md`.
const MAX_QUOTED_TOKENS: u32 = 1024;

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

    /// The sandbox this plan *is*, once someone has approved it.
    ///
    /// Built out of what `session` already grants rather than out of the plan's
    /// own words: a plan cannot invent access, so every path it names keeps
    /// exactly the access the policy file gave it and a path the file never
    /// granted is simply absent. [`Plan::unmet`] refuses that plan before this
    /// is ever called, and this is the same rule applied a second time, where
    /// it bites.
    ///
    /// **Narrowing is on extent, not on level.** A plan has no way to say
    /// *read* versus *write*, so a file granted `read-write` by the policy stays
    /// read-write here; making it read-only would answer that open question as a
    /// side effect of this one and break every task that edits a file it named.
    /// `network` and `enforcement` are the session's for the same reason: a plan
    /// declares neither.
    pub fn narrow(&self, session: &Sandbox, task: TaskId) -> Result<Sandbox, SandboxError> {
        let mut policy = SandboxPolicy {
            paths: Vec::new(),
            commands: Vec::new(),
            network: session.network(),
            enforcement: session.enforcement(),
        };
        for file in &self.files {
            if let Some(access) = session.access_for(std::path::Path::new(file)) {
                policy.allow(file, access);
            }
        }
        for command in &self.commands {
            if session.commands().iter().any(|allowed| allowed == command) {
                policy.allow_command(command);
            }
        }
        Ok(Sandbox::new(&policy, session.base())?.under(Authority::Plan(task)))
    }

    /// Adds what a person put in at the gate, ignoring what is already there.
    ///
    /// The amendment is checked with [`Plan::unmet`] like any other plan, so
    /// the human at the gate can widen the plan up to the policy file and not
    /// past it — otherwise the gate is the policy and `luu.toml` is a
    /// suggestion.
    pub fn amend(&mut self, files: &[String], commands: &[String]) {
        for file in files {
            if !self.files.contains(file) {
                self.files.push(file.clone());
            }
        }
        for command in commands {
            if !self.commands.contains(command) {
                self.commands.push(command.clone());
            }
        }
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

/// A plan as the model proposes it, before anyone has approved it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Proposal {
    pub objective: String,
    #[serde(flatten)]
    pub plan: Plan,
}

/// Reads a proposal out of what the model said.
///
/// The same shape as [`crate::tools::parse_call`], and for the same reason:
/// how a model expresses a plan is a transport detail — a fenced block today, a
/// GBNF grammar next — and nothing above this line should care which.
///
/// `None` covers the ordinary case of a small model answering the planning call
/// in prose. The caller proposes the user's own ask instead; a gate that
/// disappeared because the model was vague would be the one failure mode this
/// whole mechanism exists to prevent.
pub fn parse_plan(text: &str) -> Option<Proposal> {
    let proposal: Proposal =
        crate::tools::fenced(text, "plan").and_then(|body| serde_json::from_str(body).ok())?;
    (!proposal.objective.trim().is_empty()).then_some(proposal)
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
    /// Proposed and refused. Kept rather than erased: that a plan was put up
    /// and turned down is worth more than a gap in the numbering, and ids are
    /// handed out by position, so removing one would make the next collide
    /// with it.
    Rejected,
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

    /// Refuses it. Nothing ran, so there is nothing to fold and nothing to
    /// summarise — the plan stays as the record of what was turned down.
    pub fn reject(&mut self) {
        self.state = TaskState::Rejected;
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
    /// `steps` is everything the task's turns did and `shown` is every fragment
    /// they were handed, both in order, and `turns` is how many turns it took.
    /// All three are facts about what happened; nothing the model said about
    /// what happened is used.
    pub fn close(
        &mut self,
        steps: &[&ToolStep],
        shown: &[&Fragment],
        turns: usize,
        counter: &dyn TokenCounter,
    ) {
        let text = summary_text(&self.objective, &self.plan, steps, shown, turns, counter);
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

/// The deterministic summary: the approved plan, what the task was shown, and
/// what the tools actually reported. Read `RECORD/2026-08-30.tasks-in-code.md`
/// before adding prose to it — it lands in the write-once region, where being
/// wrong is unrecoverable for the rest of the session.
///
/// A fragment is quoted from the file's own bytes for that reason: it cannot be
/// a hallucination, and it is the thing the grounded probe showed the fold
/// losing — `RECORD/2026-08-30.the-fold-probe-run.md` measured it and
/// `RECORD/2026-08-30.what-a-summary-should-carry.md` argues the fix.
fn summary_text(
    objective: &str,
    plan: &Plan,
    steps: &[&ToolStep],
    shown: &[&Fragment],
    turns: usize,
    counter: &dyn TokenCounter,
) -> String {
    let mut text = format!("[task closed] {objective}\n");
    if !plan.steps.is_empty() {
        text.push_str("approved plan:\n");
        for step in &plan.steps {
            text.push_str(&format!("  - {step}\n"));
        }
    }

    let shown = quoted(shown, counter);
    if !shown.is_empty() {
        text.push_str(&format!("shown, from {turns} turn(s):\n"));
        for (fragment, tokens) in &shown {
            let lines = fragment.text.lines().count();
            text.push_str(&match tokens {
                Some(tokens) => format!("  {} ({lines} line(s), {tokens} tokens)\n", fragment.path),
                None => format!("  {} — over the cap, not kept\n", fragment.path),
            });
        }
    }

    let evidence = evidence(steps);
    match (evidence.is_empty(), shown.is_empty()) {
        // Said out loud rather than left off: "this task ran no tools" is a
        // fact about it, and a summary that simply stops reads like one that
        // was cut short.
        (true, true) => text.push_str(&format!(
            "no tools ran; {turns} turn(s) folded, their text not kept\n"
        )),
        // The same fact, without the half of that sentence which is no longer
        // true. A model reading "their text not kept" answers the question that
        // sentence asks — the probe's turn 18 refused rather than answered — so
        // it must not be said over a quote that is right there.
        (true, false) => text.push_str("no tools ran; the turns' own text is not kept\n"),
        (false, _) => {
            text.push_str(&format!("evidence, from {turns} turn(s):\n"));
            for line in &evidence {
                text.push_str(&format!("  {line}\n"));
            }
        }
    }

    for (fragment, tokens) in &shown {
        if tokens.is_some() {
            text.push_str(&format!("--- {}\n{}", fragment.path, fragment.text));
            if !fragment.text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("---\n");
        }
    }
    text
}

/// The distinct fragments the task was handed, in the order it saw them, each
/// with its token count — or `None` for one the cap left out.
///
/// Newest first while choosing, because the last thing a task was shown is the
/// likeliest to be what a later turn asks about; then rendered in the order
/// they were shown, because that is the order they were read in.
///
/// Whole or not at all: half a file quoted under a heading naming the whole one
/// is a summary nobody can account for, which is the same reason
/// [`crate::tools::MAX_OUTPUT_BYTES`] says so in the text when it cuts.
fn quoted<'a>(
    shown: &[&'a Fragment],
    counter: &dyn TokenCounter,
) -> Vec<(&'a Fragment, Option<u32>)> {
    let mut distinct: Vec<&Fragment> = Vec::new();
    for fragment in shown {
        if !distinct.iter().any(|seen| seen.path == fragment.path) {
            distinct.push(fragment);
        }
    }

    let mut kept: Vec<(&Fragment, Option<u32>)> =
        distinct.iter().map(|fragment| (*fragment, None)).collect();
    let mut budget = MAX_QUOTED_TOKENS;
    for entry in kept.iter_mut().rev() {
        let tokens = counter.count(&entry.0.text);
        if tokens <= budget {
            budget -= tokens;
            entry.1 = Some(tokens);
        }
    }
    kept
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
    use crate::sandbox::{Enforcement, PathRule, SandboxPolicy, Verdict};
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
        task.close(
            &steps.iter().collect::<Vec<_>>(),
            &[],
            3,
            &ApproximateCounter,
        );

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
        task.close(
            &steps.iter().collect::<Vec<_>>(),
            &[],
            8,
            &ApproximateCounter,
        );

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
        task.close(&[], &[], 4, &ApproximateCounter);
        let text = &task.summary.as_ref().unwrap().text;
        assert!(text.contains("no tools ran"), "{text}");
        assert!(text.contains("4 turn(s) folded"), "{text}");
    }

    fn fragment(path: &str, text: &str) -> Fragment {
        Fragment {
            path: path.into(),
            text: text.into(),
        }
    }

    #[test]
    fn what_the_task_was_shown_is_quoted_from_the_file_and_not_described() {
        let mut task = Task::new(
            3,
            "work out what the sandbox policy grants",
            Plan::default(),
        );
        let shown = [fragment(
            "luu.toml:1-3",
            "[sandbox]\ncommands = [\"cargo\", \"rg\"]\n",
        )];
        task.close(
            &[],
            &shown.iter().collect::<Vec<_>>(),
            5,
            &ApproximateCounter,
        );

        let text = &task.summary.as_ref().unwrap().text;
        assert!(text.contains("shown, from 5 turn(s):"), "{text}");
        assert!(text.contains("luu.toml:1-3 (2 line(s),"), "{text}");
        assert!(
            text.contains("commands = [\"cargo\", \"rg\"]"),
            "the file's own bytes, which cannot be a hallucination: {text}",
        );
        assert!(
            !text.contains("their text not kept"),
            "the half of that sentence which the quote makes false is what made \
             the probe's turn 18 refuse a question it could answer: {text}",
        );
    }

    #[test]
    fn the_same_fragment_shown_twice_is_quoted_once() {
        let mut task = Task::new(1, "read it twice", Plan::default());
        let twice = [
            fragment("luu.toml:1-2", "[sandbox]\npaths = []\n"),
            fragment("luu.toml:1-2", "[sandbox]\npaths = []\n"),
        ];
        task.close(
            &[],
            &twice.iter().collect::<Vec<_>>(),
            2,
            &ApproximateCounter,
        );

        let text = &task.summary.as_ref().unwrap().text;
        assert_eq!(
            text.matches("[sandbox]").count(),
            1,
            "twice in the prefix is twice paid for on every later call: {text}",
        );
    }

    #[test]
    fn over_the_cap_the_newest_is_kept_and_the_rest_are_named() {
        let mut task = Task::new(1, "read a lot", Plan::default());
        let big = "a word ".repeat(MAX_QUOTED_TOKENS as usize);
        let shown = [
            fragment("old.rs", &big),
            fragment("new.rs", "the last thing it was shown\n"),
        ];
        task.close(
            &[],
            &shown.iter().collect::<Vec<_>>(),
            4,
            &ApproximateCounter,
        );

        let text = &task.summary.as_ref().unwrap().text;
        assert!(
            text.contains("old.rs — over the cap, not kept"),
            "what was dropped is named rather than silently absent: {text}",
        );
        assert!(!text.contains("--- old.rs"), "{text}");
        assert!(
            text.contains("the last thing it was shown"),
            "the newest is the likeliest to be asked about: {text}",
        );
    }

    #[test]
    fn reopening_drops_the_summary_without_recovering_anything() {
        let mut task = Task::new(1, "x", Plan::default());
        task.close(&[], &[], 1, &ApproximateCounter);
        task.reopen();
        assert!(task.is_open());
        assert!(
            task.summary.is_none(),
            "the fold stops applying; the turns were never deleted to recover",
        );
    }

    #[test]
    fn a_proposal_is_read_out_of_a_fenced_block() {
        let proposal = parse_plan(
            "I will need to touch the CLI.\n\n             ```plan\n             {\"objective\": \"add a --dry-run flag\", \"steps\": [\"read the CLI\"],              \"files\": [\"crates/luu/src/lib.rs\"], \"commands\": [\"cargo\"]}\n             ```",
        )
        .unwrap();

        assert_eq!(proposal.objective, "add a --dry-run flag");
        assert_eq!(proposal.plan.files, ["crates/luu/src/lib.rs"]);
        assert_eq!(proposal.plan.commands, ["cargo"]);
    }

    #[test]
    fn a_proposal_may_declare_nothing_at_all() {
        let proposal = parse_plan("```plan\n{\"objective\": \"explain the design\"}\n```").unwrap();
        assert_eq!(proposal.objective, "explain the design");
        assert!(proposal.plan.steps.is_empty());
        assert!(proposal.plan.files.is_empty());
    }

    #[test]
    fn prose_is_not_a_plan_and_says_so_by_returning_nothing() {
        // The ordinary answer from a small model, and the caller's cue to
        // propose the user's own ask instead. A gate that vanished here would
        // be the failure this mechanism exists to prevent.
        assert!(parse_plan("I'll read the CLI and then add the flag.").is_none());
        assert!(parse_plan("```plan\n{\"objective\": \"  \"}\n```").is_none());
        assert!(parse_plan("```plan\nnot json\n```").is_none());
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

    /// The session's sandbox for the narrowing tests: read-write on the tree,
    /// two commands, which is roughly what `luu.toml` grants.
    ///
    /// `best-effort` because these tests are about *who granted what*, and
    /// under `kernel` every `prepare_command` on a machine without Landlock is
    /// denied before the allowlist is ever consulted — which would make the
    /// assertions below pass or fail for a reason that is not theirs.
    fn session() -> Sandbox {
        Sandbox::new(
            &SandboxPolicy {
                paths: vec![PathRule::new(".", Access::ReadWrite)],
                commands: vec!["cargo".into(), "git".into()],
                enforcement: Enforcement::BestEffort,
                ..SandboxPolicy::default()
            },
            &std::env::current_dir().unwrap(),
        )
        .unwrap()
    }

    #[test]
    fn an_approved_plan_grants_what_it_named_and_nothing_else() {
        let session = session();
        let plan = Plan {
            steps: vec![],
            files: vec!["Cargo.toml".into()],
            commands: vec!["cargo".into()],
        };
        let narrowed = plan.narrow(&session, 1).unwrap();

        assert!(
            narrowed
                .check_path(std::path::Path::new("Cargo.toml"), Access::Read)
                .verdict
                .allowed,
        );
        let refused = narrowed.check_path(std::path::Path::new("src/task.rs"), Access::Read);
        assert!(!refused.verdict.allowed);
        assert!(
            refused
                .verdict
                .rule
                .contains("the approved plan for task 1"),
            "a denial has to say which authority refused: {}",
            refused.verdict.rule,
        );
        assert!(
            session
                .check_path(std::path::Path::new("src/task.rs"), Access::Read)
                .verdict
                .allowed,
            "the same file is still readable under the policy the task narrows",
        );

        assert!(narrowed.prepare_command("cargo").is_ok());
        let refused = narrowed.prepare_command("git").expect_err("not planned");
        assert!(
            refused.rule.contains("the approved plan for task 1"),
            "{refused:?}"
        );
    }

    /// Narrowing is on extent, not on level: a plan cannot say *read* yet, so a
    /// file the policy grants read-write stays read-write inside the task.
    #[test]
    fn a_planned_file_keeps_the_access_the_policy_gave_it() {
        let plan = Plan {
            files: vec!["Cargo.toml".into()],
            ..Plan::default()
        };
        let narrowed = plan.narrow(&session(), 1).unwrap();

        assert!(
            narrowed
                .check_path(std::path::Path::new("Cargo.toml"), Access::ReadWrite)
                .verdict
                .allowed,
        );
    }

    /// A plan that declares nothing grants nothing. That is the point of
    /// narrowing and it is also its sharpest edge, so it is pinned here.
    #[test]
    fn a_plan_that_declares_nothing_narrows_to_nothing() {
        let narrowed = Plan::default().narrow(&session(), 4).unwrap();

        assert!(
            !narrowed
                .check_path(std::path::Path::new("Cargo.toml"), Access::Read)
                .verdict
                .allowed,
        );
        assert!(narrowed.prepare_command("cargo").is_err());
    }

    /// What a person adds at the gate, and what they cannot add: the amendment
    /// is a plan like any other and `unmet` is what refuses it.
    #[test]
    fn an_amendment_widens_the_plan_and_is_still_checked_against_the_file() {
        let session = session();
        let mut plan = Plan {
            files: vec!["Cargo.toml".into()],
            ..Plan::default()
        };
        plan.amend(
            &["src/task.rs".into(), "Cargo.toml".into()],
            &["git".into()],
        );

        assert_eq!(plan.files, ["Cargo.toml", "src/task.rs"], "no duplicate");
        assert!(plan.unmet(&session).is_empty());

        let narrowed = plan.narrow(&session, 1).unwrap();
        assert!(
            narrowed
                .check_path(std::path::Path::new("src/task.rs"), Access::Read)
                .verdict
                .allowed,
            "the file the person added at the gate is in the task's sandbox",
        );

        let mut past_the_file = Plan::default();
        past_the_file.amend(&["/etc/passwd".into()], &["curl".into()]);
        assert_eq!(
            past_the_file.unmet(&session).len(),
            2,
            "the human at the gate widens up to the policy file and not past it",
        );
    }
}
