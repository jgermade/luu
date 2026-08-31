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

use agent_core::backend::mock::Mock;
use agent_core::context::{ApproximateCounter, Budget, Eviction, TokenCounter};
use agent_core::sandbox::{Access, Sandbox, SandboxPolicy};
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

async fn server_full(
    replies: Vec<String>,
    delay: Duration,
    policy: SandboxPolicy,
    budget: Budget,
) -> String {
    let base = std::env::current_dir().expect("the working directory");
    let agency = Agency {
        tools: Arc::new(Tools::standard()),
        sandbox: Arc::new(Sandbox::new(&policy, &base).expect("the sandbox")),
        max_steps: 4,
    };
    let serving = bind(ServeOptions {
        address: "127.0.0.1:0".parse().expect("a loopback address"),
        backend: Arc::new(Mock::replies(replies).delay(delay)),
        model: "mock".into(),
        record: None,
        budget,
        counter: Arc::new(ApproximateCounter),
        agency,
        temperature: None,
        seed: None,
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

#[tokio::test]
async fn a_prompt_is_planned_approved_and_answered_over_the_socket() {
    let address = server().await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{address}/ws"))
        .await
        .expect("the websocket handshake");

    let hello = next_message(&mut socket).await;
    assert_eq!(hello["type"], "hello");
    // 3 since `evicted`, 2 since `refused`: a new variant of a tagged enum is
    // a change an older reader cannot parse, which is what this number is for.
    assert_eq!(hello["protocol"], 3);
    assert_eq!(hello["backend"], "mock");
    assert!(hello["turn"].is_null(), "nothing is running yet");

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

    let (proposed, _) = until(&mut socket, "task_proposed").await;
    assert_eq!(proposed["task"], 1);
    assert_eq!(proposed["objective"], "add a flag");
    assert_eq!(proposed["plan"]["files"][0], "Cargo.toml");

    // Nothing has run under the task yet. A second prompt here is a second
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
            .contains("task 1"),
        "{refused}",
    );

    send(
        &mut socket,
        serde_json::json!({"type": "approve_task", "task": 1}),
    )
    .await;

    let (approved, _) = until(&mut socket, "task_approved").await;
    assert_eq!(approved["task"], 1);

    let (started, _) = until(&mut socket, "turn_started").await;
    assert_eq!(started["turn"], 2);
    assert_eq!(started["prompt"], "add a flag", "the held prompt, now run");
    assert_eq!(started["task"], 1, "inside the task it was approved under");

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
    assert_eq!(turns[1]["task"], 1);
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
    assert_eq!(session["tasks"].as_array().expect("the tasks").len(), 1);
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
    until(&mut socket, "task_proposed").await;
    send(
        &mut socket,
        serde_json::json!({"type": "approve_task", "task": 1}),
    )
    .await;

    let (result, _) = until(&mut socket, "tool_result").await;
    assert_eq!(result["name"], "read_file");
    assert_eq!(result["verdict"]["allowed"], false);
    assert!(
        result["verdict"]["rule"]
            .as_str()
            .expect("a rule")
            .contains("the approved plan for task 1"),
        "a denial has to say which authority refused: {}",
        result["verdict"]["rule"],
    );

    // The same file, under the policy file the task narrowed: still granted.
    // The refusal is the task's, not the session's.
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
    until(&mut socket, "task_proposed").await;

    // Approving *with* an amendment, plus one entry the policy file does not
    // grant: the gate widens a plan up to the file and not past it.
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_task",
            "task": 1,
            "files": ["src/serve.rs", "/etc/passwd"],
            "commands": [],
        }),
    )
    .await;

    let (approved, _) = until(&mut socket, "task_approved").await;
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
    assert_eq!(session["tasks"][0]["plan"]["files"][1], "src/serve.rs");
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
        ("close_task", "task"),
        ("reopen_task", "task"),
        ("reject_task", "task"),
        ("approve_task", "task"),
    ] {
        send(&mut socket, serde_json::json!({"type": request, "task": 7})).await;
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
    until(&mut socket, "task_proposed").await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_task",
            "task": 1,
            "files": ["/etc/passwd"],
            "commands": [],
        }),
    )
    .await;

    let (refused, _) = until(&mut socket, "refused").await;
    assert_eq!(refused["request"], "approve_task");
    assert_eq!(refused["reason"], "not_granted");
    assert!(
        refused["detail"]
            .as_str()
            .expect("a detail")
            .contains("/etc/passwd"),
        "{refused}",
    );

    // And the task still runs: the approval was not thrown away with the part
    // of it nobody may grant.
    let (approved, _) = until(&mut socket, "task_approved").await;
    assert_eq!(approved["plan"]["files"], serde_json::json!(["Cargo.toml"]));
}

/// The lifecycle is a state machine and the socket is open to anyone: closing
/// a task that was only *proposed* used to succeed, which took the gate off
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
    until(&mut socket, "task_proposed").await;

    send(
        &mut socket,
        serde_json::json!({"type": "close_task", "task": 1}),
    )
    .await;
    let (refused, _) = until(&mut socket, "refused").await;
    assert_eq!(refused["request"], "close_task");
    assert_eq!(refused["reason"], "task");

    // Still waiting on a person, and still answerable.
    let session = get(&address, "/api/sessions/live").await;
    assert_eq!(session["tasks"][0]["state"], "proposed");

    send(
        &mut socket,
        serde_json::json!({"type": "approve_task", "task": 1}),
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
    until(&mut socket, "task_proposed").await;
    send(
        &mut socket,
        serde_json::json!({"type": "approve_task", "task": 1}),
    )
    .await;

    let (result, _) = until(&mut socket, "tool_result").await;
    assert_eq!(result["name"], "write_file");
    assert_eq!(result["verdict"]["allowed"], false);
    assert!(
        result["verdict"]["rule"]
            .as_str()
            .expect("a rule")
            .contains("the approved plan for task 1"),
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
    until(&mut socket, "task_proposed").await;
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_task",
            "task": 1,
            "writes": [scratch.display().to_string()],
        }),
    )
    .await;

    let (approved, _) = until(&mut socket, "task_approved").await;
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

    // Through the gate once, so the prompts after it run inside a live task
    // rather than each buying a proposal of its own.
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "add a flag"}),
    )
    .await;
    until(&mut socket, "task_proposed").await;
    send(
        &mut socket,
        serde_json::json!({"type": "approve_task", "task": 1}),
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

        let (proposed, _) = until(&mut socket, "task_proposed").await;
        assert_eq!(proposed["source"], expected, "{proposed}");

        let session = get(&address, "/api/sessions/live").await;
        assert_eq!(
            session["tasks"][0]["source"], expected,
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
    until(&mut socket, "task_proposed").await;

    // The person adds the file the model forgot, which is the half that makes
    // narrowing survivable.
    send(
        &mut socket,
        serde_json::json!({
            "type": "approve_task",
            "task": 1,
            "files": ["AGENTS.md"],
        }),
    )
    .await;
    until(&mut socket, "task_approved").await;

    let task = get(&address, "/api/sessions/live").await["tasks"][0].clone();
    assert_eq!(
        task["proposed"]["files"],
        serde_json::json!(["Cargo.toml"]),
        "the plan as the model proposed it",
    );
    assert_eq!(
        task["plan"]["files"],
        serde_json::json!(["Cargo.toml", "AGENTS.md"]),
        "the plan as approved, which is what the sandbox is built from",
    );
}
