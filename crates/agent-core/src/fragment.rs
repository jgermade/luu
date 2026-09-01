//! Putting a real file into a turn.
//!
//! [`Fragment`] has existed since the context manager did, and the `code` bucket
//! with it — and nothing ever filled either. The consequence only showed up
//! against a real model: every script in `scripts/tasks/` is ungrounded Q&A, so
//! a 7B asked "what does the context manager do?" answered about Python's
//! `__enter__`/`__exit__`, and *does a closed task's summary lose something the
//! task needed* could not be asked at all — there was no grounded answer for a
//! fold to lose. See `RECORD/2026-08-27.the-m4-pro-run.completed.md`, the fourth pass.
//!
//! Two decisions here, both easy to get wrong:
//!
//! - **A fragment goes through the sandbox.** The user typed the path, not the
//!   model, so this is not the rule about model output reaching the filesystem.
//!   It is the other one: the sandbox is this program's answer to "what may it
//!   read", and a path that `read_file` would refuse must not become readable by
//!   spelling it differently. The verdict names the rule either way.
//! - **A fragment is attached to one turn.** It is fused into that turn's user
//!   message and stored with it; it does not follow the conversation. Which
//!   turns a file belongs in is the question relevance selection exists to
//!   answer later, and attaching it to all of them would answer it wrong now,
//!   in the most expensive possible way.

use std::path::{Path, PathBuf};

use crate::context::Fragment;
use crate::sandbox::{Access, Sandbox, Verdict};
use crate::tools::MAX_OUTPUT_BYTES;

/// A file, and optionally the lines of it that are wanted.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Spec {
    pub path: PathBuf,
    /// 1-based and inclusive, as an editor counts.
    pub lines: Option<(usize, usize)>,
    /// As it was written, so the prompt labels the fragment the way the person
    /// asked for it rather than with a canonical path that changes per machine.
    pub source: String,
}

impl Spec {
    /// Parses `path` or `path:START-END`.
    ///
    /// Read from the right, and a suffix that is not a range is part of the
    /// path — a file called `notes:1.txt` is a file, not a syntax error.
    pub fn parse(spec: &str) -> Self {
        let source = spec.to_string();
        if let Some((path, range)) = spec.rsplit_once(':')
            && let Some((start, end)) = range.split_once('-')
            && let (Ok(start), Ok(end)) = (start.parse::<usize>(), end.parse::<usize>())
            && start >= 1
            && end >= start
        {
            return Self {
                path: PathBuf::from(path),
                lines: Some((start, end)),
                source,
            };
        }
        Self {
            path: PathBuf::from(spec),
            lines: None,
            source,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    /// The sandbox said no. Carried whole, because a denial that does not name
    /// the rule it broke is unreadable the moment a symlink is involved.
    #[error("{}: {}", .source_path, .verdict.rule)]
    Denied {
        source_path: String,
        verdict: Box<Verdict>,
    },
    #[error("reading {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path} has {had} line(s), so {start}-{end} is not in it")]
    OutOfRange {
        path: String,
        had: usize,
        start: usize,
        end: usize,
    },
}

/// Reads one fragment, through the sandbox.
pub fn load(sandbox: &Sandbox, spec: &Spec) -> Result<Fragment, LoadError> {
    let check = sandbox.check_path(&spec.path, Access::Read);
    if !check.verdict.allowed {
        return Err(LoadError::Denied {
            source_path: spec.source.clone(),
            verdict: Box::new(check.verdict),
        });
    }

    let text = read(&check.path).map_err(|source| LoadError::Read {
        path: spec.source.clone(),
        source,
    })?;

    let text = match spec.lines {
        None => text,
        Some((start, end)) => {
            let lines: Vec<&str> = text.lines().collect();
            if start > lines.len() {
                return Err(LoadError::OutOfRange {
                    path: spec.source.clone(),
                    had: lines.len(),
                    start,
                    end,
                });
            }
            let mut cut = lines[start - 1..end.min(lines.len())].join("\n");
            cut.push('\n');
            cut
        }
    };

    Ok(Fragment {
        path: spec.source.clone(),
        text: clamp(text, &spec.source),
    })
}

fn read(path: &Path) -> std::io::Result<String> {
    std::fs::read_to_string(path)
}

/// The same cap a tool result gets, and for the same reason: one file must not
/// be able to blow the window open while nothing is yet deciding what fits.
/// Cutting says so in the text, because a prompt that was silently truncated is
/// a prompt nobody can account for.
fn clamp(mut text: String, source: &str) -> String {
    if text.len() <= MAX_OUTPUT_BYTES {
        return text;
    }
    let mut end = MAX_OUTPUT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    text.truncate(end);
    text.push_str(&format!("\n// {source} cut at {MAX_OUTPUT_BYTES} bytes\n"));
    text
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::{PathRule, SandboxPolicy};

    struct Fixture {
        root: PathBuf,
        sandbox: Sandbox,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "luu-fragment-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(&root).unwrap();
            std::fs::write(root.join("notes.txt"), "one\ntwo\nthree\nfour\n").unwrap();
            let root = root.canonicalize().unwrap();
            let sandbox = Sandbox::new(
                &SandboxPolicy {
                    paths: vec![PathRule::new(".", Access::Read)],
                    ..SandboxPolicy::default()
                },
                &root,
            )
            .unwrap();
            Self { root, sandbox }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn a_range_is_parsed_and_a_colon_that_is_not_one_is_part_of_the_path() {
        let spec = Spec::parse("src/context.rs:10-20");
        assert_eq!(spec.path, PathBuf::from("src/context.rs"));
        assert_eq!(spec.lines, Some((10, 20)));

        assert_eq!(
            Spec::parse("notes:1.txt").path,
            PathBuf::from("notes:1.txt")
        );
        assert_eq!(
            Spec::parse("a.txt:20-10").lines,
            None,
            "backwards is not a range"
        );
        assert_eq!(Spec::parse("a.txt:0-3").lines, None, "editors count from 1");
    }

    #[test]
    fn a_fragment_is_labelled_as_it_was_asked_for() {
        // Absolute rather than relative because `set_current_dir` is
        // process-global and these tests run in threads beside every other one.
        let fixture = Fixture::new("label");
        let spec = format!("{}/notes.txt:2-3", fixture.root.display());

        let fragment = load(&fixture.sandbox, &Spec::parse(&spec)).unwrap();
        assert_eq!(fragment.text, "two\nthree\n");
        assert_eq!(
            fragment.path, spec,
            "a canonicalised path would move the prompt between machines",
        );
    }

    #[test]
    fn a_path_the_sandbox_refuses_is_a_denial_that_names_the_rule() {
        // Reading a file into the prompt is reading a file. Spelling the path
        // in a flag instead of a tool call must not widen what this program can
        // read.
        let fixture = Fixture::new("denied");
        let error = load(&fixture.sandbox, &Spec::parse("/etc/hostname")).unwrap_err();
        assert!(
            matches!(&error, LoadError::Denied { verdict, .. } if !verdict.allowed),
            "{error}",
        );
        assert!(error.to_string().contains("no rule grants read"), "{error}");
    }

    #[test]
    fn a_range_past_the_end_is_an_error_rather_than_an_empty_fragment() {
        // An empty fragment is a prompt that quietly lost its grounding.
        let fixture = Fixture::new("range");
        let spec = format!("{}/notes.txt:9-12", fixture.root.display());
        let error = load(&fixture.sandbox, &Spec::parse(&spec)).unwrap_err();
        assert!(
            matches!(error, LoadError::OutOfRange { had: 4, .. }),
            "{error}"
        );
    }
}
