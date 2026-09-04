//! `luu serve` — the local HTTP server behind the debug UI.
//!
//! Loopback by default and unauthenticated: it exposes an agent that runs
//! commands. Binding it anywhere else requires a bearer token, and [`bind`]
//! refuses to hold the port without one — see [`crate::auth`].

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use agent_core::agent::run_agent_turn;
use agent_core::api::SessionView;
use agent_core::backend::{Backend, CompletionRequest};
use agent_core::context::{Budget, Context as AgentContext, TokenCounter};
use agent_core::protocol::{self, ClientMessage, Refusal, ServerMessage, TurnId};
use agent_core::repo_map::{Order, RepoMap};
use agent_core::sandbox::Sandbox;
use agent_core::task::{ClosedBy, Plan, PlanSource, Proposal, TaskId, parse_plan};
use agent_core::trace::TraceMessage;
use agent_core::turn::{EndReason, TurnEvent, run_turn};

use crate::auth::Auth;
use crate::session::{Agency, Event, PLANNING, PrefixTracker, Recorder, SYSTEM, now_ms, rendered};
use crate::store::SessionStore;
use anyhow::{Context, Result};
use axum::Json;
use axum::Router;
use axum::extract::Path;
use axum::extract::State;
use axum::extract::ws::{Message as WsMessage, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, Request};
use axum::http::{StatusCode, Uri, header};
use axum::middleware::{self, Next};
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
    /// The live task's own sandbox: the approved plan, resolved against the
    /// policy file. Every turn inside the task is checked against this rather
    /// than against the session's, which is what makes the task boundary the
    /// scope permission is granted at instead of a comment saying it is.
    /// `None` outside a task, where the policy file is the whole answer.
    narrowed: Option<(TaskId, Arc<Sandbox>)>,
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
    /// Pinned sampling, forwarded to every call the same way `budget` is.
    /// `None` leaves it to the server's own default.
    temperature: Option<f32>,
    seed: Option<u32>,
    /// The read side, folded from the same events the sockets carry — so
    /// `GET /api/...` can never disagree with what a client watched happen.
    view: Mutex<SessionView>,
    started_at: u64,
    /// What this session is called on disk. The live view is served as
    /// [`LIVE_SESSION`] whatever it is called, because a client watching *the*
    /// session should not have to learn its name first — but a store keyed on
    /// "live" would overwrite the previous session on every restart, which is a
    /// history that only ever holds one thing.
    session_id: String,
    /// Where the fold is cached between restarts. `None` leaves the session in
    /// memory, which is what every run did before this existed.
    store: Option<Mutex<SessionStore>>,
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
        // At checkpoints, not per event. The store is a *cache* of the fold, so
        // it is allowed to lag the record — and writing the whole fold on every
        // token is quadratic in the length of a session. What it is never
        // allowed to do is contradict the record, which is why the checkpoints
        // are the moments the session's shape changed rather than a timer.
        if is_checkpoint(&event) {
            self.checkpoint().await;
        }

        // No subscribers is the ordinary state of a server nobody has opened yet.
        let _ = self.events.send(event);
    }

    /// Writes the fold as it stands. Failures are reported once and do not stop
    /// a turn: a session whose cache could not be written is still a session,
    /// and the record beside it is the account that matters.
    async fn checkpoint(&self) {
        let Some(store) = &self.store else {
            return;
        };
        let stored = {
            let view = self.view.lock().await;
            let mut stored = view.clone();
            // The id is the session's *name* rather than part of the fold —
            // `SessionView::from_record` takes it as an argument for the same
            // reason, and `luu export` takes it from the file stem.
            stored.id = self.session_id.clone();
            if stored.title == LIVE_SESSION {
                stored.title = self.session_id.clone();
            }
            stored
        };
        if let Err(error) = store.lock().await.save(&stored) {
            eprintln!("warning: could not write the session store: {error:#}");
        }
    }
}

/// What a session is called on disk: when it started, then enough to keep two
/// of them apart.
///
/// Sortable and readable first, because the one thing a person listing sessions
/// wants is when it happened. Unique second, and it has to be: two servers
/// started in the same millisecond would otherwise be one row, and the second
/// would silently overwrite the first — a history that loses sessions is worse
/// than none, because nothing says it did.
fn session_id(started_at: u64) -> String {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    format!("session-{started_at}-{:x}{n:x}", std::process::id())
}

/// Which events are worth a write: the ones after which the session is a
/// different shape. Tokens are not — a turn's text is only complete at
/// `ended`, and a cache of half a turn is a cache of nothing.
fn is_checkpoint(event: &Event) -> bool {
    match event {
        Event::Protocol(message) => matches!(
            message,
            ServerMessage::Ended { .. }
                | ServerMessage::Failed { .. }
                | ServerMessage::TaskProposed { .. }
                | ServerMessage::TaskApproved { .. }
                | ServerMessage::TaskClosed { .. }
                | ServerMessage::TaskRejected { .. }
                | ServerMessage::TaskReopened { .. }
        ),
        Event::Trace(_) => false,
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
    pub temperature: Option<f32>,
    pub seed: Option<u32>,
    /// Tokens of repository outline for the prefix. 0 is off — see
    /// `agent_core::repo_map`.
    pub map_tokens: u32,
    /// Which files that budget buys. Path order unless asked otherwise — see
    /// [`Order`], which carries the measurement that keeps it the default.
    pub map_order: Order,
    /// The file holding the bearer token this server requires, if any.
    /// `None` on a loopback address means no auth; `None` on any other
    /// address means [`bind`] refuses.
    pub auth_token_file: Option<PathBuf>,
    /// Where sessions are cached between restarts. `None` keeps the session in
    /// memory for the life of the process, which is what `serve` did before the
    /// store existed.
    pub store: Option<PathBuf>,
}

/// A server that has its port and has not started answering yet.
///
/// `serve` used to bind and serve in one call, which left a test no way to
/// learn which port an ephemeral bind got — and a test that binds a *fixed*
/// port is a test that fails whenever two jobs share a runner.
pub struct Serving {
    address: SocketAddr,
    listener: tokio::net::TcpListener,
    router: Router,
}

impl Serving {
    /// The address actually bound, which is what a `:0` request resolved to.
    pub fn address(&self) -> SocketAddr {
        self.address
    }

    pub async fn run(self) -> Result<()> {
        axum::serve(self.listener, self.router)
            .await
            .context("serving")
    }
}

pub async fn serve(options: ServeOptions) -> Result<()> {
    let serving = bind(options).await?;
    println!("luu serve → http://{}", serving.address());
    serving.run().await
}

pub async fn bind(options: ServeOptions) -> Result<Serving> {
    let ServeOptions {
        address,
        backend,
        model,
        record,
        budget,
        counter,
        agency,
        temperature,
        seed,
        map_tokens,
        map_order,
        auth_token_file,
        store,
    } = options;
    // Before anything else, and before the listener exists: a port that would
    // publish task approval to the network is not a port this binds and then
    // warns about.
    let auth = Arc::new(crate::auth::resolve(&address, auth_token_file.as_deref())?);
    let started_at = now_ms();

    // Built once, before the socket is up: the map is the last block of the
    // cached prefix, and a block rebuilt mid-session is not a prefix. What that
    // costs is named in `RECORD/2026-08-31.the-repo-map.completed.md`.
    let map = RepoMap::build(
        agency.sandbox.as_ref(),
        map_tokens,
        counter.as_ref(),
        map_order,
    );
    if !map.is_empty() {
        eprintln!(
            "repository map — {} file(s), {} left out, {} of {map_tokens} tokens",
            map.files.len(),
            map.left_out,
            map.tokens,
        );
    }

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

    // Opened before the listener, like the token: a store that cannot be
    // opened is a configuration error, and finding out four turns in means
    // four turns nobody can resume.
    let sessions = match &store {
        Some(path) => {
            let store = SessionStore::open(path)?;
            eprintln!("sessions — {}", path.display());
            Some(Mutex::new(store))
        }
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
            context: AgentContext::new(SYSTEM)
                .with_tools(agency.definitions())
                .with_map(map.render()),
            prefix: PrefixTracker::default(),
            pending: None,
            narrowed: None,
        }),
        events: broadcast::channel(1024).0,
        recorder,
        counter,
        budget,
        agency,
        temperature,
        seed,
        view: Mutex::new({
            let mut view = SessionView::new(LIVE_SESSION, backend_name, &model_name);
            view.started_at = started_at;
            view
        }),
        started_at,
        session_id: session_id(started_at),
        store: sessions,
    });

    // Two halves, because they are two surfaces. `/ws` is authority and
    // `/api/*` is this session's prompts and source — both behind the token
    // when there is one. The embedded UI is not: it is the same bytes in every
    // copy of a public binary, and a browser navigating to a page cannot carry
    // an `Authorization` header, so gating it would only make the guarded
    // server unusable from the client written for it.
    let guarded = Router::new()
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
        .layer(middleware::from_fn_with_state(auth.clone(), require_token));

    let router = guarded
        .route("/", get(|| serve_asset("index.html")))
        .route("/{*path}", get(asset_handler))
        .with_state(AppRouterState { app: app.clone() });

    let listener = tokio::net::TcpListener::bind(address)
        .await
        .with_context(|| format!("binding {address}"))?;
    // Not `address`: a `:0` request would print the port nobody can connect to.
    let address = listener.local_addr().context("the bound address")?;

    Ok(Serving {
        address,
        listener,
        router,
    })
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

/// The bearer check, on the control and read surfaces only.
///
/// Two ways to present the token, and the second one is a browser
/// concession rather than a preference: `Authorization: Bearer <token>` is
/// what `curl` and `fetch` send, and `?token=<token>` is accepted on `/ws`
/// because the browser's `WebSocket` constructor cannot set a header. It is
/// not accepted anywhere else — a query string is the part of a URL that ends
/// up in shell history and proxy logs, so the exception stays as narrow as the
/// thing that forces it.
async fn require_token(State(auth): State<Arc<Auth>>, request: Request, next: Next) -> Response {
    let header = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "))
        .map(str::to_string);
    let presented = match header {
        Some(token) => Some(token),
        None if request.uri().path().starts_with("/ws") => {
            Query::<TokenQuery>::try_from_uri(request.uri())
                .ok()
                .and_then(|Query(query)| query.token)
        }
        None => None,
    };

    if !auth.admits(presented.as_deref()) {
        return (
            StatusCode::UNAUTHORIZED,
            [(header::WWW_AUTHENTICATE, "Bearer")],
            "this server requires a bearer token: --auth-token-file named one\n",
        )
            .into_response();
    }
    next.run(request).await
}

#[derive(serde::Deserialize)]
struct TokenQuery {
    token: Option<String>,
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
                    Ok(ClientMessage::ApproveTask {
                        task,
                        files,
                        writes,
                        commands,
                        closes_on,
                        network,
                    }) => {
                        approve_task(app.clone(), task, files, writes, commands, closes_on, network).await;
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

/// Says no, and why.
///
/// Every early return in this file that a client could not otherwise
/// distinguish from a dropped message goes through here. See
/// `RECORD/2026-08-30.a-refusal-is-a-message.completed.md`.
async fn refuse(app: &Arc<App>, request: &str, reason: Refusal, detail: impl Into<String>) {
    app.publish(Event::Protocol(ServerMessage::Refused {
        request: request.to_string(),
        reason,
        detail: detail.into(),
    }))
    .await;
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
        if let Some(pending) = &session.pending {
            let task = pending.task;
            drop(session);
            refuse(
                &app,
                "prompt",
                Refusal::Pending,
                format!("task {task} is waiting to be approved or refused"),
            )
            .await;
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
        //
        // Which of the two happened travels with the proposal. It is the gate's
        // headline number — how often a 7B plans at all — and the panel used to
        // infer it from an empty plan, which cannot tell a model that answered
        // in prose from one that declared an empty list.
        let (proposal, source) = match parse_plan(&outcome.text) {
            Some(proposal) => (proposal, PlanSource::Model),
            None => (
                Proposal {
                    objective: prompt.clone(),
                    plan: Plan::default(),
                },
                PlanSource::Prose,
            ),
        };

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
            source: Some(source),
        }))
        .await;
    });
}

/// Approves it — with whatever the person added to the plan — and runs the
/// prompt that was held.
///
/// The amendment is checked against the policy file exactly as the model's plan
/// was: an entry the file does not grant is dropped rather than approved, and
/// the plan that comes back on `task_approved` is what was actually approved.
/// That is the whole feedback: a client that adds a path nobody may touch sees
/// it missing from the plan it gets back, rather than being told nothing.
async fn approve_task(
    app: Arc<App>,
    task: TaskId,
    files: Vec<String>,
    writes: Vec<String>,
    commands: Vec<String>,
    closes_on: Option<String>,
    network: Option<bool>,
) {
    let approved = {
        let mut session = app.session.lock().await;
        // Nothing can be running here — a prompt behind the gate is refused, so
        // the planning turn is the last one there was. Checked anyway, because
        // the alternative to being wrong about it is a held prompt taken out of
        // `pending` and then silently dropped by a turn that could not start.
        match session.current.is_none() && session.pending.as_ref().is_some_and(|p| p.task == task)
        {
            true => {
                let (granted, mut dropped) =
                    permitted(&app.agency.sandbox, files, writes, commands, network);
                // Checked against the plan as it will *be* rather than against
                // the amendment alone: the person types `closes_on` for a
                // command the model already declared more often than for one
                // they are adding in the same breath.
                let (closes_on, refused) = closing_condition(
                    &app.agency.sandbox,
                    session.context.task(task).map(|t| &t.plan),
                    &granted,
                    closes_on,
                );
                dropped.extend(refused);
                let plan = session
                    .context
                    .amend_plan(
                        task,
                        &granted.files,
                        &granted.writes,
                        &granted.commands,
                        closes_on.as_deref(),
                        network.map(|_| granted.network),
                    )
                    .unwrap_or_default();
                session.context.approve_task(task);
                session
                    .pending
                    .take()
                    .map(|pending| (pending.prompt, plan, dropped))
            }
            // An approval for something else, or for a second time. Not an
            // error — two clients watching the same session can both press it —
            // but not silence either: the second one's button did nothing and
            // this is what says so.
            false => None,
        }
    };
    let Some((prompt, plan, dropped)) = approved else {
        refuse(
            &app,
            "approve_task",
            Refusal::Task,
            format!("task {task} is not waiting to be approved"),
        )
        .await;
        return;
    };

    // What the person added that the policy file does not grant. It was left
    // out of the plan, and until now that was the whole feedback: a path
    // missing from a message nobody reads that closely.
    if !dropped.is_empty() {
        refuse(
            &app,
            "approve_task",
            Refusal::NotGranted,
            format!(
                "approved without what the sandbox policy does not grant: {}",
                dropped.join("; "),
            ),
        )
        .await;
    }

    // The task's own sandbox, from here until it closes. A plan that names
    // nothing narrows to nothing, which is the point: a turn inside a task may
    // touch what the task was approved for.
    let narrowed = match plan.narrow(app.agency.sandbox.as_ref(), task) {
        Ok(sandbox) => Some(Arc::new(sandbox)),
        // Only a path that stopped existing between the check and here can do
        // this. Falling back to the session's sandbox would silently un-narrow
        // the task, so the task runs with nothing granted and the denials say
        // which plan refused.
        Err(error) => {
            eprintln!("task {task}: the approved plan could not be resolved: {error}");
            None
        }
    };
    {
        let mut session = app.session.lock().await;
        session.narrowed = narrowed.map(|sandbox| (task, sandbox));
    }

    app.publish(Event::Protocol(ServerMessage::TaskApproved { task, plan }))
        .await;
    start_turn(app, prompt).await;
}

/// The half of an amendment the policy file grants, and the half it does not —
/// which is what the refusal beside it is made of.
///
/// The person at the gate widens a plan up to the file and not past it —
/// otherwise the gate is the policy and `luu.toml` is a suggestion.
fn permitted(
    sandbox: &Sandbox,
    files: Vec<String>,
    writes: Vec<String>,
    commands: Vec<String>,
    network: Option<bool>,
) -> (Plan, Vec<String>) {
    let asked = Plan {
        steps: Vec::new(),
        files,
        writes,
        commands,
        // Not here: a closing condition is checked against the merged plan,
        // which this function has never seen. See `closing_condition`.
        closes_on: None,
        network: network.unwrap_or(false),
    };
    let refused = asked.unmet(sandbox);
    let keep = |item: &String, kind: &str| {
        !refused
            .iter()
            .any(|line| line.starts_with(&format!("{kind} {item}:")))
    };
    let granted_network = match network {
        Some(true) => sandbox.network(),
        Some(false) => false,
        None => false,
    };
    let granted = Plan {
        steps: Vec::new(),
        files: asked
            .files
            .iter()
            .filter(|f| keep(f, "file"))
            .cloned()
            .collect(),
        writes: asked
            .writes
            .iter()
            .filter(|f| keep(f, "write"))
            .cloned()
            .collect(),
        commands: asked
            .commands
            .iter()
            .filter(|c| keep(c, "command"))
            .cloned()
            .collect(),
        closes_on: None,
        network: granted_network,
    };
    (granted, refused)
}

/// The closing condition the person typed, if the task will actually be able to
/// run it — and the one line that says why not, when it will not.
///
/// Checked against the *merged* plan: what the model declared, plus what the
/// gate is adding in the same approval, intersected with what the policy file
/// allows. A condition naming a command the task may not run can never be met,
/// and a task that can never close is worse than one closed by hand, because it
/// looks like it will close itself.
fn closing_condition(
    sandbox: &Sandbox,
    existing: Option<&Plan>,
    granted: &Plan,
    closes_on: Option<String>,
) -> (Option<String>, Option<String>) {
    let Some(closes_on) = closes_on
        .map(|it| it.trim().to_string())
        .filter(|it| !it.is_empty())
    else {
        return (None, None);
    };

    let mut commands: Vec<String> = existing.map(|p| p.commands.clone()).unwrap_or_default();
    for command in &granted.commands {
        if !commands.contains(command) {
            commands.push(command.clone());
        }
    }
    let merged = Plan {
        steps: Vec::new(),
        files: Vec::new(),
        writes: Vec::new(),
        // What the plan may really run: `narrow` drops a declared command the
        // policy file never granted, silently, so checking against the
        // declaration alone would accept a condition the sandbox will deny.
        commands: commands
            .into_iter()
            .filter(|c| sandbox.commands().iter().any(|allowed| allowed == c))
            .collect(),
        closes_on: Some(closes_on.clone()),
        network: false,
    };
    match merged
        .unmet(sandbox)
        .into_iter()
        .find(|line| line.starts_with("closes_on "))
    {
        Some(refusal) => (None, Some(refusal)),
        None => (Some(closes_on), None),
    }
}

/// Refuses it. The held prompt goes with it: a prompt whose plan was turned
/// down is not a prompt that was approved on its own.
async fn reject_task(app: Arc<App>, task: TaskId) {
    {
        let mut session = app.session.lock().await;
        if session.pending.as_ref().is_none_or(|p| p.task != task) {
            drop(session);
            refuse(
                &app,
                "reject_task",
                Refusal::Task,
                format!("task {task} is not waiting to be approved"),
            )
            .await;
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
        if let Some(running) = session.current {
            drop(session);
            refuse(
                &app,
                "close_task",
                Refusal::Busy,
                format!("turn {running} is running; closing would fold the history under it"),
            )
            .await;
            return;
        }
        let counter = app.counter.clone();
        let summary = session.context.close_task(task, counter.as_ref());
        // The task's sandbox goes with the task. Outside one, the policy file
        // is the whole answer again — and the next prompt proposes a new task
        // before it runs anything.
        if summary.is_some() && session.narrowed.as_ref().is_some_and(|(id, _)| *id == task) {
            session.narrowed = None;
        }
        summary
    };
    let Some(summary) = summary else {
        refuse(
            &app,
            "close_task",
            Refusal::Task,
            format!("task {task} is not open"),
        )
        .await;
        return;
    };

    app.publish(Event::Protocol(ServerMessage::TaskClosed {
        task,
        summary,
        by: Some(ClosedBy::User),
    }))
    .await;
}

/// Unfolds it. Nothing is recovered, because nothing was deleted.
async fn reopen_task(app: Arc<App>, task: TaskId) {
    {
        let mut session = app.session.lock().await;
        if let Some(running) = session.current {
            drop(session);
            refuse(
                &app,
                "reopen_task",
                Refusal::Busy,
                format!("turn {running} is running"),
            )
            .await;
            return;
        }
        if !session.context.reopen_task(task) {
            drop(session);
            refuse(
                &app,
                "reopen_task",
                Refusal::Task,
                format!("task {task} is not closed"),
            )
            .await;
            return;
        }
        // Live again, so its plan is the authority again. Rebuilt rather than
        // remembered: the sandbox is a resolution of the plan, and the plan is
        // what the session keeps.
        let plan = session.context.task(task).map(|task| task.plan.clone());
        session.narrowed = plan
            .and_then(|plan| plan.narrow(app.agency.sandbox.as_ref(), task).ok())
            .map(|sandbox| (task, Arc::new(sandbox)));
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
        if let Some(running) = session.current {
            drop(session);
            refuse(
                app,
                "prompt",
                Refusal::Busy,
                format!("turn {running} is running; one at a time until sessions exist"),
            )
            .await;
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
    // Before the prompt it explains: this is what the turn no longer carries,
    // and a client reading in order should learn that the history was cut
    // before it is handed the prompt that was cut from.
    if let Some(evicted) = selection.eviction.clone() {
        app.publish(Event::Protocol(ServerMessage::Evicted {
            turn,
            turns: evicted.turns,
            tokens: evicted.tokens,
            counter: evicted.counter,
            policy: evicted.policy,
        }))
        .await;
    }
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
            // The window we budgeted against, sent so the server serves it. The
            // same `None` the budget means by "unknown".
            context_limit: app.budget.limit,
            temperature: app.temperature,
            seed: app.seed,
        },
    ))
}

async fn start_turn(app: Arc<App>, prompt: String) {
    let Some((turn, cancel_rx, request)) = begin_turn(&app, &prompt, None).await else {
        return;
    };

    // Inside a task, the plan it was approved with is what holds this turn;
    // outside one, the policy file. A turn is never checked against both.
    let sandbox = {
        let session = app.session.lock().await;
        match (session.context.live_task(), &session.narrowed) {
            (Some(live), Some((task, sandbox))) if live == *task => sandbox.clone(),
            _ => app.agency.sandbox.clone(),
        }
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
            app.agency.executor(),
            sandbox.as_ref(),
            app.agency.max_steps,
            tx,
            cancel_rx,
        )
        .await;
        let _ = forwarder.await;

        let closed = {
            let mut session = app.session.lock().await;
            // A cancelled turn keeps its partial answer: the user saw it, so
            // the model should too. A turn that produced nothing at all is not
            // remembered — an empty assistant message is not a thing that
            // happened, and several chat templates render it as a prompt to
            // continue.
            if !outcome.text.is_empty() || !outcome.steps.is_empty() {
                session.context.push_turn_with_steps(
                    turn,
                    prompt,
                    outcome.text,
                    vec![],
                    outcome.steps,
                    app.counter.as_ref(),
                );
            }
            session.current = None;
            session.cancel = None;

            // After the turn is in the history, so the close sees this turn's
            // steps — the ones that just ran the command it closes on. Before
            // the lock is dropped, so a prompt arriving between the two cannot
            // start a turn inside a task that is already folding.
            let counter = app.counter.clone();
            let closed = session.context.close_if_met(counter.as_ref());
            if let Some((task, _)) = &closed
                && session.narrowed.as_ref().is_some_and(|(id, _)| id == task)
            {
                // The task's sandbox goes with the task, exactly as it does
                // when a person closes one.
                session.narrowed = None;
            }
            closed
        };

        if let Some((task, summary)) = closed {
            app.publish(Event::Protocol(ServerMessage::TaskClosed {
                task,
                summary,
                by: Some(ClosedBy::ExitCode),
            }))
            .await;
        }
    });
}

/// Strips the `.json` a static mirror needs, so both spellings reach one handler.
fn bare(id: &str) -> &str {
    id.strip_suffix(".json").unwrap_or(id)
}

fn not_found(what: &str) -> Response {
    (StatusCode::NOT_FOUND, format!("no such {what}")).into_response()
}

/// The live session, then whatever the store has — the live one first because
/// it is the one a client that just connected is watching.
///
/// Its own row is left out of the stored half rather than shown twice: the
/// store is a cache of this very fold, and a listing that showed both would be
/// showing one session under two names, the older of them by however long ago
/// the last checkpoint was.
async fn list_sessions(State(state): State<AppRouterState>) -> Response {
    let live = state.app.view.lock().await.summary();
    let mut sessions = vec![live];
    if let Some(store) = &state.app.store {
        match store.lock().await.list() {
            Ok(stored) => sessions.extend(
                stored
                    .into_iter()
                    .filter(|row| row.id != state.app.session_id),
            ),
            Err(error) => eprintln!("warning: could not list the session store: {error:#}"),
        }
    }
    Json(sessions).into_response()
}

/// The fold for one session: the live one under either of its names, and
/// anything else out of the store.
///
/// The live view is answered from memory rather than from the store even when
/// both have it, because the store is allowed to lag by design and the live
/// fold never is.
async fn view_for(state: &AppRouterState, id: &str) -> Option<SessionView> {
    let id = bare(id);
    {
        let view = state.app.view.lock().await;
        if id == view.id || id == state.app.session_id {
            return Some(view.clone());
        }
    }
    let store = state.app.store.as_ref()?;
    match store.lock().await.load(id) {
        Ok(found) => found,
        Err(error) => {
            eprintln!("warning: could not read the session store: {error:#}");
            None
        }
    }
}

async fn get_session(Path(id): Path<String>, State(state): State<AppRouterState>) -> Response {
    match view_for(&state, &id).await {
        Some(view) => Json(view).into_response(),
        None => not_found("session"),
    }
}

async fn get_turns(Path(id): Path<String>, State(state): State<AppRouterState>) -> Response {
    match view_for(&state, &id).await {
        Some(view) => Json(view.turns).into_response(),
        None => not_found("session"),
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
    let Some(number) = parse_turn(&turn) else {
        return not_found("turn");
    };
    match view_for(&state, &id)
        .await
        .and_then(|view| view.turn(number).cloned())
    {
        Some(turn) => Json(turn).into_response(),
        None => not_found("turn"),
    }
}

async fn get_prompt(
    Path((id, turn)): Path<(String, String)>,
    State(state): State<AppRouterState>,
) -> Response {
    let Some(number) = parse_turn(&turn) else {
        return not_found("turn");
    };
    let view = view_for(&state, &id).await;
    let found = view.as_ref().and_then(|view| view.turn(number));
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
    let Some(view) = view_for(&state, &id).await else {
        return not_found("session");
    };
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
            worker: None,
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
                narrowed: None,
            }),
            events: broadcast::channel(1024).0,
            recorder: None,
            counter: Arc::new(ApproximateCounter),
            budget: Budget::new(0, 0, Eviction::Turn),
            agency,
            temperature: None,
            seed: None,
            view: Mutex::new(SessionView::new(LIVE_SESSION, "mock", "mock")),
            session_id: "session-test".into(),
            // In memory, like everything else these handler tests touch: the
            // store's own behaviour is `tests/store_parity.rs`.
            store: None,
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

        approve_task(app.clone(), 1, vec![], vec![], vec![], None, None).await;
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
        approve_task(app.clone(), 1, vec![], vec![], vec![], None, None).await;
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
        approve_task(app.clone(), 1, vec![], vec![], vec![], None, None).await;
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
