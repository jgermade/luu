//! What crosses a border, and what the crossing does to it.
//!
//! A session moves between hosts as **its own record stream** plus an envelope,
//! never as a snapshot of the fold. That is the first collision in
//! `RECORD/2026-08-31.the-portal-and-the-gate.completed.md` and the reason is
//! not taste: the stream is checked against the fold by a test that runs the
//! binary, so a transfer built on it inherits that check, and one built on a
//! rendering of it needs its own and will not get one.
//!
//! ```text
//! sess-2026-09-04/
//!   manifest.json     what folding the stream cannot answer
//!   record.jsonl      the stream, verbatim
//! ```
//!
//! Nothing here reads or writes a file: this module is the *format* and the
//! border rule. Where the bundle lives, and which store it comes out of, is the
//! surface's business — see `luu::transfer`.
//!
//! Argued in `RECORD/2026-09-04.the-border-and-the-gate.completed.md`.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::SessionView;
use crate::job::{JobId, JobState};
use crate::sandbox::{Access, Enforcement, Limits, Sandbox};

/// The first field of every manifest, and the first thing a reader checks. A
/// bundle that does not say this is not one, whatever its file names are.
pub const KIND: &str = "luu-transfer v1";

/// What the bundle's two files are called. Named here rather than at the call
/// sites so the writer and the reader cannot disagree about them.
pub const MANIFEST_FILE: &str = "manifest.json";
pub const RECORD_FILE: &str = "record.jsonl";

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum TransferError {
    #[error("this is not a luu transfer bundle: it says `{0}` where `{KIND}` was expected")]
    Kind(String),
    /// The same rule the socket handshake applies, applied to a file: a
    /// mismatch in either direction, because a newer bundle is the case this
    /// host cannot parse and an older one is the case it cannot repair.
    #[error("this host speaks protocol {ours} and the bundle was written by one speaking {theirs}")]
    Protocol { ours: u32, theirs: u32 },
    #[error("this host reads record format {ours} and the bundle carries {theirs}")]
    Format { ours: u32, theirs: u32 },
}

/// One granted tree of the origin's sandbox, as it resolved there.
///
/// Canonical paths are facts about the machine that resolved them — which is
/// exactly why they travel: the person at the destination gate is being asked
/// to judge the difference between two machines, and a policy file's *words*
/// are the half that is identical on both.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedRoot {
    pub path: PathBuf,
    pub access: Access,
    /// As it was written in the policy file, beside what it turned into.
    pub source: String,
    /// True for the roots the sandbox adds itself, so a reader can tell a grant
    /// somebody typed from one a command's interpreter needed.
    pub implicit: bool,
}

/// The origin's sandbox, resolved.
///
/// **Resolved when the transfer was written, not when the job ran.** The stream
/// carries a `Verdict` per tool call and never the sandbox itself, so this is
/// what the origin's policy file resolves to *now*. The plans in the stream are
/// exact and are what the destination's gate re-checks; this is the outer bound
/// they sat inside, for a person to read.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ResolvedSandbox {
    pub base: PathBuf,
    pub roots: Vec<ResolvedRoot>,
    pub commands: Vec<String>,
    pub network: bool,
    #[serde(default)]
    pub egress: Vec<String>,
    pub enforcement: Enforcement,
    pub limits: Limits,
}

impl From<&Sandbox> for ResolvedSandbox {
    fn from(sandbox: &Sandbox) -> Self {
        Self {
            base: sandbox.base().to_path_buf(),
            roots: sandbox
                .roots()
                .iter()
                .map(|root| ResolvedRoot {
                    path: root.path.clone(),
                    access: root.access,
                    source: root.source.clone(),
                    implicit: root.implicit,
                })
                .collect(),
            commands: sandbox.commands().to_vec(),
            network: sandbox.network(),
            egress: sandbox.egress().to_vec(),
            enforcement: sandbox.enforcement(),
            limits: sandbox.limits(),
        }
    }
}

impl ResolvedSandbox {
    /// One line per grant, the implicit ones included — the same shape
    /// `luu tools` prints, so the two sandboxes a person is comparing are
    /// described in the same words.
    pub fn describe(&self) -> String {
        let mut text = format!("base {}\n", self.base.display());
        for root in &self.roots {
            text.push_str(&format!(
                "  {:<10} {}{}\n",
                root.access.as_str(),
                root.path.display(),
                match root.implicit {
                    true => "   (implicit)",
                    false => "",
                }
            ));
        }
        text.push_str(&format!(
            "  commands   {}\n  network    {}\n  enforce    {}\n",
            match self.commands.is_empty() {
                true => "(none)".to_string(),
                false => self.commands.join(", "),
            },
            match self.network {
                true => "allowed",
                false => "denied",
            },
            self.enforcement.as_str(),
        ));
        if !self.egress.is_empty() {
            text.push_str(&format!("  egress     {}\n", self.egress.join(", ")));
        }
        if let Some(limits) = self.limits.describe() {
            text.push_str(&format!("  limits     {limits}\n"));
        }
        text
    }
}

/// Where a session came from. Travels in the manifest *and* on the wire, as one
/// type: the envelope says what arrived and the `imported` line says what this
/// host did with it, and a reader that had to reconcile two spellings of
/// "where from" would be reconciling two truths.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Origin {
    /// The machine's name as it calls itself. Provenance for a person, never an
    /// identity anything checks — the signature on an approval is that.
    pub host: String,
    /// What the session was called over there. Kept even when the import
    /// renames it, because an id is how the origin's own record refers to it.
    pub session: String,
    pub sandbox: ResolvedSandbox,
}

/// The bundle's envelope: what folding the stream cannot answer, and nothing
/// else. Anything a fold produces is left out on purpose — two spellings of the
/// same fact are two facts the day they disagree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    pub kind: String,
    pub protocol: u32,
    pub format: u32,
    /// Unix milliseconds, and the one timestamp here that is about the *move*
    /// rather than about the session.
    pub created_at: u64,
    pub origin: Origin,
}

impl Manifest {
    pub fn new(origin: Origin, created_at: u64) -> Self {
        Self {
            kind: KIND.to_string(),
            protocol: crate::protocol::VERSION,
            format: crate::record::FORMAT,
            created_at,
            origin,
        }
    }

    /// Whether this host may read it — checked before the stream is parsed,
    /// which is the whole point of an envelope. Out loud on a mismatch, in
    /// either direction, rather than by misreading the first line.
    pub fn check(&self) -> Result<(), TransferError> {
        if self.kind != KIND {
            return Err(TransferError::Kind(self.kind.clone()));
        }
        if self.protocol != crate::protocol::VERSION {
            return Err(TransferError::Protocol {
                ours: crate::protocol::VERSION,
                theirs: self.protocol,
            });
        }
        if self.format != crate::record::FORMAT {
            return Err(TransferError::Format {
                ours: crate::record::FORMAT,
                theirs: self.format,
            });
        }
        Ok(())
    }
}

/// What the border did to one job.
///
/// On the wire and in the fold, because the alternative is an importer that
/// edits the fold behind the stream that produced it — which is the second
/// truth this whole design is arranged to avoid.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImportedJob {
    pub job: JobId,
    /// What it is on this host now: `Proposed` for anything that has to be
    /// approved again, `Rejected` for a plan this host does not grant.
    pub state: JobState,
    /// Why it was refused, in the words `Plan::unmet` uses at the local gate.
    /// Empty for a job that crossed to the gate.
    #[serde(default)]
    pub unmet: Vec<String>,
}

/// What the border does, given what arrived and what this host grants.
///
/// Three rules, and each one is a case that would otherwise go wrong:
///
/// - **`Closed` jobs cross unchanged.** They run nothing — they are the summary
///   their turns are sent as — so re-gating them would be asking a person to
///   approve a paragraph.
/// - **Everything else returns to the gate**, whatever it was over there. An
///   approval is a statement about resolved paths on one tree, and `src/auth.rs`
///   on the origin and `src/auth.rs` here are two files.
/// - **A plan this host does not grant crosses refused**, carrying the lines
///   that refused it. A proposal nobody can approve is worse than a refusal:
///   it looks like work waiting on a person.
///
/// `Rejected` jobs on the origin are left alone: nothing ran under them and
/// nothing will, and turning a refusal into a proposal would re-ask a question
/// somebody already answered.
pub fn regate(view: &SessionView, sandbox: &Sandbox) -> Vec<ImportedJob> {
    view.jobs
        .iter()
        .filter(|job| matches!(job.state, JobState::Proposed | JobState::Approved))
        .map(|job| {
            let unmet = job.plan.unmet(sandbox);
            ImportedJob {
                job: job.id,
                state: match unmet.is_empty() {
                    true => JobState::Proposed,
                    false => JobState::Rejected,
                },
                unmet,
            }
        })
        .collect()
}

/// This machine's name, for [`Origin::host`]. `hostname` is not portable enough
/// to shell out to and not important enough to take a dependency for: the
/// environment answers on every platform this runs on, and `unknown` is a
/// truthful answer where it does not.
pub fn hostname() -> String {
    for key in ["HOSTNAME", "HOST", "COMPUTERNAME"] {
        if let Ok(name) = std::env::var(key)
            && !name.trim().is_empty()
        {
            return name;
        }
    }
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| "unknown".to_string())
}

/// Where the two files of a bundle live under one directory.
pub fn manifest_path(bundle: &Path) -> PathBuf {
    bundle.join(MANIFEST_FILE)
}

pub fn record_path(bundle: &Path) -> PathBuf {
    bundle.join(RECORD_FILE)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::Plan;
    use crate::sandbox::{PathRule, SandboxPolicy};

    fn sandbox(base: &Path) -> Sandbox {
        let policy = SandboxPolicy {
            paths: vec![PathRule::new(base, Access::ReadWrite)],
            commands: vec!["cargo".into()],
            ..SandboxPolicy::default()
        };
        Sandbox::new(&policy, base).expect("a sandbox")
    }

    fn origin(sandbox: &Sandbox) -> Origin {
        Origin {
            host: "origin".into(),
            session: "session-1".into(),
            sandbox: ResolvedSandbox::from(sandbox),
        }
    }

    #[test]
    fn a_manifest_is_refused_before_the_stream_is_read() {
        let base = std::env::temp_dir();
        let sandbox = sandbox(&base);
        let mut manifest = Manifest::new(origin(&sandbox), 1);
        assert_eq!(manifest.check(), Ok(()));

        manifest.protocol = crate::protocol::VERSION + 1;
        assert_eq!(
            manifest.check(),
            Err(TransferError::Protocol {
                ours: crate::protocol::VERSION,
                theirs: crate::protocol::VERSION + 1,
            }),
            "a newer bundle is the case this host cannot parse, and it says so",
        );

        let mut manifest = Manifest::new(origin(&sandbox), 1);
        manifest.format = crate::record::FORMAT - 1;
        assert!(matches!(
            manifest.check(),
            Err(TransferError::Format { .. })
        ));

        let mut manifest = Manifest::new(origin(&sandbox), 1);
        manifest.kind = "loude-transfer v1".into();
        assert!(matches!(manifest.check(), Err(TransferError::Kind(_))));
    }

    #[test]
    fn the_manifest_round_trips_through_json() {
        let base = std::env::temp_dir();
        let sandbox = sandbox(&base);
        let manifest = Manifest::new(origin(&sandbox), 1_757_000_000_000);
        let json = serde_json::to_string(&manifest).expect("writing");
        let back: Manifest = serde_json::from_str(&json).expect("reading");
        assert_eq!(back, manifest);
        assert!(
            back.origin.sandbox.commands.contains(&"cargo".to_string()),
            "the origin's commands travel: they are half of what a person is judging",
        );
    }

    #[test]
    fn an_approved_job_returns_to_the_gate_and_an_unreachable_plan_is_refused() {
        let base = std::env::temp_dir().join("luu-regate");
        std::fs::create_dir_all(&base).expect("a base");
        let sandbox = sandbox(&base);

        let mut view = SessionView::new("s", "mock", "mock");
        view.apply_protocol(
            0,
            &crate::protocol::ServerMessage::JobProposed {
                job: 1,
                objective: "the one that crosses".into(),
                plan: Plan {
                    files: vec![".".into()],
                    commands: vec!["cargo".into()],
                    ..Plan::default()
                },
                source: None,
            },
        );
        view.apply_protocol(
            1,
            &crate::protocol::ServerMessage::JobApproved {
                job: 1,
                plan: Plan {
                    files: vec![".".into()],
                    commands: vec!["cargo".into()],
                    ..Plan::default()
                },
                approved_by: None,
            },
        );
        view.apply_protocol(
            2,
            &crate::protocol::ServerMessage::JobProposed {
                job: 2,
                objective: "the one that cannot".into(),
                plan: Plan {
                    writes: vec!["/etc/hosts".into()],
                    ..Plan::default()
                },
                source: None,
            },
        );
        view.apply_protocol(
            3,
            &crate::protocol::ServerMessage::JobClosed {
                job: 3,
                summary: "nothing".into(),
                by: None,
            },
        );

        let regated = regate(&view, &sandbox);
        assert_eq!(
            regated.len(),
            2,
            "a job that does not exist is not invented"
        );
        assert_eq!(
            regated[0],
            ImportedJob {
                job: 1,
                state: JobState::Proposed,
                unmet: Vec::new(),
            },
            "approved over there is proposed over here",
        );
        assert_eq!(regated[1].state, JobState::Rejected);
        assert!(
            regated[1].unmet[0].starts_with("write /etc/hosts:"),
            "refused in the words the local gate uses: {:?}",
            regated[1].unmet,
        );
    }
}
