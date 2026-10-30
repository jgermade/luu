//! What a draft may still do when its floor refuses a call: ask a person, or
//! run what a person already allowed for the session.
//!
//! Step 2a of `RECORD/2026-10-22.policies-not-postures.WIP.md`. Asked in a
//! draft only — inside an approved job the plan is what was approved.
//!
//! **Two zones since 2026-10-23**: what a list allows runs, what it denies is
//! never asked, and *everything else is asked* — the `ask` zone went, since
//! it said what no list now says anyway, and the loader refuses one. So:
//!
//! - **a write** the floor refused, inside the working directory and under no
//!   files `deny`: the write tools check their path before they touch it, so
//!   a yes runs the same call with the folder writable;
//! - **a command** the floor does not run because the shell list does not
//!   allow it, and does not deny it. A yes runs it **with everything it
//!   implies**: the folder writable and the session's network, as decided on
//!   2026-10-23 and to be looked at again.
//!
//! Outside the working directory nothing is asked: the folder is the
//! boundary.
//!
//! A person answers once, or for the session; for the session a yes runs what
//! it covers without asking again, and — since protocol 11 — a no refuses it
//! without asking again, until either is taken back.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent_core::agent::{Ceiling, Escalation};
use agent_core::protocol::{Grant, GrantKind};
use agent_core::sandbox::{Authority, Enforcement, Sandbox};
use agent_core::tools::{ToolCall, writes_one_path};

use crate::policies::Resolved;

/// What a person answered for the session and is still in force: what runs
/// without asking, and what is refused without asking. Never both for one
/// thing: an answer in force means nobody is asked about it again.
#[derive(Debug, Default)]
pub struct Answers {
    pub allowed: BTreeSet<Grant>,
    pub forbidden: BTreeSet<Grant>,
}

impl Answers {
    /// What a session's list leaves in force: every answer not taken back.
    pub fn in_force(list: &[agent_core::api::SessionAnswer]) -> Self {
        let mut answers = Self::default();
        for answer in list.iter().filter(|answer| answer.holds()) {
            answers.give(answer.grant(), answer.allow);
        }
        answers
    }

    /// A yes or a no for the session. Whether it is news.
    pub fn give(&mut self, grant: Grant, allow: bool) -> bool {
        let (to, from) = match allow {
            true => (&mut self.allowed, &mut self.forbidden),
            false => (&mut self.forbidden, &mut self.allowed),
        };
        from.remove(&grant);
        to.insert(grant)
    }

    /// Takes back whatever is in force for `grant`. Whether there was one.
    pub fn take_back(&mut self, grant: &Grant) -> bool {
        // Both, not either: `|` rather than `||`.
        self.allowed.remove(grant) | self.forbidden.remove(grant)
    }

    pub fn clear(&mut self) {
        self.allowed.clear();
        self.forbidden.clear();
    }
}

/// [`Answers`], shared between the turn that asks and the socket that takes
/// one back.
pub type Answered = Arc<Mutex<Answers>>;

/// How far a refused call in a draft may go.
pub struct DraftCeiling {
    /// The session's sandbox with its working directory writable, stamped as
    /// a person's: what a yes to a write runs under, and what a yes to a
    /// command starts from.
    wider: Sandbox,
    /// Programs the shell list denies: never asked.
    denied_commands: Vec<String>,
    /// Paths the files list denies, relative to the working directory: never
    /// asked to be written.
    denied_paths: Vec<String>,
    answers: Answered,
}

impl DraftCeiling {
    pub fn new(
        session: &Sandbox,
        environment: Option<&Resolved>,
        answers: Answered,
        authority: Authority,
    ) -> Self {
        let mut policy = session.to_policy();
        crate::policies::widen(
            &mut policy,
            Some(session.base()),
            std::iter::once(agent_core::sandbox::PathRule::new(
                ".",
                agent_core::sandbox::Access::ReadWrite,
            )),
        );
        let wider = Sandbox::new(&policy, session.base())
            .unwrap_or_else(|_| session.clone())
            .under(authority);
        Self {
            wider,
            denied_commands: environment
                .map(|environment| environment.shell().deny.clone())
                .unwrap_or_default(),
            denied_paths: environment
                .map(|environment| environment.files().deny.clone())
                .unwrap_or_default(),
            answers,
        }
    }
}

impl Ceiling for DraftCeiling {
    fn escalate(&self, call: &ToolCall, refused_under: &Sandbox) -> Escalation {
        let Some(grant) = grant_for(call, refused_under.base()) else {
            return Escalation::Refused;
        };
        let wider = match grant.kind {
            GrantKind::Write => {
                // Refused for something other than where it writes: the same
                // refusal under any sandbox, so nobody is asked. Nor is a write
                // outside the folder, or one a files `deny` covers.
                if writes_one_path(call, refused_under)
                    || !writes_one_path(call, &self.wider)
                    || denied(&self.denied_paths, &grant.target)
                {
                    return Escalation::Refused;
                }
                self.wider.clone()
            }
            GrantKind::Command => {
                let program = grant.target.as_str();
                // Denied, or already allowed and refused for something a yes
                // would not change. Allowed and refused all the same is the
                // kernel's gap — the one refusal left once the name passes —
                // which a yes may run past, below.
                let listed = refused_under
                    .commands()
                    .iter()
                    .any(|allowed| allowed == program);
                if self.denied_commands.iter().any(|denied| denied == program)
                    || (listed && refused_under.prepare_command(program).is_ok())
                {
                    return Escalation::Refused;
                }
                let mut policy = self.wider.to_policy();
                policy.allow_command(program);
                let Ok(mut sandbox) = Sandbox::new(&policy, self.wider.base()) else {
                    return Escalation::Refused;
                };
                // **A person's yes runs what the kernel here cannot hold.**
                // Off Linux there is no Landlock and no seccomp, and under
                // `kernel` every command is refused — so a yes would be a
                // question whose answer changes nothing. The yes is the
                // person's, about this command, told before the click that
                // only its resource limits will hold it (`agent::unheld`), and
                // the verdict names the gap. What nobody answered is still
                // refused: the floor is not widened. See
                // `RECORD/2026-10-27.a-yes-runs-what-the-kernel-cannot-hold.completed.md`.
                if sandbox.prepare_command(program).is_err() {
                    policy.enforcement = Enforcement::BestEffort;
                    match Sandbox::new(&policy, self.wider.base()) {
                        Ok(lenient) => sandbox = lenient,
                        Err(_) => return Escalation::Refused,
                    }
                }
                sandbox.under(self.wider.authority().clone())
            }
        };
        // Only what could be asked about can have been answered, so a no for
        // the session is looked for here rather than before the lists.
        let answers = self
            .answers
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if answers.forbidden.contains(&grant) {
            Escalation::Forbidden
        } else if answers.allowed.contains(&grant) {
            Escalation::Granted(wider)
        } else {
            Escalation::Ask(wider)
        }
    }
}

/// Whether `target` — relative to the working directory — is under a denied
/// path: the path itself or anything inside it.
fn denied(paths: &[String], target: &str) -> bool {
    let target = Path::new(target);
    paths.iter().any(|path| {
        let path = Path::new(path.trim_start_matches("./"));
        path.as_os_str() == "." || target.starts_with(path)
    })
}

/// What a session grant for `call` would be: the program `run_command` runs,
/// by its name — `apt-get` for `install` — or the path a file tool writes, relative to `base` where it is
/// under it. `None` for a call no grant is about.
pub fn grant_for(call: &ToolCall, base: &Path) -> Option<Grant> {
    match call.name.as_str() {
        "run_command" => {
            let program = call.arguments.get("argv")?.as_array()?.first()?.as_str()?;
            // A path is not a program's name: the allowlist names programs.
            (!program.is_empty() && !program.contains('/')).then(|| Grant {
                kind: GrantKind::Command,
                target: program.to_string(),
            })
        }
        // `install` is `apt-get` to the shell list: a yes to it is a yes to
        // the program it runs. See `agent_core::tools::install`.
        "install" => Some(Grant {
            kind: GrantKind::Command,
            target: agent_core::tools::install::APT.to_string(),
        }),
        "write_file" | "edit_file" => {
            let path = call.arguments.get("path")?.as_str()?;
            Some(Grant {
                kind: GrantKind::Write,
                target: relative(base, Path::new(path)),
            })
        }
        _ => None,
    }
}

/// `path` without `.` and `..`, and relative to `base` where it is inside it:
/// `./src/../a.txt` and `/base/a.txt` are one grant.
fn relative(base: &Path, path: &Path) -> String {
    let joined = match path.is_absolute() {
        true => path.to_path_buf(),
        false => base.join(path),
    };
    let mut clean = PathBuf::new();
    for part in joined.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                clean.pop();
            }
            other => clean.push(other),
        }
    }
    match clean.strip_prefix(base) {
        Ok(inside) => inside.display().to_string(),
        Err(_) => clean.display().to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::policies::Zones;
    use agent_core::sandbox::{Access, PathRule, SandboxPolicy};

    fn call(name: &str, arguments: serde_json::Value) -> ToolCall {
        ToolCall {
            name: name.into(),
            arguments,
        }
    }

    fn scratch(name: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("luu-grants-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(&root).unwrap();
        root.canonicalize().unwrap()
    }

    fn environment(shell: &str) -> Resolved {
        let text = format!(
            "{shell}\n[network.n]\n[files.f]\nallow = [{{ path = \".\", access = \"read-write\" }}]\n\
             [environment.e]\nshell = \"s\"\nnetwork = \"n\"\nfiles = \"f\"\n"
        );
        crate::provider::Config::from_toml(&text, "config.toml")
            .expect("the test's config")
            .policies()
            .resolve("e")
            .expect("named")
    }

    fn sandboxes(root: &Path, environment: &Resolved) -> (Sandbox, Sandbox) {
        let session = Sandbox::new(&environment.policy, root).unwrap();
        let floor = session.read_only();
        (session, floor)
    }

    #[test]
    fn a_grant_names_the_program_or_the_path_as_one_thing() {
        let base = Path::new("/work/project");
        let run = call(
            "run_command",
            serde_json::json!({ "argv": ["npm", "install"] }),
        );
        assert_eq!(
            grant_for(&run, base),
            Some(Grant {
                kind: GrantKind::Command,
                target: "npm".into()
            })
        );
        let by_path = call(
            "run_command",
            serde_json::json!({ "argv": ["/usr/bin/npm"] }),
        );
        assert_eq!(
            grant_for(&by_path, base),
            None,
            "a path is not a program's name"
        );
        for path in ["src/../a.txt", "./a.txt", "/work/project/a.txt"] {
            let write = call(
                "write_file",
                serde_json::json!({ "path": path, "content": "" }),
            );
            assert_eq!(grant_for(&write, base).unwrap().target, "a.txt", "{path}");
        }
        assert_eq!(
            grant_for(
                &call("read_file", serde_json::json!({ "path": "a.txt" })),
                base
            ),
            None
        );
        // `install` is the program it runs, whatever it installs.
        assert_eq!(
            grant_for(
                &call("install", serde_json::json!({ "packages": ["jq"] })),
                base
            ),
            Some(Grant {
                kind: GrantKind::Command,
                target: "apt-get".into()
            })
        );
    }

    /// Two zones: what the list allows runs, what it denies is never asked,
    /// and everything else — what it names in neither — is.
    #[test]
    fn a_command_not_denied_is_asked_and_one_denied_is_not() {
        let root = scratch("commands");
        let environment = environment("[shell.s]\nallow = [\"ls\"]\ndeny = [\"curl\"]\n");
        let (session, floor) = sandboxes(&root, &environment);
        let answers = Answered::default();
        let ceiling = DraftCeiling::new(
            &session,
            Some(&environment),
            answers.clone(),
            Authority::Person(None),
        );
        let run = |program: &str| call("run_command", serde_json::json!({ "argv": [program] }));

        match ceiling.escalate(&run("git"), &floor) {
            Escalation::Ask(wider) => {
                assert!(wider.commands().iter().any(|c| c == "git"));
                assert_eq!(
                    wider.access_for(&root),
                    Some(Access::ReadWrite),
                    "with its writes"
                );
                assert!(matches!(wider.authority(), Authority::Person(None)));
            }
            _ => panic!("git is in ask"),
        }
        assert!(matches!(
            ceiling.escalate(&run("curl"), &floor),
            Escalation::Refused
        ));
        assert!(
            matches!(ceiling.escalate(&run("make"), &floor), Escalation::Ask(_)),
            "in no list: asked, since 2026-10-23"
        );
        // ls is allowed: where the kernel holds it the floor runs it, so a
        // refusal was for something a yes would not change; where it cannot,
        // the refusal was the kernel's, and a yes runs past it.
        match floor.prepare_command("ls") {
            Ok(_) => assert!(
                matches!(ceiling.escalate(&run("ls"), &floor), Escalation::Refused),
                "ls was not refused for its name, so a yes would change nothing"
            ),
            Err(_) => assert!(matches!(
                ceiling.escalate(&run("ls"), &floor),
                Escalation::Ask(_)
            )),
        }

        let git = Grant {
            kind: GrantKind::Command,
            target: "git".into(),
        };
        answers.lock().unwrap().give(git.clone(), true);
        assert!(matches!(
            ceiling.escalate(&run("git"), &floor),
            Escalation::Granted(_)
        ));

        // A no for the session replaces the yes, and is refused unasked.
        answers.lock().unwrap().give(git.clone(), false);
        assert!(matches!(
            ceiling.escalate(&run("git"), &floor),
            Escalation::Forbidden
        ));
        // Taken back: asked again.
        assert!(answers.lock().unwrap().take_back(&git));
        assert!(
            !answers.lock().unwrap().take_back(&git),
            "nothing left to take"
        );
        assert!(matches!(
            ceiling.escalate(&run("git"), &floor),
            Escalation::Ask(_)
        ));
        // And a no never reaches past the lists: curl is denied, not forbidden.
        answers.lock().unwrap().give(
            Grant {
                kind: GrantKind::Command,
                target: "curl".into(),
            },
            false,
        );
        assert!(matches!(
            ceiling.escalate(&run("curl"), &floor),
            Escalation::Refused
        ));
    }

    /// A yes runs: whatever holds a command on this machine, the sandbox a
    /// person's yes runs it under can run it — the kernel where there is one,
    /// its resource limits where there is none — and the question says what
    /// will not hold it exactly where something will not.
    #[test]
    fn a_yes_to_a_command_can_run_it_here() {
        let root = scratch("unheld");
        let environment = environment("[shell.s]\nallow = [\"ls\"]\n");
        let (session, floor) = sandboxes(&root, &environment);
        let ceiling = DraftCeiling::new(
            &session,
            Some(&environment),
            Answered::default(),
            Authority::Person(None),
        );
        let git = call("run_command", serde_json::json!({ "argv": ["git"] }));
        let Escalation::Ask(wider) = ceiling.escalate(&git, &floor) else {
            panic!("git is asked")
        };
        assert!(wider.prepare_command("git").is_ok(), "a yes runs it");
        let held_here = session.prepare_command("ls").is_ok();
        assert_eq!(
            agent_core::agent::unheld(&git, &wider).is_none(),
            held_here,
            "said where the kernel cannot hold it, and only there"
        );
    }

    /// A resume puts back what the list leaves in force, yes and no.
    #[test]
    fn what_is_in_force_is_what_was_not_taken_back() {
        use agent_core::api::SessionAnswer;
        let answer = |target: &str, allow: bool, revoked: bool| SessionAnswer {
            kind: GrantKind::Command,
            target: target.into(),
            allow,
            turn: 1,
            step: 1,
            revoked,
        };
        let answers = Answers::in_force(&[
            answer("git", true, true),
            answer("npm", true, false),
            answer("curl", false, false),
            answer("make", false, true),
        ]);
        let names = |set: &BTreeSet<Grant>| -> Vec<String> {
            set.iter().map(|grant| grant.target.clone()).collect()
        };
        assert_eq!(names(&answers.allowed), ["npm"]);
        assert_eq!(names(&answers.forbidden), ["curl"]);
    }

    /// At the gate the approval is the person's yes, for the job: a plan may
    /// get whatever the lists do not deny, and never what they deny.
    #[test]
    fn the_gate_may_grant_what_the_lists_do_not_deny() {
        let root = scratch("gate");
        let text = "[shell.s]\nallow = [\"ls\"]\ndeny = [\"curl\"]\n\
                    [network.n]\n\
                    [files.f]\n\
                    [environment.e]\nshell = \"s\"\nnetwork = \"n\"\nfiles = \"f\"\n";
        let environment = std::sync::Arc::new(
            crate::provider::Config::from_toml(text, "config.toml")
                .unwrap()
                .policies()
                .resolve("e")
                .unwrap(),
        );
        let session = Sandbox::new(&environment.policy, &root).unwrap();
        let agency = crate::session::Agency::new(
            std::sync::Arc::new(agent_core::tools::Tools::standard()),
            std::sync::Arc::new(session),
            agent_core::agent::Limits::default(),
            None,
        )
        .with_environment(environment);
        assert!(
            !agency.sandbox.network(),
            "the session itself reaches nothing"
        );
        assert_eq!(
            agency.sandbox.access_for(&root),
            Some(Access::Read),
            "an empty files list still reads the folder"
        );

        let gate = agency.gate_sandbox(&["git".into(), "curl".into()]);
        assert!(
            gate.commands().iter().any(|c| c == "git"),
            "asked for by the plan, and in no list"
        );
        assert!(
            !gate.commands().iter().any(|c| c == "curl"),
            "denied stays denied"
        );
        assert!(gate.network(), "no host is denied");
        assert!(gate.egress().is_empty(), "so any");
        assert_eq!(gate.access_for(&root), Some(Access::ReadWrite));

        // A host denied keeps a plan to what is allowed: the egress list
        // cannot say *all but these*.
        let text = text.replace("[network.n]\n", "[network.n]\ndeny = [\"evil.example\"]\n");
        let denying = std::sync::Arc::new(
            crate::provider::Config::from_toml(&text, "config.toml")
                .unwrap()
                .policies()
                .resolve("e")
                .unwrap(),
        );
        let session = Sandbox::new(&denying.policy, &root).unwrap();
        let agency = crate::session::Agency::new(
            std::sync::Arc::new(agent_core::tools::Tools::standard()),
            std::sync::Arc::new(session),
            agent_core::agent::Limits::default(),
            None,
        )
        .with_environment(denying);
        assert!(!agency.gate_sandbox(&[]).network());
    }

    /// A write inside the folder is asked, one a files `deny` covers is not,
    /// and one outside the folder is not: the folder is the boundary. The
    /// loader refuses a files `deny` until a command can be held to it (row
    /// 49), so the lists are built here without it.
    #[test]
    fn a_write_is_asked_inside_the_folder_and_not_where_it_is_denied() {
        let root = scratch("writes");
        let mut policies = crate::policies::Policies::default();
        policies.shell.insert("s".into(), Zones::default());
        policies.network.insert("n".into(), Zones::default());
        policies.files.insert(
            "f".into(),
            crate::policies::FilesList {
                deny: vec![".git".into()],
                ..Default::default()
            },
        );
        let denying = policies
            .compose(
                &crate::policies::Place::host(),
                Some("s"),
                Some("n"),
                Some("f"),
            )
            .expect("composes");
        let (session, floor) = sandboxes(&root, &denying);
        let ceiling = DraftCeiling::new(
            &session,
            Some(&denying),
            Answered::default(),
            Authority::Person(None),
        );
        let write = |path: &str| {
            call(
                "write_file",
                serde_json::json!({ "path": path, "content": "x" }),
            )
        };
        assert!(matches!(
            ceiling.escalate(&write("notes.md"), &floor),
            Escalation::Ask(_)
        ));
        assert!(matches!(
            ceiling.escalate(&write(".git/config"), &floor),
            Escalation::Refused
        ));
        assert!(matches!(
            ceiling.escalate(&write("/etc/elsewhere"), &floor),
            Escalation::Refused
        ));
    }

    /// A posture — a policy file — has no lists, so nothing is denied: a
    /// command it does not allow is asked, as a write is.
    #[test]
    fn under_a_posture_what_it_does_not_allow_is_asked() {
        let root = scratch("posture");
        let policy = SandboxPolicy {
            paths: vec![PathRule::new(".", Access::ReadWrite)],
            commands: vec!["ls".into()],
            ..SandboxPolicy::default()
        };
        let session = Sandbox::new(&policy, &root).unwrap();
        let floor = session.read_only();
        let ceiling =
            DraftCeiling::new(&session, None, Answered::default(), Authority::Person(None));
        let write = call(
            "write_file",
            serde_json::json!({ "path": "a.txt", "content": "x" }),
        );
        assert!(matches!(
            ceiling.escalate(&write, &floor),
            Escalation::Ask(_)
        ));
        let run = call("run_command", serde_json::json!({ "argv": ["git"] }));
        assert!(matches!(ceiling.escalate(&run, &floor), Escalation::Ask(_)));
    }
}
