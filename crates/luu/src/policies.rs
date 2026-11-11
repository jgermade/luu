//! What a session may do, as lists by axis — shell, network, files — in
//! `config.toml` beside the providers.
//!
//! See `RECORD/2026-10-22.policies-not-postures.WIP.md`. **Two zones.** A
//! list's `allow` is what the session's sandbox grants, exactly as a policy
//! file's `[sandbox]` does; its `deny` is never granted, by the gate or by a
//! person. What a list names in neither is asked: of the person in a draft
//! ([`crate::grants::DraftCeiling`]), and given to a plan at the gate when the
//! plan names it. Every session reads its working directory, whatever its
//! files list says.
//!
//! ```toml
//! [shell.rust]
//! allow = ["cargo", "rustc", "git", "rg", "ls"]
//! deny  = ["curl"]
//!
//! [network.registries]
//! allow = ["crates.io", "*.crates.io"]
//!
//! [files.project]
//! allow = [{ path = ".", access = "read-write" }]
//! ```
//!
//! **A session is a runtime and one list of each axis, copied.** The lists
//! are the instance's — this file's — and a session takes the ones a person
//! chose, whole, into its own header ([`Policies::compose`],
//! [`Policies::recompose`]). Nothing names a combination: `[environment.*]`
//! and `[posture.*]` are gone, and a `config.toml` that still has one does
//! not load. See
//! `RECORD/2026-11-10.a-session-is-a-runtime-and-three-lists.completed.md`.

use std::collections::BTreeMap;

use agent_core::record::{Copied, Hashed, Lists};
use agent_core::sandbox::{PathRule, SandboxPolicy};
use agent_core::worker::{Runtime, WorkerConfig};
use serde::Serialize;
use sha2::{Digest, Sha256};

pub use agent_core::record::{FilesList, Zones};

/// Every list `config.toml` names.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Policies {
    pub shell: BTreeMap<String, Zones>,
    pub network: BTreeMap<String, Zones>,
    pub files: BTreeMap<String, FilesList>,
}

/// Where a session runs: a runtime and, in a container, the image and the
/// trees that exist only inside it — taken from an `[image.<name>]` or the
/// image `serve` was started on, never typed by a page. See
/// [`Policies::compose`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Place {
    pub runtime: Runtime,
    pub image: Option<String>,
    pub image_paths: Vec<PathRule>,
}

impl Place {
    /// This machine, with no container.
    pub fn host() -> Self {
        Self {
            runtime: Runtime::Host,
            image: None,
            image_paths: Vec::new(),
        }
    }
}

/// A place and three lists, resolved into what luu builds a session from.
#[derive(Debug, Clone)]
pub struct Resolved {
    /// Its lists' names: `tools · registries · empty`, or `empty`.
    pub name: String,
    /// What the sandbox grants: the working directory to read, and every
    /// list's `allow`.
    pub policy: SandboxPolicy,
    pub worker: WorkerConfig,
    shell: (String, Zones),
    network: (String, Zones),
    files: (String, FilesList),
}

/// The name an absent list has in the record: the empty list, which allows
/// nothing and so asks about everything. Not a bare TOML key, so no list of
/// the file can be called it.
pub const EMPTY: &str = "(empty)";

impl Policies {
    /// Whether each list holds together.
    pub fn check(&self) -> Result<(), String> {
        for (name, list) in &self.shell {
            check_zones(&format!("[shell.{name}]"), list, false)?;
        }
        for (name, list) in &self.network {
            check_zones(&format!("[network.{name}]"), list, true)?;
        }
        for (name, list) in &self.files {
            check_files(&format!("[files.{name}]"), list)?;
        }
        Ok(())
    }

    /// A session's policy made from where it runs and one list of each axis
    /// by name — `None` for the empty one, [`EMPTY`] in the record. Named by
    /// its lists; a resume makes it again from the copy it recorded
    /// ([`Policies::recompose`]), not from these names.
    pub fn compose(
        &self,
        place: &Place,
        shell: Option<&str>,
        network: Option<&str>,
        files: Option<&str>,
    ) -> Result<Resolved, String> {
        fn pick<T: Clone + Default>(
            kind: &str,
            lists: &BTreeMap<String, T>,
            name: Option<&str>,
        ) -> Result<(String, T), String> {
            match name {
                None | Some(EMPTY) => Ok((EMPTY.to_string(), T::default())),
                Some(name) => lists
                    .get(name)
                    .map(|list| (name.to_string(), list.clone()))
                    .ok_or_else(|| format!("there is no [{kind}.{name}]")),
            }
        }
        let shell = pick("shell", &self.shell, shell)?;
        let network = pick("network", &self.network, network)?;
        let files = pick("files", &self.files, files)?;
        let shown = |name: &str| match name {
            EMPTY => "empty".to_string(),
            name => name.to_string(),
        };
        let name = match (shell.0.as_str(), network.0.as_str(), files.0.as_str()) {
            (EMPTY, EMPTY, EMPTY) => "empty".to_string(),
            _ => format!(
                "{} · {} · {}",
                shown(&shell.0),
                shown(&network.0),
                shown(&files.0)
            ),
        };
        let mut policy = SandboxPolicy {
            paths: Vec::new(),
            commands: shell.1.allow.clone(),
            ..SandboxPolicy::default()
        };
        // Every session reads the folder it works in: what a draft is for,
        // whatever the files list says beyond it. Since 2026-10-23 — before,
        // a list with no `.` read nothing.
        widen(
            &mut policy,
            None,
            std::iter::once(PathRule::new(".", agent_core::sandbox::Access::Read))
                .chain(files.1.allow.iter().cloned()),
        );
        (policy.network, policy.egress) = network_of(&network.1.allow);
        let worker = WorkerConfig {
            runtime: place.runtime,
            image: place.image.clone(),
            paths: place.image_paths.clone(),
            ..WorkerConfig::default()
        };
        Ok(Resolved {
            name,
            policy,
            worker,
            shell,
            network,
            files,
        })
    }

    /// A session made again from the lists it copied when it started — not
    /// from `config.toml`, whose lists may have been edited, renamed or
    /// deleted since — on `place`, under the names it recorded. `None` for
    /// lists recorded without their copy: a policy file's, or a stream from
    /// before format 26, which are read and not resumed.
    pub fn recompose(place: &Place, lists: &Lists) -> Option<Resolved> {
        let copied = lists.copied.as_deref()?;
        let mut policies = Policies::default();
        policies
            .shell
            .insert(lists.shell.name.clone(), copied.shell.clone());
        policies
            .network
            .insert(lists.network.name.clone(), copied.network.clone());
        policies
            .files
            .insert(lists.files.name.clone(), copied.files.clone());
        policies
            .compose(
                place,
                Some(&lists.shell.name),
                Some(&lists.network.name),
                Some(&lists.files.name),
            )
            .ok()
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
    /// `deny` as written.
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
            // As written, and not read back off the sandbox as the hashes
            // are: what [`Policies::recompose`] is given, so the flags are
            // laid over it again rather than baked into it.
            copied: Some(Box::new(Copied {
                shell: self.shell.1.clone(),
                network: self.network.1.clone(),
                files: self.files.1.clone(),
            })),
        }
    }
}

/// The lists a session was built from, as the page shows them: by name, each
/// zone as written. A policy file's are all `allow`, named after the file.
#[derive(Debug, Clone, Serialize, PartialEq, Eq)]
pub struct ListsView {
    /// Made from lists the session copied — every session of `serve` — so a
    /// list named [`EMPTY`] is the empty one rather than a table. False for
    /// a one-shot's policy file, whose grants are all `allow`.
    #[serde(skip_serializing_if = "std::ops::Not::not")]
    pub composed: bool,
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
    /// A session's lists, as written.
    pub fn of_lists(resolved: &Resolved) -> Self {
        Self {
            composed: true,
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
            composed: false,
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

/// The lists of a one-shot built from a policy file (`luu chat --sandbox`),
/// which has everything in `allow`. `label` names all three:
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
        // A policy file is not a session's to carry: it is not resumed.
        copied: None,
    }
}

/// Adds `rules` to `policy`'s paths, a path already there taking the greater
/// of the two accesses. Two rules for one path would otherwise both stand, and
/// the sandbox answers with the first — so a list's `allow` for writing `.`
/// beside the read every session has of it would read as read-only. `base` is what relative
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
    let zones = [("allow", &list.allow), ("deny", &list.deny)];
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
                    "deny" => {
                        "what a list does not name is asked, so deny names what is never given"
                    }
                    _ => "write the programs: what a list does not name is asked",
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
    if let Some(rule) = list
        .allow
        .iter()
        .find(|rule| rule.path.as_os_str().is_empty())
    {
        return Err(format!("{at} allow has a path that is empty ({rule:?})"));
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
    use serde::Deserialize;

    fn policies(text: &str) -> Policies {
        #[derive(Deserialize)]
        struct File {
            #[serde(default)]
            shell: BTreeMap<String, Zones>,
            #[serde(default)]
            network: BTreeMap<String, Zones>,
            #[serde(default)]
            files: BTreeMap<String, FilesList>,
        }
        let file: File = toml::from_str(text).expect("the test's TOML");
        Policies {
            shell: file.shell,
            network: file.network,
            files: file.files,
        }
    }

    const LISTS: &str = r#"
        [shell.rust]
        allow = ["cargo", "rustc", "git"]
        deny  = ["curl"]

        [network.registries]
        allow = ["crates.io", "*.crates.io"]

        [network.off]

        [files.project]
        allow = [{ path = ".", access = "read-write" }]
    "#;

    fn docker() -> Place {
        Place {
            runtime: Runtime::Docker,
            image: Some("luu-worker:dev".into()),
            image_paths: vec![PathRule::new("/usr/local/cargo", Access::Execute)],
        }
    }

    /// What the lists' `allow` becomes is exactly what a policy file's
    /// `[sandbox]` and `[worker]` would have said.
    #[test]
    fn lists_resolve_into_the_sandbox_and_the_worker() {
        let policies = policies(LISTS);
        policies.check().expect("it holds together");
        let resolved = policies
            .compose(&docker(), Some("rust"), Some("registries"), Some("project"))
            .expect("named");

        assert_eq!(resolved.policy.commands, ["cargo", "rustc", "git"]);
        assert_eq!(
            resolved.policy.paths,
            [PathRule::new(".", Access::ReadWrite)]
        );
        assert!(resolved.policy.network);
        assert_eq!(resolved.policy.egress, ["crates.io", "*.crates.io"]);
        assert_eq!(resolved.worker.runtime, Runtime::Docker);
        assert_eq!(resolved.worker.image.as_deref(), Some("luu-worker:dev"));
        assert_eq!(
            resolved.worker.paths,
            [PathRule::new("/usr/local/cargo", Access::Execute)]
        );

        // `curl` in deny grants nothing.
        assert!(!resolved.policy.commands.iter().any(|c| c == "curl"));

        let local = policies
            .compose(&Place::host(), Some("rust"), Some("off"), Some("project"))
            .expect("named");
        assert!(!local.policy.network, "an empty allow is no network");
        assert_eq!(local.worker.runtime, Runtime::Host);
    }

    /// A policy file and lists that grant the same are the same lists.
    #[test]
    fn a_policy_file_and_lists_that_allow_the_same_hash_the_same() {
        let policies = policies(
            r#"
            [shell.s]
            allow = ["git", "cargo"]
            [network.n]
            [files.f]
            allow = [{ path = ".", access = "read-write" }]
            "#,
        );
        let resolved = policies
            .compose(&Place::host(), Some("s"), Some("n"), Some("f"))
            .expect("named");
        let from_file = lists_of_file(
            "luu.toml",
            &SandboxPolicy {
                commands: vec!["cargo".into(), "git".into()],
                ..SandboxPolicy::default()
            },
        );
        let from_lists = resolved.lists(&resolved.policy);
        assert!(
            from_lists.same(&from_file),
            "order is not content, and names are labels"
        );
        assert_eq!(from_lists.shell.name, "s");
        assert_eq!(from_file.shell.name, "luu.toml");
        assert_eq!(from_file.shell.sha256.len(), 64);
    }

    /// The page's view: a session's zones as written; a policy file's
    /// grants in `allow`, with paths under the base spelt from it.
    #[test]
    fn the_lists_are_shown_as_written_and_a_policy_file_as_allow() {
        let policies = policies(LISTS);
        let view = ListsView::of_lists(
            &policies
                .compose(&Place::host(), Some("rust"), Some("off"), Some("project"))
                .expect("named"),
        );
        assert!(view.composed);
        assert_eq!(view.shell.name, "rust");
        assert_eq!(view.shell.list.allow, ["cargo", "rustc", "git"]);
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
        assert!(!view.composed);
        assert_eq!(view.shell.list.allow, ["git"]);
        assert!(view.shell.list.deny.is_empty());
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

    /// What is denied is part of what a session could do — without it, a
    /// person could be asked for it — so it is in the hash.
    #[test]
    fn deny_is_in_the_hash_and_a_flag_that_widens_is_too() {
        let policies = policies(LISTS);
        let compose = |policies: &Policies| {
            policies
                .compose(&docker(), Some("rust"), Some("registries"), Some("project"))
                .expect("named")
        };
        let resolved = compose(&policies);
        let plain = resolved.lists(&resolved.policy);

        let mut wider = policies.clone();
        wider.shell.get_mut("rust").unwrap().deny.clear();
        let denied_less = compose(&wider);
        assert!(!denied_less.lists(&denied_less.policy).same(&plain));

        let mut widened = resolved.policy.clone();
        widened.commands.push("make".into());
        assert!(
            !resolved.lists(&widened).same(&plain),
            "an --allow-command is recorded"
        );
    }

    /// A session made from lists by name: the empty list where none is
    /// chosen, the folder read whatever the lists say, and the names in the
    /// record — the empty one under a name no list can have.
    #[test]
    fn lists_by_name_compose_into_a_session_and_an_absent_one_is_empty() {
        let policies = policies(LISTS);
        let resolved = policies
            .compose(&Place::host(), Some("rust"), None, Some("project"))
            .expect("both are there");
        assert_eq!(resolved.name, "rust · empty · project");
        assert_eq!(resolved.policy.commands, ["cargo", "rustc", "git"]);
        assert!(
            !resolved.policy.network,
            "the empty network list reaches nothing"
        );
        assert_eq!(resolved.worker.runtime, Runtime::Host);
        let lists = resolved.lists(&resolved.policy);
        assert_eq!(lists.network.name, EMPTY);
        assert_eq!(lists.shell.name, "rust");

        let empty = policies
            .compose(&Place::host(), None, None, None)
            .expect("nothing to be missing");
        assert_eq!(empty.name, "empty");
        assert!(empty.policy.commands.is_empty());
        assert_eq!(empty.policy.paths, [PathRule::new(".", Access::Read)]);

        let contained = docker();
        let there = policies
            .compose(
                &contained,
                Some("rust"),
                Some("registries"),
                Some("project"),
            )
            .expect("composes");
        assert_eq!(there.worker.image.as_deref(), Some("luu-worker:dev"));
        assert_eq!(there.worker.paths, contained.image_paths);

        assert_eq!(
            policies
                .compose(&Place::host(), Some("nope"), None, None)
                .err()
                .as_deref(),
            Some("there is no [shell.nope]")
        );
    }

    /// Since format 26: a session made from lists carries them, and is made
    /// again from what it carries — the file's lists edited, renamed or gone.
    #[test]
    fn a_session_is_made_again_from_the_lists_it_copied() {
        let policies = policies(LISTS);
        let started = policies
            .compose(&Place::host(), Some("rust"), None, Some("project"))
            .expect("both are there");
        let recorded = started.lists(&started.policy);
        let copied = recorded.copied.as_deref().expect("a session copies");
        assert_eq!(copied.shell, *started.shell());
        assert_eq!(copied.network, Zones::default());

        // Through JSON, as a resume reads it, and with nothing of the file.
        let read: Lists =
            serde_json::from_str(&serde_json::to_string(&recorded).unwrap()).expect("round-trips");
        let again = Policies::recompose(&Place::host(), &read).expect("made from its copy");
        assert_eq!(again.name, started.name);
        assert_eq!(again.policy, started.policy);
        assert!(
            again.lists(&again.policy).same(&recorded),
            "the same hashes, so a resume compares equal",
        );

        // Somewhere else, the same lists.
        let contained = Place {
            runtime: Runtime::Docker,
            image: Some("luu-worker:dev".into()),
            image_paths: Vec::new(),
        };
        let moved = Policies::recompose(&contained, &read).expect("made from its copy");
        assert_eq!(moved.policy, started.policy);
        assert_eq!(moved.worker.runtime, Runtime::Docker);

        // A policy file's lists, and a stream from before 26, have no copy.
        let file = lists_of_file("luu.toml", &started.policy);
        assert!(file.copied.is_none());
        assert!(Policies::recompose(&Place::host(), &file).is_none());
    }

    #[test]
    fn what_does_not_hold_together_is_refused_and_says_where() {
        let refused = |text: &str| policies(text).check().expect_err("refused");

        assert!(refused("[shell.s]\nallow = [\"*\"]\n").contains("write the programs"));
        assert!(refused("[shell.s]\nallow = [\"sh -c\"]\n").contains("no spaces"));
        assert!(refused("[shell.s]\nallow = [\"/bin/sh\"]\n").contains("never a path"));
        assert!(
            refused("[shell.s]\nallow = [\"git\"]\ndeny = [\"git\"]\n")
                .contains("in both allow and deny")
        );
        assert!(refused("[network.n]\ndeny = [\"*\"]\n").contains("never given"));
        assert!(
            refused("[files.f]\nallow = [{ path = \".\" }]\ndeny = [\".env\"]\n")
                .contains("not accepted until it can")
        );

        // Two zones: what neither names is asked, so there is no third.
        let asking = toml::from_str::<Zones>("ask = [\"*\"]").expect_err("no ask zone");
        assert!(asking.to_string().contains("unknown field `ask`"));

        // `*` on a network list is any host, as `network = true` with no egress.
        let any = policies("[network.n]\nallow = [\"*\"]\n");
        any.check().expect("any host is a thing to say");
        assert_eq!(network_of(&any.network["n"].allow), (true, Vec::new()));
    }
}
