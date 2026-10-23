//! What a session may do, as lists by axis — shell, network, files — and the
//! environments that combine them, in `config.toml` beside the providers.
//!
//! Step 1 of `RECORD/2026-10-22.policies-not-postures.WIP.md`: the model, read
//! and resolved into the sandbox and the worker luu already has, and recorded.
//! **Nothing asks yet.** A list's `allow` is what the session's sandbox
//! grants, exactly as a policy file's `[sandbox]` does — the gate still asks
//! for whatever a plan wants beyond the draft's floor. `ask` and `deny` are
//! read, checked and recorded, and grant nothing: an entry in `ask` is closed
//! until step 2 lets a person open it.
//!
//! ```toml
//! [shell.rust]
//! allow = ["cargo", "rustc", "git", "rg", "ls"]
//! ask   = ["*"]
//! deny  = ["curl"]
//!
//! [network.registries]
//! allow = ["crates.io", "*.crates.io"]
//!
//! [files.project]
//! allow = [{ path = ".", access = "read-write" }]
//!
//! [environment.container]
//! runtime = "docker"
//! image   = "luu-worker:dev"
//! shell   = "rust"
//! network = "registries"
//! files   = "project"
//! ```
//!
//! A `[posture.<name>]` is read as an environment too: its file's `[sandbox]`
//! with everything in `allow`, which is what a posture has always meant. Both
//! are recorded the same way — each list by its name and the SHA-256 of what
//! it holds — so a resume compares what a session could do, not what it was
//! called.

use std::collections::BTreeMap;

use agent_core::record::{Hashed, Lists};
use agent_core::sandbox::{Enforcement, Limits, PathRule, SandboxPolicy};
use agent_core::worker::{Runtime, WorkerConfig};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// A shell or a network list: program names, or host patterns.
///
/// `*` is any program or any host. It may be asked for, and on a network list
/// allowed — *any host*, which is what a policy file's `network = true` with no
/// `egress` has always meant. A shell list may not allow it: an allowlist that
/// lets every program through is the failure a sandbox exists to avoid, and
/// the way to say it is to write the programs.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Zones {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ask: Vec<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

/// A files list: trees with their access, relative to the working directory.
///
/// `deny` is read and refused for now. Landlock only grants, so a path denied
/// inside a tree that is allowed cannot be held for a subprocess, and a
/// sandbox that held it for luu's own file tools and not for `run_command`
/// would be two sandboxes in one file, the weaker one where the danger is
/// (`RECORD/2026-08-27.tools-and-sandbox.completed.md`). It is accepted when a
/// later step can hold it — by luu per call, and in a container by a mount.
#[derive(Debug, Clone, Default, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct FilesList {
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub allow: Vec<PathRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub ask: Vec<PathRule>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub deny: Vec<String>,
}

/// One list of each kind, where they run, and what holds them there.
#[derive(Debug, Clone, Deserialize, Serialize, PartialEq, Eq)]
#[serde(deny_unknown_fields, rename_all = "kebab-case")]
pub struct Environment {
    /// `host` by default, as a policy file's `[worker]` is.
    #[serde(default)]
    pub runtime: Runtime,
    /// The image, for the runtimes that take one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub image: Option<String>,
    /// Trees that exist only inside the image — its toolchain — which the host
    /// must not try to resolve. A policy file's `[[worker.paths]]`.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub image_paths: Vec<PathRule>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    pub shell: String,
    pub network: String,
    pub files: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub enforcement: Option<Enforcement>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub limits: Option<Limits>,
}

/// Every list and environment `config.toml` names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policies {
    pub shell: BTreeMap<String, Zones>,
    pub network: BTreeMap<String, Zones>,
    pub files: BTreeMap<String, FilesList>,
    pub environments: BTreeMap<String, Environment>,
}

/// An environment, resolved into what luu builds a session from.
#[derive(Debug, Clone)]
pub struct Resolved {
    pub name: String,
    /// What the sandbox grants: every list's `allow`, and nothing else.
    pub policy: SandboxPolicy,
    pub worker: WorkerConfig,
    shell: (String, Zones),
    network: (String, Zones),
    files: (String, FilesList),
}

impl Policies {
    /// Whether the lists and environments hold together. `postures` are the
    /// names an environment may not take: a session names one or the other, and
    /// one name for two things would make what it ran under depend on which
    /// table was looked in first.
    pub fn check<'a>(&self, postures: impl IntoIterator<Item = &'a String>) -> Result<(), String> {
        for (name, list) in &self.shell {
            check_zones(&format!("[shell.{name}]"), list, false)?;
        }
        for (name, list) in &self.network {
            check_zones(&format!("[network.{name}]"), list, true)?;
        }
        for (name, list) in &self.files {
            check_files(&format!("[files.{name}]"), list)?;
        }
        let postures: Vec<&String> = postures.into_iter().collect();
        for (name, environment) in &self.environments {
            let at = format!("[environment.{name}]");
            if postures.contains(&name) {
                return Err(format!(
                    "{at} and [posture.{name}] share a name: a session names one or the other"
                ));
            }
            if !self.shell.contains_key(&environment.shell) {
                return Err(format!(
                    "{at} names shell = \"{}\", and there is no [shell.{}]",
                    environment.shell, environment.shell
                ));
            }
            if !self.network.contains_key(&environment.network) {
                return Err(format!(
                    "{at} names network = \"{}\", and there is no [network.{}]",
                    environment.network, environment.network
                ));
            }
            if !self.files.contains_key(&environment.files) {
                return Err(format!(
                    "{at} names files = \"{}\", and there is no [files.{}]",
                    environment.files, environment.files
                ));
            }
        }
        Ok(())
    }

    /// Every environment, resolved. Only after [`Policies::check`].
    pub fn resolve_all(&self) -> BTreeMap<String, Resolved> {
        self.environments
            .keys()
            .filter_map(|name| Some((name.clone(), self.resolve(name)?)))
            .collect()
    }

    /// One environment, resolved: `None` when there is no such name, or one
    /// of its lists is missing — which [`Policies::check`] refuses first.
    pub fn resolve(&self, name: &str) -> Option<Resolved> {
        let environment = self.environments.get(name)?;
        let shell = self.shell.get(&environment.shell)?.clone();
        let network = self.network.get(&environment.network)?.clone();
        let files = self.files.get(&environment.files)?.clone();
        let mut policy = SandboxPolicy {
            paths: Vec::new(),
            commands: shell.allow.clone(),
            ..SandboxPolicy::default()
        };
        widen(&mut policy, None, files.allow.iter().cloned());
        (policy.network, policy.egress) = network_of(&network.allow);
        if let Some(enforcement) = environment.enforcement {
            policy.enforcement = enforcement;
        }
        if let Some(limits) = environment.limits {
            policy.limits = limits;
        }
        let mut worker = WorkerConfig {
            runtime: environment.runtime,
            image: environment.image.clone(),
            paths: environment.image_paths.clone(),
            ..WorkerConfig::default()
        };
        if let Some(timeout_ms) = environment.timeout_ms {
            worker.timeout_ms = timeout_ms;
        }
        Some(Resolved {
            name: name.to_string(),
            policy,
            worker,
            shell: (environment.shell.clone(), shell),
            network: (environment.network.clone(), network),
            files: (environment.files.clone(), files),
        })
    }
}

impl Resolved {
    /// Its shell list, as written.
    pub fn shell(&self) -> &Zones {
        &self.shell.1
    }

    /// Its files list, as written.
    pub fn files(&self) -> &FilesList {
        &self.files.1
    }

    /// Its network list, as written.
    pub fn network(&self) -> &Zones {
        &self.network.1
    }

    /// The lists as recorded, with `allow` read back off the sandbox the
    /// session actually got — the command line's `--allow-*` included — and
    /// `ask` and `deny` as written.
    pub fn lists(&self, granted: &SandboxPolicy) -> Lists {
        Lists {
            shell: hashed(
                &self.shell.0,
                &Zones {
                    allow: granted.commands.clone(),
                    ..self.shell.1.clone()
                },
            ),
            network: hashed(
                &self.network.0,
                &Zones {
                    allow: network_allow(granted),
                    ..self.network.1.clone()
                },
            ),
            files: hashed_files(
                &self.files.0,
                &FilesList {
                    allow: granted.paths.clone(),
                    ..self.files.1.clone()
                },
            ),
        }
    }
}

/// The lists a session was built from, as the page shows them: by name, each
/// zone as written. A policy file's are all `allow`, named after the file.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ListsView {
    /// The environment's name, or `None` for a policy file.
    pub environment: Option<String>,
    pub shell: Named<Zones>,
    pub network: Named<Zones>,
    pub files: Named<FilesList>,
    /// What happens to a command the kernel cannot hold — refused under
    /// `kernel`, run and reported under `best-effort`. Not a list's, but it
    /// refuses what the lists allow, so a refusal can be traced to it.
    pub enforcement: agent_core::sandbox::Enforcement,
}

/// A list and its name.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct Named<T> {
    pub name: String,
    #[serde(flatten)]
    pub list: T,
}

impl ListsView {
    /// An environment's lists, as written.
    pub fn of_environment(resolved: &Resolved) -> Self {
        Self {
            environment: Some(resolved.name.clone()),
            shell: Named {
                name: resolved.shell.0.clone(),
                list: resolved.shell.1.clone(),
            },
            network: Named {
                name: resolved.network.0.clone(),
                list: resolved.network.1.clone(),
            },
            files: Named {
                name: resolved.files.0.clone(),
                list: resolved.files.1.clone(),
            },
            enforcement: resolved.policy.enforcement,
        }
    }

    /// A policy file's, read back off the sandbox it gave: everything in
    /// `allow`, paths relative to `base` where they are under it.
    pub fn of_policy(label: &str, granted: &SandboxPolicy, base: &std::path::Path) -> Self {
        let named = |list| Named {
            name: label.to_string(),
            list,
        };
        Self {
            environment: None,
            shell: named(Zones {
                allow: granted.commands.clone(),
                ..Zones::default()
            }),
            network: named(Zones {
                allow: network_allow(granted),
                ..Zones::default()
            }),
            files: Named {
                name: label.to_string(),
                list: FilesList {
                    allow: granted
                        .paths
                        .iter()
                        .map(|rule| {
                            let path = match rule.path.strip_prefix(base) {
                                Ok(under) if under.as_os_str().is_empty() => ".".into(),
                                Ok(under) => under.to_path_buf(),
                                Err(_) => rule.path.clone(),
                            };
                            PathRule::new(path, rule.access)
                        })
                        .collect(),
                    ..FilesList::default()
                },
            },
            enforcement: granted.enforcement,
        }
    }
}

/// The lists of a session built from a policy file — a posture, or the
/// server's own — which has everything in `allow`. `label` names all three:
/// the file is the only name they have.
pub fn lists_of_file(label: &str, granted: &SandboxPolicy) -> Lists {
    Lists {
        shell: hashed(
            label,
            &Zones {
                allow: granted.commands.clone(),
                ..Zones::default()
            },
        ),
        network: hashed(
            label,
            &Zones {
                allow: network_allow(granted),
                ..Zones::default()
            },
        ),
        files: hashed_files(
            label,
            &FilesList {
                allow: granted.paths.clone(),
                ..FilesList::default()
            },
        ),
    }
}

/// Adds `rules` to `policy`'s paths, a path already there taking the greater
/// of the two accesses. Two rules for one path would otherwise both stand, and
/// the sandbox answers with the first — so a list's `ask` for writing `.` under
/// an `allow` for reading it would read as read-only. `base` is what relative
/// paths are read against, so `.` and the working directory written out are
/// one path; without it, paths are compared as written.
pub fn widen(
    policy: &mut SandboxPolicy,
    base: Option<&std::path::Path>,
    rules: impl IntoIterator<Item = PathRule>,
) {
    let place = |path: &std::path::Path| match base {
        None => path.to_path_buf(),
        Some(base) => {
            let joined = base.join(path);
            joined.canonicalize().unwrap_or(joined)
        }
    };
    for rule in rules {
        let at = place(&rule.path);
        match policy.paths.iter_mut().find(|held| place(&held.path) == at) {
            Some(held) => held.access = held.access.max(rule.access),
            None => policy.paths.push(rule),
        }
    }
}

/// What a network list's `allow` grants: no network for none, any host for
/// `*`, and those hosts through the proxy otherwise.
fn network_of(allow: &[String]) -> (bool, Vec<String>) {
    match allow {
        [] => (false, Vec::new()),
        _ if allow.iter().any(|host| host == "*") => (true, Vec::new()),
        _ => (true, allow.to_vec()),
    }
}

/// [`network_of`] backwards: the `allow` a sandbox's network amounts to.
fn network_allow(granted: &SandboxPolicy) -> Vec<String> {
    match (granted.network, granted.egress.is_empty()) {
        (false, _) => Vec::new(),
        (true, true) => vec!["*".to_string()],
        (true, false) => granted.egress.clone(),
    }
}

fn check_zones(at: &str, list: &Zones, hosts: bool) -> Result<(), String> {
    let zones = [
        ("allow", &list.allow),
        ("ask", &list.ask),
        ("deny", &list.deny),
    ];
    for (zone, entries) in zones {
        for entry in entries {
            if entry.is_empty() || entry.chars().any(char::is_whitespace) {
                return Err(format!(
                    "{at} {zone} has \"{entry}\": one word, with no spaces"
                ));
            }
            if !hosts && entry.contains('/') {
                return Err(format!(
                    "{at} {zone} has \"{entry}\": a program's name, never a path or a command line"
                ));
            }
            if entry == "*" && (zone == "deny" || (zone == "allow" && !hosts)) {
                let why = match zone {
                    "deny" => "what no list names is denied already",
                    _ => "write the programs, or ask = [\"*\"]",
                };
                return Err(format!("{at} {zone} has \"*\": {why}"));
            }
        }
    }
    for (index, (zone, entries)) in zones.iter().enumerate() {
        for (other, others) in &zones[index + 1..] {
            if let Some(entry) = entries.iter().find(|entry| others.contains(entry)) {
                return Err(format!("{at} has \"{entry}\" in both {zone} and {other}"));
            }
        }
    }
    Ok(())
}

fn check_files(at: &str, list: &FilesList) -> Result<(), String> {
    if let Some(entry) = list.deny.first() {
        return Err(format!(
            "{at} deny has \"{entry}\": a path denied inside one that is allowed cannot be held \
             for a command yet, so it is not accepted until it can — leave it out of allow instead"
        ));
    }
    for (zone, entries) in [("allow", &list.allow), ("ask", &list.ask)] {
        if let Some(rule) = entries.iter().find(|rule| rule.path.as_os_str().is_empty()) {
            return Err(format!("{at} {zone} has a path that is empty ({rule:?})"));
        }
    }
    if let Some(rule) = list
        .ask
        .iter()
        .find(|rule| list.allow.iter().any(|allowed| allowed == *rule))
    {
        return Err(format!(
            "{at} has {} ({}) in both allow and ask",
            rule.path.display(),
            rule.access.as_str()
        ));
    }
    Ok(())
}

/// The order a list was written in is not what it holds: each zone is sorted,
/// and duplicates are one entry, before it is hashed.
fn hashed(name: &str, list: &Zones) -> Hashed {
    let canonical = |entries: &[String]| {
        let mut entries = entries.to_vec();
        entries.sort();
        entries.dedup();
        entries
    };
    digest(
        name,
        &Zones {
            allow: canonical(&list.allow),
            ask: canonical(&list.ask),
            deny: canonical(&list.deny),
        },
    )
}

fn hashed_files(name: &str, list: &FilesList) -> Hashed {
    let canonical = |rules: &[PathRule]| {
        let mut rules = rules.to_vec();
        rules.sort_by(|a, b| (&a.path, a.access).cmp(&(&b.path, b.access)));
        rules.dedup();
        rules
    };
    let mut deny = list.deny.clone();
    deny.sort();
    deny.dedup();
    digest(
        name,
        &FilesList {
            allow: canonical(&list.allow),
            ask: canonical(&list.ask),
            deny,
        },
    )
}

fn digest(name: &str, value: &impl Serialize) -> Hashed {
    let json = serde_json::to_vec(value).expect("a list is plain data");
    let sum = Sha256::digest(&json);
    Hashed {
        name: name.to_string(),
        sha256: sum.iter().map(|byte| format!("{byte:02x}")).collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::sandbox::Access;

    fn policies(text: &str) -> Policies {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            shell: BTreeMap<String, Zones>,
            #[serde(default)]
            network: BTreeMap<String, Zones>,
            #[serde(default)]
            files: BTreeMap<String, FilesList>,
            #[serde(default)]
            environment: BTreeMap<String, Environment>,
        }
        let file: File = toml::from_str(text).expect("the test's TOML");
        Policies {
            shell: file.shell,
            network: file.network,
            files: file.files,
            environments: file.environment,
        }
    }

    const CONTAINER: &str = r#"
        [shell.rust]
        allow = ["cargo", "rustc", "git"]
        ask   = ["*"]
        deny  = ["curl"]

        [network.registries]
        allow = ["crates.io", "*.crates.io"]

        [network.off]

        [files.project]
        allow = [{ path = ".", access = "read-write" }]

        [environment.container]
        runtime = "docker"
        image   = "luu-worker:dev"
        image-paths = [{ path = "/usr/local/cargo", access = "execute" }]
        timeout-ms = 60000
        shell   = "rust"
        network = "registries"
        files   = "project"
        limits  = { cpu-seconds = 600, file-size-mb = 1024 }

        [environment.local]
        shell   = "rust"
        network = "off"
        files   = "project"
    "#;

    /// What an environment's `allow` becomes is exactly what a policy file's
    /// `[sandbox]` and `[worker]` would have said.
    #[test]
    fn an_environment_resolves_into_the_sandbox_and_the_worker() {
        let policies = policies(CONTAINER);
        policies.check(&[]).expect("it holds together");
        let resolved = policies.resolve("container").expect("named");

        assert_eq!(resolved.policy.commands, ["cargo", "rustc", "git"]);
        assert_eq!(
            resolved.policy.paths,
            [PathRule::new(".", Access::ReadWrite)]
        );
        assert!(resolved.policy.network);
        assert_eq!(resolved.policy.egress, ["crates.io", "*.crates.io"]);
        assert_eq!(resolved.policy.limits.cpu_seconds, Some(600));
        assert_eq!(resolved.worker.runtime, Runtime::Docker);
        assert_eq!(resolved.worker.image.as_deref(), Some("luu-worker:dev"));
        assert_eq!(resolved.worker.timeout_ms, 60000);
        assert_eq!(
            resolved.worker.paths,
            [PathRule::new("/usr/local/cargo", Access::Execute)]
        );

        // Nothing asks yet: `*` in ask and `curl` in deny grant nothing.
        assert!(
            !resolved
                .policy
                .commands
                .iter()
                .any(|c| c == "*" || c == "curl")
        );

        let local = policies.resolve("local").expect("named");
        assert!(!local.policy.network, "an empty allow is no network");
        assert_eq!(local.worker.runtime, Runtime::Host);
    }

    /// A posture and an environment that grant the same are the same lists.
    #[test]
    fn a_policy_file_and_an_environment_that_allow_the_same_hash_the_same() {
        let policies = policies(
            r#"
            [shell.s]
            allow = ["git", "cargo"]
            [network.n]
            [files.f]
            allow = [{ path = ".", access = "read-write" }]
            [environment.e]
            shell = "s"
            network = "n"
            files = "f"
            "#,
        );
        let resolved = policies.resolve("e").expect("named");
        let from_file = lists_of_file(
            "luu.toml",
            &SandboxPolicy {
                commands: vec!["cargo".into(), "git".into()],
                ..SandboxPolicy::default()
            },
        );
        let from_environment = resolved.lists(&resolved.policy);
        assert!(
            from_environment.same(&from_file),
            "order is not content, and names are labels"
        );
        assert_eq!(from_environment.shell.name, "s");
        assert_eq!(from_file.shell.name, "luu.toml");
        assert_eq!(from_file.shell.sha256.len(), 64);
    }

    /// The page's view: an environment's zones as written; a policy file's
    /// grants in `allow`, with paths under the base spelt from it.
    #[test]
    fn the_lists_are_shown_as_written_and_a_policy_file_as_allow() {
        let policies = policies(CONTAINER);
        let view = ListsView::of_environment(&policies.resolve("local").expect("named"));
        assert_eq!(view.environment.as_deref(), Some("local"));
        assert_eq!(view.shell.name, "rust");
        assert_eq!(view.shell.list.ask, ["*"]);
        assert_eq!(view.shell.list.deny, ["curl"]);
        let json = serde_json::to_value(&view).expect("serialises");
        assert_eq!(
            json["shell"]["deny"][0], "curl",
            "the zones sit beside the name"
        );
        assert_eq!(
            json["enforcement"], "kernel",
            "the default, and the strict one"
        );

        let base = std::path::Path::new("/work/repo");
        let view = ListsView::of_policy(
            "luu.toml",
            &SandboxPolicy {
                commands: vec!["git".into()],
                paths: vec![
                    PathRule::new(base, Access::ReadWrite),
                    PathRule::new(base.join("docs"), Access::Read),
                    PathRule::new("/opt/tools", Access::Read),
                ],
                network: true,
                ..SandboxPolicy::default()
            },
            base,
        );
        assert_eq!(view.environment, None);
        assert_eq!(view.shell.list.allow, ["git"]);
        assert!(view.shell.list.ask.is_empty() && view.shell.list.deny.is_empty());
        assert_eq!(
            view.network.list.allow,
            ["*"],
            "the network, with no egress list"
        );
        let paths: Vec<_> = view
            .files
            .list
            .allow
            .iter()
            .map(|rule| rule.path.display().to_string())
            .collect();
        assert_eq!(paths, [".", "docs", "/opt/tools"]);
    }

    /// What is asked for is part of what a session could do, so it is in the hash.
    #[test]
    fn ask_and_deny_are_in_the_hash_and_a_flag_that_widens_is_too() {
        let policies = policies(CONTAINER);
        let resolved = policies.resolve("container").expect("named");
        let plain = resolved.lists(&resolved.policy);

        let mut narrower = policies.clone();
        narrower.shell.get_mut("rust").unwrap().ask.clear();
        let asked_less = narrower.resolve("container").unwrap();
        assert!(!asked_less.lists(&asked_less.policy).same(&plain));

        let mut widened = resolved.policy.clone();
        widened.commands.push("make".into());
        assert!(
            !resolved.lists(&widened).same(&plain),
            "an --allow-command is recorded"
        );
    }

    #[test]
    fn what_does_not_hold_together_is_refused_and_says_where() {
        let refused =
            |text: &str, postures: &[String]| policies(text).check(postures).expect_err("refused");
        let env = "[network.n]\n[files.f]\n[environment.e]\nshell = \"s\"\nnetwork = \"n\"\nfiles = \"f\"\n";

        assert!(refused(env, &[]).contains("there is no [shell.s]"));
        assert!(
            refused(&format!("[shell.s]\nallow = [\"*\"]\n{env}"), &[])
                .contains("write the programs")
        );
        assert!(refused("[shell.s]\nallow = [\"sh -c\"]\n", &[]).contains("no spaces"));
        assert!(refused("[shell.s]\nallow = [\"/bin/sh\"]\n", &[]).contains("never a path"));
        assert!(
            refused("[shell.s]\nallow = [\"git\"]\ndeny = [\"git\"]\n", &[])
                .contains("in both allow and deny")
        );
        assert!(refused("[network.n]\ndeny = [\"*\"]\n", &[]).contains("denied already"));
        assert!(
            refused(
                "[files.f]\nallow = [{ path = \".\" }]\ndeny = [\".env\"]\n",
                &[]
            )
            .contains("not accepted until it can")
        );
        assert!(refused(&format!("[shell.s]\n{env}"), &["e".to_string()]).contains("share a name"));

        // `*` on a network list is any host, as `network = true` with no egress.
        let any = policies("[network.n]\nallow = [\"*\"]\n");
        any.check(&[]).expect("any host is a thing to say");
        assert_eq!(network_of(&any.network["n"].allow), (true, Vec::new()));
    }
}
