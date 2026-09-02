//! The one assertion the store has to keep passing.
//!
//! ```text
//! fold(record) -> SessionView  ==  save(store, it) ; load(store, id) -> SessionView
//! ```
//!
//! The store is a **cache of a fold, not a second log** — that is the whole of
//! `loude-design.md` §Persistence, and this is what stops it becoming one. A
//! store that can drift is a store that will, and without this the drift is a
//! support question years later instead of a red test now.
//!
//! The recordings are **produced by running the binary**, the same way
//! `scripts/make-fixtures.sh` produces the ones the static deploy replays —
//! not hand-written `RecordLine` vectors. A fixture written by hand can only
//! contain the variants whoever wrote it remembered; one produced by a run
//! contains what the code actually emits, including whatever lands next
//! without anyone remembering to update this file.
//!
//! Argued in `RECORD/2026-09-02.sessions-in-sqlite.completed.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use agent_core::api::SessionView;
use luu::export::read_record;
use luu::store::SessionStore;

/// The repository root, which is where the scripts and `luu.toml` are relative
/// to. Cargo runs an integration test from its package directory.
fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root")
}

/// A directory per recording, not per process: the tests in this file run on
/// their own threads and record the same names, and a shared path makes one
/// test read the half-written file of another.
fn scratch() -> PathBuf {
    static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let n = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let dir = std::env::temp_dir().join(format!("luu-store-parity-{}-{n}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    dir
}

/// One recorded run of the real binary. `--mock-delay-ms 0` throughout: the
/// fixtures the deploy replays are paced so a human can watch them, and this
/// wants the same *messages* without the twenty-seven seconds of watching.
fn record(name: &str, args: &[&str]) -> Vec<agent_core::record::RecordLine> {
    let path = scratch().join(format!("{name}.jsonl"));
    let status = Command::new(env!("CARGO_BIN_EXE_luu"))
        .current_dir(root())
        .arg("chat")
        .args(args)
        .args(["--mock-delay-ms", "0"])
        .arg("--record")
        .arg(&path)
        .stdout(std::process::Stdio::null())
        .status()
        .expect("running luu");
    assert!(
        status.success() || name == "backend-failure",
        "{name} exited with {status}",
    );
    read_record(&path).expect("reading the recording back")
}

/// Recordings covering every kind of line the format has: turns, the task
/// lifecycle with a close, eviction, tool calls with both verdicts, a fused
/// fragment, and a turn that failed.
fn recordings() -> Vec<(String, Vec<agent_core::record::RecordLine>)> {
    let policy = root().join("luu.toml");
    let policy = policy.to_str().expect("a utf-8 path");
    let tasks = root().join("scripts/tasks/one-task.txt");
    let tasks = tasks.to_str().expect("a utf-8 path");

    vec![
        (
            "completed-turn".into(),
            record("completed-turn", &["hola", "--context-limit", "8192"]),
        ),
        (
            // A window small enough that the history has to give way, so the
            // recording carries `evicted` lines — what a session forgot, which
            // nothing can work out afterwards.
            "eviction".into(),
            record(
                "eviction",
                &[
                    "--script",
                    root()
                        .join("scripts/tasks/steady-state.txt")
                        .to_str()
                        .expect("a utf-8 path"),
                    "--context-limit",
                    "1024",
                    "--reserve",
                    "64",
                ],
            ),
        ),
        (
            // The one shape where the history is rewritten rather than dropped:
            // a task proposed, approved, run and closed to a summary.
            "one-task".into(),
            record(
                "one-task",
                &[
                    "--script",
                    tasks,
                    "--context-limit",
                    "8192",
                    "--sandbox",
                    policy,
                ],
            ),
        ),
        (
            // The tool loop with the sandbox answering both ways, so the fold
            // carries a `ToolCallView` with each verdict.
            "tool-calls".into(),
            record(
                "tool-calls",
                &[
                    "What is in AGENTS.md, and what is in /etc/hostname?",
                    "--context-limit",
                    "8192",
                    "--sandbox",
                    policy,
                    "--mock-reply",
                    "Let me read it.\n```tool\n{\"name\":\"read_file\",\
                     \"arguments\":{\"path\":\"AGENTS.md\",\"max_lines\":3}}\n```",
                    "--mock-reply",
                    "Now the other.\n```tool\n{\"name\":\"read_file\",\
                     \"arguments\":{\"path\":\"/etc/hostname\"}}\n```",
                    "--mock-reply",
                    "The second path is outside the sandbox.",
                ],
            ),
        ),
        (
            // A real file fused into a turn: the `code` bucket, which the
            // fusion rule keeps apart from the prompt.
            "grounded-turn".into(),
            record(
                "grounded-turn",
                &[
                    "which two commitments does this file open with?",
                    "--fragment",
                    "crates/agent-core/src/context.rs:1-15",
                    "--context-limit",
                    "8192",
                    "--sandbox",
                    policy,
                ],
            ),
        ),
        (
            // The failure path, without depending on a backend being there.
            "backend-failure".into(),
            record(
                "backend-failure",
                &[
                    "anything",
                    "--backend",
                    "ollama",
                    "--ollama-url",
                    "http://127.0.0.1:1",
                ],
            ),
        ),
    ]
}

#[test]
fn what_the_store_gives_back_is_what_folding_the_record_produces() {
    let store = SessionStore::in_memory().expect("a store");
    let recordings = recordings();
    assert!(
        recordings.iter().all(|(_, lines)| lines.len() > 1),
        "a recording with only a header proves nothing",
    );

    for (id, lines) in &recordings {
        let folded = SessionView::from_record(id.clone(), lines);
        store.save(&folded).expect("saving the fold");
        let loaded = store
            .load(id)
            .expect("loading it back")
            .unwrap_or_else(|| panic!("{id} was saved and is not there"));

        assert_eq!(
            serde_json::to_value(&loaded).expect("the stored fold"),
            serde_json::to_value(&folded).expect("the folded record"),
            "the store disagrees with the record for {id}",
        );
    }

    // And the listing is the same set, derived from the same folds rather than
    // accumulated beside them.
    let listed = store.list().expect("listing");
    assert_eq!(listed.len(), recordings.len());
    for (id, lines) in &recordings {
        let summary = SessionView::from_record(id.clone(), lines).summary();
        let row = listed
            .iter()
            .find(|row| &row.id == id)
            .unwrap_or_else(|| panic!("{id} is not in the listing"));
        assert_eq!(
            serde_json::to_value(row).expect("the listed row"),
            serde_json::to_value(&summary).expect("the fold's own summary"),
        );
    }
}

/// The half of the rule the round trip alone does not cover: what comes back
/// has to survive being read by the *fold's* own reader, not just by
/// `serde_json`. A stored view that only round-trips through this test's
/// comparison could still be missing a field every client reads.
#[test]
fn a_stored_fold_still_answers_the_read_side_questions() {
    let store = SessionStore::in_memory().expect("a store");
    let (id, lines) = recordings()
        .into_iter()
        .find(|(id, _)| id == "one-task")
        .expect("the task recording");
    let folded = SessionView::from_record(id.clone(), &lines);
    store.save(&folded).expect("saving");

    let loaded = store.load(&id).expect("loading").expect("the session");
    assert!(!loaded.turns.is_empty(), "a session with no turns");
    assert!(
        !loaded.tasks.is_empty(),
        "the task lifecycle is in the fold"
    );
    let closed = loaded
        .tasks
        .iter()
        .find(|task| task.summary.is_some())
        .expect("a closed task");
    assert!(
        !closed.summary.as_deref().unwrap_or_default().is_empty(),
        "the summary is what its turns are sent as; an empty one is a fold that lost the work",
    );
    assert_eq!(
        loaded.turn(1).map(|turn| turn.turn),
        Some(1),
        "the view's own accessors work on what came out of the store",
    );
}
