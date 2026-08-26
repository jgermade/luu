//! The record format: the same messages, on disk.
//!
//! JSON-lines. The first line is a [`RecordLine::Header`]; every line after it
//! is one protocol or trace message with the milliseconds since the header.
//!
//! The header is what makes two runs comparable — which is the only reason the
//! format exists. A file of bare frames replays, but cannot answer "which model
//! produced this", and comparing a run against one whose model you have to
//! remember is the failure mode the format was meant to remove.

use serde::{Deserialize, Serialize};

use crate::protocol::ServerMessage;
use crate::trace::TraceMessage;

/// Bumped when an older reader could not make sense of a newer file.
pub const FORMAT: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "channel", rename_all = "snake_case")]
pub enum RecordLine {
    Header {
        format: u32,
        protocol: u32,
        backend: String,
        model: String,
        /// Unix milliseconds. Every later line is relative to this.
        started_at: u64,
    },
    Protocol { at_ms: u64, message: ServerMessage },
    Trace { at_ms: u64, message: TraceMessage },
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{self, ServerMessage};

    #[test]
    fn a_header_and_a_message_are_one_line_each() {
        let header = RecordLine::Header {
            format: FORMAT,
            protocol: protocol::VERSION,
            backend: "mock".into(),
            model: "mock".into(),
            started_at: 1_700_000_000_000,
        };
        let token = RecordLine::Protocol {
            at_ms: 12,
            message: ServerMessage::Token { turn: 1, text: "hola".into() },
        };

        let lines = format!(
            "{}\n{}",
            serde_json::to_string(&header).unwrap(),
            serde_json::to_string(&token).unwrap()
        );
        assert_eq!(lines.lines().count(), 2, "one line per record");

        let parsed: Vec<RecordLine> =
            lines.lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert!(matches!(parsed[0], RecordLine::Header { format: 1, .. }));
        assert!(matches!(parsed[1], RecordLine::Protocol { at_ms: 12, .. }));
    }
}
