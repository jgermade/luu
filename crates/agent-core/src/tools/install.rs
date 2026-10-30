//! `install` — system packages, in the session's container.
//!
//! A tool and not a command, so that what was installed is a fact the session
//! holds rather than something read back out of `run_command`'s arguments: an
//! `apt-get install` inside a script, behind flags or under `sudo` is missed
//! by any such reading, and a session that moves to another container has to
//! know what to install there again. See
//! `RECORD/2026-10-30.a-session-carries-its-policies.completed.md` §3.
//!
//! **Only in a container.** There it runs as the container's root, from the
//! host (`<runtime> exec --user 0`), outside the worker's own Landlock: the
//! container is the boundary, and `apt-get` has to write `/usr` and `/var`.
//! On this machine it would be `sudo` over the person's own system, so it
//! fails, plainly, and nobody is asked — a yes would change nothing. The
//! in-process [`Install::run`] below is that answer; the container's is
//! [`crate::worker::Worker`]'s.
//!
//! **apt only**, because the images luu runs are Debian. It is held to the
//! shell list as the program `apt-get`: a list that does not allow it leaves
//! it to a person, and one that denies it denies it.

use serde_json::json;

use crate::sandbox::{Applied, Sandbox, Verdict};

use super::{Tool, ToolFuture, ToolOutcome};

/// The program the shell list allows or denies `install` as.
pub const APT: &str = "apt-get";

/// The packages a call asks for, each a Debian package name with an optional
/// `=version` — never a flag, never a path, never two words.
pub fn packages_of(arguments: &serde_json::Value) -> Result<Vec<String>, String> {
    let Some(list) = arguments
        .get("packages")
        .and_then(serde_json::Value::as_array)
    else {
        return Err("`packages` is required and must be an array of package names".into());
    };
    if list.is_empty() {
        return Err("`packages` is empty: name at least one".into());
    }
    list.iter()
        .map(|value| {
            let name = value.as_str().unwrap_or_default();
            match is_package(name) {
                true => Ok(name.to_string()),
                false => Err(format!(
                    "`{value}` is not a Debian package name (lower case, digits, `+`, `-`, `.`, \
                     and an optional `=version`)"
                )),
            }
        })
        .collect()
}

fn is_package(text: &str) -> bool {
    let (name, version) = match text.split_once('=') {
        Some((name, version)) => (name, Some(version)),
        None => (text, None),
    };
    let named = name.len() >= 2
        && name.starts_with(|c: char| c.is_ascii_lowercase() || c.is_ascii_digit())
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "+-.".contains(c));
    let versioned = version.is_none_or(|version| {
        !version.is_empty()
            && version
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || "+-.:~".contains(c))
    });
    named && versioned
}

/// Whether `sandbox` lets `install` run at all: its shell list allows
/// `apt-get`. The refusal names it, so a person asked about it is asked about
/// `apt-get`, which is what the grant is.
pub fn allowed(sandbox: &Sandbox) -> Result<(), Verdict> {
    match sandbox.commands().iter().any(|program| program == APT) {
        true => Ok(()),
        false => Err(Verdict::deny(format!(
            "install runs `{APT}`, and `{APT}` is not in commands, under {}",
            sandbox.authority(),
        ))),
    }
}

pub struct Install;

impl Tool for Install {
    fn name(&self) -> &'static str {
        "install"
    }

    fn description(&self) -> &'static str {
        "Install system packages with apt in the session's container. Not available on this machine."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "packages": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Debian package names, e.g. [\"ripgrep\", \"jq\"].",
                },
            },
            "required": ["packages"],
        })
    }

    /// This machine's answer, and a worker without a container's: nothing is
    /// installed here. **Not a refusal**: a refused call is put to a person,
    /// and no yes would make this one work, so it fails before the shell list
    /// is consulted and nobody is asked.
    fn run<'a>(&'a self, arguments: &'a serde_json::Value, _: &'a Sandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            if let Err(why) = packages_of(arguments) {
                return ToolOutcome::failed(Verdict::deny(format!("install: {why}")), why);
            }
            ToolOutcome::failed(
                Verdict::allow("install: not in a container", Applied::Process),
                "install runs only in a container, and this session runs on this machine: \
                 ask the person to install it, or to move the session to a container",
            )
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_package_is_a_debian_name_and_nothing_else() {
        let ok = |list: serde_json::Value| packages_of(&json!({ "packages": list }));
        assert_eq!(
            ok(json!(["ripgrep", "jq", "g++", "libssl-dev=3.0.11-1"])).unwrap(),
            ["ripgrep", "jq", "g++", "libssl-dev=3.0.11-1"]
        );
        for bad in [
            "-y",
            "--allow-unauthenticated",
            "a b",
            "../x",
            "Ripgrep",
            "x",
            "",
            "jq=",
        ] {
            assert!(ok(json!([bad])).is_err(), "{bad:?} is not a package");
        }
        assert!(ok(json!([])).is_err());
        assert!(packages_of(&json!({})).is_err());
    }

    /// On this machine nothing is installed, and the answer is a failure and
    /// not a refusal — a refusal is put to a person, and no yes would help.
    #[tokio::test]
    async fn on_this_machine_it_fails_and_asks_nobody() {
        let dir = std::env::temp_dir();
        let sandbox = Sandbox::new(&crate::sandbox::SandboxPolicy::default(), &dir).unwrap();
        let outcome = crate::tools::Tools::standard()
            .call(
                &crate::tools::ToolCall {
                    name: "install".into(),
                    arguments: json!({ "packages": ["jq"] }),
                },
                &sandbox,
            )
            .await;
        assert!(outcome.verdict.allowed, "{:?}", outcome.verdict);
        assert!(outcome.error.unwrap().contains("only in a container"));
        assert!(allowed(&sandbox).is_err(), "no `apt-get` in an empty list");
    }
}
