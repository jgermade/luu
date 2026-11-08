//! The mixed-output corpus's harness: proved without a model.
//!
//! [`scripts/tasks/mixed-session.txt`](../../../scripts/tasks/mixed-session.txt)
//! is roadmap row 38: eleven turns over a small Python project, where each
//! answer lives in a different kind of tool output — a README, a failing test
//! run, a module and its test, a grep through a log, the script that wrote the
//! log, the whole log, a table, a count, an edit and the run after it — and one
//! question, turn 10, whose only answer is a command's output from eight turns
//! back.
//!
//! This runs the **real binary** in the **real sandbox** with the mock playing
//! the most frugal agent the corpus allows: the fewest tool calls that answer
//! each turn, scripted. What it proves is about the corpus, not about a model:
//!
//! - **The overflow is the corpus's.** That session already evicts at 8192 and
//!   fits at 98304, so an 8K arm that evicts on a model is not evicting because
//!   the model wandered. `long-session.txt` could not say that: a frugal agent
//!   reads one range of `context.rs` and fits.
//! - **Turn 10 separates the four arms row 38(b) is for**, by what its prompt
//!   carries: plain 8192 has evicted turn 2, 98304 keeps it, `cited_reads`
//!   prunes the reads around it and keeps its command output, and `cited` cites
//!   that output too. That is the case
//!   `RECORD/2026-09-22.what-rule-c-is-worth-with-a-model.completed.md` could
//!   not build: every command in that run was an empty grep.
//! - **The key asks for nothing the corpus does not show.** Every literal in
//!   `mixed-session.key` is in some tool's output in the 98304 run.
//!
//! See `RECORD/2026-11-08.a-corpus-that-overflows-with-mixed-output.completed.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use agent_core::api::{Counts, SessionView};
use luu::export::read_record;
use serde_json::Value;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root")
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).expect("a scratch directory");
    for entry in std::fs::read_dir(from).expect("the template").flatten() {
        let target = to.join(entry.file_name());
        if entry.file_type().expect("a file type").is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), &target).expect("copying the template");
        }
    }
}

/// A copy of `scripts/tasks/mixed-session/`, because turn 9 edits it and
/// Python writes `__pycache__` beside whatever it runs.
fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("luu-mixed-session-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    copy_tree(&root().join("scripts/tasks/mixed-session"), &dir);
    dir
}

/// The frugal agent: one reply per **model call**, a call or an answer.
///
/// Its answers are short on purpose and never repeat what turn 10 asks for —
/// the failing test's class and the line its assertion failed on — so turn
/// 10's prompt carries that only where turn 2's command output survived.
fn replies() -> Vec<String> {
    let call = |name: &str, arguments: Value| {
        format!(
            "```tool\n{}\n```",
            serde_json::json!({"name": name, "arguments": arguments})
        )
    };
    let read = |path: &str| call("read_file", serde_json::json!({"path": path}));
    let run = |argv: &[&str]| call("run_command", serde_json::json!({"argv": argv}));
    let tests = run(&["python3", "-m", "unittest", "discover", "-s", "tests"]);
    vec![
        read("README.md"), // 1
        "Invoices; the tests run with unittest.".into(),
        tests.clone(), // 2
        "The discount test fails.".into(),
        read("tests/test_invoice.py"), // 3
        read("orderbook/invoice.py"),
        "invoice_total discounts twice.".into(),
        run(&[
            "grep",
            "-n",
            "-A",
            "14",
            "cannot invoice",
            "logs/nightly.log",
        ]), // 4
        "One order, an unknown region.".into(),
        read("orderbook/nightly.py"), // 5
        "Invoice numbers are gapless.".into(),
        // 6: every tool's output is cut at 8 KiB, and the log is 15 — so the
        // whole log is two ranged reads, and the end is in the second.
        call(
            "read_file",
            serde_json::json!({"path": "logs/nightly.log", "max_lines": 80}),
        ),
        call(
            "read_file",
            serde_json::json!({"path": "logs/nightly.log", "start_line": 81, "max_lines": 80}),
        ),
        "Most went to one region.".into(),
        run(&["grep", "A-4471", "data/orders.csv"]), // 7
        read("orderbook/rates.py"),
        "That region has no rate.".into(),
        run(&["grep", "-c", ",IS,", "data/orders.csv"]), // 8
        "A few.".into(),
        call(
            // 9
            "edit_file",
            serde_json::json!({
                "path": "orderbook/invoice.py",
                "old_string": "net = subtotal(order) * discount_factor(order)",
                "new_string": "net = subtotal(order)",
            }),
        ),
        tests,
        "They pass.".into(),
        "I no longer have that run.".into(), // 10
        "A discount twice, fixed; a region with no rate, not.".into(), // 11
    ]
}

struct Run {
    counts: Counts,
    lines: Vec<Value>,
    dir: PathBuf,
}

impl Run {
    /// The text of the first prompt `turn` sent.
    fn prompt(&self, turn: u64) -> &str {
        self.lines
            .iter()
            .filter_map(|line| line.get("message"))
            .find(|m| m["type"] == "prompt" && m["turn"] == turn)
            .and_then(|m| m["text"].as_str())
            .unwrap_or_else(|| panic!("no prompt for turn {turn}"))
    }

    /// Every tool's output, in order.
    fn outputs(&self) -> Vec<&str> {
        self.lines
            .iter()
            .filter_map(|line| line.get("message"))
            .filter(|m| m["type"] == "tool_result")
            .filter_map(|m| m["output"].as_str())
            .collect()
    }
}

/// One run of the corpus. `results` is `[resend] results`, which no flag
/// reaches, so it goes in the scratch state directory's `config.toml`.
fn run(name: &str, limit: &str, results: Option<&str>, extra: &[&str]) -> Run {
    let dir = scratch(name);
    let home = dir.join(".luu-home");
    std::fs::create_dir_all(&home).expect("a state directory");
    if let Some(results) = results {
        std::fs::write(
            home.join("config.toml"),
            format!("[resend]\nresults = \"{results}\"\n"),
        )
        .expect("config.toml");
    }
    let record = dir.join("run.jsonl");
    let corpus = root().join("scripts/tasks/mixed-session.txt");

    let mut command = Command::new(env!("CARGO_BIN_EXE_luu"));
    command
        .current_dir(&dir)
        .env("LUU_HOME", &home)
        .arg("chat")
        .args(["--script", corpus.to_str().expect("a utf-8 path")])
        .args(["--allow-write", "."])
        .args(["--allow-command", "python3", "--allow-command", "grep"])
        .args(["--context-limit", limit])
        .args(["--mock-delay-ms", "0"])
        .args(extra)
        .arg("--record")
        .arg(&record);
    for reply in replies() {
        command.args(["--mock-reply", &reply]);
    }
    let status = command
        .stdout(std::process::Stdio::null())
        .status()
        .expect("running luu");
    assert!(status.success(), "{name} exited with {status}");

    let text = std::fs::read_to_string(&record).expect("the recording");
    let lines = text
        .lines()
        .filter(|line| !line.trim().is_empty())
        .map(|line| serde_json::from_str(line).expect("a json line"))
        .collect();
    let parsed = read_record(&record).expect("reading the recording back");
    Run {
        counts: SessionView::from_record(name, &parsed).counts(),
        lines,
        dir,
    }
}

/// The calls ran, and did what the key says they did. Without this, an arm
/// whose `python3` was refused would read as a window that lost the run.
fn assert_the_session_happened(run: &Run) {
    assert_eq!(run.counts.turns, 11, "eleven turns");
    assert_eq!(
        run.counts.calls.denied, 0,
        "nothing refused: {:?}",
        run.counts.calls
    );
    let outputs = run.outputs();
    assert!(
        outputs
            .iter()
            .any(|o| o.contains("AssertionError: 4901 != 5445")),
        "turn 2's run failed the way the key says",
    );
    assert!(
        outputs
            .iter()
            .any(|o| o.contains("Ran 19 tests") && o.contains("OK")),
        "turn 9's run passed",
    );
    let fixed = std::fs::read_to_string(run.dir.join("orderbook/invoice.py")).expect("the edit");
    assert!(
        fixed.contains("    net = subtotal(order)\n"),
        "turn 9's edit landed"
    );
}

/// Turn 2's command output, as turn 10's prompt carries it or does not.
fn holds_turn_2s_traceback(run: &Run) -> bool {
    let prompt = run.prompt(10);
    let class = prompt.contains("test_invoice.Totals");
    let line = prompt.contains("line 66");
    assert_eq!(
        class, line,
        "both come from the one traceback, or neither does"
    );
    class
}

#[test]
fn the_frugal_session_overflows_8k_and_fits_96k() {
    let small = run("8k", "8192", None, &[]);
    assert_the_session_happened(&small);
    assert!(
        small.counts.evicted.renders > 0,
        "the fewest calls that answer the corpus overflow 8192: {:?}",
        small.counts.evicted,
    );

    let large = run("96k", "98304", None, &[]);
    assert_the_session_happened(&large);
    assert_eq!(large.counts.evicted.renders, 0, "and fit in 98304");
    assert_eq!(large.counts.pruned.renders, 0);
}

/// Row 38(b), on the mock: which arms still carry turn 2's command output when
/// turn 10 asks for something only it said.
#[test]
fn turn_10_holds_the_command_output_only_where_commands_are_kept() {
    let plain = run("plain", "8192", None, &[]);
    let large = run("large", "98304", None, &[]);
    let reads = run(
        "cited-reads",
        "8192",
        Some("cited_reads"),
        &["--prune-behind"],
    );
    let all = run("cited", "8192", Some("cited"), &["--prune-behind"]);
    for arm in [&plain, &large, &reads, &all] {
        assert_the_session_happened(arm);
    }

    assert!(
        !holds_turn_2s_traceback(&plain),
        "plain 8192 evicted turn 2"
    );
    assert!(
        holds_turn_2s_traceback(&large),
        "98304 never had to let it go"
    );
    assert!(
        holds_turn_2s_traceback(&reads),
        "cited_reads prunes the reads around it and keeps a command's output",
    );
    assert!(
        !holds_turn_2s_traceback(&all),
        "cited cites the command's output too",
    );

    // The two pruning arms reach turn 10 without evicting it: what they lost
    // is the bytes, not the exchange.
    assert!(reads.counts.pruned.renders > 0, "{:?}", reads.counts.pruned);
    assert!(all.counts.pruned.renders > 0, "{:?}", all.counts.pruned);
    assert!(
        all.prompt(10).contains("[run_command] output"),
        "and the citation is the line that stands where the output was",
    );
}

/// Every backticked literal in the key is in some tool's output, and the key
/// has one line per turn.
#[test]
fn every_literal_in_the_key_is_in_what_the_tools_returned() {
    let key =
        std::fs::read_to_string(root().join("scripts/tasks/mixed-session.key")).expect("the key");
    let lines: Vec<&str> = key
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    assert_eq!(lines.len(), 11, "one line per turn");

    let large = run("key", "98304", None, &[]);
    assert_the_session_happened(&large);
    let outputs = large.outputs();
    let mut literals = 0;
    for line in &lines {
        let facts = line.split(" <- ").next().expect("facts");
        for literal in facts.split('`').skip(1).step_by(2) {
            assert!(
                outputs.iter().any(|output| output.contains(literal)),
                "`{literal}` is in the key and in no tool's output",
            );
            literals += 1;
        }
    }
    assert!(literals >= 15, "the key's literals were found: {literals}");
}
