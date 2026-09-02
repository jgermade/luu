//! What a project declares the agent may reach.
//!
//! This is the *declaration* — paths as they were written, before anything is
//! resolved. [`super::Sandbox`] is what it becomes once the paths are real.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// How much of a tree is granted.
///
/// Three and not two, because "may read this tree" and "may run binaries out of
/// this tree" are different grants and the difference costs one line. Ordered:
/// a rule granting more satisfies a check needing less.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Access {
    /// Open for reading, and list directories.
    Read,
    /// The above, plus running what is in the tree.
    Execute,
    /// The above, plus writing, creating and deleting.
    ReadWrite,
}

impl Access {
    /// How a verdict names it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Read => "read",
            Self::Execute => "execute",
            Self::ReadWrite => "read-write",
        }
    }
}

impl std::fmt::Display for Access {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// One granted tree.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PathRule {
    pub path: PathBuf,
    #[serde(default = "default_access")]
    pub access: Access,
}

fn default_access() -> Access {
    Access::Read
}

impl PathRule {
    pub fn new(path: impl Into<PathBuf>, access: Access) -> Self {
        Self {
            path: path.into(),
            access,
        }
    }
}

/// What to do when the kernel cannot hold a subprocess.
///
/// The only place in this design where a security property is a setting, which
/// is why the default is the strict one and the lenient one has to be typed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Enforcement {
    /// A subprocess runs only if the kernel took the ruleset and the filter.
    /// Where it cannot — macOS, a kernel without Landlock — `run_command` is
    /// denied, and the denial says what is missing.
    #[default]
    Kernel,
    /// Apply what this kernel has and report the gap.
    BestEffort,
}

impl Enforcement {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Kernel => "kernel",
            Self::BestEffort => "best-effort",
        }
    }
}

/// What a child may spend, beyond what it may reach.
///
/// The filesystem sandbox says *where* a subprocess may write; nothing said
/// **how much**. A `cargo` that forks, allocates or writes without bound runs
/// to completion inside `run_command`'s 30-second clock, and the 8 KiB output
/// cap trims what is reported about it rather than what it did — a fork bomb, a
/// disk bomb and a memory bomb were all held by a stopwatch.
///
/// `setrlimit` is POSIX, not Linux, which is what makes this the first thing
/// that holds a child on macOS as well: see
/// `RECORD/2026-09-01.what-the-audit-left.completed.md`.
///
/// Every field is `Option` and every one of them is a *number someone chose*.
/// Two are off by default, and the reason is the same in both cases — a default
/// that breaks an ordinary build teaches people to turn the whole thing off:
///
/// - `memory_mb` is `RLIMIT_AS`, and a Rust toolchain **reserves** address
///   space far above what it commits. A number that looks like "what a build
///   needs" kills builds that were fine.
/// - `processes` is `RLIMIT_NPROC`, which the kernel counts **per real uid, not
///   per process tree** — every process the person is already running counts
///   against it, so a default would deny `fork` based on how many browser tabs
///   are open. It is the one limit whose right answer is a pids cgroup, which
///   arrives with the container.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Limits {
    /// `RLIMIT_CPU`, in seconds, per process. A second belt to the wall clock:
    /// the timeout kills what `run_command` is still waiting for, and this
    /// holds what outlived it. Generous on purpose — one `rustc` with eight
    /// threads spends CPU seconds eight times as fast as wall-clock ones.
    #[serde(default = "default_cpu_seconds")]
    pub cpu_seconds: Option<u64>,
    /// `RLIMIT_FSIZE`, in MiB. Per file, not per tree: it stops one file
    /// growing without bound, which is the shape a disk bomb has.
    #[serde(default = "default_file_size_mb")]
    pub file_size_mb: Option<u64>,
    /// `RLIMIT_AS`, in MiB. Off by default — see the type's own note.
    #[serde(default)]
    pub memory_mb: Option<u64>,
    /// `RLIMIT_NPROC`. Off by default, and it is per-uid — see above.
    #[serde(default)]
    pub processes: Option<u64>,
}

fn default_cpu_seconds() -> Option<u64> {
    Some(300)
}

fn default_file_size_mb() -> Option<u64> {
    Some(1024)
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            cpu_seconds: default_cpu_seconds(),
            file_size_mb: default_file_size_mb(),
            memory_mb: None,
            processes: None,
        }
    }
}

impl Limits {
    /// Every limit off: the child is held by the clock alone, as it was before
    /// this existed.
    pub const NONE: Self = Self {
        cpu_seconds: None,
        file_size_mb: None,
        memory_mb: None,
        processes: None,
    };

    pub fn is_empty(&self) -> bool {
        *self == Self::NONE
    }

    /// How a verdict names them, or `None` when there are none to name.
    ///
    /// The numbers are in the string on purpose: "rlimits" without them is the
    /// same claim for a 300-second limit and a 3-second one, and afterwards the
    /// recording is the only thing that could tell those runs apart.
    pub fn describe(&self) -> Option<String> {
        let named: Vec<String> = [
            self.cpu_seconds.map(|value| format!("cpu {value}s")),
            self.file_size_mb.map(|value| format!("file {value}M")),
            self.memory_mb.map(|value| format!("memory {value}M")),
            self.processes.map(|value| format!("procs {value}")),
        ]
        .into_iter()
        .flatten()
        .collect();
        (!named.is_empty()).then(|| format!("rlimits ({})", named.join(", ")))
    }
}

/// The declared policy.
///
/// There is deliberately no deny list: Landlock is allow-only, so a subtraction
/// could be honoured in-process and not in a subprocess — two sandboxes wearing
/// one config file, with the weaker one applying where the danger is. The way
/// to deny is to not grant. See `RECORD/2026-08-27.tools-and-sandbox.completed.md`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SandboxPolicy {
    /// The granted trees. Longest match wins, so a narrower rule under a
    /// broader one grants more.
    #[serde(default)]
    pub paths: Vec<PathRule>,
    /// Program names `run_command` may run. Never a shell string: a shell
    /// string makes the allowlist meaningless, because `sh -c "a; b"` passes
    /// any check that looks at the first word. Putting `sh` here is allowed and
    /// then the grant reads as what it is.
    #[serde(default)]
    pub commands: Vec<String>,
    #[serde(default)]
    pub network: bool,
    #[serde(default)]
    pub enforcement: Enforcement,
    /// What a subprocess may spend. Applies to the child only: an in-process
    /// tool runs in *our* process, and an `RLIMIT_AS` that held `read_file`
    /// would be holding the agent.
    #[serde(default)]
    pub limits: Limits,
}

impl Default for SandboxPolicy {
    /// Read-write on the working directory, no network, and **no commands** —
    /// an empty allowlist that meant "anything" is the failure mode this whole
    /// type exists to avoid.
    fn default() -> Self {
        Self {
            paths: vec![PathRule::new(".", Access::ReadWrite)],
            commands: Vec::new(),
            network: false,
            enforcement: Enforcement::default(),
            limits: Limits::default(),
        }
    }
}

/// The file `luu.toml` is, so the sandbox can live under `[sandbox]` and the
/// file can grow other sections later.
#[derive(Debug, Clone, Default, Deserialize)]
struct PolicyFile {
    #[serde(default)]
    sandbox: Option<SandboxPolicy>,
}

#[derive(Debug, thiserror::Error)]
pub enum PolicyError {
    #[error("reading {path}: {source}")]
    Read {
        path: String,
        #[source]
        source: std::io::Error,
    },
    #[error("{path}: {source}")]
    Parse {
        path: String,
        #[source]
        source: toml::de::Error,
    },
}

impl SandboxPolicy {
    /// Reads `[sandbox]` out of a TOML file. A file with no `[sandbox]` block
    /// is not an error — it is a `luu.toml` that configures something else, and
    /// the default policy applies.
    pub fn from_file(path: &std::path::Path) -> Result<Self, PolicyError> {
        let text = std::fs::read_to_string(path).map_err(|source| PolicyError::Read {
            path: path.display().to_string(),
            source,
        })?;
        Self::from_toml(&text).map_err(|source| PolicyError::Parse {
            path: path.display().to_string(),
            source,
        })
    }

    pub fn from_toml(text: &str) -> Result<Self, toml::de::Error> {
        let file: PolicyFile = toml::from_str(text)?;
        Ok(file.sandbox.unwrap_or_default())
    }

    /// Grants a tree. Used by the CLI's `--allow-*` flags, which add to what the
    /// file said rather than replacing it.
    pub fn allow(&mut self, path: impl Into<PathBuf>, access: Access) -> &mut Self {
        self.paths.push(PathRule::new(path, access));
        self
    }

    pub fn allow_command(&mut self, name: impl Into<String>) -> &mut Self {
        self.commands.push(name.into());
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_policy_file_reads_as_it_looks() {
        let policy = SandboxPolicy::from_toml(
            r#"
            [sandbox]
            enforcement = "best-effort"
            network = true
            commands = ["cargo", "git"]

            [[sandbox.paths]]
            path = "."
            access = "read-write"

            [[sandbox.paths]]
            path = "/usr/lib"
            "#,
        )
        .unwrap();

        assert_eq!(policy.enforcement, Enforcement::BestEffort);
        assert!(policy.network);
        assert_eq!(policy.commands, ["cargo", "git"]);
        assert_eq!(policy.paths[0].access, Access::ReadWrite);
        assert_eq!(
            policy.paths[1].access,
            Access::Read,
            "an omitted access grants the least, not the most"
        );
    }

    #[test]
    fn limits_default_to_the_two_that_cannot_break_a_build() {
        let policy = SandboxPolicy::default();
        assert_eq!(policy.limits.cpu_seconds, Some(300));
        assert_eq!(policy.limits.file_size_mb, Some(1024));
        // The two that are off, and stay off until someone types a number: a
        // default `RLIMIT_AS` kills `cargo` for reserving address space, and a
        // default `RLIMIT_NPROC` denies `fork` because of processes this agent
        // never started.
        assert_eq!(policy.limits.memory_mb, None);
        assert_eq!(policy.limits.processes, None);
        assert_eq!(
            policy.limits.describe().as_deref(),
            Some("rlimits (cpu 300s, file 1024M)"),
        );
    }

    #[test]
    fn limits_are_named_with_their_numbers_or_not_at_all() {
        assert!(Limits::NONE.is_empty());
        assert_eq!(Limits::NONE.describe(), None);

        let policy = SandboxPolicy::from_toml(
            "[sandbox.limits]
cpu-seconds = 5
file-size-mb = 1
memory-mb = 512
processes = 8
",
        )
        .unwrap();
        assert_eq!(
            policy.limits.describe().as_deref(),
            Some("rlimits (cpu 5s, file 1M, memory 512M, procs 8)"),
        );
    }

    #[test]
    fn a_misspelled_limit_is_an_error_like_every_other_grant() {
        // Same reason as `commmands`: a key that parsed and did nothing would
        // read as a limit that is being enforced.
        assert!(
            SandboxPolicy::from_toml(
                "[sandbox.limits]
cpu_secs = 5
"
            )
            .is_err()
        );
    }

    #[test]
    fn the_default_denies_every_command() {
        let policy = SandboxPolicy::default();
        assert!(
            policy.commands.is_empty(),
            "an empty allowlist must mean nothing, and the default must be it"
        );
        assert!(!policy.network);
        assert_eq!(policy.enforcement, Enforcement::Kernel);
    }

    #[test]
    fn a_file_without_a_sandbox_block_is_the_default_and_not_an_error() {
        let policy = SandboxPolicy::from_toml("[something-else]\nkey = 1\n").unwrap();
        assert_eq!(policy, SandboxPolicy::default());
    }

    #[test]
    fn a_misspelled_key_is_an_error_rather_than_a_silently_ignored_grant() {
        // `commmands = [...]` that parsed and did nothing would read as a
        // policy that allows those commands.
        let error = SandboxPolicy::from_toml("[sandbox]\ncommmands = [\"cargo\"]\n").unwrap_err();
        assert!(error.to_string().contains("commmands"), "{error}");
    }

    #[test]
    fn a_deny_list_is_not_a_key_this_accepts() {
        // It was in the design and is deliberately gone: Landlock cannot
        // express a subtraction, so it would apply in-process only.
        assert!(SandboxPolicy::from_toml("[sandbox]\ndenied = [\"./.env\"]\n").is_err());
    }

    #[test]
    fn access_is_ordered_so_that_a_wider_grant_satisfies_a_narrower_need() {
        assert!(Access::ReadWrite > Access::Execute);
        assert!(Access::Execute > Access::Read);
    }
}
