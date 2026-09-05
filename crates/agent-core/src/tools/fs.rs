//! The filesystem tools.
//!
//! Every one of them asks [`Sandbox::check_path`] first and works on the path
//! it hands back — the canonical one, not the argument. Comparing the argument
//! and then opening the argument is how a symlink walks out.
//!
//! And the check is no longer the whole of it. Between an answer about a path
//! and the `open` that follows, the path is a *string*, and a string is not a
//! file — so the ones that open a file do it with `openat2(RESOLVE_BENEATH)`
//! relative to the root that granted it, which makes the kernel refuse the
//! escape during resolution rather than us refuse it beforehand. The verdict
//! then says the kernel held it. See
//! `RECORD/2026-09-05.beneath-the-root.completed.md`.

use std::io::{Read, Write};
use std::path::Path;

use serde_json::json;

use crate::sandbox::{Access, Sandbox, beneath};

use super::{Tool, ToolFuture, ToolOutcome};

/// How much of a file one call returns by default. A whole file is often the
/// wrong answer for an 8K window, and a tool that cannot be asked for a range
/// leaves the model no way to say so.
const DEFAULT_MAX_LINES: usize = 400;

/// Pulls a string argument, or says which one was missing.
fn string_arg<'a>(arguments: &'a serde_json::Value, name: &str) -> Result<&'a str, String> {
    arguments
        .get(name)
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("`{name}` is required and must be a string"))
}

fn usize_arg(arguments: &serde_json::Value, name: &str, default: usize) -> usize {
    arguments
        .get(name)
        .and_then(serde_json::Value::as_u64)
        .map_or(default, |value| value as usize)
}

/// Turns an argument error into an outcome. The sandbox was never consulted, so
/// the verdict says so rather than claiming a rule allowed something.
fn bad_arguments(tool: &str, message: String) -> ToolOutcome {
    ToolOutcome::failed(
        crate::sandbox::Verdict::deny(format!("{tool}: {message}")),
        message,
    )
}

/// Runs one blocking filesystem call somewhere it can be abandoned.
///
/// **Without this the deadline above it is a lie.** `std::fs::read_to_string`
/// on a path that never answers — a FIFO with no writer, a dead network mount —
/// blocks inside `poll`, so the task never yields and the timer that is meant to
/// give up on it is never polled either. Moving the syscall to a blocking thread
/// is what lets the loop drop the call and carry on.
///
/// The honest cost: the thread is not cancelled, because a blocking syscall
/// cannot be. It stays there until the kernel answers, holding one slot of
/// tokio's blocking pool. That is a leak with a bound, against a hang without
/// one.
async fn off_thread<T: Send + 'static>(work: impl FnOnce() -> T + Send + 'static) -> T {
    match tokio::task::spawn_blocking(work).await {
        Ok(done) => done,
        // The only way this fails is a panic inside the closure, which is a bug
        // in a `std::fs` call rather than something a turn can act on.
        Err(error) => std::panic::resume_unwind(error.into_panic()),
    }
}

/// Opens a checked path the way the kernel will keep inside its tree, on a
/// thread the loop can abandon.
///
/// Two rules meeting: the open must be `openat2` beneath the granting root, and
/// it must not block the poll thread
/// (`RECORD/2026-09-05.a-clock-where-there-is-no-seam.completed.md`). Both are
/// about the same syscall, so they are one helper.
async fn open_beneath(
    check: &crate::sandbox::PathCheck,
    mode: beneath::Mode,
) -> std::io::Result<beneath::Opened> {
    let Some((root, relative)) = check.beneath() else {
        return Err(std::io::Error::other("no rule granted this path"));
    };
    off_thread(move || beneath::open(&root, &relative, mode)).await
}

/// What a failed open says to the model.
///
/// A resolution refusal is a **denial** and reads as one: `EXDEV` from
/// `RESOLVE_BENEATH` means the path left the tree, and reporting it as "no such
/// file" would send a model hunting for a typo instead of telling it the
/// sandbox said no.
fn open_failed(path: &str, error: &std::io::Error) -> String {
    match beneath::is_escape(error) {
        true => format!("{path}: resolves outside the tree that granted it, and was refused"),
        false => format!("{path}: {error}"),
    }
}

pub struct ReadFile;

impl Tool for ReadFile {
    fn name(&self) -> &'static str {
        "read_file"
    }

    fn description(&self) -> &'static str {
        "Read a UTF-8 text file. Returns the text with no line numbers added."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "File to read."},
                "start_line": {"type": "integer", "description": "1-based first line. Default 1."},
                "max_lines": {
                    "type": "integer",
                    "description": "Lines to return. Default 400.",
                },
            },
            "required": ["path"],
        })
    }

    fn run<'a>(&'a self, arguments: &'a serde_json::Value, sandbox: &'a Sandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            let path = match string_arg(arguments, "path") {
                Ok(path) => path,
                Err(message) => return bad_arguments(self.name(), message),
            };
            let start = usize_arg(arguments, "start_line", 1).max(1);
            let count = usize_arg(arguments, "max_lines", DEFAULT_MAX_LINES);

            let check = sandbox.check_path(Path::new(path), Access::Read);
            if !check.verdict.allowed {
                return ToolOutcome::denied(check.verdict);
            }

            let opened = match open_beneath(&check, beneath::Mode::Read).await {
                Ok(opened) => opened,
                Err(error) => {
                    return ToolOutcome::failed(check.verdict, open_failed(path, &error));
                }
            };
            // The verdict said `in-process check` because it was written before
            // the open. Now the open has happened and, on Linux, something
            // better than our own check is what kept it inside the tree.
            let verdict = check.verdict.held_by(opened.applied);
            let mut file = opened.file;
            let read = off_thread(move || {
                let mut text = String::new();
                file.read_to_string(&mut text).map(|_| text)
            })
            .await;
            match read {
                Ok(text) => {
                    let lines: Vec<&str> = text.lines().skip(start - 1).take(count).collect();
                    ToolOutcome::ok(verdict, lines.join("\n"))
                }
                Err(error) => ToolOutcome::failed(verdict, format!("{path}: {error}")),
            }
        })
    }
}

pub struct ListDir;

impl Tool for ListDir {
    fn name(&self) -> &'static str {
        "list_dir"
    }

    fn description(&self) -> &'static str {
        "List a directory. Directories are shown with a trailing slash."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string", "description": "Directory to list. Default \".\"."},
            },
            "required": [],
        })
    }

    fn run<'a>(&'a self, arguments: &'a serde_json::Value, sandbox: &'a Sandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            let path = arguments
                .get("path")
                .and_then(serde_json::Value::as_str)
                .unwrap_or(".");

            let check = sandbox.check_path(Path::new(path), Access::Read);
            if !check.verdict.allowed {
                return ToolOutcome::denied(check.verdict);
            }

            // The whole listing off the thread, not only `read_dir`: every
            // entry's `file_type` is another syscall, and a directory of ten
            // thousand of them is ten thousand chances to block.
            //
            // **This one is still the old two-step**, and deliberately: there is
            // no stable way to read a directory from a file descriptor, so
            // `openat2` cannot be used here without `fdopendir` and a hand-
            // written iterator. What leaks through the window is a list of
            // names rather than the contents of anything, which is a smaller
            // exposure than the ones just closed and not a closed one — see
            // `RECORD/2026-09-05.beneath-the-root.completed.md` §Still open.
            let path_on_disk = check.path.clone();
            let listed = off_thread(move || {
                std::fs::read_dir(&path_on_disk).map(|entries| {
                    entries
                        .filter_map(Result::ok)
                        .map(|entry| {
                            let name = entry.file_name().to_string_lossy().into_owned();
                            match entry.file_type().is_ok_and(|kind| kind.is_dir()) {
                                true => format!("{name}/"),
                                false => name,
                            }
                        })
                        .collect::<Vec<String>>()
                })
            })
            .await;
            let mut names = match listed {
                Ok(names) => names,
                Err(error) => {
                    return ToolOutcome::failed(check.verdict, format!("{path}: {error}"));
                }
            };
            // Sorted, because `read_dir` returns whatever order the filesystem
            // felt like and a listing that reshuffles between turns is a diff
            // the model has to read as a change.
            names.sort();
            ToolOutcome::ok(check.verdict, names.join("\n"))
        })
    }
}

pub struct EditFile;

impl Tool for EditFile {
    fn name(&self) -> &'static str {
        "edit_file"
    }

    fn description(&self) -> &'static str {
        "Replace an exact, unique piece of a file. Fails if it appears zero or several times."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "old_string": {"type": "string", "description": "Exact text to replace, unique in the file."},
                "new_string": {"type": "string", "description": "What replaces it."},
            },
            "required": ["path", "old_string", "new_string"],
        })
    }

    fn run<'a>(&'a self, arguments: &'a serde_json::Value, sandbox: &'a Sandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            let (path, old, new) = match (
                string_arg(arguments, "path"),
                string_arg(arguments, "old_string"),
                string_arg(arguments, "new_string"),
            ) {
                (Ok(path), Ok(old), Ok(new)) => (path, old, new),
                (path, old, new) => {
                    let message = [path.err(), old.err(), new.err()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join("; ");
                    return bad_arguments(self.name(), message);
                }
            };

            let check = sandbox.check_path(Path::new(path), Access::ReadWrite);
            if !check.verdict.allowed {
                return ToolOutcome::denied(check.verdict);
            }

            let opened = match open_beneath(&check, beneath::Mode::Read).await {
                Ok(opened) => opened,
                Err(error) => {
                    return ToolOutcome::failed(check.verdict, open_failed(path, &error));
                }
            };
            // The read and the write are two opens, and both are resolved
            // beneath the root: an edit is not one file handle held across a
            // decision, it is two decisions the kernel makes separately.
            let verdict = check.verdict.clone().held_by(opened.applied);
            let mut file = opened.file;
            let text = match off_thread(move || {
                let mut text = String::new();
                file.read_to_string(&mut text).map(|_| text)
            })
            .await
            {
                Ok(text) => text,
                Err(error) => {
                    return ToolOutcome::failed(verdict, format!("{path}: {error}"));
                }
            };

            // Uniqueness is the safety property: a replacement that matched
            // twice would edit the one the model was not looking at, and it
            // cannot see which.
            match text.matches(old).count() {
                1 => {}
                0 => {
                    return ToolOutcome::failed(
                        verdict,
                        format!("{path}: `old_string` is not in the file"),
                    );
                }
                n => {
                    return ToolOutcome::failed(
                        verdict,
                        format!("{path}: `old_string` appears {n} times; include more context"),
                    );
                }
            }

            let replaced = text.replacen(old, new, 1);
            match open_beneath(&check, beneath::Mode::Write).await {
                Ok(opened) => {
                    let verdict = verdict.held_by(opened.applied);
                    let mut file = opened.file;
                    match off_thread(move || file.write_all(replaced.as_bytes())).await {
                        Ok(()) => {
                            ToolOutcome::ok(verdict, format!("{path}: replaced 1 occurrence"))
                        }
                        Err(error) => ToolOutcome::failed(verdict, format!("{path}: {error}")),
                    }
                }
                Err(error) => ToolOutcome::failed(verdict, open_failed(path, &error)),
            }
        })
    }
}

pub struct WriteFile;

impl Tool for WriteFile {
    fn name(&self) -> &'static str {
        "write_file"
    }

    fn description(&self) -> &'static str {
        "Create a file or replace it whole. Prefer edit_file for a change to an existing file."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "path": {"type": "string"},
                "content": {"type": "string"},
            },
            "required": ["path", "content"],
        })
    }

    fn run<'a>(&'a self, arguments: &'a serde_json::Value, sandbox: &'a Sandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            let (path, content) = match (
                string_arg(arguments, "path"),
                string_arg(arguments, "content"),
            ) {
                (Ok(path), Ok(content)) => (path, content),
                (path, content) => {
                    let message = [path.err(), content.err()]
                        .into_iter()
                        .flatten()
                        .collect::<Vec<_>>()
                        .join("; ");
                    return bad_arguments(self.name(), message);
                }
            };

            let check = sandbox.check_path(Path::new(path), Access::ReadWrite);
            if !check.verdict.allowed {
                return ToolOutcome::denied(check.verdict);
            }

            // The parent is checked in its own right: creating `a/b/c.rs` makes
            // the directories, and a grant on the file does not imply one on
            // the tree it would have to create.
            if let Some(parent) = check.path.parent()
                && !parent.exists()
            {
                let parent_check = sandbox.check_path(parent, Access::ReadWrite);
                if !parent_check.verdict.allowed {
                    return ToolOutcome::denied(parent_check.verdict);
                }
                let parent = parent.to_path_buf();
                if let Err(error) = off_thread(move || std::fs::create_dir_all(parent)).await {
                    return ToolOutcome::failed(check.verdict, format!("{path}: {error}"));
                }
            }

            let bytes = content.to_string();
            match open_beneath(&check, beneath::Mode::Write).await {
                Ok(opened) => {
                    let verdict = check.verdict.held_by(opened.applied);
                    let mut file = opened.file;
                    match off_thread(move || file.write_all(bytes.as_bytes())).await {
                        Ok(()) => ToolOutcome::ok(
                            verdict,
                            format!("{path}: wrote {} bytes", content.len()),
                        ),
                        Err(error) => ToolOutcome::failed(verdict, format!("{path}: {error}")),
                    }
                }
                Err(error) => ToolOutcome::failed(check.verdict, open_failed(path, &error)),
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::{Access, PathRule, SandboxPolicy};
    use crate::tools::{ToolCall, Tools};

    struct Fixture {
        root: std::path::PathBuf,
        sandbox: Sandbox,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "luu-tools-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("proj/src")).unwrap();
            std::fs::write(
                root.join("proj/src/main.rs"),
                "fn main() {\n    todo!()\n}\n",
            )
            .unwrap();
            std::fs::write(root.join("secret"), "shh").unwrap();
            let sandbox = Sandbox::new(
                &SandboxPolicy {
                    paths: vec![PathRule::new(".", Access::ReadWrite)],
                    ..SandboxPolicy::default()
                },
                &root.join("proj").canonicalize().unwrap(),
            )
            .unwrap();
            Self { root, sandbox }
        }

        async fn call(&self, name: &str, arguments: serde_json::Value) -> ToolOutcome {
            Tools::standard()
                .call(
                    &ToolCall {
                        name: name.into(),
                        arguments,
                    },
                    &self.sandbox,
                )
                .await
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    #[tokio::test]
    async fn read_file_returns_the_text_and_can_be_asked_for_a_range() {
        let fixture = Fixture::new("read");
        let whole = fixture
            .call("read_file", json!({"path": "src/main.rs"}))
            .await;
        assert!(whole.error.is_none(), "{whole:?}");
        assert!(whole.output.starts_with("fn main"));

        let ranged = fixture
            .call(
                "read_file",
                json!({"path": "src/main.rs", "start_line": 2, "max_lines": 1}),
            )
            .await;
        assert_eq!(ranged.output, "    todo!()");
    }

    /// A symlink out of the tree, standing still.
    ///
    /// **Level one catches this one**, and the assertion says so: `check_path`
    /// canonicalizes, sees where the link lands, and denies before anything is
    /// opened. What it cannot catch is the same link appearing *after* that
    /// answer and before the open — the window `openat2` closes, tested where
    /// it is closed (`sandbox::beneath`), because a race cannot be staged from
    /// up here without a hook in the middle of the tool.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_symlink_out_of_the_tree_reads_nothing_whichever_layer_refuses() {
        let fixture = Fixture::new("beneath");
        // The sandbox grants `proj` and nothing else; `secret` is its sibling,
        // and the link lives inside the tree and points at it.
        std::os::unix::fs::symlink(
            fixture.root.join("secret"),
            fixture.root.join("proj/way-out"),
        )
        .unwrap();

        let outcome = fixture.call("read_file", json!({"path": "way-out"})).await;

        assert!(!outcome.verdict.allowed, "{outcome:?}");
        assert!(
            !outcome.output.contains("shh"),
            "the file outside the tree was read: {outcome:?}",
        );
    }

    /// And the other half of the same rule: a symlink that stays *inside* the
    /// tree is ordinary and still works. `RESOLVE_BENEATH` is about where a
    /// path lands, not about how it got there — `RESOLVE_NO_SYMLINKS` would
    /// have broken every checkout that has one.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn a_symlink_that_stays_inside_the_tree_still_reads() {
        let fixture = Fixture::new("beneath-ok");
        std::os::unix::fs::symlink("main.rs", fixture.root.join("proj/src/alias.rs")).unwrap();

        let outcome = fixture
            .call("read_file", json!({"path": "src/alias.rs"}))
            .await;
        assert!(outcome.verdict.allowed, "{outcome:?}");
        assert!(outcome.output.contains("fn main"), "{outcome:?}");
    }

    /// Who held it, said rather than assumed.
    ///
    /// An in-process read was `Applied::Process` — *our own check, before the
    /// fact, all an in-process tool can have* — since the sandbox existed. On a
    /// kernel with `openat2` that sentence is no longer true, and the verdict is
    /// where that has to show up: two machines running one policy get two
    /// guarantees, and a recording has to say which one this was.
    #[cfg(target_os = "linux")]
    #[tokio::test]
    async fn the_verdict_says_the_kernel_held_the_read() {
        let fixture = Fixture::new("beneath-verdict");
        let outcome = fixture
            .call("read_file", json!({"path": "src/main.rs"}))
            .await;
        assert!(outcome.verdict.allowed);
        assert_eq!(
            outcome.verdict.enforced_by,
            crate::sandbox::Applied::Kernel {
                how: crate::sandbox::beneath::HOW.to_string(),
            },
            "an in-process read is held by the kernel now, and says so",
        );
    }

    #[tokio::test]
    async fn reading_outside_the_sandbox_is_denied_and_names_the_rule() {
        let fixture = Fixture::new("readout");
        let outcome = fixture
            .call("read_file", json!({"path": "../secret"}))
            .await;
        assert!(!outcome.verdict.allowed);
        assert!(outcome.output.is_empty(), "a denial returns no content");
        assert!(outcome.error.unwrap().contains("no rule grants read"));
    }

    #[tokio::test]
    async fn list_dir_is_sorted_and_marks_directories() {
        let fixture = Fixture::new("list");
        let outcome = fixture.call("list_dir", json!({})).await;
        assert_eq!(outcome.output, "src/");
    }

    #[tokio::test]
    async fn edit_file_replaces_a_unique_occurrence() {
        let fixture = Fixture::new("edit");
        let outcome = fixture
            .call(
                "edit_file",
                json!({"path": "src/main.rs", "old_string": "todo!()", "new_string": "println!(\"hola\")"}),
            )
            .await;
        assert!(outcome.error.is_none(), "{outcome:?}");
        let text = std::fs::read_to_string(fixture.root.join("proj/src/main.rs")).unwrap();
        assert!(text.contains("println!(\"hola\")"));
    }

    #[tokio::test]
    async fn edit_file_refuses_an_ambiguous_replacement() {
        let fixture = Fixture::new("editdup");
        std::fs::write(fixture.root.join("proj/src/main.rs"), "a\na\n").unwrap();
        let outcome = fixture
            .call(
                "edit_file",
                json!({"path": "src/main.rs", "old_string": "a", "new_string": "b"}),
            )
            .await;
        // Replacing the first of several edits the one the model was not
        // looking at, and it cannot see which.
        assert!(outcome.error.unwrap().contains("appears 2 times"));
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("proj/src/main.rs")).unwrap(),
            "a\na\n",
            "a refused edit changes nothing"
        );
    }

    #[tokio::test]
    async fn write_file_creates_the_directories_it_needs() {
        let fixture = Fixture::new("write");
        let outcome = fixture
            .call(
                "write_file",
                json!({"path": "a/b/new.rs", "content": "// hi\n"}),
            )
            .await;
        assert!(outcome.error.is_none(), "{outcome:?}");
        assert_eq!(
            std::fs::read_to_string(fixture.root.join("proj/a/b/new.rs")).unwrap(),
            "// hi\n"
        );
    }

    #[tokio::test]
    async fn writing_outside_the_sandbox_is_denied_before_anything_is_created() {
        let fixture = Fixture::new("writeout");
        let outcome = fixture
            .call(
                "write_file",
                json!({"path": "../escaped/file", "content": "x"}),
            )
            .await;
        assert!(!outcome.verdict.allowed);
        assert!(!fixture.root.join("escaped").exists());
    }

    #[tokio::test]
    async fn a_read_only_grant_stops_a_write_but_not_a_read() {
        let fixture = Fixture::new("readonly");
        let sandbox = Sandbox::new(
            &SandboxPolicy {
                paths: vec![PathRule::new(".", Access::Read)],
                ..SandboxPolicy::default()
            },
            &fixture.root.join("proj").canonicalize().unwrap(),
        )
        .unwrap();
        let tools = Tools::standard();

        let read = tools
            .call(
                &ToolCall {
                    name: "read_file".into(),
                    arguments: json!({"path": "src/main.rs"}),
                },
                &sandbox,
            )
            .await;
        assert!(read.verdict.allowed);

        let write = tools
            .call(
                &ToolCall {
                    name: "edit_file".into(),
                    arguments: json!({"path": "src/main.rs", "old_string": "todo!()", "new_string": "x"}),
                },
                &sandbox,
            )
            .await;
        assert!(!write.verdict.allowed, "{:?}", write.verdict);
    }

    #[tokio::test]
    async fn a_missing_argument_is_reported_without_claiming_a_rule_allowed_it() {
        let fixture = Fixture::new("badargs");
        let outcome = fixture.call("read_file", json!({})).await;
        assert!(!outcome.verdict.allowed);
        assert!(outcome.error.unwrap().contains("`path` is required"));
    }

    #[tokio::test]
    async fn an_unknown_tool_says_what_there_is_instead() {
        let fixture = Fixture::new("unknown");
        let outcome = fixture.call("delete_everything", json!({})).await;
        assert!(!outcome.verdict.allowed);
        assert!(outcome.verdict.rule.contains("read_file"));
    }
}
