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
//! - **It is not ranked.** Files are outlined in path order until the budget
//!   runs out, and the map says how many it left out. Path order is a bias —
//!   `crates/agent-core/` before `crates/luu/`, for no reason but the alphabet —
//!   and it is the baseline the reference graph has to beat. Ranking would
//!   personalize the map, and a map that changes per turn is not a prefix.
//!
//! See `RECORD/2026-08-31.the-repo-map.completed.md`.

use std::path::{Path, PathBuf};

use tree_sitter::StreamingIterator;

use crate::context::{Counter, TokenCounter};
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

/// The map, built once and rendered as one block.
#[derive(Debug, Clone)]
pub struct RepoMap {
    pub files: Vec<FileOutline>,
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
    pub fn build(sandbox: &Sandbox, budget: u32, counter: &dyn TokenCounter) -> Self {
        let mut map = Self {
            files: Vec::new(),
            left_out: 0,
            budget,
            tokens: 0,
            counted_by: counter.id(),
        };
        if budget == 0 {
            return map;
        }

        let paths = sources(sandbox);
        // The header is part of the block, so it is part of the budget. Counted
        // against the file count it will end up with rather than the one the
        // walk found — the two differ the moment anything is left out.
        let mut spent = counter.count(&header(paths.len()));
        let mut left_out = false;
        for path in &paths {
            let Some(outline) = read_outline(sandbox, path) else {
                continue;
            };
            if outline.entries.is_empty() {
                continue;
            }
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
            if left_out || spent + cost > budget {
                left_out = true;
                map.left_out += 1;
                continue;
            }
            spent += cost;
            map.files.push(outline);
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

/// Reads one file through the sandbox and outlines it. `None` when the sandbox
/// refuses it, when it cannot be read, or when it holds no definitions.
fn read_outline(sandbox: &Sandbox, path: &Path) -> Option<FileOutline> {
    let check = sandbox.check_path(path, Access::Read);
    if !check.verdict.allowed {
        return None;
    }
    let source = std::fs::read_to_string(&check.path).ok()?;
    Some(FileOutline {
        path: display_path(sandbox, path),
        entries: outline(&source),
    })
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

/// The definitions in one Rust source, in source order.
///
/// Pure, so it is testable without a filesystem — which matters, because this
/// is the half that a grammar upgrade can change under us.
pub fn outline(source: &str) -> Vec<Entry> {
    let language: tree_sitter::Language = tree_sitter_rust::LANGUAGE.into();
    let Ok(query) = tree_sitter::Query::new(&language, tree_sitter_rust::TAGS_QUERY) else {
        return Vec::new();
    };
    let mut parser = tree_sitter::Parser::new();
    if parser.set_language(&language).is_err() {
        return Vec::new();
    }
    let Some(tree) = parser.parse(source, None) else {
        return Vec::new();
    };

    let names = query.capture_names();
    let mut entries: Vec<Entry> = Vec::new();
    let mut cursor = tree_sitter::QueryCursor::new();
    let mut matches = cursor.matches(&query, tree.root_node(), source.as_bytes());
    while let Some(matched) = matches.next() {
        for capture in matched.captures() {
            if !names[capture.index as usize].starts_with("definition") {
                continue;
            }
            let node = capture.node;
            if under_cfg_test(node, source) {
                continue;
            }
            // The enclosing `impl`/`trait`/`mod` is emitted too, so a method
            // does not float free of the type it belongs to. Same node, same
            // line, so the dedup below keeps it once however many methods it
            // holds.
            let mut container = node.parent();
            while let Some(parent) = container {
                if matches!(parent.kind(), "impl_item" | "trait_item" | "mod_item") {
                    push(&mut entries, parent, source);
                }
                container = parent.parent();
            }
            push(&mut entries, node, source);
        }
    }

    entries.sort_by_key(|entry| (entry.line, entry.depth));
    entries.dedup_by_key(|entry| entry.line);
    entries
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
        let map = RepoMap::build(&sandbox, 4096, &ApproximateCounter);
        let paths: Vec<&str> = map.files.iter().map(|file| file.path.as_str()).collect();

        assert_eq!(paths, vec!["src/one.rs", "src/two.rs"], "in path order");
        assert!(
            !map.render().contains("generated"),
            "a build directory is not the repository",
        );
    }

    #[test]
    fn a_budget_too_small_stops_on_a_file_boundary_and_says_how_many_it_dropped() {
        let (_dir, sandbox) = fixture();
        let counter = ApproximateCounter;
        let whole = RepoMap::build(&sandbox, 4096, &counter);
        assert_eq!(whole.left_out, 0);

        // Room for the header and the first file in path order, and not for
        // the second.
        let first = counter.count(&whole.files[0].render());
        let tight = RepoMap::build(&sandbox, counter.count(&header(2)) + first, &counter);

        assert_eq!(tight.files.len(), 1);
        assert_eq!(
            tight.files[0].path, "src/one.rs",
            "path order, not size order"
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
        let (_dir, sandbox) = fixture();
        let counter = ApproximateCounter;

        let mut previous: Vec<String> = Vec::new();
        for budget in [40, 80, 200, 4096] {
            let map = RepoMap::build(&sandbox, budget, &counter);
            let paths: Vec<String> = map.files.iter().map(|file| file.path.clone()).collect();
            assert!(
                paths.starts_with(&previous),
                "at {budget}: {paths:?} does not extend {previous:?}",
            );
            assert_eq!(
                map.files.len() + map.left_out,
                2,
                "every file is either in the map or counted as left out",
            );
            previous = paths;
        }
    }

    #[test]
    fn no_budget_means_no_map_rather_than_an_unbounded_one() {
        let (_dir, sandbox) = fixture();
        let map = RepoMap::build(&sandbox, 0, &ApproximateCounter);
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

        let map = RepoMap::build(&sandbox, 4096, &ApproximateCounter);
        assert!(!map.render().contains("outside"), "{}", map.render());
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
