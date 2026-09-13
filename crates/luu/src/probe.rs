//! `luu probe` — what a run did about *calling* a tool, per prompt.
//!
//! [`parse_call`](agent_core::tools::parse_call) answers whether a call could
//! be read out of a reply. This answers the two questions it hides — whether
//! generation stopped at the fence, and whether the format held at all — over a
//! recording, so the taxonomy can change without a box being booked twice. See
//! `RECORD/2026-09-13.a-probe-for-tool-calls.completed.md`.
//!
//! Only the **first reply of each turn** is scored: a turn that drifts never
//! gets a second call, so counting later steps would average a question about
//! the first call with a question about recovery.

use agent_core::protocol::{ServerMessage, TurnId};
use agent_core::record::RecordLine;
use agent_core::tools::CallShape;

/// One prompt, and what its first reply did.
#[derive(Debug, Clone)]
pub struct Row {
    pub turn: TurnId,
    pub prompt: String,
    pub shape: CallShape,
    /// The tool the loop actually executed, when one was executed. `None` under
    /// every shape but `Parsed` and `Continued` — and under a recovered drift,
    /// where it is the parse's doing rather than the format's.
    pub called: Option<String>,
    /// What the key says this prompt needed, when there is a key.
    pub expected: Option<String>,
}

impl Row {
    /// Whether this prompt got what it asked for: the format held, generation
    /// stopped, and the call was the one the key names. A `Parsed` reply that
    /// called the wrong tool is not a success, which is the whole reason the
    /// key is on disk.
    pub fn is_hit(&self) -> bool {
        self.shape == CallShape::Parsed
            && match (&self.called, &self.expected) {
                (Some(called), Some(expected)) => called == expected,
                (_, None) => true,
                (None, Some(_)) => false,
            }
    }
}

#[derive(Debug, Clone, Default)]
pub struct Score {
    pub rows: Vec<Row>,
}

impl Score {
    pub fn count(&self, shape: CallShape) -> usize {
        self.rows.iter().filter(|row| row.shape == shape).count()
    }

    /// Drift under either sub-count — what the *format* did, which is the
    /// number a grammar arm would be read against.
    pub fn drifted(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| matches!(row.shape, CallShape::Drifted { .. }))
            .count()
    }

    /// Of those, the ones the text parse turned into a call anyway. It is what
    /// `bare_object` is buying, and therefore what dropping it would cost.
    pub fn recovered(&self) -> usize {
        self.count(CallShape::Drifted { recovered: true })
    }

    pub fn hits(&self) -> usize {
        self.rows.iter().filter(|row| row.is_hit()).count()
    }

    /// The wrong tool, called perfectly. Separate from every shape above,
    /// because the format holding and the choice being right are two results
    /// and a probe that adds them can report neither.
    pub fn wrong_tool(&self) -> usize {
        self.rows
            .iter()
            .filter(|row| {
                matches!(row.shape, CallShape::Parsed | CallShape::Continued)
                    && matches!((&row.called, &row.expected),
                        (Some(called), Some(expected)) if called != expected)
            })
            .count()
    }
}

/// Scores a recording against the tool names the run offered, and a key.
///
/// The key is one tool name per prompt, in order, and may be empty: without it
/// every shape is still counted and only *the wrong tool, called perfectly*
/// goes unmeasured.
pub fn score(lines: &[RecordLine], names: &[&str], key: &[String]) -> Score {
    let mut rows: Vec<Row> = Vec::new();
    // The first reply of the turn, as it streamed, until the loop either called
    // something or ended the turn.
    let mut reply = String::new();
    let mut open: Option<(TurnId, String)> = None;
    let mut first_call: Option<String> = None;
    let mut closed = false;

    let flush =
        |turn: TurnId, prompt: String, reply: &str, called: Option<String>, rows: &mut Vec<Row>| {
            let expected = key.get(rows.len()).cloned();
            rows.push(Row {
                turn,
                prompt,
                shape: agent_core::tools::shape_of(reply, names.iter().copied()),
                called,
                expected,
            });
        };

    for line in lines {
        let RecordLine::Protocol { message, .. } = line else {
            continue;
        };
        match message {
            ServerMessage::TurnStarted { turn, prompt, .. } => {
                if let Some((turn, prompt)) = open.take() {
                    flush(turn, prompt, &reply, first_call.take(), &mut rows);
                }
                open = Some((*turn, prompt.clone()));
                reply.clear();
                first_call = None;
                closed = false;
            }
            ServerMessage::Token { text, .. } if !closed => reply.push_str(text),
            ServerMessage::ToolCall { step: 1, name, .. } => {
                first_call = Some(name.clone());
                closed = true;
            }
            ServerMessage::Ended { .. } | ServerMessage::Failed { .. } => closed = true,
            _ => {}
        }
    }
    if let Some((turn, prompt)) = open.take() {
        flush(turn, prompt, &reply, first_call.take(), &mut rows);
    }
    Score { rows }
}

/// The table, for a person. One line per prompt and then the counts, because a
/// probe whose per-prompt rows are not printed can only ever be argued with in
/// aggregate.
pub fn report(score: &Score) -> String {
    let mut out = String::new();
    for row in &score.rows {
        let shape = match row.shape {
            CallShape::Parsed => "parsed".to_string(),
            CallShape::Continued => "continued past the fence".to_string(),
            CallShape::Drifted { recovered: true } => "drifted (parse recovered)".to_string(),
            CallShape::Drifted { recovered: false } => "drifted".to_string(),
            CallShape::NoCall => "no call".to_string(),
        };
        let called = match (&row.called, &row.expected) {
            (Some(called), Some(expected)) if called != expected => {
                format!(" — called {called}, wanted {expected}")
            }
            (Some(called), _) => format!(" — {called}"),
            (None, Some(expected)) => format!(" — wanted {expected}"),
            (None, None) => String::new(),
        };
        let prompt: String = row.prompt.chars().take(48).collect();
        out.push_str(&format!(
            "{:>3}  {:<26}{}\n     {}\n",
            row.turn, shape, called, prompt
        ));
    }
    let n = score.rows.len();
    out.push_str(&format!(
        "\n{n} prompts: {} parsed, {} continued past the fence, {} drifted ({} recovered by the \
         text parse), {} no call\n",
        score.count(CallShape::Parsed),
        score.count(CallShape::Continued),
        score.drifted(),
        score.recovered(),
        score.count(CallShape::NoCall),
    ));
    if score.rows.iter().any(|row| row.expected.is_some()) {
        out.push_str(&format!(
            "{} of {n} hit the key; {} called the wrong tool\n",
            score.hits(),
            score.wrong_tool(),
        ));
    }
    out
}
