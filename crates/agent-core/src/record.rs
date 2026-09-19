//! The record format: the same messages, on disk.
//!
//! JSON-lines. The first line is a [`RecordLine::Header`]; every line after it
//! is one protocol or trace message with the milliseconds since the header.
//!
//! The header is what makes two runs comparable — which is the only reason the
//! format exists. A file of bare frames replays, but cannot answer "which model
//! produced this", and comparing a run against one whose model you have to
//! remember is the failure mode the format was meant to remove.

use serde::{Deserialize, Serialize};

use crate::context::{Counter, Eviction, Prune, Repeat, Results};
use crate::protocol::ServerMessage;
use crate::trace::TraceMessage;

/// Bumped when an older reader could not make sense of a newer file.
///
/// 2: the budget's `limit` became nullable and gained a `counter`, and the
/// header says what the run was measured against. A format-1 reader would
/// choke on `"limit": null`.
///
/// 3: the task lifecycle is on the protocol, so a recording carries
/// `task_proposed`, `task_approved`, `task_closed` and `task_reopened` lines. A
/// format-2 reader would choke on the first of them — an unknown `type` in a
/// tagged enum is a parse error, not a line to skip. Turns also gained the task
/// they belong to, and that half is backwards compatible: absent means none.
///
/// 4: `refused` lines, for the same reason as 3 — a new variant of a tagged
/// enum. `task_approved` also gained the plan as approved, and that half is
/// backwards compatible: absent means the proposal is the best answer there is.
///
/// 5: `evicted` lines — what the window dropped and never took back. Same rule
/// as 3 and 4, a new variant of a tagged enum. A format-4 file does not say what
/// its session forgot, and nothing can work it out afterwards: the floor lived
/// in memory. See `RECORD/2026-08-31.eviction-tombstones.completed.md`.
///
/// 6: `job_proposed`, `job_approved`, `job_closed`, `job_reopened`, `job_rejected`
/// lines, and plans carrying model tasks checklists. See
/// `RECORD/2026-09-04.from-tasks-to-jobs.completed.md`.
///
/// 7: `refused` lines can carry `version` and `signature` refusals, and
/// `job_approved` carries who approved. The first half is the same rule as 3,
/// 4 and 5 — a new value of a tagged enum — and the second is additive: absent
/// means the operator, because that is what every approval before it was. See
/// `RECORD/2026-09-04.signed-approvals.completed.md`.
///
/// 8 was `imported` lines, and is not a format anything writes: a session
/// belongs to the host that made it, so no stream ever arrives from elsewhere.
/// Un-made with the protocol bump beside it, for the same reason. See
/// `RECORD/2026-09-04.sessions-stay-home.completed.md`.
///
/// 8, taken this time: `pruned` trace lines, written by a run under
/// `--prune-behind`. Same rule as 3, 4 and 5 — a new variant of a tagged enum,
/// and an older reader chokes on it rather than skipping it. The number is the
/// one the paragraph above un-made, which is free precisely because nothing
/// ever wrote it: a format number says what a file may contain, and no file
/// contains `imported`. The protocol is untouched at 5, because what a prune
/// changes is the prompt and not the conversation. See
/// `RECORD/2026-09-08.prune-behind.completed.md`.
///
/// 10: the header names the **posture** a session ran under — the policy file
/// that decides its sandbox and its seam together. `luu.container.toml` opens by
/// saying that probes run under it are not comparable with probes run without
/// it, which is the same sentence `context_limit`, `counter` and `eviction` are
/// in the header for; a posture a session chooses and the recording does not
/// name is a corpus nobody can read back. `None` in every stream written before
/// it. See `RECORD/2026-09-08.a-session-picks-its-executor.completed.md`.
///
/// 9: `job_closed` carries what the fold replaced — which turns stopped being
/// sent, and what they were worth in the prompt they are no longer in. Additive
/// to an existing message, which is the rule 7 was taken under, and absent means
/// *not recorded* rather than zero. The protocol is untouched at 5: a client
/// that ignores the field lacks a panel, it does not misread a conversation. See
/// `RECORD/2026-09-08.what-a-fold-writes-down.completed.md`.
///
/// 11: the header names the three **resend rules** a run rendered its window
/// under — `repeat`, `prune` and `results`. They are in the header for the
/// reason `context_limit`, `counter`, `eviction` and `posture` are, and the two
/// builders that set them say it themselves: *"a recording made under it is not
/// the same arm as one made without it"*. Until this, the only thing on the
/// wire was the `pruned` trace line, which fires when rule B prunes something —
/// so a `--prune-behind` run that pruned nothing was indistinguishable from a
/// run without the flag, and a run under `--repeat-once` said nothing at all.
/// `None` in every stream written before it, for the same reason `eviction` is
/// `None` in one written before the policy was a choice: every one of those ran
/// under the defaults, but the file does not say so, and inventing the field on
/// the reader's behalf would put a claim in a record the record never made.
/// See `RECORD/2026-09-18.the-window-rules-are-a-session-fact.completed.md`.
///
/// 12: a `diverged` **trace** line, naming a path the prompt sent under more
/// than one body. `code_context` is written once when a turn closes and never
/// refreshed while every turn re-reads its own spans, so a file edited between
/// two turns goes out as two blocks under one `// path` header — and until this
/// line, nothing anywhere said so. A trace and not protocol, on the same
/// argument the `pruned` line is a trace: every turn is still asked, answered
/// and in the transcript, and what changed is the prompt. A stream written
/// before this simply has no such line, which is not the same claim as a stream
/// that has none because nothing diverged — the format number is what tells
/// those two apart. See `RECORD/2026-09-19.one-path-two-bodies.WIP.md`.
///
/// 13: `grounded` lines, and the **protocol** moves with them to 6 — the first
/// time since format 3 that a bump is a protocol bump rather than a trace one.
/// A turn says what it was asked with, by reference: the specs, never the
/// bytes, because bytes in a store come back without being re-read and a
/// session resumed under a narrower posture would get back a file it is no
/// longer allowed to open. Same rule as 3, 4 and 5 — a new variant of a tagged
/// enum, and a format-12 reader chokes on it rather than skipping it. A stream
/// written before this says no turn was grounded, which is indistinguishable
/// from a session that grounded nothing; the format number is what tells the
/// two apart, and it is why a resume of an older stream still comes back
/// without code and is right to. See
/// `RECORD/2026-09-19.fragments-by-reference.completed.md`.
pub const FORMAT: u32 = 13;

/// The posture a session ran under, as a recording names it.
///
/// A name and not only a name: a name is a label, and the file behind it can be
/// edited tomorrow — `counter` is in the header as an id rather than as "the
/// counter I thought was good", for the same reason. Three facts are enough to
/// tell two recordings apart without copying a policy file into every stream.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Posture {
    /// `None` when the run took the server's own policy file rather than a
    /// posture somebody named in `config.toml`.
    ///
    /// Always written, `null` included: *no name* is a fact about the run, and
    /// a reader that has to tell an absent key from an absent posture is one
    /// that will get it wrong. A missing key read as `null` is also how the
    /// first test of this passed while the check that reads it by name did not.
    #[serde(default)]
    pub name: Option<String>,
    /// Where tool calls ran: `host`, `direct`, or a container runtime.
    pub runtime: String,
    /// What held a child process, as the policy asked for it.
    pub enforcement: String,
    /// Whether the session could reach the network at all. The per-job grant
    /// narrows inside this and never widens it.
    pub network: bool,
}

impl Posture {
    /// Whether two runs were allowed to do the same things.
    ///
    /// The three facts, and **never the name** — for the reason the name is
    /// `Option` in the first place: a name is a label and the file behind it
    /// can be edited tomorrow, so two sessions that agree on it can disagree
    /// about everything that matters, and two that disagree on it can be the
    /// same place reached by two spellings.
    ///
    /// This is what a resume compares before it hands a person a gate. See
    /// `RECORD/2026-09-17.what-an-approval-was-granted-under.completed.md`.
    pub fn same_place(&self, other: &Self) -> bool {
        self.runtime == other.runtime
            && self.enforcement == other.enforcement
            && self.network == other.network
    }

    /// The three facts, for a refusal that has to name both sides.
    ///
    /// The name is left out: the caller has it and knows whose it is, and a
    /// message that put both names in would be inviting the comparison this
    /// type refuses to make.
    pub fn describe(&self) -> String {
        let network = match self.network {
            true => "network",
            false => "no network",
        };
        format!("{}, {}, {network}", self.runtime, self.enforcement)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "channel", rename_all = "snake_case")]
pub enum RecordLine {
    Header {
        format: u32,
        protocol: u32,
        backend: String,
        model: String,
        /// The window this run was measured against, `None` when unknown.
        /// Part of the header because two runs with different windows are not
        /// comparable, and by the time you are comparing them, nobody
        /// remembers which was which.
        #[serde(default)]
        context_limit: Option<u32>,
        /// Which counter produced this run's budgets. `None` in a format-1
        /// file, where the numbers came from the backend instead.
        #[serde(default)]
        counter: Option<Counter>,
        /// How the history gave way under pressure. Beside `context_limit` and
        /// `counter` for the same reason those are here: two runs under
        /// different policies are not comparable, and by the time anyone is
        /// comparing them nobody remembers which was which.
        ///
        /// `None` in a file recorded before the policy was a choice. Every one
        /// of those ran per-turn, but the file does not say so, and inventing
        /// the field on the reader's behalf would put a claim in a record that
        /// the record never made.
        #[serde(default)]
        eviction: Option<Eviction>,
        /// What this session was allowed to do: the policy file that decides
        /// its sandbox and its seam, by name and by the three facts about it a
        /// reader compares two runs on. Here for the reason the three fields
        /// above are — `luu.container.toml` says in its own first paragraph that
        /// runs under it are not comparable with runs without it — and `None` in
        /// a stream written before format 10, or by a run that named no posture.
        #[serde(default, skip_serializing_if = "Option::is_none")]
        posture: Option<Posture>,
        /// What a span already in the window cost the turn that selected it
        /// again — rule A. Beside `eviction` and with the same `None`
        /// semantics: a stream written before format 11 does not say.
        #[serde(default)]
        repeat: Option<Repeat>,
        /// Whether an old turn gave up its spans once the window stopped
        /// fitting — rule B. The `pruned` trace line says when the rule *fired*;
        /// this says the run was under it, which is the difference between a
        /// run that pruned nothing and a run that could not.
        #[serde(default)]
        prune: Option<Prune>,
        /// Whether an old turn's tool output went with its spans — rule C.
        /// Inert without `prune`, and recorded anyway: a reader comparing two
        /// runs needs to know which arm it is holding, not which arm had an
        /// effect.
        #[serde(default)]
        results: Option<Results>,
        /// Unix milliseconds. Every later line is relative to this.
        started_at: u64,
    },
    Protocol {
        at_ms: u64,
        message: ServerMessage,
    },
    Trace {
        at_ms: u64,
        message: TraceMessage,
    },
}

/// What a session's `diverged` lines add up to.
///
/// [`TraceMessage::Diverged`] is emitted per render and says nothing about the
/// session it belongs to: a reader with a recording in front of them can see
/// that turn 6's prompt contradicted itself and cannot see whether that
/// happened once or in half the turns. **The second number is the one item 19's
/// fix waits on** — newest body wins trades measured prefix reuse for
/// truthfulness, and the trade turns on how often a window carries a path whose
/// bytes moved, which nothing counted until this.
///
/// It lives here, beside the format, rather than in the probe that first wanted
/// it: a tally computed by a test is a tally only that test can quote, and the
/// next thing to want this number is a run against a model rather than against
/// the mock.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct Divergence {
    /// Renders that carried at least one contradicted path.
    ///
    /// Not the same as `lines`: one render can contradict itself about several
    /// paths at once, and a *rate* is per render, because the render is what
    /// was sent to a model.
    pub renders: usize,
    /// Renders where at least one contradiction included the asking turn —
    /// the history disagreeing with what is on disk right now, which is the
    /// case [`TraceMessage::Diverged::asking`] exists to name.
    pub asking: usize,
    /// One entry per distinct path, in the order the session first reported it,
    /// carrying the most bodies ever sent under it in a single render.
    ///
    /// A `Vec` of pairs and not a map, for [`crate::context::Selection`]'s own
    /// reason one along: a session reports a handful of paths, the order is
    /// part of the answer, and a map would spend an allocation to save
    /// comparisons nobody can measure.
    pub paths: Vec<(String, usize)>,
    /// Every `diverged` line in the file, which is what a reader grepping for
    /// them would count and is the number the other three are not.
    pub lines: usize,
}

/// Folds a recording's `diverged` lines into one [`Divergence`].
///
/// Over [`RecordLine`]s rather than over a path, so that a caller holding a
/// stream it recorded itself does not have to write it out to read it back —
/// which is what the probe does, and is the difference between a test that
/// proves the corpus and a test that proves the file system.
pub fn divergence(lines: &[RecordLine]) -> Divergence {
    let mut found = Divergence::default();
    let mut current: Option<crate::protocol::TurnId> = None;
    let mut already_asking = false;

    for line in lines {
        let RecordLine::Trace {
            message:
                TraceMessage::Diverged {
                    turn,
                    path,
                    bodies,
                    asking,
                    ..
                },
            ..
        } = line
        else {
            continue;
        };
        found.lines += 1;
        // A render is one turn's worth of lines, and they are written together
        // by the caller that emitted them — so the turn changing is the render
        // changing, and a turn cannot be rendered twice in one stream.
        if current != Some(*turn) {
            current = Some(*turn);
            already_asking = false;
            found.renders += 1;
        }
        // Counted once per render however many of its paths are asking, for
        // the reason `renders` is: the question is how many prompts went out
        // contradicting the disk, not how many spans did.
        if *asking && !already_asking {
            already_asking = true;
            found.asking += 1;
        }
        match found.paths.iter_mut().find(|(seen, _)| seen == path) {
            Some((_, most)) => *most = (*most).max(*bodies),
            None => found.paths.push((path.clone(), *bodies)),
        }
    }

    found
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{self, ServerMessage};

    #[test]
    fn a_header_and_a_message_are_one_line_each() {
        let header = RecordLine::Header {
            format: FORMAT,
            protocol: protocol::VERSION,
            backend: "mock".into(),
            model: "mock".into(),
            context_limit: Some(8192),
            counter: Some(Counter::Approximate),
            eviction: Some(Eviction::Turn),
            posture: None,
            repeat: Some(Repeat::Once),
            prune: Some(Prune::Behind),
            results: Some(Results::Cited),
            started_at: 1_700_000_000_000,
        };
        let token = RecordLine::Protocol {
            at_ms: 12,
            message: ServerMessage::Token {
                turn: 1,
                text: "hola".into(),
            },
        };

        let lines = format!(
            "{}\n{}",
            serde_json::to_string(&header).unwrap(),
            serde_json::to_string(&token).unwrap()
        );
        assert_eq!(lines.lines().count(), 2, "one line per record");

        let parsed: Vec<RecordLine> = lines
            .lines()
            .map(|l| serde_json::from_str(l).unwrap())
            .collect();
        assert!(matches!(
            parsed[0],
            RecordLine::Header { format: FORMAT, .. }
        ));
        assert!(matches!(parsed[1], RecordLine::Protocol { at_ms: 12, .. }));
    }

    /// The comparison a resume makes before it hands a person a gate.
    #[test]
    fn two_postures_are_the_same_place_by_their_facts_and_never_by_their_name() {
        let contained = Posture {
            name: Some("container".into()),
            runtime: "docker (luu-worker:dev)".into(),
            enforcement: "kernel".into(),
            network: false,
        };

        assert!(
            contained.same_place(&Posture {
                name: Some("sandboxed".into()),
                ..contained.clone()
            }),
            "a name is a label and the file behind it can be edited tomorrow",
        );
        assert!(
            contained.same_place(&Posture {
                name: None,
                ..contained.clone()
            }),
            "a session that named no posture can still be in the same place as one that did",
        );

        // The three that are not a label. Enforcement is the one worth spelling
        // out: `kernel` on the host and `kernel` inside a container are the same
        // four bytes and two very different places for a command to run.
        assert!(!contained.same_place(&Posture {
            runtime: "host".into(),
            ..contained.clone()
        }));
        assert!(!contained.same_place(&Posture {
            enforcement: "best-effort".into(),
            ..contained.clone()
        }));
        assert!(!contained.same_place(&Posture {
            network: true,
            ..contained.clone()
        }));
    }

    /// Each of the three rules, both ways, through JSON and back.
    ///
    /// One test per value rather than one header carrying all six, because what
    /// this is pinning is that a reader can tell the arms apart — and a
    /// round-trip of one arm proves nothing about that on its own.
    #[test]
    fn a_header_says_which_arm_of_each_rule_a_run_was() {
        let header = |repeat, prune, results| RecordLine::Header {
            format: FORMAT,
            protocol: protocol::VERSION,
            backend: "mock".into(),
            model: "mock".into(),
            context_limit: Some(8192),
            counter: Some(Counter::Approximate),
            eviction: Some(Eviction::Turn),
            posture: None,
            repeat: Some(repeat),
            prune: Some(prune),
            results: Some(results),
            started_at: 1_700_000_000_000,
        };
        let arms =
            |line: &RecordLine| match serde_json::from_str(&serde_json::to_string(line).unwrap())
                .unwrap()
            {
                RecordLine::Header {
                    repeat,
                    prune,
                    results,
                    ..
                } => (repeat, prune, results),
                other => panic!("{other:?}"),
            };

        let off = header(Repeat::Always, Prune::Never, Results::Kept);
        let on = header(Repeat::Once, Prune::Behind, Results::Cited);

        assert_eq!(
            arms(&off),
            (
                Some(Repeat::Always),
                Some(Prune::Never),
                Some(Results::Kept)
            ),
        );
        assert_eq!(
            arms(&on),
            (
                Some(Repeat::Once),
                Some(Prune::Behind),
                Some(Results::Cited)
            ),
        );
        assert_ne!(
            arms(&off),
            arms(&on),
            "two arms that read back the same are a header that cannot tell two runs apart, \
             which is the only reason this format exists",
        );
    }

    /// The `None` semantics `eviction` already has, on the three fields beside
    /// it.
    ///
    /// A stream written before format 11 ran under the defaults — `Always`,
    /// `Never`, `Kept` — and does not say so. Reading the default back would put
    /// a claim in a record the record never made, so the absent key stays absent
    /// all the way to the reader.
    #[test]
    fn a_format_10_header_does_not_say_which_rules_its_run_was_under() {
        let before = serde_json::json!({
            "channel": "header",
            "format": 10,
            "protocol": protocol::VERSION,
            "backend": "mock",
            "model": "mock",
            "context_limit": 8192,
            "counter": {"kind": "approximate"},
            "eviction": {"policy": "turn"},
            "started_at": 1_700_000_000_000_u64,
        });

        match serde_json::from_value(before).unwrap() {
            RecordLine::Header {
                repeat,
                prune,
                results,
                ..
            } => assert_eq!(
                (repeat, prune, results),
                (None, None, None),
                "a file that does not say must not be read as one that does",
            ),
            other => panic!("{other:?}"),
        }
    }

    /// One line per path and one render per turn: the four numbers a reader
    /// of `Divergence` has to be able to tell apart.
    ///
    /// Two paths in one render must not count as two renders, and two
    /// `asking` paths in one render must not count as two prompts that
    /// contradicted the disk — the rate is per prompt, because the prompt is
    /// what went to a model.
    #[test]
    fn a_render_that_contradicts_itself_twice_is_still_one_render() {
        let lines = vec![
            diverged(6, "src/greeting.rs:1-11", 2, true),
            diverged(6, "src/tally.rs:1-6", 2, true),
            diverged(12, "src/greeting.rs:1-11", 3, true),
            diverged(13, "src/greeting.rs:1-11", 3, false),
        ];

        let found = divergence(&lines);
        assert_eq!(found.lines, 4);
        assert_eq!(found.renders, 3, "three turns, four lines");
        assert_eq!(found.asking, 2, "turn 13 attached nothing");
        assert_eq!(
            found.paths,
            vec![
                ("src/greeting.rs:1-11".to_string(), 3),
                ("src/tally.rs:1-6".to_string(), 2),
            ],
            "first seen first, each carrying the most bodies it ever reached",
        );
    }

    /// A session that never edits a file it has quoted reports nothing, and
    /// the tally of nothing is zero rather than absent. The control in
    /// `scripts/tasks/edit-reread.txt` exists to produce exactly this.
    #[test]
    fn a_session_with_nothing_to_report_tallies_to_zero() {
        let quiet = vec![RecordLine::Protocol {
            at_ms: 1,
            message: ServerMessage::Token {
                turn: 1,
                text: "hola".into(),
            },
        }];
        assert_eq!(divergence(&quiet), Divergence::default());
    }

    fn diverged(turn: u64, path: &str, bodies: usize, asking: bool) -> RecordLine {
        RecordLine::Trace {
            at_ms: turn,
            message: TraceMessage::Diverged {
                turn,
                path: path.into(),
                turns: Vec::new(),
                asking,
                bodies,
            },
        }
    }
}
