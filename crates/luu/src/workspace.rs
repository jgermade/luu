//! The workspace, as the person at the browser looks at it: a file tree, what
//! git says changed, and the contents of one file or one diff.
//!
//! **Read-only, and outside the job gate on purpose.** Every other way this
//! program touches the filesystem is a *model's* action — `read_file` and
//! `run_command` are tools, and a tool call is held until somebody approves the
//! job that proposed it. Nothing here is proposed by anything: a person clicked
//! a directory. Routing that through the approval flow would mean either
//! rubber-stamping a job nobody asked the model to make, or building a second
//! read path that skips the gate — and then there are two ways to read a file
//! and only one of them is audited. The read side of the session API
//! (`/api/sessions/*`) already answers plain ungated `GET`s for the same
//! reason. See `RECORD/2026-09-15.a-three-pane-inspector.WIP.md`.
//!
//! What still bounds it is the sandbox: every path is resolved through
//! [`Sandbox::check_path`], so this surface can reach exactly what the session's
//! own policy grants and nothing above it.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;

use agent_core::sandbox::{Access, Sandbox};
use serde::Serialize;
use tokio::process::Command;

/// How long any one `git` call may take before it is abandoned. A repository
/// on a dead network mount is the case this exists for: the panel showing
/// "could not read git" beats the page hanging on a click.
const GIT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// One entry in a directory listing.
#[derive(Debug, Clone, Serialize)]
pub struct Entry {
    /// Relative to the sandbox base, `/`-separated, and the id the page sends
    /// back to open it. Never absolute: an absolute path in a URL is an
    /// invitation to send a different one.
    pub path: String,
    pub name: String,
    pub dir: bool,
    /// Whether git ignores it. Shown dimmed rather than hidden: a person
    /// looking for `target/` wants to see that it is there and ignored, not to
    /// wonder whether the tree is broken.
    pub ignored: bool,
    /// Git's own two-letter status, when it has one — `M`, `??`, `A`, `D`.
    /// `None` is unchanged, which is most of a tree.
    pub status: Option<String>,
}

/// One directory's children.
#[derive(Debug, Serialize)]
pub struct Tree {
    /// What was listed, relative to the base. Empty string is the base itself.
    pub path: String,
    pub entries: Vec<Entry>,
    /// What went wrong asking git, when something did. The tree still lists —
    /// a workspace that is not a git repository at all is an ordinary case,
    /// not an error, and it renders as a tree with nothing marked.
    pub git: Option<String>,
}

/// A file, read for display.
#[derive(Debug, Serialize)]
pub struct FileView {
    pub path: String,
    pub text: String,
    /// True when the file was cut at [`MAX_FILE_BYTES`]. The page says so
    /// rather than showing a truncated file as if it were whole.
    pub truncated: bool,
}

/// The cap on a file the viewer will render. A debug UI has no business
/// streaming a gigabyte into a browser, and the number is the same order as
/// the tool output cap the agent already lives under.
pub const MAX_FILE_BYTES: usize = 512 * 1024;

/// One line of a diff, already classified so the browser renders structure
/// rather than parsing text.
#[derive(Debug, Serialize)]
pub struct DiffLine {
    /// `add`, `del`, or `ctx`.
    pub kind: &'static str,
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct Hunk {
    /// Git's own `@@ … @@` header, kept verbatim: it is the one line here a
    /// person may want to read as git wrote it.
    pub header: String,
    pub lines: Vec<DiffLine>,
}

#[derive(Debug, Serialize)]
pub struct Diff {
    pub path: String,
    /// Staged against `HEAD` when `staged`, working tree against the index
    /// otherwise.
    pub staged: bool,
    pub hunks: Vec<Hunk>,
    /// Set when git answered but there was nothing to show — a file that is
    /// untracked has no diff, and saying so beats an empty panel.
    pub note: Option<String>,
}

/// Why a request could not be answered. Kept as one type because every one of
/// these becomes the same shape of HTTP error, and the caller should not have
/// to remember which variant maps to which status.
#[derive(Debug)]
pub enum Error {
    /// The path escaped the sandbox, or the sandbox does not grant it.
    Refused(String),
    /// It is not there.
    Missing(String),
    /// Something under it failed — a read, a spawn, a decode.
    Failed(String),
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Refused(why) | Error::Missing(why) | Error::Failed(why) => write!(f, "{why}"),
        }
    }
}

/// Resolves a caller-supplied relative path against the base, through the
/// sandbox.
///
/// The sandbox is what makes this safe rather than any string handling here:
/// `check_path` resolves symlinks and `..` and answers against the policy's
/// roots, which is the same check `read_file` goes through.
fn resolve(sandbox: &Sandbox, relative: &str) -> Result<PathBuf, Error> {
    let relative = relative.trim_start_matches('/');
    let joined = match relative.is_empty() {
        true => sandbox.base().to_path_buf(),
        false => sandbox.base().join(relative),
    };
    let check = sandbox.check_path(&joined, Access::Read);
    if !check.verdict.allowed {
        return Err(Error::Refused(format!(
            "{relative}: {}",
            check.verdict.rule
        )));
    }
    Ok(check.path)
}

/// The path an entry is known by: relative to the base, `/`-separated.
fn relative_of(base: &Path, path: &Path) -> String {
    path.strip_prefix(base)
        .unwrap_or(path)
        .components()
        .map(|part| part.as_os_str().to_string_lossy().into_owned())
        .collect::<Vec<_>>()
        .join("/")
}

/// One directory's children, with git's opinion of each attached.
///
/// Ignored entries are found by difference rather than by a second matcher:
/// `ignore`'s walker answers "what is *not* ignored" for this directory, and
/// `read_dir` answers "what is there". Everything in the second and not the
/// first is ignored — which gets nested `.gitignore` files, negations and the
/// global config right for free, because the walker already implements all of
/// that and a hand-rolled matcher on the root file alone would not.
pub async fn tree(sandbox: &Sandbox, relative: &str) -> Result<Tree, Error> {
    let dir = resolve(sandbox, relative)?;
    if !dir.is_dir() {
        return Err(Error::Missing(format!("{relative}: not a directory")));
    }

    let mut listed: Vec<(String, bool, PathBuf)> = Vec::new();
    let mut reader = tokio::fs::read_dir(&dir)
        .await
        .map_err(|error| Error::Failed(format!("{relative}: {error}")))?;
    while let Some(item) = reader
        .next_entry()
        .await
        .map_err(|error| Error::Failed(format!("{relative}: {error}")))?
    {
        let name = item.file_name().to_string_lossy().into_owned();
        // `.git` is machinery, not content. It is the one directory whose
        // contents nobody opens this panel to read, and it is enormous.
        if name == ".git" {
            continue;
        }
        let is_dir = item
            .file_type()
            .await
            .map(|kind| kind.is_dir())
            .unwrap_or(false);
        listed.push((name, is_dir, item.path()));
    }

    let unignored = not_ignored(&dir);
    // A workspace that is not a git repository still has a tree worth listing,
    // so git failing is a note on the answer rather than an error instead of
    // one: nothing is marked, and the panel says why.
    let (statuses, git_error) = match git_status(sandbox.base()).await {
        Ok(map) => (map, None),
        Err(error) => (BTreeMap::new(), Some(error.to_string())),
    };

    let mut entries: Vec<Entry> = listed
        .into_iter()
        .map(|(name, dir_flag, full)| {
            let path = relative_of(sandbox.base(), &full);
            // A directory carries the loudest status under it, so a collapsed
            // tree still shows that something inside changed.
            let status = match dir_flag {
                false => statuses.get(&path).cloned(),
                true => {
                    let prefix = format!("{path}/");
                    statuses
                        .iter()
                        .find(|(key, _)| key.starts_with(&prefix))
                        .map(|(_, value)| value.clone())
                }
            };
            Entry {
                ignored: !unignored.contains(&name),
                path,
                name,
                dir: dir_flag,
                status,
            }
        })
        .collect();

    // Directories first, then by name: the order a file tree is read in.
    entries.sort_by(|a, b| b.dir.cmp(&a.dir).then_with(|| a.name.cmp(&b.name)));

    Ok(Tree {
        path: relative.trim_start_matches('/').to_string(),
        entries,
        git: git_error,
    })
}

/// The names in `dir` that git does *not* ignore.
///
/// Depth 1 and `parents(true)`: the walker has to read `.gitignore` files above
/// this directory to answer correctly for a subdirectory, and must not descend
/// below it.
fn not_ignored(dir: &Path) -> std::collections::HashSet<String> {
    ignore::WalkBuilder::new(dir)
        .max_depth(Some(1))
        .parents(true)
        .hidden(false)
        .git_ignore(true)
        .git_global(true)
        .git_exclude(true)
        .build()
        .filter_map(Result::ok)
        // The walker yields the root itself first; it is not one of its own
        // children.
        .filter(|entry| entry.path() != dir)
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect()
}

/// Every path git considers changed, mapped to its two-letter status.
///
/// `-z` because a filename may contain anything, newlines and quotes included,
/// and `--porcelain`'s default output escapes those in a format that then has
/// to be un-escaped. The NUL-separated form is the one that needs no parsing
/// of quoting rules.
pub async fn git_status(base: &Path) -> Result<BTreeMap<String, String>, Error> {
    let out = git(base, &["status", "--porcelain=v1", "-z"]).await?;
    let mut map = BTreeMap::new();
    let mut fields = out.split('\0').filter(|part| !part.is_empty());
    while let Some(record) = fields.next() {
        // `XY <path>`, and for a rename `XY <to>\0<from>` — the extra field is
        // consumed here so it is not read as the next record.
        //
        // **Both characters, untrimmed.** X is the index against `HEAD` and Y
        // the working tree against the index, so ` M` and `M ` are opposite
        // facts and the space carries which one this is. Trimming it made
        // every unstaged change look staged, and the diff panel asked git for
        // the staged side and got nothing.
        let (code, path) = match record.len() > 3 {
            true => (record[..2].to_string(), record[3..].to_string()),
            false => continue,
        };
        if code.starts_with('R') || code.starts_with('C') {
            let _ = fields.next();
        }
        map.insert(path, code);
    }
    Ok(map)
}

/// A file, for the middle column.
pub async fn file(sandbox: &Sandbox, relative: &str) -> Result<FileView, Error> {
    let path = resolve(sandbox, relative)?;
    if path.is_dir() {
        return Err(Error::Missing(format!("{relative}: is a directory")));
    }
    let bytes = tokio::fs::read(&path)
        .await
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => Error::Missing(format!("{relative}: not found")),
            _ => Error::Failed(format!("{relative}: {error}")),
        })?;
    let truncated = bytes.len() > MAX_FILE_BYTES;
    let bytes = match truncated {
        true => &bytes[..MAX_FILE_BYTES],
        false => &bytes[..],
    };
    // Lossy rather than refused: a file with one bad byte in it is still worth
    // looking at, and the alternative is a panel that says nothing about a file
    // the tree says exists.
    Ok(FileView {
        path: relative.trim_start_matches('/').to_string(),
        text: String::from_utf8_lossy(bytes).into_owned(),
        truncated,
    })
}

/// One file's diff, parsed into hunks.
///
/// Parsed here rather than shipped as text, which is the call
/// `luu-design.md` already made for prompt diffs: "compute the diff in Rust and
/// send resolved spans". Git already produces a unified diff, so this parses
/// that rather than re-diffing the blobs — same destination, no second diff
/// implementation and no new dependency.
pub async fn diff(sandbox: &Sandbox, relative: &str, staged: bool) -> Result<Diff, Error> {
    // Resolved for the same reason a read is: a path that the sandbox refuses
    // must not become a `git diff` argument either.
    resolve(sandbox, relative)?;
    let path = relative.trim_start_matches('/');
    let mut args = vec!["diff", "--no-color", "--no-ext-diff"];
    if staged {
        args.push("--cached");
    }
    args.push("--");
    args.push(path);
    let out = git(sandbox.base(), &args).await?;

    let mut hunks: Vec<Hunk> = Vec::new();
    for line in out.lines() {
        if line.starts_with("@@") {
            hunks.push(Hunk {
                header: line.to_string(),
                lines: Vec::new(),
            });
            continue;
        }
        let Some(hunk) = hunks.last_mut() else {
            // Everything before the first `@@` is the file header git writes
            // (`diff --git`, `index`, `---`, `+++`), which the page already
            // knows: it asked about this path.
            continue;
        };
        let (kind, text) = match line.as_bytes().first() {
            Some(b'+') => ("add", &line[1..]),
            Some(b'-') => ("del", &line[1..]),
            Some(b'\\') => continue, // "\ No newline at end of file"
            _ => ("ctx", line.get(1..).unwrap_or("")),
        };
        hunk.lines.push(DiffLine {
            kind,
            text: text.to_string(),
        });
    }

    let note = match hunks.is_empty() {
        true => Some(match staged {
            true => "Nothing staged for this file.".to_string(),
            false => {
                "No unstaged changes — the file may be untracked, or already staged.".to_string()
            }
        }),
        false => None,
    };

    Ok(Diff {
        path: path.to_string(),
        staged,
        hunks,
        note,
    })
}

/// Runs git in the workspace and hands back its stdout.
///
/// **Not through the tool sandbox**, and that is the one thing worth being
/// explicit about: this is the server reading its own working directory on
/// behalf of the person looking at it, not a model running a command. It is
/// also why the `/dev/null` and `~/.gitconfig` findings in
/// `RECORD/2026-09-15.git-could-not-open-dev-null.completed.md` do not reach
/// it — a child here inherits the server's own environment.
async fn git(base: &Path, args: &[&str]) -> Result<String, Error> {
    let run = Command::new("git")
        .arg("-C")
        .arg(base)
        .args(args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .output();

    let out = tokio::time::timeout(GIT_TIMEOUT, run)
        .await
        .map_err(|_| Error::Failed("git did not answer in ten seconds".to_string()))?
        .map_err(|error| Error::Failed(format!("could not run git: {error}")))?;

    if !out.status.success() {
        let why = String::from_utf8_lossy(&out.stderr);
        let why = why.trim();
        return Err(Error::Failed(match why.is_empty() {
            true => format!("git exited with {}", out.status),
            false => why.to_string(),
        }));
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The rename record eats its second field. Written because the loop that
    /// does it is the one piece of parsing here that a reader cannot check by
    /// eye: `-z` output has no line breaks to count.
    #[test]
    fn a_rename_does_not_leave_its_old_name_as_a_record() {
        // `R  new\0old\0 M other\0` — one rename and one modification.
        let raw = "R  new\0old\0 M other\0";
        let mut map = BTreeMap::new();
        let mut fields = raw.split('\0').filter(|part| !part.is_empty());
        while let Some(record) = fields.next() {
            let (code, path) = match record.len() > 3 {
                true => (record[..2].to_string(), record[3..].to_string()),
                false => continue,
            };
            if code.starts_with('R') || code.starts_with('C') {
                let _ = fields.next();
            }
            map.insert(path, code);
        }
        // Both positions kept: "R " is a rename staged in the index, where
        // " R" would be one in the working tree, and the space is the only
        // thing that says which.
        assert_eq!(map.get("new").map(String::as_str), Some("R "));
        assert_eq!(map.get("other").map(String::as_str), Some(" M"));
        assert!(
            !map.contains_key("old"),
            "the old name is not its own entry"
        );
    }
}
