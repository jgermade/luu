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

use crate::context::{Counter, Eviction};
use crate::protocol::ServerMessage;
use crate::trace::TraceMessage;

/// Bumped when an older reader could not make sense of a newer file.
///
/// 2: the budget's `limit` became nullable and gained a `counter`, and the
/// header says what the run was measured against. A format-1 reader would
/// choke on `"limit": null`.
///
/// 3: the task lifecycle is on the protocol, so a recording carries
/// `task_proposed`, `task_approved`, `task_closed` and `task_reopened` lines. A
/// format-2 reader would choke on the first of them — an unknown `type` in a
/// tagged enum is a parse error, not a line to skip. Turns also gained the task
/// they belong to, and that half is backwards compatible: absent means none.
///
/// 4: `refused` lines, for the same reason as 3 — a new variant of a tagged
/// enum. `task_approved` also gained the plan as approved, and that half is
/// backwards compatible: absent means the proposal is the best answer there is.
///
/// 5: `evicted` lines — what the window dropped and never took back. Same rule
/// as 3 and 4, a new variant of a tagged enum. A format-4 file does not say what
/// its session forgot, and nothing can work it out afterwards: the floor lived
/// in memory. See `RECORD/2026-08-31.eviction-tombstones.completed.md`.
///
/// 6: `job_proposed`, `job_approved`, `job_closed`, `job_reopened`, `job_rejected`
/// lines, and plans carrying model tasks checklists. See
/// `RECORD/2026-09-04.from-tasks-to-jobs.completed.md`.
pub const FORMAT: u32 = 6;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "channel", rename_all = "snake_case")]
pub enum RecordLine {
    Header {
        format: u32,
        protocol: u32,
        backend: String,
        model: String,
        /// The window this run was measured against, `None` when unknown.
        /// Part of the header because two runs with different windows are not
        /// comparable, and by the time you are comparing them, nobody
        /// remembers which was which.
        #[serde(default)]
        context_limit: Option<u32>,
        /// Which counter produced this run's budgets. `None` in a format-1
        /// file, where the numbers came from the backend instead.
        #[serde(default)]
        counter: Option<Counter>,
        /// How the history gave way under pressure. Beside `context_limit` and
        /// `counter` for the same reason those are here: two runs under
        /// different policies are not comparable, and by the time anyone is
        /// comparing them nobody remembers which was which.
        ///
        /// `None` in a file recorded before the policy was a choice. Every one
        /// of those ran per-turn, but the file does not say so, and inventing
        /// the field on the reader's behalf would put a claim in a record that
        /// the record never made.
        #[serde(default)]
        eviction: Option<Eviction>,
        /// Unix milliseconds. Every later line is relative to this.
        started_at: u64,
    },
    Protocol {
        at_ms: u64,
        message: ServerMessage,
    },
    Trace {
        at_ms: u64,
        message: TraceMessage,
    },
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
            context_limit: Some(8192),
            counter: Some(Counter::Approximate),
            eviction: Some(Eviction::Turn),
            started_at: 1_700_000_000_000,
        };
        let token = RecordLine::Protocol {
            at_ms: 12,
            message: ServerMessage::Token {
                turn: 1,
                text: "hola".into(),
            },
        };

        let lines = format!(
            "{}\n{}",
            serde_json::to_string(&header).unwrap(),
            serde_json::to_string(&token).unwrap()
        );
        assert_eq!(lines.lines().count(), 2, "one line per record");

        let parsed: Vec<RecordLine> = lines
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(matches!(
            parsed[0],
            RecordLine::Header { format: FORMAT, .. }
        ));
        assert!(matches!(parsed[1], RecordLine::Protocol { at_ms: 12, .. }));
    }
}
