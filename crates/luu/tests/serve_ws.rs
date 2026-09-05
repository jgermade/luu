//! One turn over `/ws`, against the running server.
//!
//! The unit tests in `serve.rs` call the handlers directly, which tests the
//! gate and not the server: nothing there binds a port, upgrades a socket,
//! serializes a `ServerMessage` onto the wire, or asks the read side what it
//! thinks happened. Every bug this repository has had was found by running it,
//! and this is the cheapest place to keep running it.
//!
//! It also asserts the one property the read side has that nothing else checks.
//! `GET /api/...` is folded from the same events the socket carries, so it
//! *cannot* disagree with what a client watched happen — a claim no test had
//! ever made it prove.

use std::sync::Arc;
use std::time::Duration;

use agent_core::approval::{Approval, ApproverKey, Approvers, Signer};
use agent_core::backend::mock::Mock;
use agent_core::context::{ApproximateCounter, Budget, Eviction, TokenCounter};
use agent_core::sandbox::{Access, Enforcement, Sandbox, SandboxPolicy};
use agent_core::tools::Tools;
use futures_util::{SinkExt, StreamExt};
use luu::serve::{ServeOptions, bind};
use luu::session::{Agency, SYSTEM};
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const PLAN: &str = "```plan\n{\"objective\":\"add a flag\",\"steps\":[\"read the CLI\"],\
                    \"files\":[\"Cargo.toml\"],\"commands\":[]}\n```";

const ANSWER: &str = "The flag is added in lib.rs.";

/// A socket that fails rather than hangs: a turn that never ends is a bug, and
/// a test that waits for it forever reports nothing about which one.
const PATIENCE: Duration = Duration::from_secs(10);

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

/// A plan that names one file, and a turn that tries to read another. The
/// mock answers the planning call, then asks for a tool, then concludes.
const PLAN_FOR_CARGO_TOML: &str = "```plan\n{\"objective\":\"read the manifest\",\
                                   \"steps\":[\"read it\"],\"files\":[\"Cargo.toml\"],\
                                   \"commands\":[]}\n```";

/// A turn that edits a file the plan only said it would read.
const WRITES_SCRATCH: &str = "Writing it.\n```tool\n                              {\"name\":\"write_file\",\"arguments\":\
                              {\"path\":\"Cargo.toml\",\"content\":\"broken\"}}\n```";

const READS_SERVE_RS: &str = "Let me look.\n```tool\n                              {\"name\":\"read_file\",\"arguments\":\
                              {\"path\":\"src/serve.rs\",\"max_lines\":1}}\n```";

/// The server on an ephemeral port, with the mock answering the planning call
/// and then the turn.
async fn server() -> String {
    server_with(vec![PLAN.into(), ANSWER.into()]).await
}

/// The same, at the pace the mock streams by default — a token every 30ms, so
/// a turn is still running when the next message arrives.
async fn server_slow(replies: Vec<String>) -> String {
    server_at(replies, Duration::from_millis(30)).await
}

async fn server_with(replies: Vec<String>) -> String {
    server_at(replies, Duration::ZERO).await
}

/// A server whose *policy* also grants write on `also`, so the gate has room
/// to widen a plan into it: the person at the gate widens up to the policy file
/// and not past it, which is the rule this exists to exercise rather than
/// bypass.
async fn server_writable(replies: Vec<String>, also: &std::path::Path) -> String {
    let mut policy = SandboxPolicy::default();
    policy.allow(
        also.parent().expect("a parent directory"),
        Access::ReadWrite,
    );
    server_with_policy(replies, Duration::ZERO, policy).await
}

/// A server whose policy allows one program, so a plan can declare it and the
/// gate can name it as what closes the task.
async fn server_running(replies: Vec<String>, command: &str) -> String {
    let mut policy = SandboxPolicy::default();
    policy.allow_command(command);
    // `best-effort`, so the test runs where the kernel rung does not: under
    // the default `kernel` a child is denied wherever Landlock is missing,
    // which is macOS and any container whose kernel lacks it. What is being
    // asserted here is the closing rung, and a test that only runs on one
    // kernel asserts it nowhere else.
    policy.enforcement = Enforcement::BestEffort;
    server_with_policy(replies, Duration::ZERO, policy).await
}

async fn server_at(replies: Vec<String>, delay: Duration) -> String {
    server_with_policy(replies, delay, SandboxPolicy::default()).await
}

/// A server with a window small enough that the history has to give way. The
/// limit is measured from the prefix this very process assembles rather than
/// picked by hand: a hard-coded one is how the degenerate 512-token fixtures
/// happened.
async fn server_evicting(replies: Vec<String>) -> String {
    let prefix = ApproximateCounter.count(SYSTEM)
        + ApproximateCounter.count(&Tools::standard().definitions());
    server_full(
        replies,
        Duration::ZERO,
        SandboxPolicy::default(),
        Budget::new(prefix + ROOM_FOR_HISTORY, 0, Eviction::Turn),
    )
    .await
}

/// Room for a couple of the mock's answers and no more, in tokens.
const ROOM_FOR_HISTORY: u32 = 220;

async fn server_with_policy(
    replies: Vec<String>,
    delay: Duration,
    policy: SandboxPolicy,
) -> String {
    server_full(replies, delay, policy, Budget::new(0, 0, Eviction::Turn)).await
}

/// A server that requires a bearer token, with the token in a file only its
/// owner can read — which is what `resolve` insists on.
async fn server_guarded(token: &str) -> (String, std::path::PathBuf) {
    let dir = std::env::temp_dir().join(format!("luu-serve-auth-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let path = dir.join("token");
    std::fs::write(&path, token).expect("writing the token");
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))
            .expect("tightening the mode");
    }
    let address = server_authed(
        vec![PLAN.into(), ANSWER.into()],
        Duration::ZERO,
        SandboxPolicy::default(),
        Budget::new(0, 0, Eviction::Turn),
        Some(path.clone()),
    )
    .await;
    (address, dir)
}

async fn server_full(
    replies: Vec<String>,
    delay: Duration,
    policy: SandboxPolicy,
    budget: Budget,
) -> String {
    server_authed(replies, delay, policy, budget, None).await
}

/// A server that caches its fold into the store at `path`, so a second one
/// pointed at the same file can be asked what the first one did.
async fn server_storing(replies: Vec<String>, path: &std::path::Path) -> String {
    let base = std::env::current_dir().expect("the working directory");
    let agency = Agency {
        tools: Arc::new(Tools::standard()),
        sandbox: Arc::new(Sandbox::new(&SandboxPolicy::default(), &base).expect("the sandbox")),
        limits: agent_core::agent::Limits::default().with_max_steps(4),
        worker: None,
    };
    let serving = bind(ServeOptions {
        approvers: Default::default(),
        address: "127.0.0.1:0".parse().expect("a loopback address"),
        backend: Arc::new(Mock::replies(replies).delay(Duration::ZERO)),
        model: "mock".into(),
        record: None,
        budget: Budget::new(0, 0, Eviction::Turn),
        counter: Arc::new(ApproximateCounter),
        agency,
        temperature: None,
        seed: None,
        map_tokens: 0,
        map_order: Default::default(),
        map_fill: Default::default(),
        auth_token_file: None,
        store: Some(path.to_path_buf()),
    })
    .await
    .expect("binding the server");
    let address = serving.address();
    tokio::spawn(serving.run());
    address.to_string()
}

/// A server that will only take a signed approval, and the key that can make
/// one. Loopback, so nothing here is testing the token — what is being asserted
/// is that the signature is checked on its own account.
async fn server_signing() -> (String, Signer) {
    let signer = Signer::generate().expect("a key");
    let approvers = Approvers {
        required: true,
        keys: vec![ApproverKey {
            name: "jgermade".into(),
            public: signer.public(),
        }],
    };
    let address = server_everything(
        vec![PLAN.into(), ANSWER.into()],
        Duration::ZERO,
        SandboxPolicy::default(),
        Budget::new(0, 0, Eviction::Turn),
        None,
        approvers,
    )
    .await;
    (address, signer)
}

async fn server_authed(
    replies: Vec<String>,
    delay: Duration,
    policy: SandboxPolicy,
    budget: Budget,
    auth_token_file: Option<std::path::PathBuf>,
) -> String {
    server_everything(
        replies,
        delay,
        policy,
        budget,
        auth_token_file,
        Approvers::default(),
    )
    .await
}

async fn server_everything(
    replies: Vec<String>,
    delay: Duration,
    policy: SandboxPolicy,
    budget: Budget,
    auth_token_file: Option<std::path::PathBuf>,
    approvers: Approvers,
) -> String {
    let base = std::env::current_dir().expect("the working directory");
    let agency = Agency {
        tools: Arc::new(Tools::standard()),
        sandbox: Arc::new(Sandbox::new(&policy, &base).expect("the sandbox")),
        limits: agent_core::agent::Limits::default().with_max_steps(4),
        worker: None,
    };
    let serving = bind(ServeOptions {
        approvers,
        address: "127.0.0.1:0".parse().expect("a loopback address"),
        backend: Arc::new(Mock::replies(replies).delay(delay)),
        model: "mock".into(),
        record: None,
        budget,
        counter: Arc::new(ApproximateCounter),
        agency,
        temperature: None,
        seed: None,
        map_tokens: 0,
        map_order: Default::default(),
        map_fill: Default::default(),
        auth_token_file,
        store: None,
    })
    .await
    .expect("binding the server");

    let address = serving.address();
    tokio::spawn(serving.run());
    address.to_string()
}

async fn next_message(socket: &mut Socket) -> Value {
    let frame = tokio::time::timeout(PATIENCE, socket.next())
        .await
        .expect("the server went quiet")
        .expect("the socket closed")
        .expect("a frame");
    match frame {
        WsMessage::Text(text) => serde_json::from_str(&text).expect("a protocol message"),
        other => panic!("expected text, got {other:?}"),
    }
}

/// Reads until the named message arrives, collecting the tokens on the way.
/// The transcript is what the assertions are about; the order tokens arrive in
/// relative to each other is `run_turn`'s business and is tested there.
async fn until(socket: &mut Socket, kind: &str) -> (Value, String) {
    let mut text = String::new();
    loop {
        let message = next_message(socket).await;
        match message["type"].as_str().expect("a typed message") {
            "token" => text.push_str(message["text"].as_str().unwrap_or_default()),
            found if found == kind => return (message, text),
            _ => continue,
        }
    }
}

async fn send(socket: &mut Socket, message: Value) {
    socket
        .send(WsMessage::Text(message.to_string().into()))
        .await
        .expect("sending");
}

async fn get(address: &str, path: &str) -> Value {
    reqwest::get(format!("http://{address}{path}"))
        .await
        .expect("the request")
        .json()
        .await
        .expect("a JSON body")
}

async fn post(address: &str, path: &str) -> reqwest::Response {
    let client = reqwest::Client::new();
    client
        .post(format!("http://{address}{path}"))
        .send()
        .await
        .expect("the request")
}

async fn delete(address: &str, path: &str) -> reqwest::Response {
    let client = reqwest::Client::new();
    client
        .delete(format!("http://{address}{path}"))
        .send()
        .await
        .expect("the request")
}

#[tokio::test]
async fn a_prompt_is_planned_approved_and_answered_over_the_socket() {
    let address = server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");

    let hello = next_message(&mut socket).await;
    assert_eq!(hello["type"], "hello");
    // 5 since the handshake, 4 since `jobs`, 3 since `evicted`, 2 since `refused`: a new
    // variant of a tagged enum is a change an older reader cannot parse, which is what this
    // number is for.
    assert_eq!(hello["protocol"], 5);
    assert_eq!(hello["backend"], "mock");
    assert!(hello["turn"].is_null(), "nothing is running yet");
    assert!(
        hello["session"].is_string(),
        "what an approval is signed against, so a signature does not replay elsewhere",
    );

    // The gate: the prompt buys a planning call and is then held.
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "add a flag"}),
    )
    .await;

    let (started, _) = until(&mut socket, "turn_started").await;
    assert_eq!(started["turn"], 1);
    assert_eq!(
        started["prompt"], "add a flag",
        "the user's ask, not the planning instruction fused in front of it",
    );

    let (ended, planning) = until(&mut socket, "ended").await;
    assert_eq!(ended["turn"], 1);
    assert_eq!(ended["reason"], "stop");
    assert_eq!(planning, PLAN, "the planning call's own text");

    let (proposed, _) = until(&mut socket, "job_proposed").await;
    assert_eq!(proposed["job"], 1);
    assert_eq!(proposed["objective"], "add a flag");
    assert_eq!(proposed["plan"]["files"][0], "Cargo.toml");

    // Nothing has run under the job yet. A second prompt here is a second
    // thing nobody approved, and the server — not the client — refuses it,
    // out loud: a refusal a client cannot tell from a dropped message is why
    // the UI used to have to guess by disabling its own composer.
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "and also this"}),
    )
    .await;

    let (refused, _) = until(&mut socket, "refused").await;
    assert_eq!(refused["request"], "prompt");
    assert_eq!(refused["reason"], "pending");
    assert!(
        refused["detail"]
            .as_str()
            .expect("a detail")
            .contains("job 1"),
        "{refused}",
    );

    send(
        &mut socket,
        serde_json::json!({"type": "approve_job", "job": 1}),
    )
    .await;

    let (approved, _) = until(&mut socket, "job_approved").await;
    assert_eq!(approved["job"], 1);

    let (started, _) = until(&mut socket, "turn_started").await;
    assert_eq!(started["turn"], 2);
    assert_eq!(started["prompt"], "add a flag", "the held prompt, now run");
    assert_eq!(started["job"], 1, "inside the job it was approved under");

    let (ended, answer) = until(&mut socket, "ended").await;
    assert_eq!(ended["turn"], 2);
    assert_eq!(answer, ANSWER);
    assert!(ended["usage"]["completion_tokens"].as_u64().unwrap_or(0) > 0);

    // The read side, folded from the events just watched. It is served by the
    // same process over the same port, so this is the live API, not an export.
    let sessions = get(&address, "/api/sessions").await;
    assert_eq!(sessions.as_array().expect("an array").len(), 1);
    assert_eq!(sessions[0]["id"], "live");
    assert_eq!(sessions[0]["backend"], "mock");
    assert_eq!(
        sessions[0]["turns"], 2,
        "the planning call is a turn: it costs a window and every panel explains it",
    );

    let turns = get(&address, "/api/sessions/live/turns").await;
    assert_eq!(turns[0]["turn"], 1);
    assert_eq!(turns[0]["text"], PLAN);
    assert_eq!(turns[1]["turn"], 2);
    assert_eq!(turns[1]["text"], ANSWER);
    assert_eq!(turns[1]["job"], 1);
    assert_eq!(
        turns[1]["reason"], "stop",
        "the read side agrees with the `ended` the socket carried",
    );

    // Both spellings answer, because a static host can only mirror one of them.
    let suffixed = get(&address, "/api/sessions/live/turns.json").await;
    assert_eq!(suffixed, turns);

    let prompt = get(&address, "/api/sessions/live/turns/1/prompt").await;
    let planning_prompt = prompt["text"].as_str().expect("the prompt as sent");
    assert!(
        planning_prompt.contains(SYSTEM),
        "the system block is the prefix every call shares: {planning_prompt}",
    );
    assert!(
        planning_prompt.contains("propose a plan"),
        "the planning instruction is fused into the user message: {planning_prompt}",
    );

    let prompt = get(&address, "/api/sessions/live/turns/2/prompt").await;
    let answer_prompt = prompt["text"].as_str().expect("the prompt as sent");
    assert!(
        !answer_prompt.contains("propose a plan"),
        "the instruction is not paid for again on the turn that answers: {answer_prompt}",
    );

    // The second prompt sent behind the gate never became anything.
    let session = get(&address, "/api/sessions/live").await;
    assert_eq!(session["jobs"].as_array().expect("the jobs").len(), 1);
    assert!(
        !session["turns"]
            .as_array()
            .expect("the turns")
            .iter()
            .any(|turn| turn["prompt"] == "and also this"),
        "a prompt sent while a proposal was pending must not have run",
    );
}

/// A client that says something the protocol does not define must not take the
/// server down for the others — the socket stays open and the next message is
/// still answered.
#[tokio::test]
async fn unparseable_input_does_not_kill_the_socket() {
    let address = server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    send(&mut socket, serde_json::json!({"type": "explode"})).await;
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "add a flag"}),
    )
    .await;

    let (started, _) = until(&mut socket, "turn_started").await;
    assert_eq!(started["turn"], 1);
}

/// A path nobody serves is a 404 and not a panic, and the UI's own index is
/// served from the embedded assets rather than from a directory that only
/// exists in a checkout.
#[tokio::test]
async fn the_page_and_the_missing_page() {
    let address = server().await;

    let index = reqwest::get(format!("http://{address}/"))
        .await
        .expect("the request");
    assert!(index.status().is_success());
    let body = index.text().await.expect("the body");
    assert!(body.contains('<'), "the index is not empty");

    let missing = reqwest::get(format!("http://{address}/api/sessions/nope"))
        .await
        .expect("the request");
    assert_eq!(missing.status(), 404);
}

/// The token gates authority and the read side, and does not gate the page.
///
/// Three surfaces, three answers: `/ws` carries `approve_task` and is refused
/// without the token, `/api/*` carries this session's prompts and is refused
/// the same way, and the embedded UI is served to anyone — it is the same
/// bytes in every copy of the binary, and a browser cannot put a header on a
/// navigation.
#[tokio::test]
async fn a_bearer_token_gates_the_socket_and_the_read_side() {
    let (address, dir) = server_guarded("s3cret").await;

    let anonymous = reqwest::get(format!("http://{address}/api/sessions"))
        .await
        .expect("the request");
    assert_eq!(anonymous.status(), 401);
    assert_eq!(
        anonymous
            .headers()
            .get("www-authenticate")
            .and_then(|value| value.to_str().ok()),
        Some("Bearer"),
    );

    let bearer = reqwest::Client::new()
        .get(format!("http://{address}/api/sessions"))
        .bearer_auth("s3cret")
        .send()
        .await
        .expect("the request");
    assert!(bearer.status().is_success(), "{:?}", bearer.status());

    // The page: open, and the only thing that is.
    let index = reqwest::get(format!("http://{address}/"))
        .await
        .expect("the request");
    assert!(index.status().is_success());

    // The socket, both ways round. The browser concession is the query
    // parameter, so that is what the passing half uses.
    tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect_err("an unauthenticated upgrade");
    let (mut socket, _) =
        tokio_tungstenite::connect_async(format!("ws://{address}/ws?token=s3cret"))
            .await
            .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    std::fs::remove_dir_all(&dir).ok();
}

/// Narrowing, over the socket: the plan names `Cargo.toml`, the turn asks for
/// `src/serve.rs`, and the sandbox that refuses is the plan rather than the
/// policy file — which still grants it.
#[tokio::test]
async fn a_turn_may_not_touch_what_its_task_was_not_approved_for() {
    let address = server_with(vec![
        PLAN_FOR_CARGO_TOML.into(),
        READS_SERVE_RS.into(),
        "I was not approved to read that.".into(),
    ])
    .await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "read the manifest"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;
    send(
        &mut socket,
        serde_json::json!({"type": "approve_job", "job": 1}),
    )
    .await;

    let (result, _) = until(&mut socket, "tool_result").await;
    assert_eq!(result["name"], "read_file");
    assert_eq!(result["verdict"]["allowed"], false);
    assert!(
        result["verdict"]["rule"]
            .as_str()
            .expect("a rule")
            .contains("the approved plan for job 1"),
        "a denial has to say which authority refused: {}",
        result["verdict"]["rule"],
    );

    // The same file, under the policy file the job narrowed: still granted.
    // The refusal is the job's, not the session's.
    let sandbox = Sandbox::new(
        &SandboxPolicy::default(),
        &std::env::current_dir().expect("the working directory"),
    )
    .expect("the sandbox");
    assert!(
        sandbox
            .check_path(std::path::Path::new("src/serve.rs"), Access::Read)
            .verdict
            .allowed,
    );
}

/// The other half, which is what makes narrowing survivable: the person at the
/// gate adds the file the plan forgot, and the turn goes through.
#[tokio::test]
async fn a_file_added_at_the_gate_is_in_the_task_sandbox() {
    let address = server_with(vec![
        PLAN_FOR_CARGO_TOML.into(),
        READS_SERVE_RS.into(),
        "Read it.".into(),
    ])
    .await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "read the manifest"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;

    // Approving *with* an amendment, plus one entry the policy file does not
    // grant: the gate widens a plan up to the file and not past it.
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_job",
            "job": 1,
            "files": ["src/serve.rs", "/etc/passwd"],
            "commands": [],
        }),
    )
    .await;

    let (approved, _) = until(&mut socket, "job_approved").await;
    let files = approved["plan"]["files"]
        .as_array()
        .expect("the plan as approved");
    assert_eq!(files, &["Cargo.toml", "src/serve.rs"]);
    assert!(
        !files.iter().any(|file| file == "/etc/passwd"),
        "what the policy file does not grant is not approved by a person asking for it",
    );

    let (result, _) = until(&mut socket, "tool_result").await;
    assert_eq!(result["name"], "read_file");
    assert_eq!(
        result["verdict"]["allowed"], true,
        "{}",
        result["verdict"]["rule"],
    );

    // And the read API carries the plan as approved, not as proposed.
    let session = get(&address, "/api/sessions/live").await;
    assert_eq!(session["jobs"][0]["plan"]["files"][1], "src/serve.rs");
}

/// The three other silences, each of which used to be an early return.
#[tokio::test]
async fn the_server_says_why_it_did_not_do_something() {
    // A slow mock, so the second prompt lands while the first turn is still
    // running rather than after it — which is the state being tested.
    let address = server_with(vec![PLAN.into(), ANSWER.into()]).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    // Nothing is open, so nothing can be closed, reopened or rejected.
    for (request, reason) in [
        ("close_job", "job"),
        ("reopen_job", "job"),
        ("reject_job", "job"),
        ("approve_job", "job"),
    ] {
        send(&mut socket, serde_json::json!({"type": request, "job": 7})).await;
        let (refused, _) = until(&mut socket, "refused").await;
        assert_eq!(refused["request"], request);
        assert_eq!(refused["reason"], reason, "{refused}");
        assert!(
            refused["detail"].as_str().expect("a detail").contains("7"),
            "a refusal names what was refused: {refused}",
        );
    }
}

/// A prompt sent while a turn is running: the one the `busy` item was named
/// after. The planning call is still streaming when the second prompt arrives.
#[tokio::test]
async fn a_prompt_while_a_turn_runs_is_refused_as_busy() {
    let address = server_slow(vec![PLAN.into(), ANSWER.into()]).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "add a flag"}),
    )
    .await;
    // The turn has started and its tokens are arriving one every 30ms, so this
    // arrives in the middle of it.
    until(&mut socket, "turn_started").await;
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "and this too"}),
    )
    .await;

    let (refused, _) = until(&mut socket, "refused").await;
    assert_eq!(refused["request"], "prompt");
    assert_eq!(refused["reason"], "busy");
    assert!(
        refused["detail"]
            .as_str()
            .expect("a detail")
            .contains("turn 1"),
        "{refused}",
    );
}

/// The fourth one, and the newest: an amendment at the gate naming something
/// the policy file does not grant is dropped from the plan — and now says so.
#[tokio::test]
async fn an_amendment_the_policy_refuses_is_reported_not_only_dropped() {
    let address = server_with(vec![
        PLAN_FOR_CARGO_TOML.into(),
        "Read it.".into(),
        "Read it.".into(),
    ])
    .await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "read the manifest"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_job",
            "job": 1,
            "files": ["/etc/passwd"],
            "commands": [],
        }),
    )
    .await;

    let (refused, _) = until(&mut socket, "refused").await;
    assert_eq!(refused["request"], "approve_job");
    assert_eq!(refused["reason"], "not_granted");
    assert!(
        refused["detail"]
            .as_str()
            .expect("a detail")
            .contains("/etc/passwd"),
        "{refused}",
    );

    // And the job still runs: the approval was not thrown away with the part
    // of it nobody may grant.
    let (approved, _) = until(&mut socket, "job_approved").await;
    assert_eq!(approved["plan"]["files"], serde_json::json!(["Cargo.toml"]));
}

/// The lifecycle is a state machine and the socket is open to anyone: closing
/// a job that was only *proposed* used to succeed, which took the gate off
/// the screen with its prompt still held and left the session with no way to
/// answer a proposal nobody could see. Found by driving the real page.
#[tokio::test]
async fn a_proposal_cannot_be_closed_out_from_under_the_gate() {
    let address = server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "add a flag"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;

    send(
        &mut socket,
        serde_json::json!({"type": "close_job", "job": 1}),
    )
    .await;
    let (refused, _) = until(&mut socket, "refused").await;
    assert_eq!(refused["request"], "close_job");
    assert_eq!(refused["reason"], "job");

    // Still waiting on a person, and still answerable.
    let session = get(&address, "/api/sessions/live").await;
    assert_eq!(session["jobs"][0]["state"], "proposed");

    send(
        &mut socket,
        serde_json::json!({"type": "approve_job", "job": 1}),
    )
    .await;
    let (started, _) = until(&mut socket, "turn_started").await;
    assert_eq!(started["turn"], 2, "the held prompt still runs");
}

/// Narrowing on level, over the socket: the plan says it will *read* the
/// manifest, so writing it is refused by the task's own plan — and the policy
/// file, which grants read-write on the tree, is not what refused.
#[tokio::test]
async fn a_turn_may_not_write_a_file_its_plan_only_reads() {
    let address = server_with(vec![
        PLAN_FOR_CARGO_TOML.into(),
        WRITES_SCRATCH.into(),
        "I was not approved to change that.".into(),
    ])
    .await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "read the manifest"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;
    send(
        &mut socket,
        serde_json::json!({"type": "approve_job", "job": 1}),
    )
    .await;

    let (result, _) = until(&mut socket, "tool_result").await;
    assert_eq!(result["name"], "write_file");
    assert_eq!(result["verdict"]["allowed"], false);
    assert!(
        result["verdict"]["rule"]
            .as_str()
            .expect("a rule")
            .contains("the approved plan for job 1"),
        "{}",
        result["verdict"]["rule"],
    );

    // The manifest is untouched, which is the only assertion that would notice
    // a check that ran after the write rather than before it.
    let manifest = std::fs::read_to_string("Cargo.toml").expect("the manifest");
    assert!(
        manifest.contains("[package]"),
        "the file was written anyway"
    );
}

/// And the gate can grant it: *add write* at the approval, and the same turn
/// goes through. The path is one nothing else reads, so the test can write it.
#[tokio::test]
async fn a_write_added_at_the_gate_goes_through() {
    let scratch = std::env::temp_dir().join(format!("luu-write-{}.txt", std::process::id()));
    let _ = std::fs::remove_file(&scratch);
    let call = format!(
        "Writing it.\n```tool\n{{\"name\":\"write_file\",\"arguments\":         {{\"path\":{},\"content\":\"written by the task\"}}}}\n```",
        serde_json::to_string(&scratch.display().to_string()).expect("a path"),
    );
    let address = server_writable(
        vec![PLAN_FOR_CARGO_TOML.into(), call, "Done.".into()],
        &scratch,
    )
    .await;

    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "write the scratch file"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_job",
            "job": 1,
            "writes": [scratch.display().to_string()],
        }),
    )
    .await;

    let (approved, _) = until(&mut socket, "job_approved").await;
    assert_eq!(
        approved["plan"]["writes"].as_array().expect("writes").len(),
        1
    );

    let (result, _) = until(&mut socket, "tool_result").await;
    assert_eq!(
        result["verdict"]["allowed"], true,
        "{}",
        result["verdict"]["rule"],
    );
    assert_eq!(
        std::fs::read_to_string(&scratch).expect("the file the task wrote"),
        "written by the task",
    );
    let _ = std::fs::remove_file(&scratch);
}

/// The window filling up, over the socket: the tombstone that says what the
/// session forgot. Before it, a client watched the history bucket shrink and
/// could not tell the policy from the arithmetic.
#[tokio::test]
async fn the_window_filling_up_says_which_turns_it_dropped() {
    // Long enough that a couple of them no longer fit together.
    let answer = "padding ".repeat(60);
    let address = server_evicting(vec![PLAN.into(), answer.clone()]).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");

    assert_eq!(next_message(&mut socket).await["type"], "hello");

    // Through the gate once, so the prompts after it run inside a live job
    // rather than each buying a proposal of its own.
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "add a flag"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;
    send(
        &mut socket,
        serde_json::json!({"type": "approve_job", "job": 1}),
    )
    .await;
    until(&mut socket, "ended").await;

    // Then prompt until the window gives way. Bounded: a run that never cuts is
    // this test failing, not this test waiting.
    let mut evicted = None;
    for n in 0..8 {
        send(
            &mut socket,
            serde_json::json!({"type": "prompt", "text": format!("question {n}")}),
        )
        .await;
        loop {
            let message = next_message(&mut socket).await;
            match message["type"].as_str().expect("a typed message") {
                "evicted" => evicted = Some(message),
                "ended" => break,
                _ => continue,
            }
        }
        if evicted.is_some() {
            break;
        }
    }

    let evicted = evicted.expect("eight turns into a window this size, something left");
    let turns: Vec<u64> = evicted["turns"]
        .as_array()
        .expect("the turns that left")
        .iter()
        .map(|turn| turn.as_u64().expect("a turn number"))
        .collect();
    assert_eq!(
        turns[0], 2,
        "the oldest in the *history* — turn 1 was the planning call, which is a \
         turn of the session and was never remembered, so it cannot be dropped",
    );
    assert!(
        turns.iter().max() < evicted["turn"].as_u64().as_ref(),
        "a turn cannot evict itself: {evicted}",
    );
    assert!(evicted["tokens"].as_u64().expect("a count") > 0);
    assert_eq!(
        evicted["counter"]["kind"], "approximate",
        "a count carries who produced it, and this one is not a measurement",
    );
    assert_eq!(evicted["policy"]["policy"], "turn");

    // The read side, folded from the same events: both halves of the mark.
    let cutting = evicted["turn"].as_u64().expect("the cutting turn");
    let api = get(&address, "/api/sessions/live/turns").await;
    let first = api
        .as_array()
        .expect("the turns")
        .iter()
        .find(|turn| turn["turn"] == 2)
        .expect("turn 2 is still in the transcript");
    assert_eq!(
        first["evicted_by"], cutting,
        "an evicted turn is kept and marked, never removed: the transcript exists \
         to show the difference between what happened and what the model still sees",
    );
    let cutter = api
        .as_array()
        .expect("the turns")
        .iter()
        .find(|turn| turn["turn"] == cutting)
        .expect("the turn that cut");
    assert_eq!(cutter["dropped"]["turns"][0], 2);
    assert!(
        api.as_array()
            .expect("the turns")
            .iter()
            .find(|turn| turn["turn"] == 1)
            .expect("the planning call is still a turn")["evicted_by"]
            .is_null(),
        "nothing evicted the planning call: it was never in the window to leave it",
    );
}

/// The gate's headline number, over the socket: whether the planning call
/// produced the plan, or answered in prose and left the proposal to be the ask
/// itself. The panel used to infer it from an empty plan, which cannot tell a
/// model that ignored the format from one that declared an empty list.
#[tokio::test]
async fn a_proposal_says_whether_a_model_wrote_it_or_only_talked() {
    for (reply, expected) in [
        (PLAN, "model"),
        // A 7B answering the planning call in prose is the ordinary case, and
        // it must not cost the gate.
        ("I could add the flag in lib.rs, I think.", "prose"),
    ] {
        let address = server_with(vec![reply.into(), ANSWER.into()]).await;
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
            .await
            .expect("the websocket handshake");
        assert_eq!(next_message(&mut socket).await["type"], "hello");
        send(
            &mut socket,
            serde_json::json!({"type": "prompt", "text": "add a flag"}),
        )
        .await;

        let (proposed, _) = until(&mut socket, "job_proposed").await;
        assert_eq!(proposed["source"], expected, "{proposed}");

        let session = get(&address, "/api/sessions/live").await;
        assert_eq!(
            session["jobs"][0]["source"], expected,
            "and on the read side"
        );
    }
}

/// What a person had to add before a small model's plan could run — the amend
/// rate, which is the cost of the gate. Readable only by diffing two lines of a
/// recording by hand until the view kept both.
#[tokio::test]
async fn the_view_keeps_the_plan_as_proposed_beside_the_plan_as_approved() {
    let address = server_with(vec![PLAN.into(), ANSWER.into()]).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "add a flag"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;

    // The person adds the file the model forgot, which is the half that makes
    // narrowing survivable.
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_job",
            "job": 1,
            "files": ["AGENTS.md"],
        }),
    )
    .await;
    until(&mut socket, "job_approved").await;

    let job = get(&address, "/api/sessions/live").await["jobs"][0].clone();
    assert_eq!(
        job["proposed"]["files"],
        serde_json::json!(["Cargo.toml"]),
        "the plan as the model proposed it",
    );
    assert_eq!(
        job["plan"]["files"],
        serde_json::json!(["Cargo.toml", "AGENTS.md"]),
        "the plan as approved, which is what the sandbox is built from",
    );
}

/// The rung above the person: a job that closes itself on an exit code.
///
/// The one test that asserts the payoff rather than the field. A plan declares
/// `sh`, the person at the gate names `sh -c exit 0` as what would convince
/// them the work is finished, the turn runs it, and the job folds with nobody
/// having clicked anything. See
/// `RECORD/2026-09-02.closing-on-an-exit-code.completed.md`.
const PLAN_THAT_RUNS_SH: &str = "```plan\n{\"objective\":\"make it pass\",\
                                 \"steps\":[\"run it\"],\"files\":[],\
                                 \"commands\":[\"sh\"]}\n```";

fn runs(script: &str) -> String {
    format!(
        "Running it.\n```tool\n{{\"name\":\"run_command\",\"arguments\":\
         {{\"command\":\"sh\",\"args\":[\"-c\",\"{script}\"]}}}}\n```"
    )
}

#[tokio::test]
async fn a_green_command_closes_the_task_with_nobody_at_the_gate() {
    let address = server_running(
        vec![
            PLAN_THAT_RUNS_SH.into(),
            runs("exit 0"),
            "It passes.".into(),
        ],
        "sh",
    )
    .await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "make the tests pass"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;

    // The one part of a plan the model was never asked for, arriving from the
    // person who is already reading the plan.
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_job",
            "job": 1,
            "closes_on": "sh -c exit 0",
        }),
    )
    .await;
    let (approved, _) = until(&mut socket, "job_approved").await;
    assert_eq!(approved["plan"]["closes_on"], "sh -c exit 0");

    let (result, _) = until(&mut socket, "tool_result").await;
    assert_eq!(
        result["verdict"]["allowed"], true,
        "the command has to run before its exit code can close anything: {result}",
    );
    assert_eq!(result["command"]["exit_code"], 0, "{result}");

    let (closed, _) = until(&mut socket, "job_closed").await;
    assert_eq!(closed["job"], 1);
    assert_eq!(
        closed["by"], "exit_code",
        "which authority folded it is on the wire, or nothing can ever count the rungs",
    );
    assert!(
        closed["summary"]
            .as_str()
            .expect("a summary")
            .contains("run_command sh -c exit 0"),
        "the close still writes the evidence: {}",
        closed["summary"],
    );

    // And the read side agrees with what the socket carried, which is the one
    // property this file exists to keep proving.
    let session = get(&address, "/api/sessions/live").await;
    assert_eq!(session["jobs"][0]["state"], "closed");
    assert_eq!(session["jobs"][0]["closed_by"], "exit_code");
}

#[tokio::test]
async fn a_red_command_leaves_the_task_open() {
    let address = server_running(
        vec![
            PLAN_THAT_RUNS_SH.into(),
            runs("exit 1"),
            "It still fails.".into(),
            "Looking again.".into(),
        ],
        "sh",
    )
    .await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "make the tests pass"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_job",
            "job": 1,
            "closes_on": "sh -c exit 0",
        }),
    )
    .await;
    until(&mut socket, "job_approved").await;
    let (result, _) = until(&mut socket, "tool_result").await;
    assert_eq!(
        result["command"]["exit_code"], 1,
        "the command has to have run for its exit code to mean anything: {result}",
    );
    until(&mut socket, "ended").await;

    // Asserted by asking for something the answer changes, rather than by
    // waiting for a message that should not arrive: inside a live job a prompt
    // is a turn, and behind a closed one it is a new proposal.
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "what failed?"}),
    )
    .await;
    let (started, _) = until(&mut socket, "turn_started").await;
    assert_eq!(
        started["job"], 1,
        "a job whose condition was not met is still the live one",
    );
}

/// The gate widens a plan up to the policy file and not past it, and a closing
/// condition is no exception — one naming a command the job may not run can
/// never be met, and a job that can never close looks like one that will.
#[tokio::test]
async fn a_closing_condition_the_plan_cannot_run_is_refused() {
    let address = server_running(vec![PLAN_THAT_RUNS_SH.into(), "Done.".into()], "sh").await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    assert_eq!(next_message(&mut socket).await["type"], "hello");

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "make the tests pass"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_job",
            "job": 1,
            "closes_on": "cargo test",
        }),
    )
    .await;

    let (approved, _) = until(&mut socket, "job_approved").await;
    assert!(
        approved["plan"]["closes_on"].is_null(),
        "the condition is missing from the plan that comes back, which is the feedback",
    );
}

/// What the store is for: a session that outlives the process that had it.
///
/// The assertion is not that a row exists — it is that the *second* server can
/// answer the read side's questions about a session it never ran, which is the
/// whole of "resume" that the fold alone can deliver. See
/// `RECORD/2026-09-02.sessions-in-sqlite.completed.md` for what it deliberately does
/// not deliver.
#[tokio::test]
async fn a_session_outlives_the_server_that_ran_it() {
    let dir = std::env::temp_dir().join(format!("luu-store-e2e-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let db = dir.join("sessions.db");

    let first = server_storing(vec![PLAN.into(), ANSWER.into()], &db).await;
    {
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{first}/ws"))
            .await
            .expect("the websocket handshake");
        assert_eq!(next_message(&mut socket).await["type"], "hello");
        send(
            &mut socket,
            serde_json::json!({"type": "prompt", "text": "add a flag"}),
        )
        .await;
        until(&mut socket, "job_proposed").await;
        send(
            &mut socket,
            serde_json::json!({"type": "approve_job", "job": 1}),
        )
        .await;
        until(&mut socket, "ended").await;
    }

    // A second server, same file, nothing shared but the store.
    let second = server_storing(vec!["another answer".into()], &db).await;
    let listed = get(&second, "/api/sessions").await;
    let sessions = listed.as_array().expect("a listing");
    assert!(
        sessions.len() >= 2,
        "the live session and the one that ended: {listed}",
    );

    // The live row is the second server's, under the name every client asks
    // for; the other is the first server's session, which nothing in this
    // process is holding any more.
    let earlier = sessions
        .iter()
        .find(|row| row["id"] != "live")
        .expect("the session the first server ran");
    let id = earlier["id"].as_str().expect("its id");
    assert_ne!(
        id, "live",
        "a store keyed on `live` holds one session forever"
    );
    assert_eq!(
        earlier["turns"].as_u64(),
        Some(2),
        "the planning call is a turn too, and the fold keeps it: {earlier}",
    );

    let session = get(&second, &format!("/api/sessions/{id}")).await;
    assert_eq!(session["turns"][0]["prompt"], "add a flag");
    assert_eq!(session["jobs"][0]["state"], "approved");
    assert_eq!(
        session["turns"][1]["text"], ANSWER,
        "the fold, not a summary of it: {session}",
    );

    let turns = get(&second, &format!("/api/sessions/{id}/turns")).await;
    assert_eq!(turns.as_array().expect("its turns").len(), 2);

    std::fs::remove_dir_all(&dir).ok();
}

/// A proposal that outlived the server that made it comes back as a gate.
///
/// Without this the job was restored in `proposed` with nothing holding it: the
/// next prompt did not belong to it, so it bought a second planning call and a
/// second proposal beside a question nobody had answered. The held prompt is
/// gone with the process that took it, so approving opens the job and starts
/// nothing — and the next prompt is a turn inside it.
#[tokio::test]
async fn a_proposal_that_outlived_its_server_comes_back_at_the_gate() {
    let dir = std::env::temp_dir().join(format!("luu-gate-resume-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let db = dir.join("sessions.db");

    // A prompt, planned, and left at the gate: nothing approves it.
    let first = server_storing(vec![PLAN.into()], &db).await;
    let session = {
        let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{first}/ws"))
            .await
            .expect("the websocket handshake");
        let hello = next_message(&mut socket).await;
        let session = hello["session"]
            .as_str()
            .expect("the session id")
            .to_string();
        send(
            &mut socket,
            serde_json::json!({"type": "prompt", "text": "add a flag"}),
        )
        .await;
        until(&mut socket, "job_proposed").await;
        session
    };

    // A second server, same file. Resuming puts the gate back up.
    let second = server_storing(vec!["a later answer".into()], &db).await;
    let resumed = post(&second, &format!("/api/sessions/{session}/resume")).await;
    assert_eq!(resumed.status(), reqwest::StatusCode::OK);

    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{second}/ws"))
        .await
        .expect("the websocket handshake");
    let _ = next_message(&mut socket).await;

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "and now something else"}),
    )
    .await;
    let (refused, _) = until(&mut socket, "refused").await;
    assert_eq!(refused["reason"], "pending");
    assert!(
        refused["detail"]
            .as_str()
            .expect("a detail")
            .contains("job 1"),
        "the refusal names the job still waiting on a person: {refused}",
    );

    send(
        &mut socket,
        serde_json::json!({"type": "approve_job", "job": 1}),
    )
    .await;
    let (approved, _) = until(&mut socket, "job_approved").await;
    assert_eq!(approved["job"], 1);

    // The prompt that bought the planning call went with the process that held
    // it, so approving starts nothing — and the next prompt is the turn.
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "now say something"}),
    )
    .await;
    let (started, _) = until(&mut socket, "turn_started").await;
    assert_eq!(
        started["job"], 1,
        "the turn belongs to the job that was approved: {started}",
    );
    assert_eq!(started["prompt"], "now say something");

    let live = get(&second, "/api/sessions/live").await;
    assert_eq!(
        live["jobs"].as_array().expect("its jobs").len(),
        1,
        "one job, answered — not a second proposal beside an unanswered one: {live}",
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn multi_session_lifecycle_and_switching() {
    let dir = std::env::temp_dir().join(format!("luu-test-multi-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let db = dir.join("sessions.db");

    let address = server_storing(
        vec![PLAN.into(), ANSWER.into(), PLAN.into(), ANSWER.into()],
        &db,
    )
    .await;

    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("websocket connection");

    let hello = next_message(&mut socket).await;
    assert_eq!(hello["type"], "hello");

    // Run a turn in the initial session
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "initial prompt"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;
    send(
        &mut socket,
        serde_json::json!({"type": "approve_job", "job": 1}),
    )
    .await;
    until(&mut socket, "ended").await;

    // Verify initial session has 2 turns
    let live_one = get(&address, "/api/sessions/live").await;
    assert_eq!(live_one["turns"].as_array().unwrap().len(), 2);

    // Create a new session via POST /api/sessions
    let res = post(&address, "/api/sessions").await;
    assert_eq!(res.status(), reqwest::StatusCode::CREATED);
    let new_summary: Value = res.json().await.expect("json");
    let session_two_id = new_summary["id"].as_str().expect("new id").to_string();
    assert_eq!(new_summary["turns"], 0);

    // Socket received a new hello broadcast
    let (hello2, _) = until(&mut socket, "hello").await;
    assert_eq!(hello2["type"], "hello");
    let live2 = get(&address, "/api/sessions/live").await;
    assert_eq!(live2["turns"].as_array().unwrap().len(), 0);

    // Verify store has 2 sessions now: "live" (new session) and the checkpointed session one
    let listed2 = get(&address, "/api/sessions").await;
    let sessions2 = listed2.as_array().expect("listing 2");
    assert_eq!(sessions2.len(), 2);

    let session_one = sessions2
        .iter()
        .find(|s| s["id"] != "live")
        .expect("checkpointed session one");
    let session_one_id = session_one["id"].as_str().expect("id").to_string();
    assert_ne!(session_one_id, session_two_id);
    assert_eq!(session_one["turns"], 2);

    // Cannot delete active session
    let del_active = delete(&address, &format!("/api/sessions/{session_two_id}")).await;
    assert_eq!(del_active.status(), reqwest::StatusCode::BAD_REQUEST);

    // Resume session one
    let resume_res = post(&address, &format!("/api/sessions/{session_one_id}/resume")).await;
    assert_eq!(resume_res.status(), reqwest::StatusCode::OK);
    let resumed_summary: Value = resume_res.json().await.expect("json");
    assert_eq!(resumed_summary["turns"], 2);

    // Socket receives hello with session one resumed
    let (hello_resumed, _) = until(&mut socket, "hello").await;
    assert_eq!(hello_resumed["type"], "hello");
    let live_resumed = get(&address, "/api/sessions/live").await;
    assert_eq!(live_resumed["turns"].as_array().unwrap().len(), 2);

    // Now session one is active, session two is inactive. Delete session two!
    let del_res = delete(&address, &format!("/api/sessions/{session_two_id}")).await;
    assert_eq!(del_res.status(), reqwest::StatusCode::NO_CONTENT);

    // Session two is gone
    let check_del = reqwest::get(format!("http://{address}/api/sessions/{session_two_id}"))
        .await
        .unwrap();
    assert_eq!(check_del.status(), reqwest::StatusCode::NOT_FOUND);

    std::fs::remove_dir_all(&dir).ok();
}

#[tokio::test]
async fn a_client_that_speaks_another_protocol_is_refused_out_loud() {
    let address = server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    let hello = next_message(&mut socket).await;
    assert_eq!(hello["type"], "hello");

    send(
        &mut socket,
        serde_json::json!({"type": "hello", "protocol": 4, "format": 6}),
    )
    .await;

    let refused = next_message(&mut socket).await;
    assert_eq!(refused["type"], "refused");
    assert_eq!(refused["reason"], "version");
    assert!(
        refused["detail"]
            .as_str()
            .expect("a detail")
            .contains("protocol 5"),
        "the refusal says what this host speaks: {refused}",
    );
    let closed = tokio::time::timeout(PATIENCE, socket.next())
        .await
        .expect("the socket stayed open");
    assert!(
        matches!(closed, None | Some(Ok(WsMessage::Close(_)))),
        "a client the host cannot parse is not left connected: {closed:?}",
    );
}

#[tokio::test]
async fn a_newer_client_is_refused_in_the_other_direction_too() {
    let address = server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    let _ = next_message(&mut socket).await;

    send(
        &mut socket,
        serde_json::json!({"type": "hello", "protocol": 6}),
    )
    .await;

    let refused = next_message(&mut socket).await;
    assert_eq!(
        refused["reason"], "version",
        "a client this host cannot parse is the same answer as one that cannot parse it",
    );
}

#[tokio::test]
async fn a_matching_client_is_greeted_and_then_ignored() {
    let address = server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    let _ = next_message(&mut socket).await;

    send(
        &mut socket,
        serde_json::json!({"type": "hello", "protocol": 5, "format": 7}),
    )
    .await;
    // Nothing comes back: a handshake that matches is not an event in the
    // session, and the next message is the one the prompt causes.
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "add a flag"}),
    )
    .await;
    let (started, _) = until(&mut socket, "turn_started").await;
    assert_eq!(started["turn"], 1);
}

#[tokio::test]
async fn a_guarded_port_hears_the_handshake_before_anything_else() {
    let (address, dir) = server_guarded("s3cret").await;
    let request = tokio_tungstenite::tungstenite::http::Request::builder()
        .uri(format!("ws://{address}/ws"))
        .header("Authorization", "Bearer s3cret")
        .header("Host", address.clone())
        .header("Connection", "Upgrade")
        .header("Upgrade", "websocket")
        .header("Sec-WebSocket-Version", "13")
        .header("Sec-WebSocket-Key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(())
        .expect("the request");
    let (mut socket, _) = tokio_tungstenite::connect_async(request)
        .await
        .expect("the websocket handshake");
    let _ = next_message(&mut socket).await;

    // The token got it through the door. It still may not drive the session
    // until it says what it speaks: reachability and version are two questions.
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "add a flag"}),
    )
    .await;

    let refused = next_message(&mut socket).await;
    assert_eq!(refused["type"], "refused");
    assert_eq!(refused["reason"], "version");
    let _ = std::fs::remove_dir_all(dir);
}

#[tokio::test]
async fn a_signed_approval_runs_the_held_prompt_and_says_who_approved() {
    let (address, signer) = server_signing().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    let hello = next_message(&mut socket).await;
    let session = hello["session"]
        .as_str()
        .expect("the session id")
        .to_string();

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "add a flag"}),
    )
    .await;
    let (proposed, _) = until(&mut socket, "job_proposed").await;
    let job = proposed["job"].as_u64().expect("a job id");

    // Unsigned first: this server was told approvals are signed, and the gate
    // is where that is enforced rather than at the door.
    send(
        &mut socket,
        serde_json::json!({"type": "approve_job", "job": job}),
    )
    .await;
    let (refused, _) = until(&mut socket, "refused").await;
    assert_eq!(refused["reason"], "signature");

    let files = vec!["Cargo.toml".to_string()];
    let signature = signer
        .sign(
            &Approval {
                session: &session,
                job,
                files: &files,
                writes: &[],
                commands: &[],
                closes_on: None,
                network: None,
                egress: None,
            },
            "jgermade",
        )
        .expect("signing");

    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_job",
            "job": job,
            "files": files,
            "signature": {"by": signature.by, "sig": signature.sig},
        }),
    )
    .await;

    let (approved, _) = until(&mut socket, "job_approved").await;
    assert_eq!(approved["approved_by"]["by"], "key");
    assert_eq!(
        approved["approved_by"]["name"], "jgermade",
        "which key, so a recording can count one authority against another",
    );

    // And the read side, folded from the same events, agrees.
    let view = get(&address, "/api/sessions/live").await;
    assert_eq!(view["jobs"][0]["approved_by"]["name"], "jgermade");
}

#[tokio::test]
async fn a_grant_widened_after_the_signature_is_refused() {
    let (address, signer) = server_signing().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");
    let hello = next_message(&mut socket).await;
    let session = hello["session"]
        .as_str()
        .expect("the session id")
        .to_string();

    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "add a flag"}),
    )
    .await;
    let (proposed, _) = until(&mut socket, "job_proposed").await;
    let job = proposed["job"].as_u64().expect("a job id");

    let signed = vec!["Cargo.toml".to_string()];
    let signature = signer
        .sign(
            &Approval {
                session: &session,
                job,
                files: &signed,
                writes: &[],
                commands: &[],
                closes_on: None,
                network: None,
                egress: None,
            },
            "jgermade",
        )
        .expect("signing");

    // What a relay between the person and the gate would do: the same
    // signature, over one more tree.
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_job",
            "job": job,
            "files": ["Cargo.toml", "src"],
            "signature": {"by": signature.by, "sig": signature.sig},
        }),
    )
    .await;

    let (refused, _) = until(&mut socket, "refused").await;
    assert_eq!(refused["reason"], "signature");
    let view = get(&address, "/api/sessions/live").await;
    assert_eq!(
        view["jobs"][0]["state"], "proposed",
        "a refused approval leaves the job exactly as it was",
    );
}
