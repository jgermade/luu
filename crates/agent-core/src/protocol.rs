//! The agent protocol: one message schema, several transports.
//!
//! These types are carried unchanged over stdio (the VSCode bridge), over a
//! WebSocket (the debug UI), and into the record format. Adding a field that
//! only one of them understands is how the schema stops being one schema.
//!
//! **Debug data does not live here.** Prompts, token budgets and anything else
//! that exists to explain the agent rather than drive it belong in
//! [`crate::trace`], on its own channel behind `--trace`, so a stdio consumer
//! never has to carry them.
//!
//! Every variant below was written after watching `run_turn` produce it —
//! see `RECORD/2026-08-26.walking-skeleton.md`.

use serde::{Deserialize, Serialize};

use crate::backend::Usage;
use crate::sandbox::Verdict;
use crate::task::{Plan, TaskId};
use crate::tools::ToolStep;
use crate::turn::{EndReason, TurnEvent};

/// Bumped when a change would break an older client. `Hello` carries it so a
/// mismatch is a message rather than a mystery.
///
/// **v1 was frozen** once every message in it had been watched being sent and
/// answered, the task lifecycle included, with the rule that a change an older
/// reader could not make sense of bumps this and [`crate::record::FORMAT`] with
/// it.
///
/// **2 is that rule being used**, for [`ServerMessage::Refused`]: an unknown
/// `type` in a tagged enum is a parse error rather than a line to skip, so a new
/// variant is exactly the change the rule names. See
/// `RECORD/2026-08-30.a-refusal-is-a-message.md`.
pub const VERSION: u32 = 2;

/// Turns are numbered per session, in order, starting at 1.
pub type TurnId = u64;

/// Why the server did not do what was asked. Small on purpose: a client
/// branches on this and shows [`ServerMessage::Refused::detail`] to a person.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Refusal {
    /// A turn is running. One at a time until sessions exist.
    Busy,
    /// A proposal is waiting on a person. Nothing runs behind the gate.
    Pending,
    /// The task named is not in a state where the ask applies — approved
    /// twice, closed while nothing was open, reopened when it was never closed.
    Task,
    /// Part of what was asked for is not granted by the policy file, which is
    /// the outer bound nobody may widen past.
    NotGranted,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Start a turn.
    Prompt { text: String },
    /// Stop the turn in flight. Cancelling when nothing is running is not an
    /// error — a client that raced the end of a turn did nothing wrong.
    Cancel,
    /// Approve a proposed task. Nothing has run under it until this arrives:
    /// the prompt that caused the proposal is held, unrun, until it does.
    ///
    /// `files` and `commands` are what the person adds to the plan before
    /// approving it — the half that makes narrowing survivable, because a small
    /// model's first act is a plan that forgets a file. They are checked against
    /// the policy file like any other plan: the gate widens a plan up to the
    /// file and not past it. Both default to empty, which is the approval that
    /// widens nothing.
    ApproveTask {
        task: TaskId,
        /// What the task may read, added to what the model declared.
        #[serde(default)]
        files: Vec<String>,
        /// What it may also change. Separate from `files` for the same reason
        /// the plan separates them: a grant that cannot say *read* cannot be
        /// held to reading.
        #[serde(default)]
        writes: Vec<String>,
        #[serde(default)]
        commands: Vec<String>,
    },
    /// Refuse it. The held prompt is dropped with it — a prompt whose plan was
    /// turned down is not a prompt that was approved on its own.
    RejectTask { task: TaskId },
    /// Close it: from here its turns are sent as their summary. The user is the
    /// only authority that closes a task today; the ladder above them
    /// (exit codes, then a judge in shadow mode) is still ahead.
    CloseTask { task: TaskId },
    /// Unfold it. Not an undo: nothing was deleted to recover.
    ReopenTask { task: TaskId },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ServerMessage {
    /// Sent once on connect. A client that attaches mid-turn learns what it
    /// missed from `turn`, rather than inferring it from the first token.
    Hello {
        protocol: u32,
        backend: String,
        model: String,
        turn: Option<TurnId>,
    },
    /// Carries the prompt, because a second client (and the record file) never
    /// saw the `ClientMessage` that started the turn.
    TurnStarted {
        turn: TurnId,
        prompt: String,
        /// The task it was asked inside, when there is one. Here rather than
        /// only on the task messages so that a client can group a transcript
        /// without replaying the whole lifecycle to work out what was open.
        #[serde(default)]
        task: Option<TaskId>,
    },
    /// A piece of work, with the plan that is about to be approved or refused.
    /// Nothing runs between this and `TaskApproved`.
    TaskProposed {
        task: TaskId,
        objective: String,
        plan: Plan,
    },
    /// Approved, with the plan as approved rather than as proposed: it is what
    /// the task's sandbox is built from, so a transcript that showed only the
    /// proposal would be naming the wrong authority.
    TaskApproved {
        task: TaskId,
        #[serde(default)]
        plan: Plan,
    },
    /// The one rewrite in an otherwise write-once session: from here on the
    /// task's turns are sent as this summary. It travels on the wire because a
    /// transcript has to be able to show what the model will see from now on.
    TaskClosed {
        task: TaskId,
        summary: String,
    },
    /// The fold stops applying. Not an undo — nothing was deleted.
    TaskReopened {
        task: TaskId,
    },
    /// The plan was put up and turned down. The task stays in the session with
    /// it: a refusal is a thing that happened.
    TaskRejected {
        task: TaskId,
    },
    Token {
        turn: TurnId,
        text: String,
    },
    /// `usage` is absent on a cancelled turn: the counts arrive on the
    /// backend's final line, which cancelling means never reading. Reporting
    /// zeros instead would be a lie the budget panel would plot.
    Ended {
        turn: TurnId,
        reason: EndReason,
        usage: Option<Usage>,
    },
    Failed {
        turn: TurnId,
        message: String,
    },
    /// The server did not do what a client asked, and why.
    ///
    /// Not a failure and not a turn: the session is in a state where the ask
    /// does not apply. Before this existed the server simply returned, and a
    /// client could not tell a refusal from a message that never arrived — so
    /// the UI guessed, by disabling its own composer, which is not a permission
    /// model and not an interface either.
    ///
    /// It travels on the protocol rather than the trace channel because it
    /// *drives* a client: it is the answer to why nothing happened.
    Refused {
        /// The `type` of the client message being refused.
        request: String,
        reason: Refusal,
        /// The same thing in words, for a person to read.
        detail: String,
    },
    /// A tool the model asked for, before it was checked. `step` counts from 1
    /// within the turn.
    ToolCall {
        turn: TurnId,
        step: u32,
        name: String,
        arguments: serde_json::Value,
    },
    /// What it did. The verdict travels with the result because "the agent ran
    /// a command" and "the kernel held the command it ran" are different facts
    /// and only one of them is worth trusting.
    ToolResult {
        turn: TurnId,
        step: u32,
        name: String,
        verdict: Verdict,
        error: Option<String>,
        /// The result as the model received it — the same bytes that went into
        /// the history, so a recording can show what the model was told.
        output: String,
        truncated: bool,
        duration_ms: u64,
    },
}

impl ServerMessage {
    /// The one conversion at the core/wire boundary.
    ///
    /// `None` for an event that is not a protocol message. There is one:
    /// [`TurnEvent::ModelCall`] explains the agent rather than driving it, so it
    /// belongs on the trace channel and a stdio consumer never sees it. That is
    /// the trace/protocol split stopping being a convention and becoming a
    /// type — an internal event that provably cannot reach the wire.
    pub fn from_turn_event(turn: TurnId, event: TurnEvent) -> Option<Self> {
        Some(match event {
            TurnEvent::ModelCall { .. } => return None,
            TurnEvent::Token(text) => Self::Token { turn, text },
            TurnEvent::Ended { reason, usage } => Self::Ended {
                turn,
                reason,
                usage,
            },
            TurnEvent::Failed(message) => Self::Failed { turn, message },
            TurnEvent::ToolCall { step, call } => Self::ToolCall {
                turn,
                step,
                name: call.name,
                arguments: call.arguments,
            },
            TurnEvent::ToolResult { step, outcome } => {
                let ToolStep {
                    call,
                    outcome,
                    duration_ms,
                    ..
                } = *outcome;
                Self::ToolResult {
                    turn,
                    step,
                    name: call.name,
                    verdict: outcome.verdict,
                    error: outcome.error,
                    output: outcome.output,
                    truncated: outcome.truncated,
                    duration_ms,
                }
            }
        })
    }

    /// Which turn this is about, if any.
    pub fn turn(&self) -> Option<TurnId> {
        match self {
            Self::Hello { turn, .. } => *turn,
            Self::TurnStarted { turn, .. }
            | Self::Token { turn, .. }
            | Self::Ended { turn, .. }
            | Self::Failed { turn, .. }
            | Self::ToolCall { turn, .. }
            | Self::ToolResult { turn, .. } => Some(*turn),
            // A task spans turns and its lifecycle happens between them, and a
            // refusal is about the ask rather than about a turn — three of the
            // four happen when there is no turn to name.
            Self::TaskProposed { .. }
            | Self::TaskApproved { .. }
            | Self::TaskClosed { .. }
            | Self::TaskReopened { .. }
            | Self::TaskRejected { .. }
            | Self::Refused { .. } => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(message: &ServerMessage) -> serde_json::Value {
        let json = serde_json::to_value(message).unwrap();
        let back: ServerMessage = serde_json::from_value(json.clone()).unwrap();
        assert_eq!(format!("{back:?}"), format!("{message:?}"));
        json
    }

    #[test]
    fn a_token_is_tagged_by_type() {
        let json = roundtrip(&ServerMessage::Token {
            turn: 1,
            text: "hola".into(),
        });
        assert_eq!(json["type"], "token");
        assert_eq!(json["text"], "hola");
    }

    #[test]
    fn a_cancelled_end_carries_a_null_usage_rather_than_zeros() {
        let json = roundtrip(&ServerMessage::Ended {
            turn: 1,
            reason: EndReason::Cancelled,
            usage: None,
        });
        assert_eq!(json["reason"], "cancelled");
        assert!(json["usage"].is_null());
    }

    #[test]
    fn a_model_call_never_reaches_the_wire() {
        // It explains the agent rather than driving it, so it belongs on the
        // trace channel and a stdio consumer never has to carry it.
        assert!(
            ServerMessage::from_turn_event(
                1,
                TurnEvent::ModelCall {
                    step: 2,
                    messages: Vec::new()
                }
            )
            .is_none()
        );
    }

    #[test]
    fn a_client_prompt_parses_from_the_wire() {
        let parsed: ClientMessage =
            serde_json::from_str(r#"{"type":"prompt","text":"hola"}"#).unwrap();
        assert!(matches!(parsed, ClientMessage::Prompt { text } if text == "hola"));
    }

    #[test]
    fn a_task_message_is_about_a_task_rather_than_a_turn() {
        let json = roundtrip(&ServerMessage::TaskProposed {
            task: 2,
            objective: "add a --dry-run flag".into(),
            plan: Plan {
                steps: vec!["read the CLI".into()],
                files: vec!["crates/luu/src/lib.rs".into()],
                writes: vec![],
                commands: vec!["cargo".into()],
            },
        });
        assert_eq!(json["type"], "task_proposed");
        assert_eq!(json["plan"]["files"][0], "crates/luu/src/lib.rs");
        assert_eq!(
            ServerMessage::TaskApproved {
                task: 2,
                plan: Plan::default(),
            }
            .turn(),
            None,
            "a task spans turns; pinning it to one would be a guess",
        );
    }

    #[test]
    fn the_client_half_of_the_lifecycle_parses_from_the_wire() {
        for (text, expected) in [
            (r#"{"type":"approve_task","task":2}"#, "ApproveTask"),
            (r#"{"type":"reject_task","task":2}"#, "RejectTask"),
            (r#"{"type":"close_task","task":2}"#, "CloseTask"),
            (r#"{"type":"reopen_task","task":2}"#, "ReopenTask"),
        ] {
            let parsed: ClientMessage = serde_json::from_str(text).unwrap();
            assert!(
                format!("{parsed:?}").starts_with(expected),
                "{text} parsed as {parsed:?}",
            );
        }
    }

    #[test]
    fn a_turn_recorded_before_tasks_existed_still_parses() {
        let parsed: ServerMessage =
            serde_json::from_str(r#"{"type":"turn_started","turn":1,"prompt":"hola"}"#).unwrap();
        assert!(matches!(
            parsed,
            ServerMessage::TurnStarted { task: None, .. }
        ));
    }

    #[test]
    fn turn_events_convert_without_losing_the_reason() {
        let event = TurnEvent::Ended {
            reason: EndReason::Length,
            usage: None,
        };
        let message = ServerMessage::from_turn_event(7, event).unwrap();
        assert_eq!(message.turn(), Some(7));
        assert!(matches!(
            message,
            ServerMessage::Ended {
                reason: EndReason::Length,
                ..
            }
        ));
    }
}
