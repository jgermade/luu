//! The tool-call probe, scored against calls that really went through the loop.
//!
//! The probe's question needs a model: *what does a 7B do about the fenced
//! format*. The probe's **scorer** does not, and this is the difference — a
//! recording is made here in which each of the four shapes was emitted on
//! purpose, by a mock told to emit it, and the counts are asserted against what
//! the fixture says it wrote.
//!
//! What this proves is that the instrument counts what it saw. What a model
//! does is unbought, and `ROADMAP/2026-09-09` item 4 stays open until a box
//! answers it. See `RECORD/2026-09-13.a-probe-for-tool-calls.completed.md`.

use std::path::{Path, PathBuf};
use std::process::Command;

use agent_core::tools::{CallShape, Tools};
use luu::export::read_record;
use luu::probe;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the workspace root")
}

fn lines(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .map(str::to_string)
        .collect()
}

/// The ten prompts of the corpus, answered by the five shapes of the fixture,
/// cycling — which is what `--mock-script` buys and what nothing on a machine
/// without a model could produce before it existed.
fn scored() -> &'static probe::Score {
    static SCORE: std::sync::OnceLock<probe::Score> = std::sync::OnceLock::new();
    SCORE.get_or_init(run)
}

/// One run of the real binary, shared by every test in the file: they run on
/// their own threads and would otherwise record over each other's file.
fn run() -> probe::Score {
    let dir = std::env::temp_dir().join(format!("luu-tool-call-probe-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("a scratch directory");
    let path = dir.join("probe.jsonl");
    let status = Command::new(env!("CARGO_BIN_EXE_luu"))
        .current_dir(root())
        .args([
            "chat",
            "--script",
            "scripts/tasks/tool-call-probe.txt",
            "--mock-script",
            "scripts/mock/tool-call-shapes.txt",
            "--mock-delay-ms",
            "0",
            "--record",
        ])
        .arg(&path)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .expect("running luu");
    assert!(status.success(), "the probe run exited with {status}");

    let record = read_record(&path).expect("reading the recording back");
    let key = lines(&root().join("scripts/tasks/tool-call-probe.key"));
    let tools = Tools::standard();
    let names: Vec<&str> = tools.names().collect();
    probe::score(&record, &names, &key)
}

#[test]
fn the_four_shapes_are_counted_as_the_fixture_wrote_them() {
    let score = scored();
    assert_eq!(score.rows.len(), 10, "ten prompts, ten rows");
    assert_eq!(score.count(CallShape::Parsed), 2);
    assert_eq!(score.count(CallShape::Continued), 2);
    assert_eq!(score.drifted(), 4);
    assert_eq!(score.count(CallShape::NoCall), 2);
}

/// The sub-count that says what `bare_object` is buying: half the drift here is
/// a call the parse recovered, and it is still drift.
#[test]
fn a_recovered_drift_is_counted_as_drift_and_as_recovered() {
    let score = scored();
    assert_eq!(score.recovered(), 2);
    assert_eq!(score.count(CallShape::Drifted { recovered: false }), 2);
}

/// The format holding and the choice being right are two results. The fixture
/// calls the key's tool on both `parsed` turns and deliberately the wrong one
/// on both `continued` turns, so a probe that added them would report 4 here
/// and be wrong twice.
#[test]
fn the_key_separates_a_call_that_held_from_a_call_that_was_right() {
    let score = scored();
    assert_eq!(score.hits(), 2);
    assert_eq!(score.wrong_tool(), 2);
}

/// The phase, which is the instrument's one fragile part: five shapes over ten
/// prompts is two clean cycles, and a drifted cycle would show up here as a
/// turn scored against the wrong prompt.
#[test]
fn the_cycle_stays_in_phase_over_two_passes() {
    let score = scored();
    let shapes: Vec<CallShape> = score.rows.iter().map(|row| row.shape).collect();
    assert_eq!(
        &shapes[..5],
        &shapes[5..],
        "the second pass repeats the first"
    );
}
