//! What a draft may still do when its floor refuses a call: ask a person, or
//! run what a person already allowed for the session.
//!
//! Step 2a of `RECORD/2026-10-22.policies-not-postures.WIP.md`. Asked in a
//! draft only — inside an approved job the plan is what was approved — and
//! only for what the lists leave to a person:
//!
//! - **a write** the floor refused and the session's files would allow — its
//!   `allow`, or its `ask` — which is what *allow once* always asked
//!   (`RECORD/2026-10-02.a-refused-write-asks.completed.md`);
//! - **a command** the floor does not run because the shell list does not
//!   allow it, where that list puts it in `ask` (by name, or by `*`) and not in
//!   `deny`. A yes runs it **with everything it implies**: the session's writes
//!   and its network, as decided on 2026-10-23 and to be looked at again.
//!
//! What `deny` names, and what no list names, is never asked. A posture has no
//! `ask`, so under one only the write is.
//!
//! A person answers once, or for the session; for the session a yes runs what
//! it covers without asking again, and — since protocol 11 — a no refuses it
//! without asking again, until either is taken back.

use std::collections::BTreeSet;
use std::path::{Component, Path, PathBuf};
use std::sync::{Arc, Mutex};

use agent_core::agent::{Ceiling, Escalation};
use agent_core::protocol::{Grant, GrantKind};
use agent_core::sandbox::{Authority, Sandbox};
use agent_core::tools::{ToolCall, writes_one_path};

use crate::policies::{Resolved, Zones};

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
    /// The session's sandbox and its files' `ask`, stamped as a person's: what
    /// a yes to a write runs under, and what a yes to a command starts from.
    wider: Sandbox,
    /// The environment's shell list, where there is one.
    shell: Option<Zones>,
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
        if let Some(environment) = environment {
            // A path a list asks about and that is not there yet is left out:
            // a grant on a missing path is a load error, and a write that would
            // create it is a write to its directory, which `allow` answers.
            crate::policies::widen(
                &mut policy,
                Some(session.base()),
                environment
                    .files()
                    .ask
                    .iter()
                    .filter(|rule| session.base().join(&rule.path).exists())
                    .cloned(),
            );
        }
        let wider = Sandbox::new(&policy, session.base())
            .unwrap_or_else(|_| session.clone())
            .under(authority);
        Self {
            wider,
            shell: environment.map(|environment| environment.shell().clone()),
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
                // refusal under any sandbox, so nobody is asked.
                if writes_one_path(call, refused_under) || !writes_one_path(call, &self.wider) {
                    return Escalation::Refused;
                }
                self.wider.clone()
            }
            GrantKind::Command => {
                let Some(shell) = &self.shell else {
                    return Escalation::Refused;
                };
                let program = grant.target.as_str();
                let askable = shell.ask.iter().any(|asked| asked == program)
                    || shell.ask.iter().any(|asked| asked == "*");
                if refused_under
                    .commands()
                    .iter()
                    .any(|allowed| allowed == program)
                    || shell.deny.iter().any(|denied| denied == program)
                    || !askable
                {
                    return Escalation::Refused;
                }
                let mut policy = self.wider.to_policy();
                policy.allow_command(program);
                match Sandbox::new(&policy, self.wider.base()) {
                    Ok(sandbox) => sandbox.under(self.wider.authority().clone()),
                    Err(_) => return Escalation::Refused,
                }
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

/// What a session grant for `call` would be: the program `run_command` runs,
/// by its name, or the path a file tool writes, relative to `base` where it is
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
    }

    #[test]
    fn a_command_in_ask_is_asked_and_one_in_deny_or_in_no_list_is_not() {
        let root = scratch("commands");
        let environment =
            environment("[shell.s]\nallow = [\"ls\"]\nask = [\"git\"]\ndeny = [\"curl\"]\n");
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
        assert!(matches!(
            ceiling.escalate(&run("make"), &floor),
            Escalation::Refused
        ));
        assert!(
            matches!(ceiling.escalate(&run("ls"), &floor), Escalation::Refused),
            "ls was not refused for its name, so a yes would change nothing"
        );

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

    #[test]
    fn a_star_in_ask_asks_for_any_program_but_what_is_denied() {
        let root = scratch("star");
        let environment = environment("[shell.s]\nask = [\"*\"]\ndeny = [\"curl\"]\n");
        let (session, floor) = sandboxes(&root, &environment);
        let ceiling = DraftCeiling::new(
            &session,
            Some(&environment),
            Answered::default(),
            Authority::Person(None),
        );
        let run = |program: &str| call("run_command", serde_json::json!({ "argv": [program] }));
        assert!(matches!(
            ceiling.escalate(&run("git"), &floor),
            Escalation::Ask(_)
        ));
        assert!(matches!(
            ceiling.escalate(&run("curl"), &floor),
            Escalation::Refused
        ));
    }

    /// At the gate the approval is the person's yes, for the job: a plan may
    /// get what the lists leave to a person, and never what they deny.
    #[test]
    fn the_gate_may_grant_what_the_lists_ask_about() {
        let root = scratch("gate");
        let text = "[shell.s]\nallow = [\"ls\"]\nask = [\"*\"]\ndeny = [\"curl\"]\n\
                    [network.n]\nask = [\"crates.io\"]\n\
                    [files.f]\nallow = [{ path = \".\", access = \"read\" }]\n\
                    ask = [{ path = \".\", access = \"read-write\" }]\n\
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

        let gate = agency.gate_sandbox(&["git".into(), "curl".into()]);
        assert!(
            gate.commands().iter().any(|c| c == "git"),
            "asked for by the plan, under *"
        );
        assert!(
            !gate.commands().iter().any(|c| c == "curl"),
            "denied stays denied"
        );
        assert!(gate.network());
        assert_eq!(gate.egress(), ["crates.io"]);
        assert_eq!(
            gate.access_for(&root),
            Some(Access::ReadWrite),
            "the files' ask"
        );
    }

    #[test]
    fn under_a_posture_only_the_write_is_asked() {
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
        assert!(matches!(
            ceiling.escalate(&run, &floor),
            Escalation::Refused
        ));
    }
}
