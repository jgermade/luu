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
use agent_core::task::{Plan, Proposal, TaskId, parse_plan};
use agent_core::trace::TraceMessage;
use agent_core::turn::{EndReason, TurnEvent, run_turn};

use crate::session::{Agency, Event, PLANNING, PrefixTracker, Recorder, SYSTEM, now_ms, rendered};
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
    /// A proposal waiting on a person, holding the prompt that caused it.
    /// While this is set, nothing runs: not a turn, not a tool, not a model
    /// call. That is the gate.
    pending: Option<Pending>,
}

/// A prompt held between the proposal and the answer to it.
struct Pending {
    task: TaskId,
    prompt: String,
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
            pending: None,
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
                        on_prompt(app.clone(), text).await;
                    }
                    Ok(ClientMessage::Cancel) => {
                        let session = app.session.lock().await;
                        if let Some(cancel) = &session.cancel {
                            let _ = cancel.send(true);
                        }
                    }
                    Ok(ClientMessage::ApproveTask { task }) => {
                        approve_task(app.clone(), task).await;
                    }
                    Ok(ClientMessage::RejectTask { task }) => {
                        reject_task(app.clone(), task).await;
                    }
                    Ok(ClientMessage::CloseTask { task }) => {
                        close_task(app.clone(), task).await;
                    }
                    Ok(ClientMessage::ReopenTask { task }) => {
                        reopen_task(app.clone(), task).await;
                    }
                    // Unparseable input from one client must not take the
                    // server down for the others.
                    Err(_) => continue,
                }
            }
        }
    }
}

/// A prompt either starts a task or belongs to one.
///
/// The gate is here and nowhere else: with no task open, the prompt buys a
/// planning call and is then held until a person answers it. With one open, it
/// is a turn inside work that was already approved — confirmation is per piece
/// of work, not per message.
async fn on_prompt(app: Arc<App>, prompt: String) {
    let propose = {
        let session = app.session.lock().await;
        // A second prompt during a confirmation is a second thing nobody
        // approved. The composer is disabled client-side; this is the half that
        // does not depend on the client behaving.
        if session.pending.is_some() {
            return;
        }
        session.context.live_task().is_none()
    };

    match propose {
        true => propose_task(app, prompt).await,
        false => start_turn(app, prompt).await,
    }
}

/// Asks the agent what it is about to do, then holds the prompt until someone
/// answers.
///
/// The planning call is a turn: a prompt goes in, tokens come out, it costs a
/// window, and every panel that explains a turn explains this one too. It is
/// the one turn that is **not** remembered — what survives it is the task, and
/// a plan block in the history would be paid for on every later call.
///
/// It runs through [`run_turn`] rather than the agent loop, so it has no tools:
/// a planning call that could execute something would be the gate leaking.
async fn propose_task(app: Arc<App>, prompt: String) {
    let Some((turn, cancel_rx, request)) = begin_turn(&app, &prompt, Some(PLANNING)).await else {
        return;
    };

    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::channel(256);
        let forwarder = {
            let app = app.clone();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    if let Some(message) = ServerMessage::from_turn_event(turn, event) {
                        app.publish(Event::Protocol(message)).await;
                    }
                }
            })
        };
        let outcome = run_turn(app.backend.as_ref(), request, tx, cancel_rx).await;
        let _ = forwarder.await;

        {
            let mut session = app.session.lock().await;
            session.current = None;
            session.cancel = None;
        }

        // Cancelled or broken: there is no proposal, and inventing one would
        // put a plan in front of a person that nothing produced.
        if outcome.error.is_some() || outcome.reason == EndReason::Cancelled {
            return;
        }

        // A small model answering in prose is the ordinary case, and it must
        // not cost the gate. Then the proposal is the ask itself, declaring
        // nothing, and the panel says the model did not declare a plan.
        let proposal = parse_plan(&outcome.text).unwrap_or_else(|| Proposal {
            objective: prompt.clone(),
            plan: Plan::default(),
        });

        let task = {
            let mut session = app.session.lock().await;
            let task = session
                .context
                .propose_task(proposal.objective.clone(), proposal.plan.clone());
            session.pending = Some(Pending { task, prompt });
            task
        };
        app.publish(Event::Protocol(ServerMessage::TaskProposed {
            task,
            objective: proposal.objective,
            plan: proposal.plan,
        }))
        .await;
    });
}

/// Approves it and runs the prompt that was held.
async fn approve_task(app: Arc<App>, task: TaskId) {
    let prompt = {
        let mut session = app.session.lock().await;
        // Nothing can be running here — a prompt behind the gate is refused, so
        // the planning turn is the last one there was. Checked anyway, because
        // the alternative to being wrong about it is a held prompt taken out of
        // `pending` and then silently dropped by a turn that could not start.
        match session.current.is_none() && session.pending.as_ref().is_some_and(|p| p.task == task)
        {
            true => {
                session.context.approve_task(task);
                session.pending.take().map(|pending| pending.prompt)
            }
            // An approval for something else, or for a second time. Not an
            // error: two clients watching the same session can both press it.
            false => None,
        }
    };
    let Some(prompt) = prompt else { return };

    app.publish(Event::Protocol(ServerMessage::TaskApproved { task }))
        .await;
    start_turn(app, prompt).await;
}

/// Refuses it. The held prompt goes with it: a prompt whose plan was turned
/// down is not a prompt that was approved on its own.
async fn reject_task(app: Arc<App>, task: TaskId) {
    {
        let mut session = app.session.lock().await;
        if session.pending.as_ref().is_none_or(|p| p.task != task) {
            return;
        }
        session.pending = None;
        session.context.reject_task(task);
    }
    app.publish(Event::Protocol(ServerMessage::TaskRejected { task }))
        .await;
}

/// Closes it: from here its turns are sent as their summary.
async fn close_task(app: Arc<App>, task: TaskId) {
    let summary = {
        let mut session = app.session.lock().await;
        // Not while a turn is in flight: it would fold the history under the
        // turn that is being answered against it.
        if session.current.is_some() {
            return;
        }
        let counter = app.counter.clone();
        session.context.close_task(task, counter.as_ref())
    };
    let Some(summary) = summary else { return };

    app.publish(Event::Protocol(ServerMessage::TaskClosed { task, summary }))
        .await;
}

/// Unfolds it. Nothing is recovered, because nothing was deleted.
async fn reopen_task(app: Arc<App>, task: TaskId) {
    {
        let mut session = app.session.lock().await;
        if session.current.is_some() || !session.context.reopen_task(task) {
            return;
        }
    }
    app.publish(Event::Protocol(ServerMessage::TaskReopened { task }))
        .await;
}

/// Everything two kinds of model call share: the turn number, the selection,
/// and the three trace messages that explain it.
///
/// `instruction` is fused into the *current user message* when there is one —
/// never into the system block, which is the part the cache reuses. `None`
/// while a turn is already running: one at a time until sessions exist.
async fn begin_turn(
    app: &Arc<App>,
    prompt: &str,
    instruction: Option<&str>,
) -> Option<(TurnId, watch::Receiver<bool>, CompletionRequest)> {
    let (turn, task, cancel_rx, selection, prompt_sent, reuse) = {
        let mut session = app.session.lock().await;
        if session.current.is_some() {
            return None;
        }
        let turn = session.next_turn;
        session.next_turn += 1;
        session.current = Some(turn);

        let (tx, rx) = watch::channel(false);
        session.cancel = Some(tx);
        let text = match instruction {
            Some(instruction) => format!("{instruction}{prompt}"),
            None => prompt.to_string(),
        };
        // Selected under the same lock that hands out the turn number, so the
        // history a turn is built from is the history at the moment it started.
        let selection = session
            .context
            .select(&text, &[], app.budget, app.counter.as_ref());
        // Measured under the same lock, so two turns cannot interleave and
        // measure themselves against each other's prompt.
        let prompt_sent = rendered(&selection.messages);
        let reuse = session
            .prefix
            .measure(turn, &prompt_sent, app.counter.as_ref());
        let task = session.context.live_task();
        (turn, task, rx, selection, prompt_sent, reuse)
    };

    // The user's ask, not the instruction fused in front of it: `prompt` is
    // what was asked and the trace below carries what was sent.
    app.publish(Event::Protocol(ServerMessage::TurnStarted {
        turn,
        prompt: prompt.to_string(),
        task,
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

    Some((
        turn,
        cancel_rx,
        CompletionRequest {
            model: app.model.clone(),
            messages: selection.messages,
        },
    ))
}

async fn start_turn(app: Arc<App>, prompt: String) {
    let Some((turn, cancel_rx, request)) = begin_turn(&app, &prompt, None).await else {
        return;
    };

    tokio::spawn(async move {
        let (tx, mut rx) = mpsc::channel(256);
        let forwarder = {
            let app = app.clone();
            tokio::spawn(async move {
                while let Some(event) = rx.recv().await {
                    // The tool round trips. `budget` and `prefix_reuse`
                    // describe the call that starts a turn; a turn that used a
                    // tool made more, and until these are published the panel
                    // shows their cost as chat-template overhead. Measured into
                    // the same chain as the turns, from the second call on.
                    if let TurnEvent::ModelCall { step, messages } = &event {
                        if *step > 1 {
                            let text = rendered(messages);
                            let measured = {
                                let mut session = app.session.lock().await;
                                session.prefix.measure(turn, &text, app.counter.as_ref())
                            };
                            if let Some(TraceMessage::PrefixReuse {
                                shared_bytes,
                                shared_tokens,
                                prompt_tokens,
                                ..
                            }) = measured
                            {
                                app.publish(Event::Trace(TraceMessage::StepCall {
                                    turn,
                                    step: *step,
                                    text,
                                    prompt_tokens,
                                    shared_bytes,
                                    shared_tokens,
                                }))
                                .await;
                            }
                        }
                        continue;
                    }
                    if let Some(message) = ServerMessage::from_turn_event(turn, event) {
                        app.publish(Event::Protocol(message)).await;
                    }
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

#[cfg(test)]
mod tests {
    use agent_core::backend::mock::Mock;
    use agent_core::context::{ApproximateCounter, Eviction};
    use agent_core::sandbox::SandboxPolicy;
    use agent_core::task::TaskState;

    use super::*;

    const PLAN: &str = "```plan\n{\"objective\":\"add a flag\",\"steps\":[\"read the CLI\"],\
                        \"files\":[\"Cargo.toml\"],\"commands\":[]}\n```";

    /// The server without a socket in front of it. The handlers are what the
    /// socket calls, one line each, so driving them directly tests the gate
    /// rather than axum.
    fn app(replies: &[&str]) -> Arc<App> {
        let base = std::env::current_dir().unwrap();
        let agency = Agency {
            tools: Arc::new(agent_core::tools::Tools::standard()),
            sandbox: Arc::new(
                agent_core::sandbox::Sandbox::new(&SandboxPolicy::default(), &base).unwrap(),
            ),
            max_steps: 4,
        };
        Arc::new(App {
            backend: Arc::new(
                Mock::replies(replies.iter().map(|r| (*r).to_string()).collect())
                    .delay(std::time::Duration::ZERO),
            ),
            model: "mock".into(),
            session: Mutex::new(Session {
                next_turn: 1,
                current: None,
                cancel: None,
                context: AgentContext::new(SYSTEM).with_tools(agency.definitions()),
                prefix: PrefixTracker::default(),
                pending: None,
            }),
            events: broadcast::channel(1024).0,
            recorder: None,
            counter: Arc::new(ApproximateCounter),
            budget: Budget::new(0, 0, Eviction::Turn),
            agency,
            view: Mutex::new(SessionView::new(LIVE_SESSION, "mock", "mock")),
            started_at: 0,
        })
    }

    /// The handlers spawn, so the assertions wait for the state they are about.
    async fn until(app: &Arc<App>, what: impl Fn(&Session) -> bool) -> bool {
        for _ in 0..200 {
            if what(&*app.session.lock().await) {
                return true;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        false
    }

    #[tokio::test]
    async fn a_prompt_with_no_task_open_is_planned_and_then_held() {
        let app = app(&[PLAN, "the answer"]);
        on_prompt(app.clone(), "add a flag".into()).await;
        assert!(until(&app, |s| s.pending.is_some()).await, "no proposal");

        let session = app.session.lock().await;
        let task = session.context.task(1).unwrap();
        assert_eq!(task.state, TaskState::Proposed);
        assert_eq!(task.plan.files, ["Cargo.toml"]);
        assert_eq!(
            session.context.turns().len(),
            0,
            "the planning call is not remembered, and nothing has run under the task",
        );
    }

    #[tokio::test]
    async fn approving_runs_the_prompt_that_was_held() {
        let app = app(&[PLAN, "the answer"]);
        on_prompt(app.clone(), "add a flag".into()).await;
        assert!(until(&app, |s| s.pending.is_some()).await);

        approve_task(app.clone(), 1).await;
        assert!(
            until(&app, |s| s.context.turns().len() == 1).await,
            "the held prompt never ran",
        );

        let session = app.session.lock().await;
        assert!(session.pending.is_none());
        assert_eq!(session.context.task(1).unwrap().state, TaskState::Approved);
        assert_eq!(session.context.turns()[0].prompt, "add a flag");
        assert_eq!(
            session.context.turns()[0].task,
            Some(1),
            "the turn belongs to the task it was approved under",
        );
    }

    #[tokio::test]
    async fn rejecting_drops_the_prompt_with_the_plan() {
        let app = app(&[PLAN, "the answer"]);
        on_prompt(app.clone(), "add a flag".into()).await;
        assert!(until(&app, |s| s.pending.is_some()).await);

        reject_task(app.clone(), 1).await;
        let session = app.session.lock().await;
        assert!(session.pending.is_none());
        assert_eq!(session.context.task(1).unwrap().state, TaskState::Rejected);
        assert!(
            session.context.turns().is_empty(),
            "a prompt whose plan was turned down is not a prompt that was approved on its own",
        );
    }

    #[tokio::test]
    async fn a_second_prompt_behind_the_gate_does_not_run() {
        let app = app(&[PLAN, "the answer"]);
        on_prompt(app.clone(), "add a flag".into()).await;
        assert!(until(&app, |s| s.pending.is_some()).await);

        on_prompt(app.clone(), "and also this".into()).await;
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;

        let session = app.session.lock().await;
        assert_eq!(session.pending.as_ref().unwrap().prompt, "add a flag");
        assert!(session.context.turns().is_empty());
        assert_eq!(
            session.context.tasks().len(),
            1,
            "a second prompt during a confirmation is a second thing nobody approved",
        );
    }

    #[tokio::test]
    async fn a_prompt_inside_a_live_task_is_a_turn_and_not_another_gate() {
        let app = app(&[PLAN, "the answer"]);
        on_prompt(app.clone(), "add a flag".into()).await;
        assert!(until(&app, |s| s.pending.is_some()).await);
        approve_task(app.clone(), 1).await;
        assert!(until(&app, |s| s.context.turns().len() == 1).await);

        on_prompt(app.clone(), "now the tests".into()).await;
        assert!(until(&app, |s| s.context.turns().len() == 2).await);

        let session = app.session.lock().await;
        assert_eq!(session.context.tasks().len(), 1, "no second proposal");
        assert!(session.pending.is_none());
    }

    #[tokio::test]
    async fn closing_folds_the_task_and_reopening_unfolds_it() {
        let app = app(&[PLAN, "the answer"]);
        on_prompt(app.clone(), "add a flag".into()).await;
        assert!(until(&app, |s| s.pending.is_some()).await);
        approve_task(app.clone(), 1).await;
        assert!(until(&app, |s| s.context.turns().len() == 1).await);

        close_task(app.clone(), 1).await;
        {
            let session = app.session.lock().await;
            let task = session.context.task(1).unwrap();
            assert_eq!(task.state, TaskState::Closed);
            assert!(task.summary.as_ref().unwrap().text.contains("add a flag"));
            assert_eq!(
                session.context.turns().len(),
                1,
                "closing is an event: the turn is still there",
            );
        }

        reopen_task(app.clone(), 1).await;
        let session = app.session.lock().await;
        assert_eq!(session.context.task(1).unwrap().state, TaskState::Approved);
        assert!(session.context.task(1).unwrap().summary.is_none());
    }

    #[tokio::test]
    async fn a_model_that_answers_in_prose_still_gets_a_gate() {
        let app = app(&["I'll read the CLI and add it.", "the answer"]);
        on_prompt(app.clone(), "add a flag".into()).await;
        assert!(until(&app, |s| s.pending.is_some()).await, "no proposal");

        let session = app.session.lock().await;
        let task = session.context.task(1).unwrap();
        assert_eq!(
            task.objective, "add a flag",
            "the ask itself becomes the objective when the model declares nothing",
        );
        assert!(task.plan.files.is_empty());
    }
}
