//! `luu serve` — the local HTTP server behind the debug UI.
//!
//! Loopback by default and unauthenticated: it exposes an agent that runs
//! commands, so binding it anywhere else needs a decision nobody has made yet.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use agent_core::backend::{Backend, CompletionRequest, Message};
use agent_core::protocol::{self, ClientMessage, ServerMessage, TurnId};
use agent_core::record::{self, RecordLine};
use agent_core::trace::{Bucket, TraceMessage};
use agent_core::turn::run_turn;
use anyhow::{Context, Result};
use axum::Router;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tokio::io::AsyncWriteExt;
use tokio::sync::{Mutex, broadcast, mpsc, watch};

/// The UI, embedded in the binary.
///
/// `rust-embed` reads these from disk in debug builds and bakes them in for
/// release, which is exactly the split we want: editing a component must not
/// cost a `cargo build`, and a shipped binary must not need the files.
#[derive(rust_embed::Embed)]
#[folder = "ui/"]
struct Ui;

/// One message on its way to every connected client and to the record file.
#[derive(Clone)]
enum Event {
    Protocol(ServerMessage),
    Trace(TraceMessage),
}

struct Session {
    next_turn: TurnId,
    current: Option<TurnId>,
    /// Present only while a turn is running.
    cancel: Option<watch::Sender<bool>>,
}

struct App {
    backend: Arc<dyn Backend>,
    model: String,
    session: Mutex<Session>,
    events: broadcast::Sender<Event>,
    recorder: Option<mpsc::UnboundedSender<RecordLine>>,
    started_at: u64,
}

fn now_ms() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).unwrap_or_default().as_millis() as u64
}

impl App {
    /// Publishes one event to every client and to the record, in that order.
    fn publish(&self, event: Event) {
        if let Some(recorder) = &self.recorder {
            let at_ms = now_ms().saturating_sub(self.started_at);
            let line = match &event {
                Event::Protocol(message) => {
                    RecordLine::Protocol { at_ms, message: message.clone() }
                }
                Event::Trace(message) => RecordLine::Trace { at_ms, message: message.clone() },
            };
            let _ = recorder.send(line);
        }
        // No subscribers is the ordinary state of a server nobody has opened yet.
        let _ = self.events.send(event);
    }
}

pub struct ServeOptions {
    pub address: SocketAddr,
    pub backend: Arc<dyn Backend>,
    pub model: String,
    pub record: Option<PathBuf>,
    pub context_limit: u32,
}

pub async fn serve(options: ServeOptions) -> Result<()> {
    let ServeOptions { address, backend, model, record, context_limit } = options;
    let started_at = now_ms();

    let recorder = match record {
        Some(path) => Some(spawn_recorder(path, &backend, &model, started_at).await?),
        None => None,
    };

    let app = Arc::new(App {
        backend,
        model,
        session: Mutex::new(Session { next_turn: 1, current: None, cancel: None }),
        events: broadcast::channel(1024).0,
        recorder,
        started_at,
    });

    let router = Router::new()
        .route("/ws", get(protocol_socket))
        .route("/ws/trace", get(trace_socket))
        .route("/", get(|| serve_asset("index.html")))
        .route("/{*path}", get(asset_handler))
        .with_state(AppRouterState { app: app.clone(), context_limit });

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("binding {address}"))?;

    println!("luu serve → http://{address}");
    axum::serve(listener, router).await.context("serving")?;
    Ok(())
}

#[derive(Clone)]
struct AppRouterState {
    app: Arc<App>,
    context_limit: u32,
}

async fn spawn_recorder(
    path: PathBuf,
    backend: &Arc<dyn Backend>,
    model: &str,
    started_at: u64,
) -> Result<mpsc::UnboundedSender<RecordLine>> {
    let mut file = tokio::fs::File::create(&path)
        .await
        .with_context(|| format!("creating {}", path.display()))?;

    let header = RecordLine::Header {
        format: record::FORMAT,
        protocol: protocol::VERSION,
        backend: backend.name().to_string(),
        model: model.to_string(),
        started_at,
    };
    file.write_all(format!("{}\n", serde_json::to_string(&header)?).as_bytes()).await?;

    let (tx, mut rx) = mpsc::unbounded_channel::<RecordLine>();
    tokio::spawn(async move {
        while let Some(line) = rx.recv().await {
            let Ok(json) = serde_json::to_string(&line) else { continue };
            if file.write_all(format!("{json}\n").as_bytes()).await.is_err() {
                break;
            }
            // Flushed per line: a session worth replaying is usually one that
            // ended badly, and a buffered tail is the part you needed.
            let _ = file.flush().await;
        }
    });
    Ok(tx)
}

async fn asset_handler(uri: Uri) -> Response {
    serve_asset(uri.path().trim_start_matches('/')).await
}

async fn serve_asset(path: &str) -> Response {
    match Ui::get(path) {
        Some(file) => {
            let mime = mime_guess::from_path(path).first_or_octet_stream();
            ([(header::CONTENT_TYPE, mime.as_ref())], file.data).into_response()
        }
        None => (StatusCode::NOT_FOUND, "not found").into_response(),
    }
}

async fn protocol_socket(
    ws: WebSocketUpgrade,
    State(state): State<AppRouterState>,
) -> Response {
    ws.on_upgrade(move |socket| run_protocol_socket(socket, state))
}

async fn trace_socket(ws: WebSocketUpgrade, State(state): State<AppRouterState>) -> Response {
    ws.on_upgrade(move |socket| run_trace_socket(socket, state.app))
}

/// The trace channel is send-only: it explains the agent, it never drives it.
async fn run_trace_socket(socket: WebSocket, app: Arc<App>) {
    let (mut sink, mut stream) = socket.split();
    let mut events = app.events.subscribe();

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(Event::Trace(message)) => {
                    let Ok(json) = serde_json::to_string(&message) else { continue };
                    if sink.send(WsMessage::Text(json.into())).await.is_err() {
                        return;
                    }
                }
                Ok(Event::Protocol(_)) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            // Only to notice the client going away.
            incoming = stream.next() => if incoming.is_none() { return },
        }
    }
}

async fn run_protocol_socket(socket: WebSocket, state: AppRouterState) {
    let AppRouterState { app, context_limit } = state;
    let (mut sink, mut stream) = socket.split();
    let mut events = app.events.subscribe();

    let hello = {
        let session = app.session.lock().await;
        ServerMessage::Hello {
            protocol: protocol::VERSION,
            backend: app.backend.name().to_string(),
            model: app.model.clone(),
            turn: session.current,
        }
    };
    let Ok(json) = serde_json::to_string(&hello) else { return };
    if sink.send(WsMessage::Text(json.into())).await.is_err() {
        return;
    }

    loop {
        tokio::select! {
            event = events.recv() => match event {
                Ok(Event::Protocol(message)) => {
                    let Ok(json) = serde_json::to_string(&message) else { continue };
                    if sink.send(WsMessage::Text(json.into())).await.is_err() {
                        return;
                    }
                }
                Ok(Event::Trace(_)) => continue,
                Err(broadcast::error::RecvError::Lagged(_)) => continue,
                Err(broadcast::error::RecvError::Closed) => return,
            },
            incoming = stream.next() => {
                let Some(Ok(WsMessage::Text(text))) = incoming else {
                    // A close, an error, or a frame we do not speak.
                    if matches!(incoming, None | Some(Err(_))) {
                        return;
                    }
                    continue;
                };
                match serde_json::from_str::<ClientMessage>(&text) {
                    Ok(ClientMessage::Prompt { text }) => {
                        start_turn(app.clone(), text, context_limit).await;
                    }
                    Ok(ClientMessage::Cancel) => {
                        let session = app.session.lock().await;
                        if let Some(cancel) = &session.cancel {
                            let _ = cancel.send(true);
                        }
                    }
                    // Unparseable input from one client must not take the
                    // server down for the others.
                    Err(_) => continue,
                }
            }
        }
    }
}

async fn start_turn(app: Arc<App>, prompt: String, context_limit: u32) {
    let (turn, cancel_rx) = {
        let mut session = app.session.lock().await;
        if session.current.is_some() {
            // One turn at a time until sessions exist.
            return;
        }
        let turn = session.next_turn;
        session.next_turn += 1;
        session.current = Some(turn);

        let (tx, rx) = watch::channel(false);
        session.cancel = Some(tx);
        (turn, rx)
    };

    let messages = vec![
        Message::system("You are Loude, a concise local coding agent."),
        Message::user(prompt.clone()),
    ];

    app.publish(Event::Protocol(ServerMessage::TurnStarted { turn, prompt }));

    // The exact text handed to the model. Once a context manager exists this is
    // what its prompt-diff panel reads.
    let sent = messages
        .iter()
        .map(|m| format!("<|{:?}|>\n{}", m.role, m.content))
        .collect::<Vec<_>>()
        .join("\n\n");
    app.publish(Event::Trace(TraceMessage::Prompt { turn, text: sent }));

    let request = CompletionRequest { model: app.model.clone(), messages };

    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::channel(256);
        let forwarder = {
            let app = app.clone();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    app.publish(Event::Protocol(ServerMessage::from_turn_event(turn, event)));
                }
            })
        };

        let outcome = run_turn(app.backend.as_ref(), request, tx, cancel_rx).await;
        let _ = forwarder.await;

        // Real numbers only: with no usage there is nothing to plot, and a zero
        // would read as a measurement.
        if let Some(usage) = outcome.usage {
            app.publish(Event::Trace(TraceMessage::Budget {
                turn,
                limit: context_limit,
                buckets: vec![
                    Bucket::new("prompt", usage.prompt_tokens),
                    Bucket::new("completion", usage.completion_tokens),
                ],
            }));
        }

        let mut session = app.session.lock().await;
        session.current = None;
        session.cancel = None;
    });
}
