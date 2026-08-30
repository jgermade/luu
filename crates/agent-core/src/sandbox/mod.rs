//! What the agent may reach, and who is holding it to that.
//!
//! Three rungs, of which this module builds the first two
//! (`RECORD/2026-08-27.tools-and-sandbox.md`):
//!
//! 1. **In-process checks.** Canonicalize, compare against the policy, refuse.
//!    Everything an in-process tool can have, and nothing a subprocess gets:
//!    the check happens before the syscall, in a program that then makes the
//!    syscall itself. A child makes its own.
//! 2. **The kernel, same process tree, no image and no daemon.** Landlock for
//!    the filesystem and seccomp for sockets, applied to the child between
//!    `fork` and `exec`. Linux-only; see [`Enforcement`] for what happens
//!    elsewhere.
//! 3. **A container.** Later, and on top of this rather than instead of it —
//!    Landlock survives `exec` and cannot be dropped.
//!
//! Nothing here may say "sandboxed" without saying by what. Every verdict
//! carries [`Applied`], because a run whose subprocesses the kernel held and a
//! run whose subprocesses nothing held are not the same run, and afterwards the
//! recording is the only thing that could tell them apart.

use std::path::{Path, PathBuf};

use crate::task::TaskId;

use serde::{Deserialize, Serialize};

pub mod policy;

pub use policy::{Access, Enforcement, PathRule, PolicyError, SandboxPolicy};

#[cfg_attr(target_os = "linux", path = "linux.rs")]
#[cfg_attr(not(target_os = "linux"), path = "fallback.rs")]
mod kernel;

/// Read and execute, never write, granted **to a subprocess** whenever any
/// command is allowed — a program cannot run without reading its own
/// interpreter and libc.
///
/// Subprocess only, and that is the whole point of the distinction: the
/// justification is "a command cannot read libc", which says nothing about
/// `read_file`. Allowing `cargo` must not quietly let the agent read `/etc`
/// itself, so [`Sandbox::check_path`] ignores these and only the Landlock
/// ruleset handed to the child carries them.
///
/// Fixed, in the code, and listed by `luu tools`, so it is a grant a reader can
/// see rather than one they have to infer. Everything else a command needs is
/// the user's to name: `cargo` wants `~/.cargo`, and having to write that line
/// is the design's one informed approval.
pub const SYSTEM_ROOTS: &[&str] = &["/usr", "/bin", "/sbin", "/lib", "/lib64", "/etc", "/opt"];

/// Who held a call, as opposed to who was asked to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "by", rename_all = "snake_case")]
pub enum Applied {
    /// Our own check, before the fact. All an in-process tool can have.
    Process,
    /// The kernel is holding it. `how` names the mechanism *and its version*:
    /// Landlock's older ABIs cover less — v1 does not restrict `truncate`, v5
    /// does not restrict `ioctl` on devices — and months later nothing else
    /// would say which one ran.
    Kernel { how: String },
    /// Some of it, and `missing` is the whole point of the variant: it is the
    /// difference between this run and a held one. Only reachable under
    /// [`Enforcement::BestEffort`].
    Partial { how: String, missing: String },
}

impl std::fmt::Display for Applied {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Process => f.write_str("in-process check"),
            Self::Kernel { how } => write!(f, "{how}"),
            Self::Partial { how, missing } => write!(f, "{how} (missing: {missing})"),
        }
    }
}

/// What the sandbox decided, and who is enforcing it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Verdict {
    pub allowed: bool,
    /// The rule that decided, as a reader would name it. Present on both
    /// answers: "denied" without which rule denied is a bug report nobody can
    /// act on, and "allowed" without which rule allowed hides the grant that
    /// was wider than someone thought.
    pub rule: String,
    pub enforced_by: Applied,
}

impl Verdict {
    pub fn allow(rule: impl Into<String>, enforced_by: Applied) -> Self {
        Self {
            allowed: true,
            rule: rule.into(),
            enforced_by,
        }
    }

    pub fn deny(rule: impl Into<String>) -> Self {
        Self {
            allowed: false,
            rule: rule.into(),
            enforced_by: Applied::Process,
        }
    }
}

/// One granted tree, resolved.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Root {
    /// Canonical, so a symlink cannot walk out of it.
    pub path: PathBuf,
    pub access: Access,
    /// As it was written, so a verdict can name the rule the user typed rather
    /// than the path it turned into.
    pub source: String,
    /// True for [`SYSTEM_ROOTS`], which the sandbox adds itself.
    pub implicit: bool,
}

impl std::fmt::Display for Root {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let implicit = match self.implicit {
            true => ", implicit",
            false => "",
        };
        write!(f, "{} ({}{implicit})", self.source, self.access)
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("{source_path}: a granted path has to exist ({source})")]
    MissingPath {
        source_path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("the working directory could not be resolved: {0}")]
    Base(#[source] std::io::Error),
}

/// Who granted what this sandbox holds.
///
/// It rides on every denial for the same reason [`Applied`] rides on every
/// allow: a run refused by the policy file and a run refused by the plan its
/// task was approved with are not the same run, and afterwards the recording is
/// the only thing that could tell them apart.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Authority {
    /// The policy file plus whatever the flags added to it.
    Policy,
    /// The plan a person approved, which narrows the policy for one task.
    Plan(TaskId),
}

impl std::fmt::Display for Authority {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Policy => write!(f, "the sandbox policy"),
            Self::Plan(task) => write!(f, "the approved plan for task {task}"),
        }
    }
}

/// A resolved policy, and the thing tools ask.
#[derive(Debug, Clone)]
pub struct Sandbox {
    base: PathBuf,
    roots: Vec<Root>,
    commands: Vec<String>,
    network: bool,
    enforcement: Enforcement,
    authority: Authority,
}

/// The answer to "may I touch this path", with the path as it resolved. The
/// resolved path is on both answers because a denial that does not say what the
/// argument turned into is unreadable the moment a symlink is involved.
#[derive(Debug, Clone)]
pub struct PathCheck {
    pub verdict: Verdict,
    pub path: PathBuf,
}

impl Sandbox {
    /// Resolves a policy against a working directory.
    ///
    /// Every granted path must exist: Landlock takes a file descriptor per
    /// root, so a path that is not there cannot be granted — and a rule that
    /// silently did nothing is worse than one that fails to load.
    pub fn new(policy: &SandboxPolicy, base: &Path) -> Result<Self, SandboxError> {
        let base = base.canonicalize().map_err(SandboxError::Base)?;

        let mut roots = Vec::with_capacity(policy.paths.len() + SYSTEM_ROOTS.len());
        for rule in &policy.paths {
            let source = rule.path.display().to_string();
            let resolved = expand_home(&rule.path);
            let resolved = match resolved.is_absolute() {
                true => resolved,
                false => base.join(resolved),
            };
            let path =
                resolved
                    .canonicalize()
                    .map_err(|source_error| SandboxError::MissingPath {
                        source_path: source.clone(),
                        source: source_error,
                    })?;
            roots.push(Root {
                path,
                access: rule.access,
                source,
                implicit: false,
            });
        }

        // A command that cannot read libc cannot run at all, so allowing any
        // command implies these. Skipped silently when absent: a distribution
        // without /opt is not a misconfiguration.
        if !policy.commands.is_empty() {
            for name in SYSTEM_ROOTS {
                let Ok(path) = Path::new(name).canonicalize() else {
                    continue;
                };
                if roots.iter().any(|root| root.path == path) {
                    continue;
                }
                roots.push(Root {
                    path,
                    access: Access::Execute,
                    source: (*name).to_string(),
                    implicit: true,
                });
            }
        }

        // Longest first, so the most specific rule is the one that answers and
        // a narrower rule under a broader one grants more rather than less.
        roots.sort_by(|a, b| {
            b.path
                .components()
                .count()
                .cmp(&a.path.components().count())
                .then_with(|| a.path.cmp(&b.path))
        });

        Ok(Self {
            base,
            roots,
            commands: policy.commands.clone(),
            network: policy.network,
            enforcement: policy.enforcement,
            authority: Authority::Policy,
        })
    }

    /// The same policy, resolved as one task's rather than the session's.
    ///
    /// Only the label changes: what a narrowed policy grants is decided by
    /// [`crate::task::Plan::narrow`], which builds it out of what this sandbox
    /// already allows. A plan cannot grant what the file does not.
    pub fn under(mut self, authority: Authority) -> Self {
        self.authority = authority;
        self
    }

    /// Which authority answers here — the policy file, or a task's plan.
    pub fn authority(&self) -> &Authority {
        &self.authority
    }

    /// What this sandbox grants on a path, if anything. `None` is "no rule
    /// covers it", which is the same answer [`Sandbox::check_path`] denies on.
    ///
    /// Implicit roots are excluded for the same reason they are excluded from
    /// `check_path`: the grant that exists so a child can read libc is not a
    /// grant a plan may inherit.
    pub fn access_for(&self, path: &Path) -> Option<Access> {
        let resolved = resolve(&self.base, path);
        self.root_for(&resolved, false).map(|root| root.access)
    }

    /// The working directory relative paths are resolved against.
    pub fn base(&self) -> &Path {
        &self.base
    }

    pub fn roots(&self) -> &[Root] {
        &self.roots
    }

    pub fn commands(&self) -> &[String] {
        &self.commands
    }

    pub fn network(&self) -> bool {
        self.network
    }

    pub fn enforcement(&self) -> Enforcement {
        self.enforcement
    }

    /// May a tool touch this path, and where does it actually point?
    ///
    /// The path is canonicalized before it is compared, or a symlink walks
    /// straight out of the sandbox. What canonicalization cannot close is the
    /// race between this answer and the `open` that follows it — that is level
    /// one's hole, the answer for subprocesses is level two, and the answer for
    /// in-process tools is `openat2(RESOLVE_BENEATH)`, which is not in this
    /// change.
    pub fn check_path(&self, path: &Path, needed: Access) -> PathCheck {
        self.check(path, needed, false)
    }

    /// The same, for the program a subprocess is about to become.
    ///
    /// This one *does* consult [`SYSTEM_ROOTS`], because the child will be
    /// running under a ruleset that includes them. Separate from
    /// [`Sandbox::check_path`] so that the grant which exists for the child
    /// cannot leak into a tool that reads files in our own process.
    pub fn check_program(&self, path: &Path) -> PathCheck {
        self.check(path, Access::Execute, true)
    }

    fn check(&self, path: &Path, needed: Access, implicit: bool) -> PathCheck {
        let resolved = resolve(&self.base, path);
        let verdict = match self.root_for(&resolved, implicit) {
            Some(root) if root.access >= needed => {
                Verdict::allow(root.to_string(), Applied::Process)
            }
            Some(root) => Verdict::deny(format!(
                "{root} does not grant {needed} on {}, under {}",
                resolved.display(),
                self.authority,
            )),
            None => Verdict::deny(format!(
                "no rule grants {needed} on {}, under {}",
                resolved.display(),
                self.authority,
            )),
        };
        PathCheck {
            verdict,
            path: resolved,
        }
    }

    /// The most specific rule covering a resolved path, if any. `roots` is
    /// sorted longest first, so the first hit is it.
    fn root_for(&self, path: &Path, implicit: bool) -> Option<&Root> {
        self.roots
            .iter()
            .filter(|root| implicit || !root.implicit)
            .find(|root| path.starts_with(&root.path))
    }

    /// Checks a program name against the allowlist and prepares what the kernel
    /// will hold the child to.
    ///
    /// `Err` is the verdict that refused, which is also what the caller reports:
    /// a denial here is an ordinary outcome of a tool call, not a failure of the
    /// program.
    pub fn prepare_command(&self, program: &str) -> Result<Restrictions, Verdict> {
        if !self.commands.iter().any(|allowed| allowed == program) {
            return Err(Verdict::deny(match self.commands.is_empty() {
                true => format!(
                    "no commands are allowed by {}: `{program}` needs a `commands` entry",
                    self.authority,
                ),
                false => format!(
                    "`{program}` is not in commands = [{}], under {}",
                    self.commands.join(", "),
                    self.authority,
                ),
            }));
        }

        let prepared = kernel::prepare(&self.roots, self.network);
        let enforced_by = match (&prepared.how, &prepared.missing) {
            (Some(how), None) => Applied::Kernel { how: how.clone() },
            (Some(how), Some(missing)) => Applied::Partial {
                how: how.clone(),
                missing: missing.clone(),
            },
            (None, missing) => Applied::Partial {
                how: "in-process check only".into(),
                missing: missing.clone().unwrap_or_else(|| "everything".into()),
            },
        };

        // The one place a security property is a setting. Under `kernel` a gap
        // is a denial that names what is missing; under `best-effort` it is a
        // verdict that carries it.
        if self.enforcement == Enforcement::Kernel
            && let Applied::Partial { missing, .. } = &enforced_by
        {
            return Err(Verdict::deny(format!(
                "the kernel cannot hold this child ({missing}); \
                 grant it anyway with enforcement = \"best-effort\""
            )));
        }

        Ok(Restrictions {
            verdict: Verdict::allow(format!("commands allows `{program}`"), enforced_by),
            prepared,
        })
    }
}

/// The kernel restrictions for one child, and the verdict that goes with them.
pub struct Restrictions {
    pub verdict: Verdict,
    prepared: kernel::Prepared,
}

impl std::fmt::Debug for Restrictions {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The prepared side is a file descriptor and a BPF program; the verdict
        // is the part anyone reading a failure wants.
        f.debug_struct("Restrictions")
            .field("verdict", &self.verdict)
            .finish_non_exhaustive()
    }
}

impl Restrictions {
    /// Arranges for the restrictions to be applied in the child, after `fork`
    /// and before `exec`.
    pub fn install(self, command: &mut std::process::Command) {
        self.prepared.install(command);
    }
}

/// `~` and `~/…`, against `$HOME`. Left alone when there is no `$HOME`, so the
/// path fails to resolve with its own name in the error rather than silently
/// meaning something else.
fn expand_home(path: &Path) -> PathBuf {
    let Ok(rest) = path.strip_prefix("~") else {
        return path.to_path_buf();
    };
    match std::env::var_os("HOME") {
        Some(home) => PathBuf::from(home).join(rest),
        None => path.to_path_buf(),
    }
}

/// An absolute, symlink-free path, for something that may not exist yet.
///
/// Canonicalizes the deepest existing ancestor and re-attaches the rest. The
/// re-attached components are by definition not there, so none of them can be a
/// symlink and resolving `..` textually across them is exact — which is what
/// makes `proj/nothing/../../etc/passwd` land on `/etc/passwd` rather than
/// inside the project.
fn resolve(base: &Path, path: &Path) -> PathBuf {
    let joined = match path.is_absolute() {
        true => path.to_path_buf(),
        false => base.join(path),
    };

    // Popped as components rather than with `file_name`, which is `None` for a
    // path ending in `..` — and stopping there would leave the `..` in the
    // result, where `starts_with` reads it as an ordinary directory name and
    // `proj/nothing/../../outside` compares as if it were inside `proj`.
    let mut prefix: Vec<std::ffi::OsString> = joined
        .components()
        .map(|component| component.as_os_str().to_os_string())
        .collect();
    let mut tail = Vec::new();
    while !prefix.is_empty() {
        if prefix.iter().collect::<PathBuf>().exists() {
            break;
        }
        tail.push(prefix.pop().expect("checked non-empty"));
    }

    let existing: PathBuf = prefix.iter().collect();
    let mut out = existing.canonicalize().unwrap_or(existing);
    for component in tail.iter().rev() {
        match component.to_str() {
            Some("..") => {
                out.pop();
            }
            Some(".") => {}
            _ => out.push(component),
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A throwaway tree with a project directory and a secret outside it.
    struct Fixture {
        root: PathBuf,
    }

    impl Fixture {
        fn new(name: &str) -> Self {
            let root = std::env::temp_dir().join(format!(
                "luu-sandbox-{name}-{}-{:?}",
                std::process::id(),
                std::thread::current().id()
            ));
            let _ = std::fs::remove_dir_all(&root);
            std::fs::create_dir_all(root.join("proj/src")).unwrap();
            std::fs::create_dir_all(root.join("outside")).unwrap();
            std::fs::write(root.join("proj/src/main.rs"), "fn main() {}").unwrap();
            std::fs::write(root.join("outside/secret"), "shh").unwrap();
            Self { root }
        }

        fn proj(&self) -> PathBuf {
            self.root.join("proj").canonicalize().unwrap()
        }

        fn sandbox(&self, policy: &SandboxPolicy) -> Sandbox {
            Sandbox::new(policy, &self.proj()).unwrap()
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.root);
        }
    }

    fn read_write_here() -> SandboxPolicy {
        SandboxPolicy {
            paths: vec![PathRule::new(".", Access::ReadWrite)],
            ..SandboxPolicy::default()
        }
    }

    #[test]
    fn a_path_inside_a_granted_root_is_allowed_and_names_the_rule() {
        let fixture = Fixture::new("inside");
        let sandbox = fixture.sandbox(&read_write_here());

        let check = sandbox.check_path(Path::new("src/main.rs"), Access::ReadWrite);
        assert!(check.verdict.allowed, "{:?}", check.verdict);
        assert!(check.verdict.rule.contains('.'), "{:?}", check.verdict);
        assert_eq!(check.path, fixture.proj().join("src/main.rs"));
    }

    #[test]
    fn dot_dot_does_not_walk_out_even_through_a_directory_that_is_not_there() {
        let fixture = Fixture::new("dotdot");
        let sandbox = fixture.sandbox(&read_write_here());

        for attempt in ["../outside/secret", "nothing/../../outside/secret"] {
            let check = sandbox.check_path(Path::new(attempt), Access::Read);
            assert!(!check.verdict.allowed, "{attempt} was allowed");
            assert!(check.verdict.rule.contains("no rule grants"));
        }
    }

    #[test]
    #[cfg(unix)]
    fn a_symlink_is_judged_by_where_it_points() {
        let fixture = Fixture::new("symlink");
        // The classic bypass: a link inside the project to a file outside it.
        std::os::unix::fs::symlink(
            fixture.root.join("outside/secret"),
            fixture.proj().join("src/link"),
        )
        .unwrap();
        let sandbox = fixture.sandbox(&read_write_here());

        let check = sandbox.check_path(Path::new("src/link"), Access::Read);
        assert!(
            !check.verdict.allowed,
            "canonicalizing before comparing is the whole point: {:?}",
            check.verdict
        );
        assert!(check.path.ends_with("outside/secret"));
    }

    #[test]
    fn the_longest_matching_rule_answers_so_a_narrow_grant_can_widen_a_broad_one() {
        let fixture = Fixture::new("longest");
        let sandbox = fixture.sandbox(&SandboxPolicy {
            paths: vec![
                PathRule::new(".", Access::Read),
                PathRule::new("./src", Access::ReadWrite),
            ],
            ..SandboxPolicy::default()
        });

        assert!(
            sandbox
                .check_path(Path::new("src/main.rs"), Access::ReadWrite)
                .verdict
                .allowed
        );
        assert!(
            !sandbox
                .check_path(Path::new("Cargo.toml"), Access::ReadWrite)
                .verdict
                .allowed,
            "the broader rule still grants only read"
        );
    }

    #[test]
    fn too_little_access_is_denied_by_the_rule_that_granted_the_rest() {
        let fixture = Fixture::new("tooLittle");
        let sandbox = fixture.sandbox(&SandboxPolicy {
            paths: vec![PathRule::new(".", Access::Read)],
            ..SandboxPolicy::default()
        });

        let check = sandbox.check_path(Path::new("src/main.rs"), Access::ReadWrite);
        assert!(!check.verdict.allowed);
        assert!(
            check.verdict.rule.contains("does not grant read-write"),
            "{}",
            check.verdict.rule
        );
    }

    #[test]
    fn a_granted_path_that_is_not_there_fails_to_load_rather_than_doing_nothing() {
        let fixture = Fixture::new("missing");
        let error = Sandbox::new(
            &SandboxPolicy {
                paths: vec![PathRule::new("./nowhere", Access::Read)],
                ..SandboxPolicy::default()
            },
            &fixture.proj(),
        )
        .unwrap_err();
        assert!(error.to_string().contains("./nowhere"), "{error}");
    }

    #[test]
    fn no_commands_means_no_command() {
        let fixture = Fixture::new("nocmd");
        let sandbox = fixture.sandbox(&read_write_here());

        let verdict = sandbox.prepare_command("ls").unwrap_err();
        assert!(!verdict.allowed);
        assert!(verdict.rule.contains("no commands are allowed"));
        assert!(
            sandbox.roots().iter().all(|root| !root.implicit),
            "the system roots are granted only once a command is"
        );
    }

    #[test]
    fn allowing_a_command_brings_the_system_roots_with_it() {
        let fixture = Fixture::new("syscmd");
        let sandbox = fixture.sandbox(&SandboxPolicy {
            commands: vec!["ls".into()],
            ..read_write_here()
        });

        assert!(
            sandbox
                .roots()
                .iter()
                .any(|root| root.implicit && root.access == Access::Execute),
            "a command that cannot read libc cannot run"
        );
        assert!(
            sandbox
                .roots()
                .iter()
                .all(|root| !root.implicit || root.access != Access::ReadWrite),
            "and it is never granted write"
        );
    }

    #[test]
    fn allowing_a_command_does_not_let_the_agent_read_the_system_itself() {
        // The implicit roots exist because a child cannot run without reading
        // libc. That justification says nothing about `read_file`, and letting
        // it apply there would mean `commands = ["ls"]` silently granted the
        // agent /etc.
        let fixture = Fixture::new("implicitread");
        let sandbox = fixture.sandbox(&SandboxPolicy {
            commands: vec!["ls".into()],
            ..read_write_here()
        });

        let system = Path::new("/etc");
        assert!(
            !sandbox.check_path(system, Access::Read).verdict.allowed,
            "an in-process tool sees only what was written down",
        );
        assert!(
            sandbox.check_program(system).verdict.allowed,
            "and the child, which will be under a ruleset that includes it, does",
        );
    }

    #[test]
    fn a_command_that_is_not_on_the_list_is_denied_by_name() {
        let fixture = Fixture::new("othercmd");
        let sandbox = fixture.sandbox(&SandboxPolicy {
            commands: vec!["ls".into()],
            ..read_write_here()
        });
        let verdict = sandbox.prepare_command("curl").unwrap_err();
        assert!(
            verdict.rule.contains("`curl` is not in commands"),
            "{verdict:?}"
        );
    }

    #[test]
    fn best_effort_reports_the_gap_rather_than_hiding_it() {
        let fixture = Fixture::new("besteffort");
        let sandbox = fixture.sandbox(&SandboxPolicy {
            commands: vec!["ls".into()],
            enforcement: Enforcement::BestEffort,
            ..read_write_here()
        });

        let verdict = sandbox.prepare_command("ls").unwrap().verdict;
        assert!(verdict.allowed);
        match &verdict.enforced_by {
            // Where the kernel can hold it, the verdict says with what.
            Applied::Kernel { how } => assert!(how.contains("seccomp"), "{how}"),
            // Where it cannot, the verdict says what is missing. Both are
            // correct answers; "sandboxed" with no mechanism named is not.
            Applied::Partial { missing, .. } => assert!(!missing.is_empty()),
            Applied::Process => panic!("a subprocess is never held by an in-process check"),
        }
    }

    #[test]
    fn kernel_enforcement_denies_rather_than_degrading() {
        let fixture = Fixture::new("required");
        let sandbox = fixture.sandbox(&SandboxPolicy {
            commands: vec!["ls".into()],
            enforcement: Enforcement::Kernel,
            ..read_write_here()
        });

        match sandbox.prepare_command("ls") {
            // On a kernel with both mechanisms this is the answer.
            Ok(restrictions) => assert!(matches!(
                restrictions.verdict.enforced_by,
                Applied::Kernel { .. }
            )),
            // On anything else, a denial that says what is missing and how to
            // lower the bar deliberately — never a quiet downgrade.
            Err(verdict) => {
                assert!(!verdict.allowed);
                assert!(verdict.rule.contains("best-effort"), "{verdict:?}");
            }
        }
    }

    #[test]
    fn a_verdict_survives_the_wire() {
        let verdict = Verdict::allow(
            ". (read-write)",
            Applied::Partial {
                how: "seccomp".into(),
                missing: "landlock".into(),
            },
        );
        let json = serde_json::to_value(&verdict).unwrap();
        assert_eq!(json["enforced_by"]["by"], "partial");
        assert_eq!(json["enforced_by"]["missing"], "landlock");
        let back: Verdict = serde_json::from_value(json).unwrap();
        assert_eq!(back, verdict);
    }
}
