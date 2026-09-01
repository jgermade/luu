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
