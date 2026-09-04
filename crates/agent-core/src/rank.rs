//! What the repository is built on top of, as an order over its files.
//!
//! The map has to leave most of the repository out — this tree's whole outline
//! is 77% of an 8K window — so *which* files it keeps is the map's real
//! decision, and until now that decision was the alphabet's. This module is the
//! answer: nodes are files, an edge is "this file references a name that file
//! defines", and PageRank over that graph says what the rest of the tree stands
//! on.
//!
//! **The teleport is uniform, and that is the design.** Aider seeds PageRank
//! with a personalization vector pointed at the files in the conversation, which
//! is what made "ranking personalizes" true when the map's record left the
//! question open. Take the seed away and the score depends on nothing but the
//! tree — so the map still changes only when the repository does, and stays a
//! prefix block rather than becoming a fragment with a bad name.
//!
//! Pure, and deliberately: it takes names, not paths to read, so the ordering
//! can be tested without a filesystem and argued about without a repository.
//!
//! See `RECORD/2026-09-02.ranking-the-map.completed.md`.

use std::collections::HashMap;

/// Damping, as PageRank has always spelled it: the share of a file's score that
/// flows along its references rather than teleporting.
const DAMPING: f64 = 0.85;

/// Enough for the score to stop moving on a tree of any size this walks, and a
/// hard stop so a pathological graph cannot spin.
const MAX_ITERATIONS: usize = 64;

/// Below this, another iteration cannot change an order.
const EPSILON: f64 = 1e-9;

/// A name defined in more than this many files is furniture — `new`, `render`,
/// `count` — and an edge drawn through it is not evidence of anything.
const OVERUSED: usize = 5;

/// How much of an edge survives being drawn through such a name.
const OVERUSED_WEIGHT: f64 = 0.1;

/// Below this many characters a name is weak evidence, because this tree is not
/// the only thing that owns names that short.
///
/// Aider's number, and this repository is the reason it is kept rather than the
/// authority for it: `push`, `new`, `count` and `render` are what the first run
/// of this ranking put at the top, every one of them short, and every one of
/// them a name the standard library owns too. See the appended half of
/// `RECORD/2026-09-02.ranking-the-map.completed.md`.
const SHORT_NAME: usize = 8;

/// How much of an edge survives being drawn through a short name.
const SHORT_NAME_WEIGHT: f64 = 0.25;

/// What the grammar saw, which is how much the edge is worth believing.
///
/// The distinction is not decoration: `x.render()` and `render()` are the same
/// capture in `tags.scm` and *not* the same evidence, because a method is
/// resolved by a type `tree-sitter` cannot see while a free call is resolved by
/// what the file has in scope.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum RefKind {
    /// `fold_history()` — resolved by what the file imported.
    Call,
    /// `x.fold_history()` — resolved by a type nobody here can see.
    Method,
    /// `impl Sandbox` / `impl Backend for …`, and the most reliable of the
    /// three: a type name is distinctive and is rarely the standard library's.
    Implementation,
}

impl RefKind {
    /// What an edge of this kind is worth before the name is looked at.
    fn weight(self) -> f64 {
        match self {
            Self::Call | Self::Implementation => 1.0,
            // Measured rather than chosen: at 1.0 the head of the ranking was
            // whichever file defined a free function called `push`, because
            // every `Vec::push` in the tree pointed at it.
            Self::Method => 0.25,
        }
    }
}

/// One name a file uses, and how much that use is worth believing.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reference {
    pub name: String,
    pub kind: RefKind,
}

/// One file's tags, which is all the ranking knows about it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FileTags {
    /// Names this file defines.
    pub defines: Vec<String>,
    /// Names this file references, repeats included: the count is the weight.
    pub references: Vec<Reference>,
}

/// One file's place in the order, and enough to say why it is there.
#[derive(Debug, Clone, PartialEq)]
pub struct Ranked {
    /// Index into the slice handed to [`rank`], so the caller keeps its own
    /// identifiers rather than being handed strings back.
    pub file: usize,
    pub score: f64,
    /// The files that reference this one, heaviest first — the "why", which a
    /// cosine distance could not have produced and which is most of the reason
    /// the graph was chosen over embeddings.
    pub referrers: Vec<(usize, f64)>,
}

/// Ranks files by what depends on them, most depended-on first.
///
/// Ties break on the index, so a caller that hands files over in path order
/// gets the alphabet as the *tie-breaker* it should always have been rather
/// than the ranking it was.
fn build_edges(files: &[FileTags]) -> HashMap<(usize, usize), f64> {
    // Where each name is defined. A name nobody defines is not an edge to
    // anywhere: `println!` and every trait method the standard library owns
    // land here, and dropping them is what keeps the graph about this tree.
    let mut definers: HashMap<&str, Vec<usize>> = HashMap::new();
    for (file, tags) in files.iter().enumerate() {
        for name in &tags.defines {
            let entry = definers.entry(name.as_str()).or_default();
            if !entry.contains(&file) {
                entry.push(file);
            }
        }
    }

    // Edges, accumulated by (from, to) so several references to one file are
    // one edge with a larger weight.
    let mut edges: HashMap<(usize, usize), f64> = HashMap::new();
    for (from, tags) in files.iter().enumerate() {
        // Counted per name *and* per kind: two ways of naming the same thing
        // are two pieces of evidence of different strength, and merging them
        // would lose the weaker one's weakness.
        let mut counts: HashMap<(&str, RefKind), u32> = HashMap::new();
        for reference in &tags.references {
            *counts
                .entry((reference.name.as_str(), reference.kind))
                .or_default() += 1;
        }
        for ((name, kind), times) in counts {
            let Some(targets) = definers.get(name) else {
                continue;
            };
            // A file calling its own helpers is not depended on by anybody, and
            // without this the longest file wins for being long.
            let targets: Vec<usize> = targets.iter().copied().filter(|to| *to != from).collect();
            if targets.is_empty() {
                continue;
            }
            let overused = definers[name].len() > OVERUSED;
            let short = name.chars().count() < SHORT_NAME;
            // A short name reached through a method call is not evidence at
            // all. `x.push(..)` and `x.is_empty()` are std's, and the only
            // thing separating them from a function of that name in this tree
            // is a type `tree-sitter` cannot see. Weighting them down is not
            // enough: they appear in *every* file, and breadth is what
            // PageRank rewards.
            if short && kind == RefKind::Method {
                continue;
            }
            let weight = f64::from(times).sqrt()
                * kind.weight()
                * if overused { OVERUSED_WEIGHT } else { 1.0 }
                * if short { SHORT_NAME_WEIGHT } else { 1.0 }
                / targets.len() as f64;
            for to in targets {
                *edges.entry((from, to)).or_default() += weight;
            }
        }
    }
    edges
}

/// Ranks files by what depends on them, most depended-on first (PageRank).
///
/// Ties break on the index, so a caller that hands files over in path order
/// gets the alphabet as the *tie-breaker* it should always have been rather
/// than the ranking it was.
pub fn rank(files: &[FileTags]) -> Vec<Ranked> {
    let count = files.len();
    if count == 0 {
        return Vec::new();
    }

    let edges = build_edges(files);

    let mut outgoing = vec![0.0f64; count];
    for ((from, _), weight) in &edges {
        outgoing[*from] += weight;
    }

    let uniform = 1.0 / count as f64;
    let mut score = vec![uniform; count];
    for _ in 0..MAX_ITERATIONS {
        let mut next = vec![0.0f64; count];
        // A file that references nothing has nowhere to send its share. Spread
        // it over everyone rather than letting it evaporate, which is what
        // keeps the scores summing to one and the comparison between two trees
        // meaningful.
        let dangling: f64 = (0..count)
            .filter(|file| outgoing[*file] == 0.0)
            .map(|file| score[file])
            .sum();
        let base = (1.0 - DAMPING) * uniform + DAMPING * dangling * uniform;
        for value in next.iter_mut() {
            *value = base;
        }
        for ((from, to), weight) in &edges {
            next[*to] += DAMPING * score[*from] * weight / outgoing[*from];
        }
        let moved: f64 = (0..count)
            .map(|file| (next[file] - score[file]).abs())
            .sum();
        score = next;
        if moved < EPSILON {
            break;
        }
    }

    let mut referrers: Vec<Vec<(usize, f64)>> = vec![Vec::new(); count];
    for ((from, to), weight) in &edges {
        referrers[*to].push((*from, *weight));
    }
    for list in referrers.iter_mut() {
        // Heaviest first, and the index breaks the tie so the explanation is as
        // reproducible as the order it explains.
        list.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    }

    let mut ranked: Vec<Ranked> = (0..count)
        .map(|file| Ranked {
            file,
            score: score[file],
            referrers: std::mem::take(&mut referrers[file]),
        })
        .collect();
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.file.cmp(&b.file)));
    ranked
}

/// Ranks files by direct weighted in-degree: total inbound reference weight multiplied
/// by the square root of distinct referring files (breadth).
pub fn rank_in_degree(files: &[FileTags]) -> Vec<Ranked> {
    let count = files.len();
    if count == 0 {
        return Vec::new();
    }

    let edges = build_edges(files);

    let mut referrers: Vec<Vec<(usize, f64)>> = vec![Vec::new(); count];
    for ((from, to), weight) in &edges {
        referrers[*to].push((*from, *weight));
    }
    for list in referrers.iter_mut() {
        list.sort_by(|a, b| b.1.total_cmp(&a.1).then(a.0.cmp(&b.0)));
    }

    let mut ranked: Vec<Ranked> = (0..count)
        .map(|file| {
            let total_inbound: f64 = referrers[file].iter().map(|(_, w)| *w).sum();
            let distinct_referrers = referrers[file].len() as f64;
            let score = total_inbound * (1.0 + distinct_referrers).sqrt();
            Ranked {
                file,
                score,
                referrers: std::mem::take(&mut referrers[file]),
            }
        })
        .collect();
    ranked.sort_by(|a, b| b.score.total_cmp(&a.score).then(a.file.cmp(&b.file)));
    ranked
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tags(defines: &[&str], references: &[&str]) -> FileTags {
        of_kind(defines, references, RefKind::Call)
    }

    fn of_kind(defines: &[&str], references: &[&str], kind: RefKind) -> FileTags {
        FileTags {
            defines: defines.iter().map(|name| name.to_string()).collect(),
            references: references
                .iter()
                .map(|name| Reference {
                    name: name.to_string(),
                    kind,
                })
                .collect(),
        }
    }

    #[test]
    fn the_file_everything_calls_into_comes_first() {
        // 0 is the leaf everybody uses; 1 and 2 are callers that nobody calls.
        let files = vec![
            tags(&["fold"], &[]),
            tags(&["serve"], &["fold"]),
            tags(&["chat"], &["fold"]),
        ];
        let ranked = rank(&files);
        assert_eq!(ranked[0].file, 0, "{ranked:?}");
        assert!(ranked[0].score > ranked[1].score);
    }

    #[test]
    fn an_entry_point_ranks_last_and_that_is_the_rankings_own_limit() {
        // Nothing calls into `main`. A graph ranked by what depends on what
        // cannot say that a person asks about it constantly — recorded as the
        // prediction this ranking is expected to fail, not as a bug.
        let files = vec![tags(&["helper"], &[]), tags(&["main"], &["helper"])];
        let ranked = rank(&files);
        assert_eq!(ranked.last().expect("a last").file, 1, "{ranked:?}");
    }

    #[test]
    fn a_file_calling_its_own_helpers_gains_nothing_from_it() {
        let alone = rank(&[tags(&["a", "b"], &[]), tags(&["z"], &[])]);
        let itself = rank(&[tags(&["a", "b"], &["a", "a", "b"]), tags(&["z"], &[])]);
        assert_eq!(
            alone[0].score, itself[0].score,
            "a self-edge would make the longest file the most important one",
        );
    }

    #[test]
    fn a_name_defined_everywhere_carries_almost_no_edge() {
        // `new` in seven files against one name defined once. The rare name has
        // to win, or the graph is a map of the standard vocabulary.
        let mut files = vec![tags(&["fold"], &[]), tags(&["caller"], &["fold", "new"])];
        for _ in 0..7 {
            files.push(tags(&["new"], &[]));
        }
        let ranked = rank(&files);
        assert_eq!(ranked[0].file, 0, "{ranked:?}");
    }

    #[test]
    fn a_reference_nobody_defines_is_not_an_edge_to_anywhere() {
        // `println` is not in this tree, so it must not tilt the order in it.
        let quiet = rank(&[tags(&["a"], &[]), tags(&["b"], &[])]);
        let noisy = rank(&[tags(&["a"], &["println"]), tags(&["b"], &["println"])]);
        assert_eq!(quiet[0].score, noisy[0].score);
    }

    #[test]
    fn ties_break_on_the_caller_s_own_order_so_two_runs_agree() {
        // Nothing references anything: every score is identical, and the order
        // is the one it was handed. The alphabet as a tie-breaker, which is
        // what it should always have been.
        let files = vec![tags(&["a"], &[]), tags(&["b"], &[]), tags(&["c"], &[])];
        let indices: Vec<usize> = rank(&files).iter().map(|entry| entry.file).collect();
        assert_eq!(indices, vec![0, 1, 2]);
    }

    #[test]
    fn a_ranked_file_can_say_who_referenced_it() {
        // The case against embeddings was that a cosine distance cannot answer
        // this. It would be a poor trade to build the graph and not ask it.
        let files = vec![tags(&["fold"], &[]), tags(&["serve"], &["fold", "fold"])];
        let ranked = rank(&files);
        let fold = ranked.iter().find(|entry| entry.file == 0).expect("fold");
        assert_eq!(fold.referrers.len(), 1);
        assert_eq!(fold.referrers[0].0, 1);
        assert!(fold.referrers[0].1 > 0.0);
    }

    #[test]
    fn no_files_is_an_empty_order_rather_than_a_division_by_zero() {
        assert!(rank(&[]).is_empty());
    }

    #[test]
    fn the_scores_are_a_distribution_so_two_trees_can_be_compared() {
        let files = vec![
            tags(&["fold"], &["serve"]),
            tags(&["serve"], &["fold"]),
            tags(&["orphan"], &[]),
        ];
        let total: f64 = rank(&files).iter().map(|entry| entry.score).sum();
        assert!((total - 1.0).abs() < 1e-6, "{total}");
    }

    #[test]
    fn in_degree_ranks_shared_dependencies_first() {
        let files = vec![
            tags(&["fold"], &[]),
            tags(&["serve"], &["fold"]),
            tags(&["chat"], &["fold"]),
        ];
        let ranked = rank_in_degree(&files);
        assert_eq!(ranked[0].file, 0, "fold has highest in-degree");
        assert!(ranked[0].score > 0.0);
    }

    #[test]
    fn in_degree_rewards_breadth_over_single_caller_volume() {
        // file 0 is called by 2 distinct callers (1 and 2).
        // file 3 is called only by 1 caller (4), though referenced multiple times.
        let files = vec![
            tags(&["broad"], &[]),
            tags(&["caller1"], &["broad"]),
            tags(&["caller2"], &["broad"]),
            tags(&["narrow"], &[]),
            tags(&["heavy_caller"], &["narrow", "narrow", "narrow"]),
        ];
        let ranked = rank_in_degree(&files);
        let broad = ranked.iter().find(|r| r.file == 0).unwrap();
        let narrow = ranked.iter().find(|r| r.file == 3).unwrap();
        assert!(
            broad.score > narrow.score,
            "breadth ({}) should beat narrow volume ({})",
            broad.score,
            narrow.score
        );
    }
}
