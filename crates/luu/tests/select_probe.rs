//! The selection probe: coverage, on the corpus that was built to compare
//! orders, with no model on the machine.
//!
//! Every probe in this repository so far has waited on a person and a model at
//! the same time, because scoring meant reading 38 answers. **Coverage does
//! not.** *Is the file a question is about inside the block a run holds* is a
//! structural question, and now that
//! [`map-order-probe.key`](../../../scripts/tasks/map-order-probe.key) is on
//! disk it is a number this test computes on every commit.
//!
//! What it cannot answer is whether a model *uses* what it was handed. That is
//! precision, it needs a box from `ROADMAP/2026-09-05/machines.md`, and it is
//! named as missing rather than skipped. See
//! `RECORD/2026-09-05.choosing-fragments.completed.md`.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

use agent_core::context::ApproximateCounter;
use agent_core::repo_map::{Order, RepoMap, walk_sources};
use agent_core::sandbox::{Access, PathRule, Sandbox, SandboxPolicy};
use agent_core::select::{self, Weights};

/// The budget both instruments are read at, and it is not a new choice: it is
/// the one `the-map-order-probe` ran at, chosen there by how many files each
/// map order fits and nothing else.
const BUDGET: u32 = 4096;

/// And a tighter one, because 4096 is where a selector can afford to be vague.
/// At 1024 it holds a handful of fragments, which is the operating point a
/// small window actually has — and the budget the map's own probes were argued
/// at first.
const TIGHT: u32 = 1024;

fn root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../..")
        .canonicalize()
        .expect("the repository root")
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

fn sandbox(root: &Path) -> Sandbox {
    let policy = SandboxPolicy {
        paths: vec![PathRule::new(root, Access::Read)],
        ..SandboxPolicy::default()
    };
    Sandbox::new(&policy, root).expect("the repository is readable")
}

/// How many of the 38 targets a map order holds. Query-independent, so it is
/// one number per order rather than one per question.
fn map_coverage(sandbox: &Sandbox, targets: &[String], order: Order, budget: u32) -> usize {
    let map = RepoMap::build(sandbox, budget, &ApproximateCounter, order);
    let held: HashSet<&str> = map.files.iter().map(|file| file.path.as_str()).collect();
    targets
        .iter()
        .filter(|target| held.contains(target.as_str()))
        .count()
}

#[test]
fn the_selector_beats_the_order_it_has_to_beat() {
    let root = root();
    let sandbox = sandbox(&root);

    let questions = lines(&root.join("scripts/tasks/map-order-probe.txt"));
    let targets = lines(&root.join("scripts/tasks/map-order-probe.key"));
    assert_eq!(
        questions.len(),
        targets.len(),
        "the key is one path per question, in the same order",
    );
    assert_eq!(
        questions.len(),
        38,
        "the corpus is not extended mid-experiment"
    );
    for target in &targets {
        assert!(
            root.join(target).exists(),
            "{target} is in the key and not in the tree: the key follows the tree",
        );
    }

    // The map orders, which are the baselines. Path order is the default and
    // the one the selector has to beat.
    println!(
        "\nof {} questions, how many targets the run holds",
        questions.len()
    );
    println!("                        1024   4096");
    for (name, order) in [
        ("map, path order  ", Order::Path),
        ("map, ranked      ", Order::Ranked),
        ("map, in-degree   ", Order::InDegree),
    ] {
        println!(
            "  {name}    {:>4}   {:>4}",
            map_coverage(&sandbox, &targets, order, TIGHT),
            map_coverage(&sandbox, &targets, order, BUDGET),
        );
    }
    let path_order = map_coverage(&sandbox, &targets, Order::Path, BUDGET);
    let path_order_tight = map_coverage(&sandbox, &targets, Order::Path, TIGHT);

    // One walk, shared: the selector and the map read the same parse of the
    // same tree, which is what makes this one flag apart rather than two runs.
    let walked = walk_sources(&sandbox);

    /// What one configuration came to. `first` is the number that survives a
    /// budget: whether the target is the **highest-scoring** file, which no
    /// amount of budget can flatter.
    struct Run {
        held: usize,
        first: usize,
        top3: usize,
        tokens: f64,
        fragments: f64,
    }

    let run = |weights: Weights, budget: u32| -> Run {
        let mut result = Run {
            held: 0,
            first: 0,
            top3: 0,
            tokens: 0.0,
            fragments: 0.0,
        };
        for (question, target) in questions.iter().zip(&targets) {
            let selection = select::select(
                &walked,
                &sandbox,
                question,
                budget,
                &ApproximateCounter,
                &weights,
            );
            if selection.chosen.iter().any(|chosen| chosen.path == *target) {
                result.held += 1;
            }
            // Ranked over everything that scored, not over what fitted: the
            // order is the selector's answer and the budget is only how much
            // of it was affordable.
            let placed = selection
                .considered
                .iter()
                .position(|(path, _)| path == target);
            if placed == Some(0) {
                result.first += 1;
            }
            if placed.is_some_and(|at| at < 3) {
                result.top3 += 1;
            }
            result.tokens += selection.tokens as f64;
            result.fragments += selection.chosen.len() as f64;
        }
        let questions = questions.len() as f64;
        result.tokens /= questions;
        result.fragments /= questions;
        result
    };

    let say = |name: &str, run: &Run, budget: u32| {
        println!(
            "  {name:<20} held {:>2}   1st {:>2}   top-3 {:>2}   {:.0} of {budget} tokens, {:.1} fragments",
            run.held, run.first, run.top3, run.tokens, run.fragments,
        );
    };

    println!("\nthe selector, at {BUDGET}");
    let full = run(Weights::default(), BUDGET);
    say("all signals", &full, BUDGET);
    say(
        "no module docs",
        &run(Weights::default().without_docs(), BUDGET),
        BUDGET,
    );
    say(
        "with the graph hop",
        &run(Weights::default().with_graph(), BUDGET),
        BUDGET,
    );

    println!("\nthe selector, at {TIGHT} — where a small window actually is");
    let tight = run(Weights::default(), TIGHT);
    say("all signals", &tight, TIGHT);
    say(
        "no module docs",
        &run(Weights::default().without_docs(), TIGHT),
        TIGHT,
    );
    say(
        "with the graph hop",
        &run(Weights::default().with_graph(), TIGHT),
        TIGHT,
    );

    assert!(
        full.held > path_order,
        "the bar is the baseline, not zero: the selector held {} of {} \
         where path order holds {path_order}. A selector that does not beat \
         the alphabet is a rejected record, not a shipped flag.",
        full.held,
        questions.len(),
    );
    assert!(
        tight.held > path_order_tight,
        "and at the tight budget too, where the map holds {path_order_tight}: {} held",
        tight.held,
    );
    assert!(
        full.tokens <= BUDGET as f64,
        "a selection cannot spend more than its budget",
    );
}

#[test]
fn a_selected_fragment_is_a_fragment_and_not_a_file() {
    // The property that separates this from a second map: what goes into the
    // turn is the definition the question points at, not everything around it.
    let root = root();
    let sandbox = sandbox(&root);
    let walked = walk_sources(&sandbox);
    let selection = select::select(
        &walked,
        &sandbox,
        "which file implements run_command, the tool whose verdict is about the kernel?",
        BUDGET,
        &ApproximateCounter,
        &Weights::default(),
    );

    let chosen = selection
        .chosen
        .iter()
        .find(|chosen| chosen.path.ends_with("tools/command.rs"))
        .unwrap_or_else(|| panic!("run_command's own file was not chosen: {selection:?}"));
    let (start, end) = chosen.lines;
    assert!(start >= 1 && end >= start);
    assert!(
        end - start < 200,
        "a fragment is a span, not a module: {start}-{end}",
    );
    assert!(
        !chosen.why.is_empty(),
        "the graph was chosen over embeddings because it can say why",
    );
}
