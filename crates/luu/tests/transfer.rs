//! A session moving between two hosts, run rather than described.
//!
//! Two halves, and the second is the one that matters:
//!
//! - the **commands** a person types — `luu transfer` writing a bundle out of a
//!   store and `luu import` reading one into another — driven as processes,
//!   because that is what a person runs;
//! - the **border**, driven through two servers in one process: a session is
//!   started and its job approved on the origin, moved, and the destination is
//!   asked to run inside that job. It refuses until somebody approves it *here*,
//!   which is the whole of stage 1 item 4.
//!
//! Argued in `RECORD/2026-09-04.the-border-and-the-gate.completed.md`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

use agent_core::api::SessionView;
use agent_core::backend::mock::Mock;
use agent_core::context::{ApproximateCounter, Budget, Eviction};
use agent_core::sandbox::{Sandbox, SandboxPolicy};
use agent_core::tools::Tools;
use futures_util::{SinkExt, StreamExt};
use luu::serve::{ServeOptions, bind};
use luu::session::Agency;
use luu::store::SessionStore;
use serde_json::Value;
use tokio_tungstenite::tungstenite::Message as WsMessage;

const PLAN: &str = "```plan\n{\"objective\":\"read the manifest\",\"tasks\":[\"read it\"],\
                    \"files\":[\"Cargo.toml\"],\"commands\":[]}\n```";
const ANSWER: &str = "It is the workspace manifest.";
const PATIENCE: Duration = Duration::from_secs(10);

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root")
}

fn scratch(name: &str) -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("luu-transfer-{name}-{}-{n}", std::process::id()));
    std::fs::remove_dir_all(&dir).ok();
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// One run of the real binary, from the repository root — where `luu.toml` is.
fn luu(args: &[&str]) -> std::process::Output {
    Command::new(env!("CARGO_BIN_EXE_luu"))
        .current_dir(root())
        .args(args)
        .output()
        .expect("running luu")
}

fn ok(output: &std::process::Output) -> String {
    assert!(
        output.status.success(),
        "luu failed: {}",
        String::from_utf8_lossy(&output.stderr),
    );
    String::from_utf8_lossy(&output.stdout).to_string()
}

/// A recorded session with a job that is **approved and still open** — the one
/// shape the border has anything to do with. A written plan is a person's
/// approval put down in advance, which is why `luu chat` can produce one.
fn origin_recording(dir: &Path, plan_file: &str, extra: &[&str]) -> PathBuf {
    let script = dir.join("open-job.txt");
    std::fs::write(
        &script,
        format!(
            "## task: find out what this file is\n## step: read it\n## file: {plan_file}\n\
             what is in it?\n",
        ),
    )
    .expect("writing the script");

    let record = dir.join("origin.jsonl");
    let mut args = vec![
        "chat",
        "--script",
        script.to_str().expect("a utf-8 path"),
        "--mock-delay-ms",
        "0",
        "--sandbox",
        "luu.toml",
        "--record",
        record.to_str().expect("a utf-8 path"),
    ];
    args.extend_from_slice(extra);
    ok(&luu(&args));
    record
}

#[test]
fn a_bundle_carries_the_stream_and_the_origins_sandbox_and_nothing_else() {
    let dir = scratch("bundle");
    let record = origin_recording(&dir, "Cargo.toml", &[]);
    let bundle = dir.join("bundle");

    let out = ok(&luu(&[
        "transfer",
        "--record",
        record.to_str().expect("a utf-8 path"),
        "--out",
        bundle.to_str().expect("a utf-8 path"),
    ]));
    assert!(out.contains("line(s) written"), "{out}");

    // The stream, verbatim: a transfer copies the account and never re-renders
    // it, so what arrives folds to exactly what the origin had.
    let shipped = std::fs::read_to_string(bundle.join("record.jsonl")).expect("the stream");
    let original = std::fs::read_to_string(&record).expect("the recording");
    assert_eq!(shipped, original, "the stream is copied, not rebuilt");

    let manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(bundle.join("manifest.json")).unwrap())
            .expect("the manifest");
    assert_eq!(manifest["kind"], "luu-transfer v1");
    assert_eq!(manifest["protocol"], agent_core::protocol::VERSION);
    assert_eq!(manifest["format"], agent_core::record::FORMAT);
    assert!(
        manifest["origin"]["sandbox"]["commands"]
            .as_array()
            .expect("the origin's commands")
            .iter()
            .any(|command| command == "cargo"),
        "the origin's sandbox travels: it is half of what the far gate is judging",
    );
    assert!(
        manifest.get("turns").is_none() && manifest.get("jobs").is_none(),
        "the envelope holds nothing folding the stream would answer: {manifest}",
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn an_imported_job_is_at_the_gate_and_says_who_approved_it_over_there() {
    let dir = scratch("import");
    let record = origin_recording(&dir, "Cargo.toml", &[]);
    let bundle = dir.join("bundle");
    let store = dir.join("dest.db");

    ok(&luu(&[
        "transfer",
        "--record",
        record.to_str().expect("a utf-8 path"),
        "--out",
        bundle.to_str().expect("a utf-8 path"),
    ]));
    let out = ok(&luu(&[
        "import",
        bundle.to_str().expect("a utf-8 path"),
        "--store",
        store.to_str().expect("a utf-8 path"),
    ]));
    assert!(out.contains("job 1 at the gate"), "{out}");

    let opened = SessionStore::open(&store).expect("the destination store");
    let view = opened
        .load("origin")
        .expect("loading")
        .expect("the imported session");
    let job = &view.jobs[0];
    assert_eq!(
        serde_json::to_value(job.state).unwrap(),
        "proposed",
        "approved over there, proposed over here",
    );
    assert!(
        job.approved_by.is_none() && job.approved_at_origin.is_some(),
        "the approval moved rather than vanished: {job:?}",
    );
    assert_eq!(
        view.imported.as_ref().map(|origin| origin.session.as_str()),
        Some("origin"),
        "a session that came from somewhere says where",
    );

    // The one invariant the store has, now that it holds both halves: what it
    // gives back is a fold of the stream it is keeping — the border included,
    // because the border is a line in that stream and not an edit on top of it.
    let stream = opened.stream("origin").expect("the stored stream");
    assert_eq!(
        serde_json::to_value(SessionView::from_record("origin", &stream)).unwrap(),
        serde_json::to_value(&view).unwrap(),
        "the stored fold is not a fold of the stored stream",
    );

    // And twice is refused: importing over a session would fork one under a
    // single name.
    let again = luu(&[
        "import",
        bundle.to_str().expect("a utf-8 path"),
        "--store",
        store.to_str().expect("a utf-8 path"),
    ]);
    assert!(!again.status.success());
    assert!(
        String::from_utf8_lossy(&again.stderr).contains("already has a session"),
        "{}",
        String::from_utf8_lossy(&again.stderr),
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_plan_this_host_does_not_grant_arrives_refused_rather_than_pending() {
    let dir = scratch("refused");
    // Approved on the origin against a wider sandbox — the flag is what a
    // person there would have typed — and imported here against `luu.toml`,
    // which grants the repository and nothing else.
    let record = origin_recording(&dir, "/etc/hostname", &["--allow-read", "/etc/hostname"]);
    let bundle = dir.join("bundle");
    let store = dir.join("dest.db");

    ok(&luu(&[
        "transfer",
        "--record",
        record.to_str().expect("a utf-8 path"),
        "--out",
        bundle.to_str().expect("a utf-8 path"),
    ]));
    let out = ok(&luu(&[
        "import",
        bundle.to_str().expect("a utf-8 path"),
        "--store",
        store.to_str().expect("a utf-8 path"),
    ]));
    assert!(out.contains("job 1 refused"), "{out}");
    assert!(
        out.contains("/etc/hostname"),
        "refused in the words the local gate uses: {out}",
    );

    let view = SessionStore::open(&store)
        .expect("the store")
        .load("origin")
        .expect("loading")
        .expect("the session");
    assert_eq!(
        serde_json::to_value(view.jobs[0].state).unwrap(),
        "rejected"
    );
    assert!(
        !view.jobs[0].unmet.is_empty(),
        "a refusal that does not say why is one nobody can answer",
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_bundle_from_a_host_speaking_something_else_is_refused_before_the_stream() {
    let dir = scratch("version");
    let record = origin_recording(&dir, "Cargo.toml", &[]);
    let bundle = dir.join("bundle");
    ok(&luu(&[
        "transfer",
        "--record",
        record.to_str().expect("a utf-8 path"),
        "--out",
        bundle.to_str().expect("a utf-8 path"),
    ]));

    let manifest_file = bundle.join("manifest.json");
    let mut manifest: Value =
        serde_json::from_str(&std::fs::read_to_string(&manifest_file).unwrap()).unwrap();
    manifest["protocol"] = Value::from(agent_core::protocol::VERSION + 1);
    std::fs::write(&manifest_file, manifest.to_string()).unwrap();
    // The stream is unreadable too, so a host that got as far as parsing it
    // would fail on the line rather than on the version — which is the whole
    // point of an envelope.
    std::fs::write(bundle.join("record.jsonl"), "not json\n").unwrap();

    let refused = luu(&[
        "import",
        bundle.to_str().expect("a utf-8 path"),
        "--store",
        dir.join("dest.db").to_str().expect("a utf-8 path"),
    ]);
    assert!(!refused.status.success());
    let said = String::from_utf8_lossy(&refused.stderr).to_string();
    assert!(
        said.contains("speaks protocol"),
        "refused for the version, out loud: {said}",
    );

    std::fs::remove_dir_all(&dir).ok();
}

#[test]
fn a_session_stored_before_the_stream_was_kept_is_refused_rather_than_rebuilt() {
    let dir = scratch("foldonly");
    let store = dir.join("fold-only.db");
    {
        // A row written the way every session before schema 2 was: the fold,
        // and no stream beside it.
        let opened = SessionStore::open(&store).expect("a store");
        let mut view = SessionView::new("older", "mock", "mock");
        view.started_at = 1;
        opened.save(&view).expect("saving the fold");
    }

    let refused = luu(&[
        "transfer",
        "older",
        "--store",
        store.to_str().expect("a utf-8 path"),
        "--out",
        dir.join("bundle").to_str().expect("a utf-8 path"),
    ]);
    assert!(!refused.status.success());
    let said = String::from_utf8_lossy(&refused.stderr).to_string();
    assert!(
        said.contains("A fold is not a stream"),
        "a fold is not shipped as if it were a stream: {said}",
    );

    std::fs::remove_dir_all(&dir).ok();
}

// ---------------------------------------------------------------------------
// The border, with a server on each side of it.
// ---------------------------------------------------------------------------

/// A server that keeps its session in `path`. The destination and the origin
/// are the same program pointed at different stores, which is what stage 1 is:
/// host to host, with no portal at all.
async fn server(replies: Vec<String>, path: &Path) -> String {
    let base = root();
    let agency = Agency {
        tools: Arc::new(Tools::standard()),
        sandbox: Arc::new(Sandbox::new(&SandboxPolicy::default(), &base).expect("the sandbox")),
        max_steps: 4,
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

async fn send(socket: &mut Socket, message: Value) {
    socket
        .send(WsMessage::Text(message.to_string().into()))
        .await
        .expect("sending");
}

async fn next_message(socket: &mut Socket) -> Value {
    loop {
        let message = tokio::time::timeout(PATIENCE, socket.next())
            .await
            .expect("a message before the patience ran out")
            .expect("the socket stayed open")
            .expect("a frame");
        if let WsMessage::Text(text) = message {
            return serde_json::from_str(&text).expect("json");
        }
    }
}

/// Reads until the named message, and returns everything it passed on the way —
/// which is how "and nothing else happened" is asserted.
async fn until(socket: &mut Socket, want: &str) -> (Value, Vec<Value>) {
    let mut seen = Vec::new();
    loop {
        let message = next_message(socket).await;
        if message["type"] == want {
            return (message, seen);
        }
        seen.push(message);
    }
}

async fn post(address: &str, path: &str) -> reqwest::Response {
    reqwest::Client::new()
        .post(format!("http://{address}{path}"))
        .send()
        .await
        .expect("the request")
}

#[tokio::test]
async fn a_job_that_crossed_a_border_runs_nothing_until_this_host_approves_it() {
    let dir = scratch("border");
    let origin_db = dir.join("origin.db");
    let destination_db = dir.join("destination.db");

    // The origin: a job proposed, approved, and left open with a turn inside it.
    let origin = server(vec![PLAN.into(), ANSWER.into()], &origin_db).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{origin}/ws"))
        .await
        .expect("the websocket handshake");
    let hello = next_message(&mut socket).await;
    let session = hello["session"]
        .as_str()
        .expect("the session id")
        .to_string();
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "what is in the manifest?"}),
    )
    .await;
    until(&mut socket, "job_proposed").await;
    send(
        &mut socket,
        serde_json::json!({"type": "approve_job", "job": 1}),
    )
    .await;
    until(&mut socket, "ended").await;
    drop(socket);

    // The move. A directory, which `scp` can carry and this test can just read.
    let sandbox = Sandbox::new(&SandboxPolicy::default(), &root()).expect("the sandbox");
    let bundle = dir.join("bundle");
    luu::transfer::write(
        &luu::transfer::Source::Store {
            path: origin_db.clone(),
            id: session.clone(),
        },
        &sandbox,
        &bundle,
    )
    .expect("writing the bundle");
    let read = luu::transfer::read(&bundle).expect("reading it back");
    let imported = luu::transfer::import(&read, &sandbox, &destination_db, Some("arrived"))
        .expect("importing");
    assert_eq!(imported.jobs.len(), 1, "one job crossed");

    // The destination: the same session, resumed, and the gate holding it.
    let destination = server(vec!["a second answer".into()], &destination_db).await;
    let resumed = post(&destination, "/api/sessions/arrived/resume").await;
    assert_eq!(resumed.status(), reqwest::StatusCode::OK);

    let (mut socket, _) = tokio_tungstenite::connect_async(format!("ws://{destination}/ws"))
        .await
        .expect("the websocket handshake");
    let _ = next_message(&mut socket).await;

    // Nothing runs behind the gate — including a turn inside a job that was
    // approved somewhere else.
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "and now change it"}),
    )
    .await;
    let (refused, _) = until(&mut socket, "refused").await;
    assert_eq!(refused["reason"], "pending");
    assert!(
        refused["detail"]
            .as_str()
            .expect("a detail")
            .contains("job 1"),
        "the refusal names the job waiting on a person: {refused}",
    );

    // Approving opens it and starts nothing: what opened it ran on the origin,
    // and this host was never handed a prompt to release.
    send(
        &mut socket,
        serde_json::json!({"type": "approve_job", "job": 1}),
    )
    .await;
    let (approved, _) = until(&mut socket, "job_approved").await;
    assert_eq!(approved["job"], 1);
    assert_eq!(
        approved["approved_by"]["by"], "operator",
        "approved here, by whoever is at this keyboard: {approved}",
    );

    // And the next prompt is a turn inside work this host has now approved.
    send(
        &mut socket,
        serde_json::json!({"type": "prompt", "text": "now say something else"}),
    )
    .await;
    let (started, skipped) = until(&mut socket, "turn_started").await;
    assert_eq!(
        started["job"], 1,
        "the turn belongs to the job that crossed: {started}",
    );
    assert!(
        skipped
            .iter()
            .all(|message| message["type"] != "job_proposed"),
        "an approved job is not proposed again: {skipped:?}",
    );
    assert_eq!(
        started["turn"], 3,
        "the history continued rather than restarted: {started}",
    );

    std::fs::remove_dir_all(&dir).ok();
}
