//! `luu` — the Loude CLI.
//!
//! The binary is installed under two names (`luu` and `loude`); both are thin
//! wrappers over [`run`].

use std::time::Duration;

use agent_core::backend::{Backend, CompletionRequest, mock::Mock, ollama::Ollama};
use agent_core::protocol::ServerMessage;
use agent_core::trace::{Bucket, TraceMessage};
use agent_core::turn::{EndReason, TurnEvent, run_turn};

use crate::session::{Event, Recorder, messages_for, now_ms, rendered};
use anyhow::Result;

pub mod serve;
pub mod session;
use clap::{Parser, Subcommand, ValueEnum};
use tokio::io::{AsyncWriteExt, stdout};
use tokio::sync::{mpsc, watch};

#[derive(Parser)]
#[command(
    name = "luu",
    version,
    about = "Local AI agent, built for small models"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Serve the debug UI and the agent protocol over HTTP.
    Serve {
        #[arg(long, default_value = "127.0.0.1:7878")]
        bind: std::net::SocketAddr,

        #[arg(long, value_enum, default_value_t = BackendKind::Mock)]
        backend: BackendKind,

        #[arg(long, default_value = "qwen2.5-coder:7b")]
        model: String,

        #[arg(long, default_value = "http://127.0.0.1:11434")]
        ollama_url: String,

        #[arg(long, default_value_t = 25)]
        mock_delay_ms: u64,

        /// Write every protocol and trace message to a replayable JSON-lines file.
        #[arg(long)]
        record: Option<std::path::PathBuf>,

        /// The model's context window, for the budget panel. 0 means unknown.
        #[arg(long, default_value_t = 0)]
        context_limit: u32,
    },

    /// Run one turn and stream the answer to stdout.
    Chat {
        /// The prompt. Reads stdin when omitted.
        prompt: Option<String>,

        #[arg(long, value_enum, default_value_t = BackendKind::Mock)]
        backend: BackendKind,

        #[arg(long, default_value = "qwen2.5-coder:7b")]
        model: String,

        #[arg(long, default_value = "http://127.0.0.1:11434")]
        ollama_url: String,

        /// Milliseconds between mock tokens, for exercising slow generation.
        #[arg(long, default_value_t = 25)]
        mock_delay_ms: u64,

        /// Stop the turn after this many milliseconds, to exercise cancelling.
        #[arg(long)]
        cancel_after_ms: Option<u64>,

        /// Write the turn to a replayable JSON-lines file. The same format
        /// `luu serve --record` writes, so the UI can load either.
        #[arg(long)]
        record: Option<std::path::PathBuf>,

        /// The model's context window, recorded with the budget. 0 means unknown.
        #[arg(long, default_value_t = 0)]
        context_limit: u32,
    },
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum BackendKind {
    Mock,
    Ollama,
}

fn build_backend(kind: BackendKind, ollama_url: &str, mock_delay_ms: u64) -> Box<dyn Backend> {
    match kind {
        BackendKind::Mock => Box::new(Mock::default().delay(Duration::from_millis(mock_delay_ms))),
        BackendKind::Ollama => Box::new(Ollama::new(ollama_url)),
    }
}

/// The mock ignores the model name; passing the Ollama default through would
/// just put a model nobody loaded into the record file's header.
fn model_for(backend: &dyn Backend, model: String) -> String {
    match backend.name() {
        "mock" => "mock".to_string(),
        _ => model,
    }
}

pub async fn run() -> Result<()> {
    let Cli { command } = Cli::parse();

    if let Command::Serve {
        bind,
        backend,
        model,
        ollama_url,
        mock_delay_ms,
        record,
        context_limit,
    } = command
    {
        let backend = build_backend(backend, &ollama_url, mock_delay_ms);
        let model = model_for(backend.as_ref(), model);
        return serve::serve(serve::ServeOptions {
            address: bind,
            backend: backend.into(),
            model,
            record,
            context_limit,
        })
        .await;
    }

    let Command::Chat {
        prompt,
        backend,
        model,
        ollama_url,
        mock_delay_ms,
        cancel_after_ms,
        record,
        context_limit,
    } = command
    else {
        unreachable!("serve is handled above");
    };

    let prompt = match prompt {
        Some(prompt) => prompt,
        None => std::io::read_to_string(std::io::stdin())?,
    };

    let backend = build_backend(backend, &ollama_url, mock_delay_ms);
    let model = model_for(backend.as_ref(), model);
    let messages = messages_for(prompt.clone());

    // One turn per `chat`, so it is always turn 1.
    const TURN: agent_core::protocol::TurnId = 1;

    let started_at = now_ms();
    let recorder = match &record {
        Some(path) => Some(Recorder::create(path, backend.name(), &model, started_at).await?),
        None => None,
    };
    if let Some(recorder) = &recorder {
        recorder.write(&Event::Protocol(ServerMessage::TurnStarted {
            turn: TURN,
            prompt,
        }));
        recorder.write(&Event::Trace(TraceMessage::Prompt {
            turn: TURN,
            text: rendered(&messages),
        }));
    }

    let request = CompletionRequest { model, messages };

    let (stop, cancel) = watch::channel(false);
    if let Some(ms) = cancel_after_ms {
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(ms)).await;
            let _ = stop.send(true);
        });
    }

    let (tx, mut rx) = mpsc::channel::<TurnEvent>(256);
    let recorder = std::sync::Arc::new(recorder);
    let printer = {
        let recorder = recorder.clone();
        tokio::spawn(async move {
            let mut out = stdout();
            while let Some(event) = rx.recv().await {
                if let Some(recorder) = recorder.as_ref() {
                    let message = ServerMessage::from_turn_event(TURN, event.clone());
                    recorder.write(&Event::Protocol(message));
                }
                match event {
                    // Written and flushed per token on purpose: this is the CLI's
                    // whole job at this stage — showing that generation streams.
                    TurnEvent::Token(text) => {
                        let _ = out.write_all(text.as_bytes()).await;
                        let _ = out.flush().await;
                    }
                    TurnEvent::Ended { reason, usage } => {
                        let _ = out.write_all(b"\n").await;
                        let counts = match usage {
                            Some(u) => format!(
                                "{} prompt / {} completion",
                                u.prompt_tokens, u.completion_tokens
                            ),
                            None => "usage unknown".to_string(),
                        };
                        let reason = match reason {
                            EndReason::Stop => "stop",
                            EndReason::Length => "length",
                            EndReason::Other => "other",
                            EndReason::Cancelled => "cancelled",
                        };
                        let _ = out
                            .write_all(format!("\n[{reason}] {counts}\n").as_bytes())
                            .await;
                        let _ = out.flush().await;
                    }
                    TurnEvent::Failed(error) => {
                        let _ = out
                            .write_all(format!("\n\n[failed] {error}\n").as_bytes())
                            .await;
                        let _ = out.flush().await;
                    }
                }
            }
        })
    };

    let outcome = run_turn(backend.as_ref(), request, tx, cancel).await;
    let _ = printer.await;

    if let (Some(recorder), Some(usage)) = (recorder.as_ref(), outcome.usage) {
        // Real numbers only: with no usage there is nothing to plot, and a zero
        // would read as a measurement.
        recorder.write(&Event::Trace(TraceMessage::Budget {
            turn: TURN,
            limit: context_limit,
            buckets: vec![
                Bucket::new("prompt", usage.prompt_tokens),
                Bucket::new("completion", usage.completion_tokens),
            ],
        }));
    }
    // Let the recorder task drain before the process exits.
    tokio::time::sleep(Duration::from_millis(50)).await;

    if outcome.error.is_some() {
        std::process::exit(1);
    }
    Ok(())
}
