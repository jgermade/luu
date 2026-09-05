//! Which fragments this turn points at.
//!
//! The differentiating item, and the oldest unbuilt line in the design:
//! *inject only the fragments the current turn points at, instead of the full
//! history.* [`crate::fragment`] has been able to put a file into a turn since
//! the context manager existed, and until now the only thing that decided
//! *which* file was a person typing `--fragment`.
//!
//! **This is the graph asked the question it is good at.** The same reference
//! graph ranks the repository map and lost twice as an ordering
//! (`RECORD/2026-09-03.the-map-order-probe.completed.md`), for a reason that
//! reads as an argument *for* this module: ranking puts the files the tree is
//! built on first, and the file a question is about is usually a leaf. A
//! selector wants the leaf.
//!
//! And the personalization `ranking-the-map` rejected is what makes a selector
//! work. That rejection was about *where* a per-turn score may live — not
//! whether one is right — and it is obeyed here: the map keeps its uniform
//! teleport and stays a cached prefix, while this scores per turn and lands in
//! the `code` bucket, which is per-turn already and cached never.
//!
//! Pure where it can be. [`score`] takes names and outlines, not paths to read,
//! so an ordering can be argued about without a repository; [`select`] is the
//! half that touches the filesystem, and it touches it **through the sandbox**,
//! because a path `read_file` would refuse must not become readable by being
//! reached a different way.
//!
//! See `RECORD/2026-09-05.choosing-fragments.completed.md`.

use std::collections::{HashMap, HashSet};

use crate::context::{Counter, TokenCounter};
use crate::fragment::Spec;
use crate::rank::{self, FileTags};
use crate::repo_map::{Entry, Walked};
use crate::sandbox::Sandbox;

/// Words every question in a code corpus contains, which is to say words that
/// separate nothing.
///
/// Short and written down rather than grown: a stop list that creeps is a
/// selector nobody can reproduce, and the corpus is what would catch it being
/// wrong. Anything shorter than three characters is dropped anyway.
const STOP: &[&str] = &[
    "which", "file", "files", "the", "that", "this", "these", "those", "and", "are", "for", "from",
    "into", "with", "what", "where", "when", "how", "does", "did", "was", "were", "has", "have",
    "had", "its", "it's", "not", "but", "out", "over", "under", "than", "then", "there", "they",
    "them", "their", "all", "any", "each", "both", "one", "two", "own", "real", "actually", "also",
    "rather", "instead", "every", "some", "such", "same", "other", "another", "here", "about",
    "code", "line", "lines", "thing", "things",
];

/// How much each kind of evidence is worth.
///
/// Numbers someone chose, in a struct rather than as constants so that an
/// ablation is one line in a test rather than an edit: the corpus can be run
/// with a signal switched off, which is the only way to find out whether it was
/// carrying its weight. See the record's measured table.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Weights {
    /// A term **is** a name this file defines. `run_command` asked about, and a
    /// `fn run_command` here.
    pub name: f64,
    /// A term is one component of a name this file defines — `command` against
    /// `run_command`. Weaker on purpose: components are what every file shares.
    pub part: f64,
    /// A term is a component of the path. `sandbox` in a question is weak
    /// evidence for `sandbox/mod.rs`, and it is the kind of evidence a person
    /// actually uses.
    pub path: f64,
    /// A term appears in the file's own module doc — what the file says it is
    /// for, in this tree's most reliable sentence.
    pub doc: f64,
    /// One hop along the reference graph, from a file that scored to a file it
    /// references or is referenced by.
    ///
    /// **Zero by default, and that is the measurement rather than a hedge.**
    /// The hop was the mechanism this module was expected to be *about*, it is
    /// implemented, and on the 38-question corpus it makes the answer worse: at
    /// 0.35 it moves the right file out of first place 5 times out of 38 and out
    /// of the top three 7 times, while holding no more targets than without it.
    /// A neighbour is not a relevance signal — it is every file that calls
    /// anything the query touched, which on a tree this connected is most of the
    /// tree. That is the graph's third measured loss and the first one where it
    /// was asked the question it was supposed to be good at.
    ///
    /// It stays here, switchable, for the same reason `--map-rank` stayed: the
    /// order the selector did not take is worth being able to read beside the
    /// one it did.
    pub graph: f64,
}

impl Default for Weights {
    fn default() -> Self {
        Self {
            name: 3.0,
            part: 1.0,
            path: 0.75,
            doc: 1.5,
            graph: 0.0,
        }
    }
}

impl Weights {
    /// The same weights with the module doc switched off — tags, path and the
    /// graph alone, which is the mechanism `luu-design.md` names and the
    /// ablation that says how much of this works on a tree that does not write
    /// its `//!` blocks.
    pub fn without_docs(self) -> Self {
        Self { doc: 0.0, ..self }
    }

    /// The same weights with the graph hop switched **on**, at the weight it
    /// was measured at. Off by default; see [`Weights::graph`].
    pub fn with_graph(self) -> Self {
        Self {
            graph: 0.35,
            ..self
        }
    }
}

/// Longest a chosen fragment may run. A definition longer than this is quoted
/// down to its head: the point is to show the turn what it pointed at, not to
/// paste a module into the prompt.
const MAX_SPAN: usize = 60;

/// What a file with no matching definition contributes: its head, which in this
/// tree is the module doc — the file saying what it is for.
const HEAD_LINES: usize = 25;

/// One file, as the selector sees it. What [`crate::repo_map`]'s walk already
/// produces, which is the point: there is one parse of the tree, not two.
#[derive(Debug, Clone, Default)]
pub struct Candidate {
    pub path: String,
    pub entries: Vec<Entry>,
    pub tags: FileTags,
    /// The leading `//!` block, lowercased by [`score`] rather than here.
    pub doc: String,
    /// How many lines the file has, so the last definition has an end.
    pub lines: usize,
}

/// One file's score, and enough to say why it has it.
///
/// The "why" is not decoration. The case against embeddings was that a cosine
/// distance cannot say why a file was chosen; having built something that can,
/// it would be a poor trade not to ask it.
#[derive(Debug, Clone, PartialEq)]
pub struct Scored {
    /// Index into the slice handed to [`score`].
    pub file: usize,
    pub score: f64,
    /// Which entry matched best, if any did. `None` means the file scored on
    /// its path, its doc or the graph, and its head is what it contributes.
    pub entry: Option<usize>,
    pub why: Vec<String>,
}

/// One fragment the turn gets.
#[derive(Debug, Clone, PartialEq)]
pub struct Chosen {
    pub path: String,
    /// 1-based and inclusive, as an editor counts and as `--fragment` writes it.
    pub lines: (usize, usize),
    pub score: f64,
    pub why: Vec<String>,
    /// What this fragment costs, by the counter that measured everything else.
    pub tokens: u32,
}

impl Chosen {
    /// The same thing `--fragment path:start-end` parses to, so a selected
    /// fragment enters the turn by the door that already exists.
    pub fn spec(&self) -> Spec {
        Spec::parse(&format!("{}:{}-{}", self.path, self.lines.0, self.lines.1))
    }
}

/// What one turn's selection came to.
#[derive(Debug, Clone)]
pub struct Selection {
    pub chosen: Vec<Chosen>,
    /// Every file that scored above zero, best first, whether or not it fitted
    /// — the order the budget did not buy, which is what `--explain` prints.
    pub considered: Vec<(String, f64)>,
    pub budget: u32,
    pub tokens: u32,
    pub counted_by: Counter,
}

impl Selection {
    pub fn is_empty(&self) -> bool {
        self.chosen.is_empty()
    }

    pub fn specs(&self) -> Vec<Spec> {
        self.chosen.iter().map(Chosen::spec).collect()
    }

    /// What a person reads: the fragments, and under `--explain` the scores.
    pub fn render(&self, explain: bool) -> String {
        let mut text = format!(
            "# {} fragment(s), {} of {} tokens, counted by {}\n",
            self.chosen.len(),
            self.tokens,
            self.budget,
            match self.counted_by.is_approximate() {
                true => "an approximation, which is not a measurement",
                false => "the model's own tokenizer",
            },
        );
        for chosen in &self.chosen {
            text.push_str(&format!(
                "{}:{}-{}   {:.3}  {} tokens — {}\n",
                chosen.path,
                chosen.lines.0,
                chosen.lines.1,
                chosen.score,
                chosen.tokens,
                chosen.why.join("; "),
            ));
        }
        if explain {
            text.push_str("\n# considered, best first\n");
            for (path, score) in &self.considered {
                text.push_str(&format!("{score:>8.3}  {path}\n"));
            }
        }
        text
    }
}

/// The terms a query contributes, lowercased and split two ways.
///
/// `run_command` yields `run_command`, `run` and `command`, because a question
/// may name the whole thing or one half of it and both are the same ask.
pub fn terms(query: &str) -> Vec<String> {
    let mut found: Vec<String> = Vec::new();
    let mut push = |word: String| {
        if word.len() >= 3 && !STOP.contains(&word.as_str()) && !found.contains(&word) {
            found.push(word);
        }
    };
    for word in query.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        let word = word.to_lowercase();
        if word.is_empty() {
            continue;
        }
        for part in components(&word) {
            push(part);
        }
        push(word);
    }
    found
}

/// A name's parts: `run_command` and `RunCommand` both give `run`, `command`.
fn components(name: &str) -> Vec<String> {
    let mut parts = Vec::new();
    let mut current = String::new();
    for character in name.chars() {
        if character == '_' || character == '-' {
            if !current.is_empty() {
                parts.push(std::mem::take(&mut current));
            }
            continue;
        }
        if character.is_uppercase() && !current.is_empty() {
            parts.push(std::mem::take(&mut current));
        }
        current.push(character.to_ascii_lowercase());
    }
    if !current.is_empty() {
        parts.push(current);
    }
    parts.retain(|part| part.len() >= 3);
    parts
}

/// The words a path contributes, minus the furniture every path in a crate has.
fn path_terms(path: &str) -> Vec<String> {
    path.split(['/', '.', '\\'])
        .flat_map(components)
        .filter(|part| !matches!(part.as_str(), "crates" | "src" | "tests" | "mod" | "lib"))
        .collect()
}

/// Scores every candidate against the query. Pure: no filesystem, no clock.
///
/// Returns everything that scored above zero, best first, with ties broken on
/// the index so a caller that hands files over in path order gets the alphabet
/// as a tie-breaker rather than as an ordering.
pub fn score(query: &str, candidates: &[Candidate], weights: &Weights) -> Vec<Scored> {
    let terms = terms(query);
    if terms.is_empty() || candidates.is_empty() {
        return Vec::new();
    }

    let mut base = vec![0.0f64; candidates.len()];
    let mut why: Vec<Vec<String>> = vec![Vec::new(); candidates.len()];
    let mut best_entry: Vec<Option<usize>> = vec![None; candidates.len()];

    for (index, candidate) in candidates.iter().enumerate() {
        let mut score = 0.0;

        // What the file defines, which is the strongest thing tags can say.
        let mut matched_names: Vec<&str> = Vec::new();
        for name in &candidate.tags.defines {
            let lowered = name.to_lowercase();
            let parts = components(&lowered);
            if terms.contains(&lowered) {
                score += weights.name;
                matched_names.push(name.as_str());
            } else if terms.iter().any(|term| parts.contains(term)) {
                score += weights.part;
                matched_names.push(name.as_str());
            }
        }
        if !matched_names.is_empty() {
            matched_names.sort_unstable();
            matched_names.dedup();
            matched_names.truncate(4);
            why[index].push(format!("defines {}", matched_names.join(", ")));
        }

        // The path, which is weak and is how a person actually looks.
        let in_path = path_terms(&candidate.path);
        let hits: Vec<&String> = terms.iter().filter(|term| in_path.contains(term)).collect();
        if !hits.is_empty() {
            score += weights.path * hits.len() as f64;
            why[index].push(format!(
                "path says {}",
                hits.iter()
                    .map(|term| term.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        // What the file says it is for. This tree writes that down in a `//!`
        // block, and it is the most reliable sentence any file has about
        // itself — which is also why a corpus written from module docs cannot
        // be used to argue that *this* signal is the clever part.
        if weights.doc > 0.0 && !candidate.doc.is_empty() {
            let words = word_set(&candidate.doc);
            let hits: Vec<&String> = terms.iter().filter(|term| words.contains(*term)).collect();
            if !hits.is_empty() {
                // Normalised by how much of the query landed, not by how long
                // the doc is: a long doc that answers the question is not worth
                // less than a short one that does.
                score += weights.doc * (hits.len() as f64 / terms.len() as f64) * 4.0;
                why[index].push(format!(
                    "doc says {}",
                    hits.iter()
                        .map(|term| term.as_str())
                        .collect::<Vec<_>>()
                        .join(", ")
                ));
            }
        }

        // Which definition, given the file. The signature is one line and it
        // carries the argument and type names, which is usually enough.
        let mut best = (0.0f64, None);
        for (position, entry) in candidate.entries.iter().enumerate() {
            let words = word_set(&entry.text);
            let hit = terms.iter().filter(|term| words.contains(*term)).count();
            if hit > 0 {
                let weight = hit as f64;
                if weight > best.0 {
                    best = (weight, Some(position));
                }
            }
        }
        best_entry[index] = best.1;
        base[index] = score;
    }

    // One hop along the graph the map already builds. A file that references
    // something that scored, or is referenced by it, is in the neighbourhood a
    // person would call related — and the hop is what puts a caller's own file
    // beside the definition it calls.
    let mut total = base.clone();
    if weights.graph > 0.0 {
        let tags: Vec<FileTags> = candidates
            .iter()
            .map(|candidate| candidate.tags.clone())
            .collect();
        let edges = rank::edges(&tags);
        let mut neighbours: HashMap<usize, f64> = HashMap::new();
        for ((from, to), weight) in &edges {
            let capped = weight.min(1.0);
            if base[*from] > 0.0 {
                *neighbours.entry(*to).or_default() += base[*from] * capped;
            }
            if base[*to] > 0.0 {
                *neighbours.entry(*from).or_default() += base[*to] * capped;
            }
        }
        for (file, carried) in neighbours {
            if carried <= 0.0 {
                continue;
            }
            total[file] += weights.graph * carried;
            if base[file] <= 0.0 {
                why[file].push("one hop from a file that matched".to_string());
            }
        }
    }

    let mut scored: Vec<Scored> = total
        .into_iter()
        .enumerate()
        .filter(|(_, score)| *score > 0.0)
        .map(|(file, score)| Scored {
            file,
            score,
            entry: best_entry[file],
            why: std::mem::take(&mut why[file]),
        })
        .collect();
    scored.sort_by(|a, b| {
        b.score
            .partial_cmp(&a.score)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(a.file.cmp(&b.file))
    });
    scored
}

/// The words in a blob, plus each word's components, as a set.
fn word_set(text: &str) -> HashSet<String> {
    let mut set = HashSet::new();
    for word in text.split(|c: char| !(c.is_alphanumeric() || c == '_')) {
        let word = word.to_lowercase();
        if word.len() < 3 {
            continue;
        }
        for part in components(&word) {
            set.insert(part);
        }
        set.insert(word);
    }
    set
}

/// The lines one candidate contributes: the matched definition, or the file's
/// head when the match was about the file rather than about anything in it.
///
/// A definition ends where the next one at the same nesting or shallower
/// begins, which is what makes this a fragment and not a file. Long ones are
/// cut to [`MAX_SPAN`]: the point is to show the turn what it pointed at.
fn span(candidate: &Candidate, entry: Option<usize>) -> (usize, usize) {
    let Some(index) = entry else {
        return (1, HEAD_LINES.min(candidate.lines.max(1)));
    };
    let start = candidate.entries[index].line;
    let depth = candidate.entries[index].depth;
    let end = candidate.entries[index + 1..]
        .iter()
        .find(|next| next.depth <= depth)
        .map(|next| next.line.saturating_sub(1))
        .unwrap_or(candidate.lines)
        .max(start);
    (start, end.min(start + MAX_SPAN - 1).max(start))
}

/// Chooses this turn's fragments, reading through the sandbox.
///
/// The fill is **non-greedy** — a file too big for what is left is skipped and
/// the next one is tried — which is the rule `in-degree-and-fill` measured for
/// the map and the right one here for a stronger reason: a selector's whole job
/// is to spend a small budget on several places, and a greedy fill would let one
/// oversized definition end the selection.
pub fn select(
    walked: &[Walked],
    sandbox: &Sandbox,
    query: &str,
    budget: u32,
    counter: &dyn TokenCounter,
    weights: &Weights,
) -> Selection {
    let mut selection = Selection {
        chosen: Vec::new(),
        considered: Vec::new(),
        budget,
        tokens: 0,
        counted_by: counter.id(),
    };
    if budget == 0 {
        return selection;
    }

    let candidates: Vec<Candidate> = walked.iter().map(Walked::candidate).collect();
    let scored = score(query, &candidates, weights);
    selection.considered = scored
        .iter()
        .map(|one| (candidates[one.file].path.clone(), one.score))
        .collect();

    let mut spent = 0u32;
    for one in &scored {
        let candidate = &candidates[one.file];
        let lines = span(candidate, one.entry);
        let spec = Spec::parse(&format!("{}:{}-{}", candidate.path, lines.0, lines.1));
        // Through the sandbox, by the same loader `--fragment` uses. A denial
        // is a file that is not selected rather than an error: the turn asked
        // for none of this by name, so a refusal is not a failed request.
        let Ok(fragment) = crate::fragment::load(sandbox, &spec) else {
            continue;
        };
        let cost = counter.count(&fragment.text);
        if spent + cost > budget {
            continue;
        }
        spent += cost;
        selection.chosen.push(Chosen {
            path: candidate.path.clone(),
            lines,
            score: one.score,
            why: one.why.clone(),
            tokens: cost,
        });
    }
    selection.tokens = spent;
    selection
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rank::{RefKind, Reference};

    fn candidate(path: &str, defines: &[&str], references: &[&str], doc: &str) -> Candidate {
        Candidate {
            path: path.to_string(),
            entries: defines
                .iter()
                .enumerate()
                .map(|(index, name)| Entry {
                    line: 10 + index * 10,
                    depth: 0,
                    text: format!("pub fn {name}(sandbox: &Sandbox)"),
                })
                .collect(),
            tags: FileTags {
                defines: defines.iter().map(|name| (*name).to_string()).collect(),
                references: references
                    .iter()
                    .map(|name| Reference {
                        name: (*name).to_string(),
                        kind: RefKind::Call,
                    })
                    .collect(),
            },
            doc: doc.to_string(),
            lines: 200,
        }
    }

    #[test]
    fn a_query_splits_the_way_a_name_does() {
        let terms = terms("which file implements run_command, the one tool?");
        assert!(terms.contains(&"run_command".to_string()));
        assert!(terms.contains(&"run".to_string()));
        assert!(terms.contains(&"command".to_string()));
        assert!(
            !terms.contains(&"file".to_string()),
            "the stop list holds the words every question in a code corpus has",
        );
    }

    #[test]
    fn the_file_that_defines_the_name_wins_over_the_file_that_calls_it() {
        // The whole ordering in one case: a definition is stronger evidence
        // than a mention, and the caller still shows up because of the hop.
        let files = [
            candidate("tools/command.rs", &["run_command"], &[], ""),
            candidate("agent.rs", &[], &["run_command"], ""),
            candidate("store.rs", &["save"], &[], ""),
        ];
        let weights = Weights::default().with_graph();
        let scored = score("which file implements run_command?", &files, &weights);
        assert_eq!(scored[0].file, 0, "{scored:?}");
        assert!(
            scored.iter().any(|one| one.file == 1),
            "the caller is one hop away and belongs in the neighbourhood: {scored:?}",
        );
        assert!(
            !scored.iter().any(|one| one.file == 2),
            "a file with nothing to do with the query does not score: {scored:?}",
        );
        // And the reason the hop is off by default: without it the caller is
        // not in the answer at all, which on this corpus was the better answer.
        let without = score(
            "which file implements run_command?",
            &files,
            &Weights::default(),
        );
        assert_eq!(
            without.len(),
            1,
            "only the file that defines it: {without:?}"
        );
    }

    #[test]
    fn a_file_says_what_it_is_for_and_that_counts() {
        let files = [
            candidate(
                "sandbox/linux.rs",
                &["install"],
                &[],
                "Landlock and seccomp, applied between fork and exec.",
            ),
            candidate(
                "sandbox/fallback.rs",
                &["install"],
                &[],
                "Where there is no kernel to ask.",
            ),
        ];
        let scored = score(
            "which file applies Landlock and seccomp to a child process?",
            &files,
            &Weights::default(),
        );
        assert_eq!(scored[0].file, 0);
        assert!(
            scored[0].why.iter().any(|why| why.contains("doc says")),
            "{:?}",
            scored[0]
        );

        // And the ablation the record measures: with the doc switched off these
        // two are told apart by nothing but the path.
        let without = score(
            "which file applies Landlock and seccomp to a child process?",
            &files,
            &Weights::default().without_docs(),
        );
        assert!(
            without.is_empty() || without[0].score < scored[0].score,
            "the doc is doing the work here, and the ablation has to show it: {without:?}",
        );
    }

    #[test]
    fn a_definition_ends_where_the_next_one_starts() {
        let file = candidate("a.rs", &["first", "second"], &[], "");
        assert_eq!(
            span(&file, Some(0)),
            (10, 19),
            "up to the line before the next"
        );
        assert_eq!(
            span(&file, Some(1)),
            (20, 79),
            "the last one is capped, not the whole file"
        );
        assert_eq!(
            span(&file, None),
            (1, HEAD_LINES),
            "a file that matched on its path or its doc contributes its head",
        );
    }

    #[test]
    fn nothing_matches_nothing() {
        let files = [candidate("store.rs", &["save"], &[], "sessions on disk")];
        assert!(score("", &files, &Weights::default()).is_empty());
        assert!(
            score("which file?", &files, &Weights::default()).is_empty(),
            "stop words alone select nothing"
        );
    }
}
