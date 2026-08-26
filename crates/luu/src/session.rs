//! What `chat` and `serve` must not each invent.
//!
//! Both drive the same loop, and both need the same prompt assembled the same
//! way. Two call sites building it by hand is exactly what the byte-identical
//! prefix commitment in `AGENTS.md` warns about, so there is one.

use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::backend::Message;
use agent_core::protocol::{self, ServerMessage};
use agent_core::record::{self, RecordLine};
use agent_core::trace::TraceMessage;
use anyhow::{Context, Result};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc;

/// The fixed head of every prompt. Kept as one `&'static str` because the
/// prompt cache reuses it byte for byte — a formatting change here is a cache
/// miss on every call, and nothing fails to tell you.
pub const SYSTEM: &str = "You are Loude, a concise local coding agent.";

/// The one place a turn's messages are assembled.
pub fn messages_for(prompt: impl Into<String>) -> Vec<Message> {
    vec![Message::system(SYSTEM), Message::user(prompt)]
}

/// What the backend is about to receive, as one string, for the trace channel.
/// Not the wire format any backend uses — it is a rendering, and the panel that
/// diffs two of them only needs them to be rendered the same way twice.
pub fn rendered(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|m| format!("<|{:?}|>\n{}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n\n")
}

/// One message on its way to clients, to the record, or both.
#[derive(Clone)]
pub enum Event {
    Protocol(ServerMessage),
    Trace(TraceMessage),
}

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Writes the JSON-lines record, on its own task so a slow disk never stalls a
/// turn.
pub struct Recorder {
    lines: mpsc::UnboundedSender<RecordLine>,
    started_at: u64,
}

impl Recorder {
    pub async fn create(path: &Path, backend: &str, model: &str, started_at: u64) -> Result<Self> {
        if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            tokio::fs::create_dir_all(parent)
                .await
                .with_context(|| format!("creating {}", parent.display()))?;
        }

        let mut file = tokio::fs::File::create(path)
            .await
            .with_context(|| format!("creating {}", path.display()))?;

        let header = RecordLine::Header {
            format: record::FORMAT,
            protocol: protocol::VERSION,
            backend: backend.to_string(),
            model: model.to_string(),
            started_at,
        };
        file.write_all(format!("{}\n", serde_json::to_string(&header)?).as_bytes())
            .await?;

        let (lines, mut rx) = mpsc::unbounded_channel::<RecordLine>();
        tokio::spawn(async move {
            while let Some(line) = rx.recv().await {
                let Ok(json) = serde_json::to_string(&line) else {
                    continue;
                };
                if file
                    .write_all(format!("{json}\n").as_bytes())
                    .await
                    .is_err()
                {
                    break;
                }
                // Flushed per line: a session worth replaying is usually one
                // that ended badly, and a buffered tail is the part you needed.
                let _ = file.flush().await;
            }
        });

        Ok(Self { lines, started_at })
    }

    pub fn write(&self, event: &Event) {
        let at_ms = now_ms().saturating_sub(self.started_at);
        let line = match event {
            Event::Protocol(message) => RecordLine::Protocol {
                at_ms,
                message: message.clone(),
            },
            Event::Trace(message) => RecordLine::Trace {
                at_ms,
                message: message.clone(),
            },
        };
        let _ = self.lines.send(line);
    }
}
