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
use crate::turn::{EndReason, TurnEvent};

/// Bumped when a change would break an older client. `Hello` carries it so a
/// mismatch is a message rather than a mystery.
pub const VERSION: u32 = 1;

/// Turns are numbered per session, in order, starting at 1.
pub type TurnId = u64;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ClientMessage {
    /// Start a turn.
    Prompt { text: String },
    /// Stop the turn in flight. Cancelling when nothing is running is not an
    /// error — a client that raced the end of a turn did nothing wrong.
    Cancel,
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
}

impl ServerMessage {
    /// The one conversion at the core/wire boundary.
    pub fn from_turn_event(turn: TurnId, event: TurnEvent) -> Self {
        match event {
            TurnEvent::Token(text) => Self::Token { turn, text },
            TurnEvent::Ended { reason, usage } => Self::Ended {
                turn,
                reason,
                usage,
            },
            TurnEvent::Failed(message) => Self::Failed { turn, message },
        }
    }

    /// Which turn this is about, if any.
    pub fn turn(&self) -> Option<TurnId> {
        match self {
            Self::Hello { turn, .. } => *turn,
            Self::TurnStarted { turn, .. }
            | Self::Token { turn, .. }
            | Self::Ended { turn, .. }
            | Self::Failed { turn, .. } => Some(*turn),
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
    fn a_client_prompt_parses_from_the_wire() {
        let parsed: ClientMessage =
            serde_json::from_str(r#"{"type":"prompt","text":"hola"}"#).unwrap();
        assert!(matches!(parsed, ClientMessage::Prompt { text } if text == "hola"));
    }

    #[test]
    fn turn_events_convert_without_losing_the_reason() {
        let event = TurnEvent::Ended {
            reason: EndReason::Length,
            usage: None,
        };
        let message = ServerMessage::from_turn_event(7, event);
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
