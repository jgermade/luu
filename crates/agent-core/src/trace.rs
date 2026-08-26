//! The trace channel: what the agent did, as opposed to what it produced.
//!
//! Separate from [`crate::protocol`] on purpose. Trace messages exist to
//! explain the context manager — the exact prompt, how the token budget was
//! split — and a stdio consumer driving the agent should never have to carry
//! them. They travel on their own channel, only when `--trace` asks for it.
//!
//! Most of this is stubbed: there is no context manager yet, so the budget has
//! one bucket and the numbers come from the backend. It ships now anyway, so
//! the channel exists from the first day rather than being retrofitted once
//! consumers already depend on protocol v1.

use serde::{Deserialize, Serialize};

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
        Self { name: name.into(), tokens }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum TraceMessage {
    /// The exact string handed to the model, before any of it was generated.
    /// The panel that diffs this against the previous turn is the one that
    /// shows how much of the stable prefix survived.
    Prompt { turn: TurnId, text: String },
    /// How the context was spent. `limit` is the model's window, so a bar can
    /// show the headroom and not just the split.
    Budget {
        turn: TurnId,
        limit: u32,
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
            limit: 8192,
            buckets: vec![Bucket::new("system", 120), Bucket::new("history", 640)],
        };
        let json = serde_json::to_value(&message).unwrap();
        assert_eq!(json["type"], "budget");
        assert_eq!(json["buckets"][1]["name"], "history");

        let back: TraceMessage = serde_json::from_value(json).unwrap();
        assert!(matches!(back, TraceMessage::Budget { buckets, .. } if buckets.len() == 2));
    }
}
