//! `luu serve` — the local HTTP server behind the debug UI.
//!
//! Loopback by default and unauthenticated: it exposes an agent that runs
//! commands, so binding it anywhere else needs a decision nobody has made yet.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use agent_core::agent::run_agent_turn;
use agent_core::api::SessionView;
use agent_core::backend::{Backend, CompletionRequest};
use agent_core::context::{Budget, Context as AgentContext, TokenCounter};
use agent_core::protocol::{self, ClientMessage, ServerMessage, TurnId};
use agent_core::trace::TraceMessage;

use crate::session::{Agency, Event, PrefixTracker, Recorder, SYSTEM, now_ms, rendered};
use anyhow::{Context, Result};
use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::http::{StatusCode, Uri, header};
use axum::response::{IntoResponse, Response};
use axum::routing::get;
use futures_util::{SinkExt, StreamExt};
use tokio::sync::{Mutex, broadcast, mpsc, watch};

/// The UI, embedded in the binary.
///
/// `rust-embed` reads these from disk in debug builds and bakes them in for
/// release, which is exactly the split we want: editing a component must not
/// cost a `cargo build`, and a shipped binary must not need the files.
#[derive(rust_embed::Embed)]
#[folder = "ui/"]
struct Ui;

struct Session {
    next_turn: TurnId,
    current: Option<TurnId>,
    /// Present only while a turn is running.
    cancel: Option<watch::Sender<bool>>,
    /// The conversation so far. In memory: enough to measure a context
    /// strategy, and lost on restart until sessions are persisted.
    context: AgentContext,
    /// Beside the context, because what it measures is a property of two
    /// consecutive prompts of *this* session.
    prefix: PrefixTracker,
}

/// The id the live session is served under. There is one until sessions are
/// persisted, and naming it beats a magic string at four call sites.
pub const LIVE_SESSION: &str = "live";

struct App {
    backend: Arc<dyn Backend>,
    model: String,
    session: Mutex<Session>,
    events: broadcast::Sender<Event>,
    recorder: Option<Recorder>,
    counter: Arc<dyn TokenCounter>,
    budget: Budget,
    agency: Agency,
    /// The read side, folded from the same events the sockets carry — so
    /// `GET /api/...` can never disagree with what a client watched happen.
    view: Mutex<SessionView>,
    started_at: u64,
}

impl App {
    /// Publishes one event to every client and to the record, in that order.
    async fn publish(&self, event: Event) {
        if let Some(recorder) = &self.recorder {
            recorder.write(&event);
        }
        {
            let at_ms = now_ms().saturating_sub(self.started_at);
            let mut view = self.view.lock().await;
            match &event {
                Event::Protocol(message) => view.apply_protocol(at_ms, message),
                Event::Trace(message) => view.apply_trace(at_ms, message),
            }
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
    pub budget: Budget,
    pub counter: Arc<dyn TokenCounter>,
    pub agency: Agency,
}

pub async fn serve(options: ServeOptions) -> Result<()> {
    let ServeOptions {
        address,
        backend,
        model,
        record,
        budget,
        counter,
        agency,
    } = options;
    let started_at = now_ms();

    let recorder = match record {
        Some(path) => Some(
            Recorder::create(
                &path,
                backend.name(),
                &model,
                budget,
                counter.id(),
                started_at,
            )
            .await?,
        ),
        None => None,
    };

    let backend_name = backend.name().to_string();
    let model_name = model.clone();
    let app = Arc::new(App {
        backend,
        model,
        session: Mutex::new(Session {
            next_turn: 1,
            current: None,
            cancel: None,
            context: AgentContext::new(SYSTEM).with_tools(agency.definitions()),
            prefix: PrefixTracker::default(),
        }),
        events: broadcast::channel(1024).0,
        recorder,
        counter,
        budget,
        agency,
        view: Mutex::new({
            let mut view = SessionView::new(LIVE_SESSION, backend_name, &model_name);
            view.started_at = started_at;
            view
        }),
        started_at,
    });

    let router = Router::new()
        .route("/ws", get(protocol_socket))
        .route("/ws/trace", get(trace_socket))
        // The read side. Every path also answers with a `.json` suffix, because
        // that is the only shape a static host can mirror — see `luu export`.
        //
        // Where the suffix sits on a parameter it is not a route of its own:
        // axum allows only one parameter per segment, so `{id}` captures
        // `completed-turn.json` whole and the handler strips it. Only the
        // literal segments get a second route.
        .route("/api/sessions", get(list_sessions))
        .route("/api/sessions.json", get(list_sessions))
        .route("/api/sessions/{id}", get(get_session))
        .route("/api/sessions/{id}/turns", get(get_turns))
        .route("/api/sessions/{id}/turns.json", get(get_turns))
        .route("/api/sessions/{id}/turns/{turn}", get(get_turn))
        .route("/api/sessions/{id}/turns/{turn}/prompt", get(get_prompt))
        .route(
            "/api/sessions/{id}/turns/{turn}/prompt.json",
            get(get_prompt),
        )
        .route("/api/sessions/{id}/context", get(get_context))
        .route("/api/sessions/{id}/context.json", get(get_context))
        .route("/", get(|| serve_asset("index.html")))
        .route("/{*path}", get(asset_handler))
        .with_state(AppRouterState { app: app.clone() });

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

async fn protocol_socket(ws: WebSocketUpgrade, State(state): State<AppRouterState>) -> Response {
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
    let AppRouterState { app } = state;
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
    let Ok(json) = serde_json::to_string(&hello) else {
        return;
    };
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
                        start_turn(app.clone(), text).await;
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

async fn start_turn(app: Arc<App>, prompt: String) {
    let (turn, cancel_rx, selection, prompt_sent, reuse) = {
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
        // Selected under the same lock that hands out the turn number, so the
        // history a turn is built from is the history at the moment it started.
        let selection = session
            .context
            .select(&prompt, &[], app.budget, app.counter.as_ref());
        // Measured under the same lock, so two turns cannot interleave and
        // measure themselves against each other's prompt.
        let prompt_sent = rendered(&selection.messages);
        let reuse = session
            .prefix
            .measure(turn, &prompt_sent, app.counter.as_ref());
        (turn, rx, selection, prompt_sent, reuse)
    };

    app.publish(Event::Protocol(ServerMessage::TurnStarted {
        turn,
        prompt: prompt.clone(),
    }))
    .await;
    app.publish(Event::Trace(TraceMessage::Prompt {
        turn,
        text: prompt_sent,
    }))
    .await;
    if let Some(reuse) = reuse {
        app.publish(Event::Trace(reuse)).await;
    }
    // Published before the call: this is what we decided to send. A turn that
    // gets cancelled has a budget too, which the old after-the-fact version
    // could not report.
    app.publish(Event::Trace(TraceMessage::Budget {
        turn,
        limit: selection.limit,
        counter: selection.counter.clone(),
        buckets: selection.buckets.clone(),
    }))
    .await;

    let request = CompletionRequest {
        model: app.model.clone(),
        messages: selection.messages,
    };

    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::channel(256);
        let forwarder = {
            let app = app.clone();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    app.publish(Event::Protocol(ServerMessage::from_turn_event(turn, event)))
                        .await;
                }
            })
        };

        let outcome = run_agent_turn(
            app.backend.as_ref(),
            request,
            app.agency.tools.as_ref(),
            app.agency.sandbox.as_ref(),
            app.agency.max_steps,
            tx,
            cancel_rx,
        )
        .await;
        let _ = forwarder.await;

        let mut session = app.session.lock().await;
        // A cancelled turn keeps its partial answer: the user saw it, so the
        // model should too. A turn that produced nothing at all is not
        // remembered — an empty assistant message is not a thing that happened,
        // and several chat templates render it as a prompt to continue.
        if !outcome.text.is_empty() || !outcome.steps.is_empty() {
            session.context.push_turn_with_steps(
                prompt,
                outcome.text,
                vec![],
                outcome.steps,
                app.counter.as_ref(),
            );
        }
        session.current = None;
        session.cancel = None;
    });
}

/// Strips the `.json` a static mirror needs, so both spellings reach one handler.
fn bare(id: &str) -> &str {
    id.strip_suffix(".json").unwrap_or(id)
}

fn not_found(what: &str) -> Response {
    (StatusCode::NOT_FOUND, format!("no such {what}")).into_response()
}

async fn list_sessions(State(state): State<AppRouterState>) -> Response {
    let view = state.app.view.lock().await;
    Json(vec![view.summary()]).into_response()
}

async fn get_session(Path(id): Path<String>, State(state): State<AppRouterState>) -> Response {
    let view = state.app.view.lock().await;
    match bare(&id) == view.id {
        true => Json(view.clone()).into_response(),
        false => not_found("session"),
    }
}

async fn get_turns(Path(id): Path<String>, State(state): State<AppRouterState>) -> Response {
    let view = state.app.view.lock().await;
    match bare(&id) == view.id {
        true => Json(view.turns.clone()).into_response(),
        false => not_found("session"),
    }
}

/// The turn segment carries the `.json` when it is the last one, so it is
/// stripped before parsing rather than after.
fn parse_turn(turn: &str) -> Option<TurnId> {
    bare(turn).parse().ok()
}

async fn get_turn(
    Path((id, turn)): Path<(String, String)>,
    State(state): State<AppRouterState>,
) -> Response {
    let view = state.app.view.lock().await;
    let Some(number) = parse_turn(&turn) else {
        return not_found("turn");
    };
    match (bare(&id) == view.id).then(|| view.turn(number)).flatten() {
        Some(turn) => Json(turn.clone()).into_response(),
        None => not_found("turn"),
    }
}

async fn get_prompt(
    Path((id, turn)): Path<(String, String)>,
    State(state): State<AppRouterState>,
) -> Response {
    let view = state.app.view.lock().await;
    let Some(number) = parse_turn(&turn) else {
        return not_found("turn");
    };
    let found = (bare(&id) == view.id).then(|| view.turn(number)).flatten();
    match found {
        Some(turn) => Json(serde_json::json!({
            "turn": turn.turn,
            "text": turn.prompt_sent,
        }))
        .into_response(),
        None => not_found("turn"),
    }
}

/// The budget of the newest turn that has one — the panel asks "what is the
/// context doing now", and a running turn has not reported yet.
async fn get_context(Path(id): Path<String>, State(state): State<AppRouterState>) -> Response {
    let view = state.app.view.lock().await;
    if bare(&id) != view.id {
        return not_found("session");
    }
    let latest = view.turns.iter().rev().find(|t| t.budget.is_some());
    Json(serde_json::json!({
        "turn": latest.map(|t| t.turn),
        "budget": latest.and_then(|t| t.budget.clone()),
    }))
    .into_response()
}
