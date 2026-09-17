//! Syntax highlighting for the content viewer, resolved here and never in the
//! browser.
//!
//! The same call `luu-design.md` makes for prompt diffs — "compute it in Rust
//! and send resolved spans" — and for the same two reasons: a highlighter in
//! the page would be the bundler this project removed on purpose, and the
//! server is already holding the bytes.
//!
//! **The wire format is pre-sliced chunks, not offsets.** A span carried as
//! `{start, end}` is a Rust *byte* index that JavaScript would read as a
//! UTF-16 index, which agrees for ASCII and quietly stops agreeing at the
//! first accented character in a comment. Sending the text already cut removes
//! the question. See `RECORD/2026-09-15.a-three-pane-inspector.completed.md`.

use std::collections::HashMap;
use std::sync::LazyLock;

use serde::Serialize;
use tree_sitter_highlight::{HighlightConfiguration, HighlightEvent, Highlighter};

/// One run of same-coloured text. `kind` is `None` for text no capture
/// matched, which is most of a file.
#[derive(Debug, Clone, Serialize)]
pub struct Chunk {
    pub text: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kind: Option<&'static str>,
}

/// The capture names asked for, and the class names the page styles.
///
/// Deliberately short. Tree-sitter's ecosystem has a hundred-odd capture names
/// and a viewer that colours all of them is a viewer nobody can read; these are
/// the distinctions that survive being looked at in a side panel. A query
/// capture that is not listed here is simply not captured, which is why the
/// list is also the allowlist.
///
/// The order matters only in that it is the index space
/// `tree_sitter_highlight` hands back.
const NAMES: &[&str] = &[
    "attribute",
    "comment",
    "constant",
    "constructor",
    "function",
    "keyword",
    "number",
    "operator",
    "property",
    "punctuation",
    "string",
    "tag",
    "type",
    "variable",
];

/// What the page gets as a CSS class, per index into [`NAMES`]. One to one
/// today; a separate table because the two are separate decisions — a capture
/// name is tree-sitter's, a class name is this page's.
fn class_of(index: usize) -> Option<&'static str> {
    NAMES.get(index).copied()
}

/// Which grammar a path gets, by extension and then by filename.
///
/// One table, so a language is one line. Returns the language's name as the
/// page shows it, alongside the grammar and its queries.
/// The grammar for a path, by filename first and extension second.
///
/// Separate from the table below on purpose: this half is a fact about
/// filenames and changes when somebody adds an extension, that half is a fact
/// about crates and changes when somebody adds a grammar. A name with no entry
/// in the table — `dockerfile`, today — simply has no grammar, and the file is
/// served as text.
fn language_of(path: &str) -> Option<&'static str> {
    let name = path.rsplit('/').next().unwrap_or(path);
    let ext = name.rsplit_once('.').map(|(_, ext)| ext).unwrap_or("");

    // Filenames first: `Makefile` and `Dockerfile` have no extension, and
    // `.gitignore`'s "extension" is the whole name.
    let key = match name {
        "Cargo.lock" => "toml",
        "Dockerfile" | "Containerfile" => "dockerfile",
        _ => ext,
    };

    Some(match key {
        "rs" => "rust",
        "js" | "mjs" | "cjs" | "jsx" => "javascript",
        "ts" | "mts" | "cts" => "typescript",
        "tsx" => "tsx",
        "html" | "htm" => "html",
        "css" => "css",
        "toml" => "toml",
        "md" | "markdown" => "markdown",
        "json" => "json",
        "py" | "pyi" => "python",
        "go" => "go",
        "c" | "h" => "c",
        "cc" | "cpp" | "cxx" | "hpp" | "hh" => "cpp",
        "yaml" | "yml" => "yaml",
        "sh" | "bash" | "zsh" => "bash",
        _ => return None,
    })
}

/// Every grammar, compiled once for the life of the process.
///
/// **This used to be built per request**, and that was most of what opening a
/// file cost: `HighlightConfiguration::new` compiles the grammar's queries,
/// measured at ~15 ms for Rust against ~25 ms to parse the 153 KB file it was
/// being rebuilt for — so thirteen bytes of Rust cost 15 ms and 153 KB cost 40.
/// A configuration is immutable once `configure` has run, so one per language
/// is all there ever needs to be. See the phase 8 section of
/// `RECORD/2026-09-15.a-three-pane-inspector.completed.md`.
///
/// A grammar whose queries will not compile is dropped rather than panicking,
/// which is what the `.ok()` was doing before: one bad grammar should not take
/// the other twelve, and the file it was for is still readable as text.
static GRAMMARS: LazyLock<HashMap<&'static str, HighlightConfiguration>> = LazyLock::new(|| {
    let mut grammars = HashMap::new();
    let mut add = |name: &'static str,
                   language: tree_sitter::Language,
                   highlights: &str,
                   injections: &str,
                   locals: &str| {
        if let Ok(mut config) =
            HighlightConfiguration::new(language, name, highlights, injections, locals)
        {
            config.configure(NAMES);
            grammars.insert(name, config);
        }
    };

    add(
        "rust",
        tree_sitter_rust::LANGUAGE.into(),
        tree_sitter_rust::HIGHLIGHTS_QUERY,
        tree_sitter_rust::INJECTIONS_QUERY,
        "",
    );
    add(
        "javascript",
        tree_sitter_javascript::LANGUAGE.into(),
        tree_sitter_javascript::HIGHLIGHT_QUERY,
        tree_sitter_javascript::INJECTIONS_QUERY,
        tree_sitter_javascript::LOCALS_QUERY,
    );
    add(
        "typescript",
        tree_sitter_typescript::LANGUAGE_TYPESCRIPT.into(),
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
        "",
        tree_sitter_typescript::LOCALS_QUERY,
    );
    add(
        "tsx",
        tree_sitter_typescript::LANGUAGE_TSX.into(),
        tree_sitter_typescript::HIGHLIGHTS_QUERY,
        "",
        tree_sitter_typescript::LOCALS_QUERY,
    );
    add(
        "html",
        tree_sitter_html::LANGUAGE.into(),
        tree_sitter_html::HIGHLIGHTS_QUERY,
        tree_sitter_html::INJECTIONS_QUERY,
        "",
    );
    add(
        "css",
        tree_sitter_css::LANGUAGE.into(),
        tree_sitter_css::HIGHLIGHTS_QUERY,
        "",
        "",
    );
    add(
        "toml",
        tree_sitter_toml_ng::LANGUAGE.into(),
        tree_sitter_toml_ng::HIGHLIGHTS_QUERY,
        "",
        "",
    );
    add(
        "markdown",
        tree_sitter_md::LANGUAGE.into(),
        tree_sitter_md::HIGHLIGHT_QUERY_BLOCK,
        tree_sitter_md::INJECTION_QUERY_BLOCK,
        "",
    );
    add(
        "json",
        tree_sitter_json::LANGUAGE.into(),
        tree_sitter_json::HIGHLIGHTS_QUERY,
        "",
        "",
    );
    add(
        "python",
        tree_sitter_python::LANGUAGE.into(),
        tree_sitter_python::HIGHLIGHTS_QUERY,
        "",
        "",
    );
    add(
        "go",
        tree_sitter_go::LANGUAGE.into(),
        tree_sitter_go::HIGHLIGHTS_QUERY,
        "",
        "",
    );
    add(
        "c",
        tree_sitter_c::LANGUAGE.into(),
        tree_sitter_c::HIGHLIGHT_QUERY,
        "",
        "",
    );
    add(
        "cpp",
        tree_sitter_cpp::LANGUAGE.into(),
        tree_sitter_cpp::HIGHLIGHT_QUERY,
        "",
        "",
    );
    add(
        "yaml",
        tree_sitter_yaml::LANGUAGE.into(),
        tree_sitter_yaml::HIGHLIGHTS_QUERY,
        "",
        "",
    );
    add(
        "bash",
        tree_sitter_bash::LANGUAGE.into(),
        tree_sitter_bash::HIGHLIGHT_QUERY,
        "",
        "",
    );
    grammars
});

/// Compiles every grammar now, so that no request is the one that pays for it.
///
/// Called from `serve`'s startup beside the icon theme, and for the same
/// reason: a cost that is going to be paid once should be paid where somebody
/// is watching the process start, not inside the first click.
pub fn warm() {
    LazyLock::force(&GRAMMARS);
}

fn language_for(path: &str) -> Option<(&'static str, &'static HighlightConfiguration)> {
    let name = language_of(path)?;
    Some((name, GRAMMARS.get(name)?))
}

/// The cap above which a file is served unhighlighted.
///
/// Below [`crate::workspace::MAX_FILE_BYTES`] on purpose: a big file still
/// opens, it just opens as text. Parsing a half-megabyte of minified
/// JavaScript to colour it is a cost nobody asked for by clicking a filename.
pub const MAX_HIGHLIGHT_BYTES: usize = 256 * 1024;

/// One line per line of the file, each a list of chunks.
///
/// Every file comes back in this shape, highlighted or not: a language with no
/// grammar, a file too big to parse and a parse that failed all produce one
/// chunk per line with no kind, so the viewer has one code path instead of
/// three.
pub fn lines(path: &str, text: &str) -> (Option<&'static str>, Vec<Vec<Chunk>>) {
    if text.len() > MAX_HIGHLIGHT_BYTES {
        return (None, plain(text));
    }
    let Some((name, config)) = language_for(path) else {
        return (None, plain(text));
    };
    match highlighted(text, config) {
        Some(lines) => (Some(name), lines),
        // A grammar that failed on this file is not worth a message: the file
        // is still readable, which is what the panel is for.
        None => (None, plain(text)),
    }
}

fn plain(text: &str) -> Vec<Vec<Chunk>> {
    text.split('\n')
        .map(|line| match line.is_empty() {
            true => Vec::new(),
            false => vec![Chunk {
                text: line.to_string(),
                kind: None,
            }],
        })
        .collect()
}

/// Runs the highlighter and cuts its events into lines.
///
/// The cutting is the fiddly half: tree-sitter reports a span that may cross
/// newlines (a block comment, a multi-line string), and a line is what the
/// viewer lays out. Each source span is split on `\n` and its pieces are
/// pushed onto the line they belong to, so a chunk never contains one.
fn highlighted(text: &str, config: &HighlightConfiguration) -> Option<Vec<Vec<Chunk>>> {
    let mut highlighter = Highlighter::new();
    // `None` twice: no encoding override, and no cancellation flag — the
    // 256 KB cap above is what bounds this, not a clock.
    let events = highlighter
        .highlight(config, text.as_bytes(), None, None, |_| None)
        .ok()?;

    let mut lines: Vec<Vec<Chunk>> = vec![Vec::new()];
    // A stack, because captures nest: a string inside a macro inside a
    // function. The innermost is the one that wins, which is what `last` is.
    let mut stack: Vec<usize> = Vec::new();

    for event in events {
        match event.ok()? {
            HighlightEvent::HighlightStart(highlight) => stack.push(highlight.0),
            HighlightEvent::HighlightEnd => {
                stack.pop();
            }
            HighlightEvent::Source { start, end } => {
                let kind = stack.last().copied().and_then(class_of);
                let piece = text.get(start..end)?;
                for (index, part) in piece.split('\n').enumerate() {
                    if index > 0 {
                        lines.push(Vec::new());
                    }
                    if part.is_empty() {
                        continue;
                    }
                    let current = lines.last_mut()?;
                    // Merged with the previous chunk when they agree, so a
                    // line of code is a handful of spans rather than one per
                    // token the parser happened to emit separately.
                    match current.last_mut() {
                        Some(last) if last.kind == kind => last.text.push_str(part),
                        _ => current.push(Chunk {
                            text: part.to_string(),
                            kind,
                        }),
                    }
                }
            }
        }
    }
    Some(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rust_gets_a_grammar_and_a_keyword() {
        let (lang, lines) = lines("a/b/main.rs", "fn main() {}\n");
        assert_eq!(lang, Some("rust"));
        let kinds: Vec<_> = lines[0].iter().map(|c| (c.text.as_str(), c.kind)).collect();
        assert!(
            kinds
                .iter()
                .any(|(text, kind)| *text == "fn" && *kind == Some("keyword")),
            "expected `fn` to be a keyword, got {kinds:?}",
        );
    }

    /// The property every consumer depends on, and the one the line-splitting
    /// is most likely to break: whatever comes back, joining it must be the
    /// file again.
    #[test]
    fn the_chunks_join_back_into_the_file() {
        let source = "/* a comment\n   over two lines */\nfn f() {\n    let s = \"ñ á\";\n}\n";
        let (_, lines) = lines("x.rs", source);
        let rebuilt = lines
            .iter()
            .map(|line| line.iter().map(|c| c.text.as_str()).collect::<String>())
            .collect::<Vec<_>>()
            .join("\n");
        assert_eq!(rebuilt, source);
    }

    #[test]
    fn no_chunk_contains_a_newline() {
        let (_, lines) = lines("x.rs", "fn a() {\n    // hi\n}\n");
        for line in &lines {
            for chunk in line {
                assert!(
                    !chunk.text.contains('\n'),
                    "chunk spans a line break: {chunk:?}"
                );
            }
        }
    }

    /// A file with no grammar and a file too big to parse take the same path
    /// out as one that highlighted, so the viewer never special-cases them.
    #[test]
    fn an_unknown_language_still_comes_back_as_lines() {
        let (lang, lines) = lines("notes.unknownext", "one\ntwo\n");
        assert_eq!(lang, None);
        assert_eq!(lines.len(), 3, "trailing newline is an empty last line");
        assert_eq!(lines[0][0].text, "one");
        assert!(lines[0][0].kind.is_none());
    }

    #[test]
    fn a_file_over_the_cap_is_served_as_text() {
        let big = "fn main() {}\n".repeat(MAX_HIGHLIGHT_BYTES / 8);
        let (lang, lines) = lines("big.rs", &big);
        assert_eq!(lang, None, "too big to parse, and that is not an error");
        assert!(!lines.is_empty());
    }
}
