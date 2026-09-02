//! The repository, as an outline: definitions with their signatures, bodies
//! elided.
//!
//! Three decisions, and the middle one is the design:
//!
//! - **`tree-sitter` and the tags query the grammar already ships.** Every
//!   `tree-sitter-*` crate exports `TAGS_QUERY` as a `pub const`, capturing
//!   `@definition.*` and `@reference.call` both — there is nothing to vendor and
//!   nothing to write. See `RECORD/2026-08-27.aider-repo-map.completed.md`.
//! - **The map goes in the cached prefix, under the tool definitions**, not
//!   fused into the turn the way a fragment is. Blocks are ordered by how often
//!   they are *rewritten*: the system block is a constant, the tools change when
//!   the tool set does, the map changes when the repository does. A map fused
//!   into every user message would be paid for on every call and would push the
//!   changed bytes to the front of the prompt.
//! - **It is ranked, and the ranking does not personalize.** Files are outlined
//!   most-depended-on first — see [`crate::rank`] — until the budget runs out,
//!   and the map says how many it left out. Path order was the baseline and it
//!   is now the tie-breaker, which is what it should always have been: a bias
//!   for `crates/agent-core/` over `crates/luu/`, for no reason but the
//!   alphabet, decided most of what a 1024-token map held. What makes ranking
//!   compatible with the bullet above is the teleport being *uniform*: Aider
//!   seeds PageRank with the files in the conversation, and that is the half
//!   that would rewrite the map every turn. Without it a score depends on
//!   nothing but the tree, so the map still changes only when the repository
//!   does.
//!
//! See `RECORD/2026-08-31.the-repo-map.completed.md` and
//! `RECORD/2026-09-02.ranking-the-map.completed.md`.

use std::path::{Path, PathBuf};

use tree_sitter::StreamingIterator;

use crate::context::{Counter, TokenCounter};
use crate::rank::{self, FileTags, RefKind, Reference};
use crate::sandbox::{Access, Sandbox};

/// How far the walk goes before it stops looking, however wide the sandbox is.
///
/// A policy file that grants a home directory should cost a bounded walk and a
/// short map, not a traversal nobody asked for.
const MAX_ENTRIES: usize = 20_000;

/// Signatures are one line each in the rendering, so a generic soup gets cut
/// rather than wrapping over the whole map.
const MAX_SIGNATURE: usize = 120;

/// One definition, as the map shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Entry {
    /// 1-based, as an editor counts — the map is meant to be followed by hand
    /// into the file, and a `--fragment` range is written in these numbers.
    pub line: usize,
    /// How deeply it nests: a method inside an `impl` is one in, so the
    /// rendering reads as the file does.
    pub depth: usize,
    /// The signature, whitespace collapsed and the body dropped.
    pub text: String,
}

/// One file's definitions, in source order.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileOutline {
    /// Relative to the sandbox base when it is under it, so a map does not
    /// change because the checkout moved.
    pub path: String,
    pub entries: Vec<Entry>,
}

impl FileOutline {
    /// The block this file contributes to the map.
    pub fn render(&self) -> String {
        let mut text = format!("{}\n", self.path);
        for entry in &self.entries {
            text.push_str(&format!(
                "{:>6}  {}{}\n",
                entry.line,
                "  ".repeat(entry.depth),
                entry.text,
            ));
        }
        text
    }
}

/// What decides which files the budget buys.
///
/// Two orders and not one, because the ranking **lost** the only comparison
/// this repository can make today: at 1024 tokens the alphabet holds five files
/// and four of the five the probe's first group asks about, and the ranking
/// holds two and none of them. Half of that gap is a flaw in the corpus — group
/// A is *defined* as the files an alphabetical map holds, so it is the
/// baseline's home turf — and half is real: rank order puts the big central
/// files first, and the fill rule stops at the first file that does not fit, so
/// a ranked map buys fewer files with the same budget.
///
/// So the ranking ships off, exactly as the map itself shipped off: a default
/// that changes what every recording holds needs a scored run behind it, and
/// that run needs a model. See `RECORD/2026-09-02.ranking-the-map.completed.md`.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Order {
    /// Path order. A bias, and the measured baseline.
    #[default]
    Path,
    /// Most depended-on first, by [`crate::rank`].
    Ranked,
}

/// One file's place in the ranking, kept whether or not it fitted.
///
/// This is the map explaining itself, and it is not optional furniture: the
/// case against embeddings was that a cosine distance cannot say *why* a file
/// was chosen. Having built a graph that can, it would be a poor trade not to
/// ask it. `luu map --explain` prints these — **outside the rendered block**,
/// because a map that explained itself to the model would spend the budget on
/// its own footnotes.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedFile {
    pub path: String,
    pub score: f64,
    /// The files that reference this one, heaviest first.
    pub referrers: Vec<(String, f64)>,
    /// Whether it fitted. The ranking covers the whole walk; the map does not.
    pub in_map: bool,
}

/// The map, built once and rendered as one block.
#[derive(Debug, Clone)]
pub struct RepoMap {
    pub files: Vec<FileOutline>,
    /// Every outlined file in the order the map used, the ones that did not
    /// fit included. The map is the prefix of this list the budget could pay
    /// for, and the scores are attached whichever order chose it — so the
    /// ranking can be looked at without being obeyed.
    pub ranked: Vec<RankedFile>,
    pub order: Order,
    /// Files the walk found, could read and could parse, that did not fit.
    /// Reported in the map itself: a block that silently ends is a block nobody
    /// can account for afterwards.
    pub left_out: usize,
    pub budget: u32,
    /// What the rendering below actually costs, by the counter that measured
    /// it — the same one every other bucket is in.
    pub tokens: u32,
    pub counted_by: Counter,
}

impl RepoMap {
    /// Walks the sandbox's readable roots and outlines what fits.
    ///
    /// Everything is read **through the sandbox**, for the same reason a
    /// fragment is: the sandbox is this program's answer to what it may read,
    /// and a path `read_file` would refuse must not become readable by being
    /// reached a different way.
    pub fn build(sandbox: &Sandbox, budget: u32, counter: &dyn TokenCounter, order: Order) -> Self {
        let mut map = Self {
            files: Vec::new(),
            ranked: Vec::new(),
            order,
            left_out: 0,
            budget,
            tokens: 0,
            counted_by: counter.id(),
        };
        if budget == 0 {
            return map;
        }

        let paths = sources(sandbox);
        // Every file the walk could read and parse, whether or not it holds a
        // definition. A file that defines nothing still *references* things,
        // and those references are evidence about somebody else — dropping it
        // here would be dropping an edge, not a node.
        let read: Vec<(FileOutline, FileTags)> = paths
            .iter()
            .filter_map(|path| read_tags(sandbox, path))
            .collect();
        let tags: Vec<FileTags> = read.iter().map(|(_, tags)| tags.clone()).collect();

        // The order, and it is decided before a single token is counted: the
        // budget says how much of it fits, never what it is. That separation is
        // what keeps a wider budget a superset of a tighter one under either
        // order.
        //
        // The graph is built either way. Under `Path` its scores are carried
        // and not obeyed, which is what lets `luu map --explain` show the order
        // the map did *not* take — the cheapest way to argue with a ranking is
        // to read it beside the thing it would have replaced.
        let mut scored: Vec<rank::Ranked> = rank::rank(&tags)
            .into_iter()
            // A file with no definitions is a node in the graph and never a
            // line in the map: there is nothing to outline.
            .filter(|ranked| !read[ranked.file].0.entries.is_empty())
            .collect();
        if order == Order::Path {
            scored.sort_by(|a, b| read[a.file].0.path.cmp(&read[b.file].0.path));
        }
        let order = scored;

        // The header is part of the block, so it is part of the budget. Counted
        // against the file count it will end up with rather than the one the
        // walk found — the two differ the moment anything is left out.
        let mut spent = counter.count(&header(order.len()));
        let mut left_out = false;
        for ranked in &order {
            let outline = &read[ranked.file].0;
            let cost = counter.count(&outline.render());
            // Whole files: half an outline is a list of methods whose type has
            // gone, which reads as a different file.
            //
            // And the first file that does not fit **stops** the map rather
            // than being skipped over. Skipping fills the budget better and is
            // not monotone: one large outline displaces several small ones, so
            // a *tighter* budget could show more files than a wider one — 13 at
            // 1500 tokens against 12 at 2048, measured on this repository. A
            // map whose contents do not nest as the budget grows cannot be
            // compared against itself, and comparing is what it is for.
            let fitted = !left_out && spent + cost <= budget;
            if fitted {
                spent += cost;
                map.files.push(outline.clone());
            } else {
                left_out = true;
                map.left_out += 1;
            }
            map.ranked.push(RankedFile {
                path: outline.path.clone(),
                score: ranked.score,
                referrers: ranked
                    .referrers
                    .iter()
                    .map(|(from, weight)| (read[*from].0.path.clone(), *weight))
                    .collect(),
                in_map: fitted,
            });
        }
        // Counted from the rendering rather than from the running total: the
        // header names how many files are in the map, so a map that left
        // anything out has a header the loop above could not have counted. The
        // block that goes into the prompt is the thing to measure, and it is
        // right here.
        //
        // It can land a few tokens over the budget, and the overshoot is the
        // footer — the line that admits what was dropped. Dropping *that* to
        // fit would be the map hiding exactly the thing it exists to say.
        map.tokens = counter.count(&map.render());
        map
    }

    /// The exact bytes that go into the prefix. `luu map` prints this.
    pub fn render(&self) -> String {
        if self.files.is_empty() && self.left_out == 0 {
            return String::new();
        }
        let mut text = header(self.files.len());
        for file in &self.files {
            text.push('\n');
            text.push_str(&file.render());
        }
        if self.left_out > 0 {
            text.push('\n');
            text.push_str(&footer(self.left_out, self.budget));
        }
        text
    }

    pub fn is_empty(&self) -> bool {
        self.files.is_empty()
    }
}

fn header(files: usize) -> String {
    format!(
        "# Repository map — {files} file(s), definitions only, bodies elided.\n\
         # Line numbers are 1-based: ask for a file to see the body.\n"
    )
}

fn footer(left_out: usize, budget: u32) -> String {
    format!("# {left_out} more file(s) not shown — the map's budget is {budget} tokens.\n")
}

/// Readable `.rs` files under the sandbox's roots, in path order.
fn sources(sandbox: &Sandbox) -> Vec<PathBuf> {
    let mut found = Vec::new();
    let mut seen = 0usize;
    for root in sandbox.roots() {
        // Implicit roots are skipped, and that is not an optimisation. Allowing
        // a command implies read+execute on `/usr` and friends, because a
        // program cannot run without reading libc — and that reasoning says
        // nothing about what belongs in a map of *this* repository. Walking
        // them would outline the standard library.
        if root.implicit {
            continue;
        }
        walk(&root.path, &mut found, &mut seen);
    }
    found.sort();
    found.dedup();
    found
}

/// Depth-first, bounded, and skipping two kinds of directory.
///
/// A name starting with `.` and a directory called `target`: a build directory
/// is not the repository, and `target/` holds generated sources that would
/// outweigh everything real in the map. It is a short list and it is a guess,
/// which is why `luu map` prints what the walk decided rather than leaving it
/// to be inferred from a number that looks wrong.
fn walk(dir: &Path, found: &mut Vec<PathBuf>, seen: &mut usize) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        if *seen >= MAX_ENTRIES {
            return;
        }
        *seen += 1;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        let Ok(kind) = entry.file_type() else {
            continue;
        };
        // Not followed: a symlink out of the tree is the sandbox's business,
        // and a symlink inside it is a file the walk already has.
        if kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            if name.starts_with('.') || name == "target" {
                continue;
            }
            walk(&path, found, seen);
            continue;
        }
        if path.extension().is_some_and(|ext| ext == "rs") {
            found.push(path);
        }
    }
}

/// Reads one file through the sandbox and tags it. `None` when the sandbox
/// refuses it or when it cannot be read.
fn read_tags(sandbox: &Sandbox, path: &Path) -> Option<(FileOutline, FileTags)> {
    let check = sandbox.check_path(path, Access::Read);
    if !check.verdict.allowed {
        return None;
    }
    let source = std::fs::read_to_string(&check.path).ok()?;
    let tags = tags(&source);
    Some((
        FileOutline {
            path: display_path(sandbox, path),
            entries: tags.entries,
        },
        FileTags {
            defines: tags.defines,
            references: tags.references,
        },
    ))
}

/// Relative to the base when it is under it: an absolute path is noise in a
/// prompt and it changes with the checkout.
fn display_path(sandbox: &Sandbox, path: &Path) -> String {
    path.strip_prefix(sandbox.base())
        .unwrap_or(path)
        .display()
        .to_string()
}

/// Whether the node is inside a module the compiler only builds for tests.
///
/// A `#[cfg(test)]` module is not the interface this map exists to describe,
/// and in this repository it is most of the lines: `agent.rs` alone spends more
/// of the outline on its test names than on everything it exports. Left in, the
/// tests crowd out the code at any budget small enough to matter.
fn under_cfg_test(node: tree_sitter::Node, source: &str) -> bool {
    let mut current = Some(node);
    while let Some(here) = current {
        if here.kind() == "mod_item" {
            let mut sibling = here.prev_sibling();
            while let Some(previous) = sibling {
                if previous.kind() != "attribute_item" {
                    break;
                }
                let text = source.get(previous.byte_range()).unwrap_or_default();
                if text.replace(char::is_whitespace, "").contains("cfg(test)") {
                    return true;
                }
                sibling = previous.prev_sibling();
            }
        }
        current = here.parent();
    }
    false
}

/// Everything one pass over a Rust source yields.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Tags {
    /// The definitions, in source order — what the map renders.
    pub entries: Vec<Entry>,
    /// The names those definitions introduce — what an edge points *at*.
    pub defines: Vec<String>,
    /// The names this file uses, repeats included, each carrying what the
    /// grammar saw: a method call and a free call are not the same evidence.
    pub references: Vec<Reference>,
}

/// The definitions in one Rust source, in source order.
pub fn outline(source: &str) -> Vec<Entry> {
    tags(source).entries
}

/// Definitions and references in one Rust source, from one pass.
///
/// Pure, so it is testable without a filesystem — which matters, because this
/// is the half that a grammar upgrade can change under us.
///
/// The two capture families come out of the same stock `TAGS_QUERY`, which is
/// the map's first decision holding: `@definition.*` names the outline and
/// `@reference.call` / `@reference.implementation` name the graph, and there is
/// still nothing to vendor and nothing to write.
pub fn tags(source: &str) -> Tags {
    let mut tags = Tags::default();
    let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let Ok(query) = tree_sitter::Query::new(&language, tree_sitter_rust::TAGS_QUERY) else {
        return tags;
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return tags;
    }
    let Some(tree) = parser.parse(source, None) else {
        return tags;
    };

    let names = query.capture_names();
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(matched) = matches.next() {
        // One match carries the item and the identifier that names it, so they
        // are read together rather than by two passes that could disagree.
        let mut item: Option<(tree_sitter::Node, bool)> = None;
        let mut name: Option<(&str, &str)> = None;
        for capture in matched.captures() {
            let kind = names[capture.index as usize];
            if kind == "name" {
                // The node's own kind is what separates `x.render()` from
                // `render()`: one query pattern, two strengths of evidence.
                name = source
                    .get(capture.node.byte_range())
                    .map(|text| (text, capture.node.kind()));
            } else if kind.starts_with("definition") {
                item = Some((capture.node, true));
            } else if kind.starts_with("reference") {
                item = Some((capture.node, false));
            }
        }
        let Some((node, is_definition)) = item else {
            continue;
        };
        // A test module calls everything, and leaving its references in would
        // flatten the graph toward whatever the suite exercises — a true signal
        // about the tests and a false one about the code. Same exclusion as the
        // outline's, for a stronger reason.
        if under_cfg_test(node, source) {
            continue;
        }
        let Some((name, node_kind)) = name else {
            continue;
        };
        if !is_definition {
            tags.references.push(Reference {
                name: name.to_string(),
                kind: match node_kind {
                    "field_identifier" => RefKind::Method,
                    "type_identifier" => RefKind::Implementation,
                    _ => RefKind::Call,
                },
            });
            continue;
        }
        tags.defines.push(name.to_string());
        // The enclosing `impl`/`trait`/`mod` is emitted too, so a method does
        // not float free of the type it belongs to. Same node, same line, so
        // the dedup below keeps it once however many methods it holds.
        let mut container = node.parent();
        while let Some(parent) = container {
            if matches!(parent.kind(), "impl_item" | "trait_item" | "mod_item") {
                push(&mut tags.entries, parent, source);
            }
            container = parent.parent();
        }
        push(&mut tags.entries, node, source);
    }

    tags.entries.sort_by_key(|entry| (entry.line, entry.depth));
    tags.entries.dedup_by_key(|entry| entry.line);
    tags.defines.sort();
    tags.defines.dedup();
    tags
}

fn push(entries: &mut Vec<Entry>, node: tree_sitter::Node, source: &str) {
    let line = node.start_position().row + 1;
    if entries.iter().any(|entry| entry.line == line) {
        return;
    }
    entries.push(Entry {
        line,
        depth: depth_of(node),
        text: signature(node, source),
    });
}

/// How many `impl`/`trait`/`mod` blocks the node sits inside.
fn depth_of(node: tree_sitter::Node) -> usize {
    let mut depth = 0;
    let mut parent = node.parent();
    while let Some(current) = parent {
        if matches!(current.kind(), "impl_item" | "trait_item" | "mod_item") {
            depth += 1;
        }
        parent = current.parent();
    }
    depth
}

/// Everything before the body, whitespace collapsed.
///
/// Cutting at the body rather than at the first newline is what makes a
/// multi-line signature readable: `pub fn push_turn_with_steps(` on its own says
/// nothing about what it takes.
fn signature(node: tree_sitter::Node, source: &str) -> String {
    let end = node
        .child_by_field_name("body")
        .map(|body| body.start_byte())
        .unwrap_or(node.end_byte());
    let text = source.get(node.start_byte()..end).unwrap_or_default();
    let mut collapsed = text.split_whitespace().collect::<Vec<_>>().join(" ");
    while collapsed.ends_with(['{', ';', ',']) {
        collapsed.pop();
        collapsed = collapsed.trim_end().to_string();
    }
    if collapsed.chars().count() > MAX_SIGNATURE {
        collapsed = collapsed.chars().take(MAX_SIGNATURE).collect::<String>() + " …";
    }
    collapsed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::context::ApproximateCounter;
    use crate::sandbox::{PathRule, SandboxPolicy};

    const SOURCE: &str = r#"
use std::fmt;

pub struct Budget {
    pub limit: Option<u32>,
}

impl Budget {
    pub fn new(
        limit: u32,
        reserve: u32,
    ) -> Self {
        Self { limit: Some(limit) }
    }
}

fn private_helper(x: u32) -> u32 {
    x + 1
}
"#;

    #[test]
    fn an_outline_is_definitions_with_their_signatures_and_no_bodies() {
        let entries = outline(SOURCE);
        let text: Vec<&str> = entries.iter().map(|entry| entry.text.as_str()).collect();

        assert!(text.contains(&"pub struct Budget"), "{text:?}");
        assert!(
            text.contains(&"impl Budget"),
            "a method needs its type: {text:?}"
        );
        assert!(
            text.contains(&"pub fn new( limit: u32, reserve: u32, ) -> Self"),
            "a multi-line signature is collapsed, not cut at the first newline: {text:?}",
        );
        assert!(
            text.contains(&"fn private_helper(x: u32) -> u32"),
            "{text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("x + 1")),
            "the body is what the map is not: {text:?}",
        );
    }

    #[test]
    fn a_method_nests_under_the_impl_it_belongs_to() {
        let entries = outline(SOURCE);
        let impl_block = entries
            .iter()
            .find(|entry| entry.text == "impl Budget")
            .expect("the impl");
        let method = entries
            .iter()
            .find(|entry| entry.text.starts_with("pub fn new"))
            .expect("the method");

        assert_eq!(impl_block.depth, 0);
        assert_eq!(method.depth, 1, "so the rendering reads as the file does");
        assert!(method.line > impl_block.line, "and follows it");
    }

    #[test]
    fn lines_are_one_based_so_a_fragment_range_can_be_written_from_the_map() {
        let entries = outline("fn first() {}\nfn second() {}\n");
        assert_eq!(entries[0].line, 1);
        assert_eq!(entries[1].line, 2);
    }

    #[test]
    fn a_cfg_test_module_is_not_in_the_map() {
        // Not a style preference: at any budget small enough to matter, the
        // test names crowd out the code the map exists to describe.
        let entries = outline(
            "pub fn exported() {}\n\n#[cfg(test)]\nmod tests {\n    fn a_test_name() {}\n}\n",
        );
        let text: Vec<&str> = entries.iter().map(|entry| entry.text.as_str()).collect();

        assert_eq!(
            text,
            vec!["pub fn exported()"],
            "the code, and only the code"
        );
        assert!(
            !text.iter().any(|line| line.contains("a_test_name")),
            "{text:?}"
        );
        assert!(
            !text.iter().any(|line| line.contains("mod tests")),
            "{text:?}"
        );
    }

    #[test]
    fn a_file_that_does_not_parse_as_rust_yields_nothing_rather_than_failing() {
        // A `.rs` that is half-written is the ordinary case while an agent is
        // editing one. tree-sitter recovers and reports what it could read;
        // what matters is that the map does not.
        let entries = outline("fn broken( {{{ ");
        assert!(entries.len() <= 1, "{entries:?}");
    }

    fn fixture() -> (tempdir::Dir, Sandbox) {
        let dir = tempdir::Dir::new("repo-map");
        std::fs::create_dir_all(dir.path().join("src")).expect("the source directory");
        std::fs::write(dir.path().join("src/one.rs"), SOURCE).expect("one.rs");
        std::fs::write(dir.path().join("src/two.rs"), "pub fn two() {}\n").expect("two.rs");
        // Build output is not the repository, and it is full of generated `.rs`.
        std::fs::create_dir_all(dir.path().join("target/debug")).expect("target");
        std::fs::write(
            dir.path().join("target/debug/gen.rs"),
            "pub fn generated() {}\n",
        )
        .expect("gen.rs");
        std::fs::write(dir.path().join("README.md"), "not rust\n").expect("README");

        let mut policy = SandboxPolicy::default();
        policy.allow(dir.path(), Access::Read);
        let sandbox = Sandbox::new(&policy, dir.path()).expect("the sandbox");
        (dir, sandbox)
    }

    #[test]
    fn the_walk_takes_rust_sources_and_leaves_the_build_directory_alone() {
        let (_dir, sandbox) = fixture();
        let map = RepoMap::build(&sandbox, 4096, &ApproximateCounter, Order::Path);
        let paths: Vec<&str> = map.files.iter().map(|file| file.path.as_str()).collect();

        assert_eq!(
            paths,
            vec!["src/one.rs", "src/two.rs"],
            "neither references the other, so the tie breaks on the path",
        );
        assert!(
            !map.render().contains("generated"),
            "a build directory is not the repository",
        );
    }

    #[test]
    fn a_budget_too_small_stops_on_a_file_boundary_and_says_how_many_it_dropped() {
        let (_dir, sandbox) = fixture();
        let counter = ApproximateCounter;
        let whole = RepoMap::build(&sandbox, 4096, &counter, Order::Path);
        assert_eq!(whole.left_out, 0);

        // Room for the header and the first file in path order, and not for
        // the second.
        let first = counter.count(&whole.files[0].render());
        let tight = RepoMap::build(
            &sandbox,
            counter.count(&header(2)) + first,
            &counter,
            Order::Path,
        );

        assert_eq!(tight.files.len(), 1);
        assert_eq!(
            tight.files[0].path, "src/one.rs",
            "the head of the ranking, not the smallest file"
        );
        assert_eq!(tight.left_out, 1);
        assert!(
            tight.render().contains("1 more file(s) not shown"),
            "a block that silently ends is one nobody can account for: {}",
            tight.render(),
        );
    }

    #[test]
    fn a_wider_budget_shows_everything_a_tighter_one_did() {
        // The map's contents nest as the budget grows, which is what makes two
        // runs at two budgets comparable. Skipping a file that does not fit and
        // carrying on would fill the budget better and break exactly this: one
        // large outline displaces several small ones, so a tighter budget could
        // show *more* files than a wider one.
        // Both orders, because the property belongs to the fill rule rather
        // than to either order — and ranking is the change most likely to
        // break it, since it is what put a large file at the head.
        let counter = ApproximateCounter;
        for order in [Order::Path, Order::Ranked] {
            let (_dir, sandbox) = referring_fixture();
            let mut previous: Vec<String> = Vec::new();
            for budget in [40, 80, 200, 4096] {
                let map = RepoMap::build(&sandbox, budget, &counter, order);
                let paths: Vec<String> = map.files.iter().map(|file| file.path.clone()).collect();
                assert!(
                    paths.starts_with(&previous),
                    "{order:?} at {budget}: {paths:?} does not extend {previous:?}",
                );
                assert_eq!(
                    map.files.len() + map.left_out,
                    2,
                    "every file is either in the map or counted as left out",
                );
                previous = paths;
            }
        }
    }

    #[test]
    fn no_budget_means_no_map_rather_than_an_unbounded_one() {
        let (_dir, sandbox) = fixture();
        let map = RepoMap::build(&sandbox, 0, &ApproximateCounter, Order::Path);
        assert!(map.is_empty());
        assert_eq!(map.tokens, 0);
        assert_eq!(map.render(), "", "and nothing reaches the prefix");
    }

    #[test]
    fn a_file_the_sandbox_refuses_is_not_in_the_map() {
        let (dir, _) = fixture();
        // The policy grants only `src`, so `outside.rs` beside it is not ours
        // to read — the same rule a fragment is held to.
        std::fs::write(dir.path().join("outside.rs"), "pub fn outside() {}\n").expect("outside");
        // Only `src`: the default policy grants the whole working directory,
        // which would make this test pass for the wrong reason.
        let policy = SandboxPolicy {
            paths: vec![PathRule::new(dir.path().join("src"), Access::Read)],
            commands: Vec::new(),
            network: false,
            enforcement: Default::default(),
            limits: Default::default(),
        };
        let sandbox = Sandbox::new(&policy, dir.path()).expect("the sandbox");

        let map = RepoMap::build(&sandbox, 4096, &ApproximateCounter, Order::Path);
        assert!(!map.render().contains("outside"), "{}", map.render());
    }

    /// Two files, and the one the other calls into sorts *second*. Every
    /// question the map answers at a tight budget turns on which of them the
    /// budget buys, and until this commit the answer was the alphabet's.
    fn referring_fixture() -> (tempdir::Dir, Sandbox) {
        let dir = tempdir::Dir::new("repo-map-rank");
        std::fs::create_dir_all(dir.path().join("src")).expect("the source directory");
        // `aaa.rs` is the entry point: it calls, and nothing calls it.
        std::fs::write(
            dir.path().join("src/aaa.rs"),
            "pub fn main_loop() { fold_history(); fold_history(); }\n",
        )
        .expect("aaa.rs");
        // `zzz.rs` is what the tree stands on, and sorts last.
        std::fs::write(
            dir.path().join("src/zzz.rs"),
            "pub fn fold_history() -> u32 { 1 }\n",
        )
        .expect("zzz.rs");

        let mut policy = SandboxPolicy::default();
        policy.allow(dir.path(), Access::Read);
        let sandbox = Sandbox::new(&policy, dir.path()).expect("the sandbox");
        (dir, sandbox)
    }

    #[test]
    fn the_file_the_others_call_into_is_outlined_first_however_it_sorts() {
        let (_dir, sandbox) = referring_fixture();
        let map = RepoMap::build(&sandbox, 4096, &ApproximateCounter, Order::Ranked);
        let paths: Vec<&str> = map.files.iter().map(|file| file.path.as_str()).collect();

        assert_eq!(
            paths,
            vec!["src/zzz.rs", "src/aaa.rs"],
            "the alphabet is the tie-breaker now, not the ranking",
        );
    }

    #[test]
    fn a_budget_for_one_file_buys_the_one_the_repository_stands_on() {
        // The whole point, in one assertion: at a budget that fits a single
        // file, path order bought the caller and ranking buys the callee.
        let (_dir, sandbox) = referring_fixture();
        let counter = ApproximateCounter;
        let whole = RepoMap::build(&sandbox, 4096, &counter, Order::Ranked);
        let first = counter.count(&whole.files[0].render());
        let tight = RepoMap::build(
            &sandbox,
            counter.count(&header(2)) + first,
            &counter,
            Order::Ranked,
        );

        assert_eq!(tight.files.len(), 1);
        assert_eq!(tight.files[0].path, "src/zzz.rs");
        assert_eq!(tight.left_out, 1);
    }

    #[test]
    fn the_map_can_say_why_a_file_is_in_it_and_what_it_dropped() {
        // A cosine distance could not answer this, and that was the argument
        // for the graph. The explanation covers the whole walk, not the part
        // that fitted — otherwise it could not account for the exclusions.
        let (_dir, sandbox) = referring_fixture();
        let counter = ApproximateCounter;
        let whole = RepoMap::build(&sandbox, 4096, &counter, Order::Ranked);
        let first = counter.count(&whole.files[0].render());
        let tight = RepoMap::build(
            &sandbox,
            counter.count(&header(2)) + first,
            &counter,
            Order::Ranked,
        );

        let paths: Vec<&str> = tight.ranked.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, vec!["src/zzz.rs", "src/aaa.rs"], "the whole walk");
        assert!(tight.ranked[0].in_map);
        assert!(!tight.ranked[1].in_map, "and which half of it fitted");
        assert_eq!(
            tight.ranked[0]
                .referrers
                .first()
                .map(|(path, _)| path.as_str()),
            Some("src/aaa.rs"),
            "named, because the map's own reason is the thing to argue with",
        );
        assert!(tight.ranked[0].score > tight.ranked[1].score);
    }

    #[test]
    fn the_ranking_is_read_through_the_sandbox_like_everything_else() {
        // A file the policy refuses cannot vote. Otherwise a path `read_file`
        // would refuse could still decide what the map holds, which is the
        // same leak the map's own walk is careful about.
        let dir = tempdir::Dir::new("repo-map-refused");
        std::fs::create_dir_all(dir.path().join("src")).expect("the source directory");
        std::fs::create_dir_all(dir.path().join("apps")).expect("the caller directory");
        std::fs::write(dir.path().join("src/zzz.rs"), "pub fn fold_history() {}\n").expect("zzz");
        std::fs::write(dir.path().join("src/aaa.rs"), "pub fn unrelated() {}\n").expect("aaa");
        std::fs::write(
            dir.path().join("apps/caller.rs"),
            "pub fn main_loop() { fold_history(); fold_history(); }\n",
        )
        .expect("the caller");

        let mut wide = SandboxPolicy::default();
        wide.allow(dir.path(), Access::Read);
        let wide = Sandbox::new(&wide, dir.path()).expect("the wide sandbox");
        let seen = RepoMap::build(&wide, 4096, &ApproximateCounter, Order::Ranked);
        assert_eq!(
            seen.ranked[0].path, "src/zzz.rs",
            "with the caller readable, what it calls leads: {:?}",
            seen.ranked,
        );

        // Only `src`: the caller is now outside what this program may read.
        let narrow = SandboxPolicy {
            paths: vec![PathRule::new(dir.path().join("src"), Access::Read)],
            commands: Vec::new(),
            network: false,
            enforcement: Default::default(),
            limits: Default::default(),
        };
        let narrow = Sandbox::new(&narrow, dir.path()).expect("the narrow sandbox");
        let map = RepoMap::build(&narrow, 4096, &ApproximateCounter, Order::Ranked);

        let paths: Vec<&str> = map.ranked.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(
            paths,
            vec!["src/aaa.rs", "src/zzz.rs"],
            "back to the tie-break"
        );
        assert!(
            map.ranked.iter().all(|file| file.referrers.is_empty()),
            "the caller is not ours to read, so it is not evidence: {:?}",
            map.ranked,
        );
    }

    #[test]
    fn a_file_with_references_and_no_definitions_is_an_edge_without_being_a_line() {
        // It has nothing to outline and it still says something about somebody
        // else. Dropping it from the walk would be dropping an edge.
        let dir = tempdir::Dir::new("repo-map-edge");
        std::fs::create_dir_all(dir.path().join("src")).expect("the source directory");
        std::fs::write(dir.path().join("src/zzz.rs"), "pub fn fold() {}\n").expect("zzz.rs");
        std::fs::write(dir.path().join("src/aaa.rs"), "pub fn other() {}\n").expect("aaa.rs");
        // Statements at the top level of a file: no definition, one reference.
        std::fs::write(dir.path().join("src/uses.rs"), "const _X: () = fold();\n").expect("uses");

        let mut policy = SandboxPolicy::default();
        policy.allow(dir.path(), Access::Read);
        let sandbox = Sandbox::new(&policy, dir.path()).expect("the sandbox");

        let map = RepoMap::build(&sandbox, 4096, &ApproximateCounter, Order::Ranked);
        let paths: Vec<&str> = map.ranked.iter().map(|file| file.path.as_str()).collect();
        assert_eq!(paths, vec!["src/zzz.rs", "src/aaa.rs"], "{paths:?}");
        assert!(
            !map.render().contains("uses.rs"),
            "a node in the graph is not a line in the map: {}",
            map.render(),
        );
    }

    #[test]
    fn a_reference_is_a_reference_and_a_definition_is_not_both() {
        let tags = tags("pub fn fold() { helper(); }\nimpl Sandbox {}\n");
        assert_eq!(tags.defines, vec!["fold"]);
        let kind_of = |name: &str| {
            tags.references
                .iter()
                .find(|reference| reference.name == name)
                .map(|reference| reference.kind)
        };
        assert_eq!(kind_of("helper"), Some(RefKind::Call), "{tags:?}");
        assert_eq!(
            kind_of("Sandbox"),
            Some(RefKind::Implementation),
            "an impl is a reference to the type it is for: {tags:?}",
        );
        assert_eq!(kind_of("fold"), None, "a definition is not both: {tags:?}");

        // A method is the same capture and not the same evidence: it resolves
        // through a type `tree-sitter` cannot see, and is weighted for it.
        let method = super::tags("pub fn a() { thing.check_path(); }\n");
        assert_eq!(
            method
                .references
                .iter()
                .find(|reference| reference.name == "check_path")
                .map(|reference| reference.kind),
            Some(RefKind::Method),
            "{method:?}",
        );
    }

    #[test]
    fn a_cfg_test_module_does_not_vote_either() {
        // Stronger than the outline's reason for excluding it: a test module
        // calls everything, so leaving its references in flattens the graph
        // toward whatever the suite exercises.
        let tags = tags(
            "pub fn exported() {}\n\n#[cfg(test)]\nmod tests {\n    fn t() { exported(); very_rare_name(); }\n}\n",
        );
        assert!(
            !tags
                .references
                .iter()
                .any(|reference| reference.name == "very_rare_name"),
            "{tags:?}",
        );
    }

    #[test]
    fn path_order_ignores_the_graph_even_where_the_graph_disagrees() {
        // The flag has to be the whole of the difference. The ranking lost the
        // only comparison this repository can make — five files against two at
        // 1024 tokens, and four of the probe's first group against none — so
        // the default staying put is the finding, not an oversight.
        let (_dir, sandbox) = referring_fixture();
        let map = RepoMap::build(&sandbox, 4096, &ApproximateCounter, Order::Path);
        let paths: Vec<&str> = map.files.iter().map(|file| file.path.as_str()).collect();

        assert_eq!(paths, vec!["src/aaa.rs", "src/zzz.rs"], "the alphabet");
        assert!(
            map.ranked[1].score > map.ranked[0].score,
            "and the scores are carried anyway, so `--explain` can show the \
             order the map did not take: {:?}",
            map.ranked,
        );
    }

    /// A directory that removes itself, so a failing test does not leave one
    /// behind and the next run does not read it.
    mod tempdir {
        pub struct Dir(std::path::PathBuf);

        impl Dir {
            pub fn new(name: &str) -> Self {
                let path = std::env::temp_dir().join(format!(
                    "luu-{name}-{}-{:?}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .expect("a clock after 1970")
                        .as_nanos(),
                ));
                std::fs::create_dir_all(&path).expect("the temporary directory");
                Self(path)
            }

            pub fn path(&self) -> &std::path::Path {
                &self.0
            }
        }

        impl Drop for Dir {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }
}
