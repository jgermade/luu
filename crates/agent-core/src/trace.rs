//! The trace channel: what the agent did, as opposed to what it produced.
//!
//! Separate from [`crate::protocol`] on purpose. Trace messages exist to
//! explain the context manager — the exact prompt, how the token budget was
//! split — and a stdio consumer driving the agent should never have to carry
//! them. They travel on their own channel, only when `--trace` asks for it.
//!
//! The budget here is measured before the call, by our own counter. The
//! backend's own count arrives afterwards on `Ended.usage`, and the two are
//! meant to differ: the chat template is applied where we cannot see it. The
//! *difference* is the number worth watching — a stable gap is template
//! overhead, a moving one means the template changed.

use serde::{Deserialize, Serialize};

use crate::context::Counter;
use crate::protocol::TurnId;

/// One slice of the token budget. The names are not an enum: the context
/// manager will invent buckets faster than a wire enum can be revised, and a
/// UI that renders a stacked bar does not need to know them in advance.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Bucket {
    pub name: String,
    pub tokens: u32,
}

impl Bucket {
    pub fn new(name: impl Into<String>, tokens: u32) -> Self {
        Self {
            name: name.into(),
            tokens,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraceMessage {
    /// The exact string handed to the model, before any of it was generated.
    /// The panel that diffs this against the previous turn is the one that
    /// shows how much of the stable prefix survived.
    Prompt { turn: TurnId, text: String },
    /// How the context was spent, decided before the call rather than reported
    /// after it — so a cancelled turn has a budget too.
    ///
    /// `limit` is the model's window and `None` means unknown: then there is no
    /// headroom to draw, and saying so beats drawing a bar against nothing.
    /// `counter` is which counter produced these numbers, because two runs
    /// measured differently are not comparable and nothing else would say so.
    Budget {
        turn: TurnId,
        limit: Option<u32>,
        counter: Counter,
        buckets: Vec<Bucket>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_budget_survives_the_wire() {
        let message = TraceMessage::Budget {
            turn: 1,
            limit: Some(8192),
            counter: Counter::Model { id: "qwen".into() },
            buckets: vec![Bucket::new("system", 120), Bucket::new("history", 640)],
        };
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["type"], "budget");
        assert_eq!(json["buckets"][1]["name"], "history");
        assert_eq!(json["counter"]["kind"], "model");

        let back: TraceMessage = serde_json::from_value(json).unwrap();
        assert!(matches!(back, TraceMessage::Budget { buckets, .. } if buckets.len() == 2));
    }

    #[test]
    fn an_unknown_window_is_null_rather_than_zero() {
        let message = TraceMessage::Budget {
            turn: 1,
            limit: None,
            counter: Counter::Approximate,
            buckets: vec![Bucket::new("prompt", 12)],
        };
        let json = serde_json::to_value(&message).unwrap();
        assert!(json["limit"].is_null(), "0 would plot as a window of zero");
        assert_eq!(json["counter"]["kind"], "approximate");
    }
}
