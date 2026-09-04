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
//! see `RECORD/2026-08-26.walking-skeleton.completed.md`.

use serde::{Deserialize, Serialize};

use crate::approval::Signature;
use crate::backend::Usage;
use crate::context::{Counter, Eviction};
use crate::job::{ApprovedBy, ClosedBy, JobId, Plan, PlanSource};
use crate::sandbox::Verdict;
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
/// `RECORD/2026-08-30.a-refusal-is-a-message.completed.md`.
///
/// **3 is the same rule again**, for [`ServerMessage::Evicted`]: what leaves the
/// window is a thing that happened to the conversation, not a debug reading, so
/// it is here rather than on the trace channel — and a client that could not
/// parse it would be watching a transcript whose turns are silently no longer
/// in the prompt. See `RECORD/2026-08-31.eviction-tombstones.completed.md`.
///
/// **4 uncrosses terminology**: the outer unit of work and approval is renamed to
/// **Job**, reserving **Task** for the fine-grained checklist items emitted by models.
///
/// **5 is the wire learning to say what it speaks**, which is federation's first
/// prerequisite: [`ClientMessage::Hello`] is a new client variant, and
/// [`Refusal`] gains `version` and `signature` — new values of a tagged enum,
/// which is the same rule as 2, 3 and 4. This is also the last bump an older
/// client learns about by failing to parse: from here a client says its version
/// and is refused out loud. See
/// `RECORD/2026-09-04.signed-approvals.completed.md`.
pub const VERSION: u32 = 5;

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
    /// The job named is not in a state where the ask applies — approved
    /// twice, closed while nothing was open, reopened when it was never closed.
    #[serde(alias = "task")]
    Job,
    /// Part of what was asked for is not granted by the policy file, which is
    /// the outer bound nobody may widen past.
    NotGranted,
    /// The client speaks a protocol or record format this host does not. Sent
    /// in answer to [`ClientMessage::Hello`], in either direction of mismatch:
    /// a newer client is the case this host cannot parse and an older one is
    /// the case it cannot, and neither side can repair it by guessing.
    Version,
    /// The approval carries no signature where one is required, names a key
    /// this host does not know, or does not verify against the grant it
    /// carries. One variant and not three: a client branches on this and shows
    /// `detail` to a person.
    Signature,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// What the client speaks. Optional on loopback and over stdio, where the
    /// peer is this machine's own browser or the process that spawned us;
    /// **required as the first message** on a port that asks for a bearer
    /// token, which is every port off loopback. A mismatch is answered with
    /// [`Refusal::Version`] and the connection closed, rather than by
    /// misreading the next message.
    Hello {
        protocol: u32,
        /// The record format the client can read, so a client that mirrors or
        /// replays a session is refused for the same reason at the same point.
        #[serde(default)]
        format: u32,
    },
    /// Start a turn.
    Prompt { text: String },
    /// Stop the turn in flight. Cancelling when nothing is running is not an
    /// error — a client that raced the end of a turn did nothing wrong.
    Cancel,
    /// Approve a proposed job. Nothing has run under it until this arrives:
    /// the prompt that caused the proposal is held, unrun, until it does.
    #[serde(alias = "approve_task")]
    ApproveJob {
        #[serde(alias = "task")]
        job: JobId,
        /// What the job may read, added to what the model declared.
        #[serde(default)]
        files: Vec<String>,
        /// What it may also change. Separate from `files` for the same reason
        /// the plan separates them: a grant that cannot say *read* cannot be
        /// held to reading.
        #[serde(default)]
        writes: Vec<String>,
        #[serde(default)]
        commands: Vec<String>,
        /// What would close this job without anyone present: a command line
        /// whose exit code of 0 folds it.
        #[serde(default)]
        closes_on: Option<String>,
        /// Whether to grant network access to this job. If `None`, preserves
        /// whatever the model's plan requested (or `false` by default).
        #[serde(default)]
        network: Option<bool>,
        /// Optional egress domain allowlist for this job.
        #[serde(default)]
        egress: Option<Vec<String>>,
        /// Ed25519 over the grant above, bound to the session it belongs to.
        /// Required when `[approvals] required` says so, verified whenever it
        /// is present. See [`crate::approval`].
        #[serde(default)]
        signature: Option<Signature>,
    },
    /// Refuse it. The held prompt is dropped with it — a prompt whose plan was
    /// turned down is not a prompt that was approved on its own.
    #[serde(alias = "reject_task")]
    RejectJob {
        #[serde(alias = "task")]
        job: JobId,
    },
    /// Close it: from here its turns are sent as their summary.
    #[serde(alias = "close_task")]
    CloseJob {
        #[serde(alias = "task")]
        job: JobId,
    },
    /// Unfold it. Not an undo: nothing was deleted to recover.
    #[serde(alias = "reopen_task")]
    ReopenJob {
        #[serde(alias = "task")]
        job: JobId,
    },
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
        /// What this session is called, so an approval can be signed against
        /// it. Without it a signature captured here replays against any other
        /// host that numbers its jobs the same way. `None` in a recording made
        /// before signatures existed.
        #[serde(default)]
        session: Option<String>,
    },
    /// Carries the prompt, because a second client (and the record file) never
    /// saw the `ClientMessage` that started the turn.
    TurnStarted {
        turn: TurnId,
        prompt: String,
        /// The job it was asked inside, when there is one.
        #[serde(default, alias = "task")]
        job: Option<JobId>,
    },
    /// A piece of work, with the plan that is about to be approved or refused.
    /// Nothing runs between this and `JobApproved`.
    #[serde(alias = "task_proposed")]
    JobProposed {
        #[serde(alias = "task")]
        job: JobId,
        objective: String,
        plan: Plan,
        #[serde(default)]
        source: Option<PlanSource>,
    },
    /// Approved, with the plan as approved rather than as proposed: it is what
    /// the job's sandbox is built from.
    #[serde(alias = "task_approved")]
    JobApproved {
        #[serde(alias = "task")]
        job: JobId,
        #[serde(default)]
        plan: Plan,
        /// Which authority approved it. Absent in a recording written before
        /// signatures existed, which reads as [`ApprovedBy::Operator`].
        #[serde(default)]
        approved_by: Option<ApprovedBy>,
    },
    /// The one rewrite in an otherwise write-once session: from here on the
    /// job's turns are sent as this summary.
    #[serde(alias = "task_closed")]
    JobClosed {
        #[serde(alias = "task")]
        job: JobId,
        summary: String,
        #[serde(default)]
        by: Option<ClosedBy>,
    },
    /// What left the window, and stays out.
    Evicted {
        turn: TurnId,
        turns: Vec<TurnId>,
        tokens: u32,
        counter: Counter,
        policy: Eviction,
    },
    /// The fold stops applying. Not an undo — nothing was deleted.
    #[serde(alias = "task_reopened")]
    JobReopened {
        #[serde(alias = "task")]
        job: JobId,
    },
    /// The plan was put up and turned down.
    #[serde(alias = "task_rejected")]
    JobRejected {
        #[serde(alias = "task")]
        job: JobId,
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
        /// What a subprocess did, for the tool where that is a fact: the exit
        /// code, the signal, the two streams unmixed. Absent for every
        /// in-process tool and in every recording made before it existed, which
        /// is why it is additive rather than a format bump — an older reader
        /// ignores a field it does not know, and this variant is not new.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        command: Option<crate::tools::CommandResult>,
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
                    command: outcome.command,
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
            | Self::ToolResult { turn, .. }
            // The turn that cut, not the turns that left: this is a thing the
            // selection for `turn` did.
            | Self::Evicted { turn, .. } => Some(*turn),
            // A task spans turns and its lifecycle happens between them, and a
            // refusal is about the ask rather than about a turn — three of the
            // four happen when there is no turn to name.
            Self::JobProposed { .. }
            | Self::JobApproved { .. }
            | Self::JobClosed { .. }
            | Self::JobReopened { .. }
            | Self::JobRejected { .. }
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
    fn an_eviction_names_the_turns_that_left_and_who_counted_them() {
        let json = roundtrip(&ServerMessage::Evicted {
            turn: 12,
            turns: vec![1, 2, 3],
            tokens: 843,
            counter: Counter::Model {
                id: "qwen2.5-coder:7b".into(),
            },
            policy: Eviction::Block { low_water: 0.5 },
        });
        assert_eq!(json["type"], "evicted");
        assert_eq!(json["turn"], 12, "the turn whose selection cut");
        assert_eq!(json["turns"], serde_json::json!([1, 2, 3]));
        assert_eq!(json["counter"]["id"], "qwen2.5-coder:7b");
        assert_eq!(
            json["policy"]["policy"], "block",
            "which cut it was, beside how much it took",
        );
        assert_eq!(
            ServerMessage::Evicted {
                turn: 12,
                turns: vec![1],
                tokens: 0,
                counter: Counter::Approximate,
                policy: Eviction::Turn,
            }
            .turn(),
            Some(12),
            "it is a thing the cutting turn did, not a message about the turns that left",
        );
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
    fn a_job_message_is_about_a_job_rather_than_a_turn() {
        let json = roundtrip(&ServerMessage::JobProposed {
            job: 2,
            objective: "add a --dry-run flag".into(),
            plan: Plan {
                tasks: vec!["read the CLI".into()],
                files: vec!["crates/luu/src/lib.rs".into()],
                writes: vec![],
                commands: vec!["cargo".into()],
                closes_on: None,
                network: false,
                ..Plan::default()
            },
            source: Some(PlanSource::Model),
        });
        assert_eq!(json["type"], "job_proposed");
        assert_eq!(json["plan"]["files"][0], "crates/luu/src/lib.rs");
        assert_eq!(
            json["source"], "model",
            "who wrote the plan, which an empty one cannot be asked",
        );

        // An older recording has no `source`, and it stays unknown rather than
        // becoming a claim the recording never made.
        let older: ServerMessage =
            serde_json::from_str(r#"{"type":"task_proposed","task":1,"objective":"x","plan":{}}"#)
                .expect("an added optional field is not a parse error");
        assert!(matches!(
            older,
            ServerMessage::JobProposed { source: None, .. }
        ));
        assert_eq!(
            ServerMessage::JobApproved {
                job: 2,
                plan: Plan::default(),
                approved_by: None,
            }
            .turn(),
            None,
            "a job spans turns; pinning it to one would be a guess",
        );
    }

    #[test]
    fn the_client_half_of_the_lifecycle_parses_from_the_wire() {
        for (text, expected) in [
            (r#"{"type":"approve_job","job":2}"#, "ApproveJob"),
            (r#"{"type":"reject_job","job":2}"#, "RejectJob"),
            (r#"{"type":"close_job","job":2}"#, "CloseJob"),
            (r#"{"type":"reopen_job","job":2}"#, "ReopenJob"),
            (r#"{"type":"approve_task","task":2}"#, "ApproveJob"),
            (r#"{"type":"reject_task","task":2}"#, "RejectJob"),
            (r#"{"type":"close_task","task":2}"#, "CloseJob"),
            (r#"{"type":"reopen_task","task":2}"#, "ReopenJob"),
        ] {
            let parsed: ClientMessage = serde_json::from_str(text).unwrap();
            assert!(
                format!("{parsed:?}").starts_with(expected),
                "{text} parsed as {parsed:?}",
            );
        }
    }

    #[test]
    fn the_client_says_what_it_speaks_and_an_approval_can_carry_a_signature() {
        let hello: ClientMessage =
            serde_json::from_str(r#"{"type":"hello","protocol":5,"format":7}"#).unwrap();
        assert!(matches!(
            hello,
            ClientMessage::Hello {
                protocol: 5,
                format: 7
            }
        ));

        // A client that only drives the wire and never reads a recording says
        // nothing about the format, and that is not a mismatch.
        let bare: ClientMessage = serde_json::from_str(r#"{"type":"hello","protocol":5}"#).unwrap();
        assert!(matches!(bare, ClientMessage::Hello { format: 0, .. }));

        let signed: ClientMessage = serde_json::from_str(
            r#"{"type":"approve_job","job":2,"signature":{"by":"jgermade","sig":"ab"}}"#,
        )
        .unwrap();
        let ClientMessage::ApproveJob { signature, .. } = signed else {
            panic!("an approval");
        };
        assert_eq!(signature.expect("the signature").by, "jgermade");

        // And an approval from before signatures existed is still an approval.
        let unsigned: ClientMessage =
            serde_json::from_str(r#"{"type":"approve_job","job":2}"#).unwrap();
        assert!(matches!(
            unsigned,
            ClientMessage::ApproveJob {
                signature: None,
                ..
            }
        ));
    }

    #[test]
    fn an_approval_recorded_before_signatures_existed_still_parses() {
        let parsed: ServerMessage =
            serde_json::from_str(r#"{"type":"task_approved","task":1,"plan":{}}"#).unwrap();
        assert!(
            matches!(
                parsed,
                ServerMessage::JobApproved {
                    approved_by: None,
                    ..
                }
            ),
            "absent is not a claim about who approved; the reader decides it was the operator",
        );
    }

    #[test]
    fn a_turn_recorded_before_tasks_existed_still_parses() {
        let parsed: ServerMessage =
            serde_json::from_str(r#"{"type":"turn_started","turn":1,"prompt":"hola"}"#).unwrap();
        assert!(matches!(
            parsed,
            ServerMessage::TurnStarted { job: None, .. }
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
