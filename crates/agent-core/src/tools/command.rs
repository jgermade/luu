//! `run_command` — the tool the sandbox exists for.
//!
//! Everything else in this crate runs in our own process, where checking a path
//! before touching it is the same program deciding about itself. A child is
//! not: it makes its own syscalls, and nothing we wrote is in the way. So this
//! is the one tool whose verdict is about the kernel.
//!
//! There is no shell. `command` is a program name and `args` is a list, because
//! a string handed to `sh -c` makes the allowlist it was checked against
//! meaningless — `sh -c "cargo test; curl …"` passes any check that looks at
//! the first word. Anyone who wants a shell puts `sh` in `commands`, and then
//! the grant reads as what it is.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde_json::json;

use crate::sandbox::{Access, Sandbox, Verdict};

use super::{Tool, ToolFuture, ToolOutcome};

/// Long enough for a test run, short enough that a hung command does not hold a
/// session open forever. A default, not a law — `timeout_ms` overrides it.
const DEFAULT_TIMEOUT_MS: u64 = 30_000;

pub struct RunCommand;

impl Tool for RunCommand {
    fn name(&self) -> &'static str {
        "run_command"
    }

    fn description(&self) -> &'static str {
        "Run an allowed program. No shell: give the program and its arguments separately."
    }

    fn parameters(&self) -> serde_json::Value {
        json!({
            "type": "object",
            "properties": {
                "command": {"type": "string", "description": "Program name, which must be in the sandbox's allowed commands."},
                "args": {"type": "array", "items": {"type": "string"}},
                "cwd": {"type": "string", "description": "Working directory. Default: the project root."},
                "timeout_ms": {"type": "integer", "description": "Default 30000."},
            },
            "required": ["command"],
        })
    }

    fn run<'a>(&'a self, arguments: &'a serde_json::Value, sandbox: &'a Sandbox) -> ToolFuture<'a> {
        Box::pin(async move {
            let Some(program) = arguments.get("command").and_then(serde_json::Value::as_str) else {
                return ToolOutcome::failed(
                    Verdict::deny("run_command: `command` is required and must be a string"),
                    "`command` is required and must be a string",
                );
            };

            let args: Vec<String> = arguments
                .get("args")
                .and_then(serde_json::Value::as_array)
                .map(|values| {
                    values
                        .iter()
                        .map(|value| match value.as_str() {
                            Some(text) => text.to_string(),
                            // A number or a bool in `args` is a small model
                            // being loose, not an attack. Rendering it is what
                            // a shell would have done.
                            None => value.to_string(),
                        })
                        .collect()
                })
                .unwrap_or_default();

            let cwd = match arguments.get("cwd").and_then(serde_json::Value::as_str) {
                Some(path) => {
                    let check = sandbox.check_path(Path::new(path), Access::Read);
                    if !check.verdict.allowed {
                        return ToolOutcome::denied(check.verdict);
                    }
                    check.path
                }
                None => sandbox.base().to_path_buf(),
            };

            // The allowlist first, because it is the rule the user wrote and it
            // is the one a denial should name. Under the default
            // `enforcement = "kernel"` a kernel that cannot hold the child is a
            // denial here rather than a quiet downgrade.
            let restrictions = match sandbox.prepare_command(program) {
                Ok(restrictions) => restrictions,
                Err(verdict) => return ToolOutcome::denied(verdict),
            };
            let verdict = restrictions.verdict.clone();

            // Where the program lives is its own grant. Resolving it here turns
            // what would be an unreadable `EACCES` from the child into a
            // verdict that names the missing rule.
            let Some(binary) = resolve_program(program, sandbox.base()) else {
                return ToolOutcome::denied(Verdict::deny(format!("`{program}` is not on PATH")));
            };
            let binary_check = sandbox.check_program(&binary);
            if !binary_check.verdict.allowed {
                return ToolOutcome::denied(binary_check.verdict);
            }

            let mut command = std::process::Command::new(&binary);
            command
                .args(&args)
                .current_dir(&cwd)
                .stdin(std::process::Stdio::null());
            restrictions.install(&mut command);

            let timeout = Duration::from_millis(
                arguments
                    .get("timeout_ms")
                    .and_then(serde_json::Value::as_u64)
                    .unwrap_or(DEFAULT_TIMEOUT_MS),
            );

            let mut command = tokio::process::Command::from(command);
            command.kill_on_drop(true);
            let output = match tokio::time::timeout(timeout, command.output()).await {
                Ok(Ok(output)) => output,
                Ok(Err(error)) => {
                    return ToolOutcome::failed(verdict, format!("{program}: {error}"));
                }
                Err(_) => {
                    return ToolOutcome::failed(
                        verdict,
                        format!("{program}: killed after {} ms", timeout.as_millis()),
                    );
                }
            };

            let mut text = String::new();
            for (stream, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
                if !bytes.is_empty() {
                    text.push_str(&format!("--- {stream}\n{}", String::from_utf8_lossy(bytes)));
                    if !text.ends_with('\n') {
                        text.push('\n');
                    }
                }
            }

            match output.status.success() {
                true => ToolOutcome::ok(verdict, text),
                // A non-zero exit is the answer to the question, not a failure
                // of the tool — but the model has to be able to tell, so it is
                // an error with the output kept.
                false => {
                    let code = match output.status.code() {
                        Some(code) => code.to_string(),
                        None => "a signal".to_string(),
                    };
                    let mut outcome = ToolOutcome::ok(verdict, text);
                    outcome.error = Some(format!("{program} exited with {code}"));
                    outcome
                }
            }
        })
    }
}

/// The program, as an absolute path.
///
/// A name with a separator in it is a path and is resolved against the base
/// like any other argument; a bare name is looked up on `PATH`.
fn resolve_program(program: &str, base: &Path) -> Option<PathBuf> {
    if program.contains(std::path::MAIN_SEPARATOR) {
        return base.join(program).canonicalize().ok();
    }
    std::env::var_os("PATH")?
        .to_str()?
        .split(':')
        .filter(|entry| !entry.is_empty())
        .map(|entry| Path::new(entry).join(program))
        .find(|candidate| candidate.is_file())
        .and_then(|candidate| candidate.canonicalize().ok())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sandbox::{Applied, Enforcement, PathRule, SandboxPolicy};
    use crate::tools::{ToolCall, Tools};

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "luu-run-{name}-{}-{:?}",
            std::process::id(),
            std::thread::current().id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir.canonicalize().unwrap()
    }

    fn sandbox_allowing(dir: &Path, commands: &[&str], enforcement: Enforcement) -> Sandbox {
        Sandbox::new(
            &SandboxPolicy {
                paths: vec![PathRule::new(".", Access::ReadWrite)],
                commands: commands.iter().map(|name| (*name).to_string()).collect(),
                network: false,
                enforcement,
                limits: Default::default(),
            },
            dir,
        )
        .unwrap()
    }

    async fn run(sandbox: &Sandbox, arguments: serde_json::Value) -> ToolOutcome {
        Tools::standard()
            .call(
                &ToolCall {
                    name: "run_command".into(),
                    arguments,
                },
                sandbox,
            )
            .await
    }

    #[tokio::test]
    async fn a_command_that_is_not_allowed_never_starts() {
        let dir = scratch("denied");
        let sandbox = sandbox_allowing(&dir, &[], Enforcement::BestEffort);
        let outcome = run(&sandbox, json!({"command": "echo", "args": ["hola"]})).await;

        assert!(!outcome.verdict.allowed);
        assert!(outcome.output.is_empty());
        assert!(outcome.error.unwrap().contains("no commands are allowed"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn an_allowed_command_runs_and_the_verdict_says_who_held_it() {
        let dir = scratch("allowed");
        let sandbox = sandbox_allowing(&dir, &["echo"], Enforcement::BestEffort);
        let outcome = run(&sandbox, json!({"command": "echo", "args": ["hola"]})).await;

        assert!(outcome.error.is_none(), "{outcome:?}");
        assert!(outcome.output.contains("hola"), "{}", outcome.output);
        assert!(
            !matches!(outcome.verdict.enforced_by, Applied::Process),
            "a subprocess is never held by an in-process check: {:?}",
            outcome.verdict.enforced_by
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_non_zero_exit_keeps_the_output_and_says_it_failed() {
        let dir = scratch("exit");
        let sandbox = sandbox_allowing(&dir, &["false"], Enforcement::BestEffort);
        let outcome = run(&sandbox, json!({"command": "false"})).await;
        assert!(outcome.error.unwrap().contains("exited with 1"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_hung_command_is_killed_rather_than_holding_the_session() {
        let dir = scratch("timeout");
        let sandbox = sandbox_allowing(&dir, &["sleep"], Enforcement::BestEffort);
        let outcome = run(
            &sandbox,
            json!({"command": "sleep", "args": ["30"], "timeout_ms": 200}),
        )
        .await;
        assert!(outcome.error.unwrap().contains("killed after"));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn there_is_no_shell_so_a_shell_string_is_not_a_command() {
        let dir = scratch("noshell");
        let sandbox = sandbox_allowing(&dir, &["echo"], Enforcement::BestEffort);
        // The whole point of taking a program and a list: this is one program
        // name with a space in it, and there is nothing on PATH called that.
        let outcome = run(&sandbox, json!({"command": "echo hola; id"})).await;
        assert!(!outcome.verdict.allowed, "{outcome:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn a_working_directory_outside_the_sandbox_is_denied() {
        let dir = scratch("cwd");
        let sandbox = sandbox_allowing(&dir, &["echo"], Enforcement::BestEffort);
        // Not `/etc`: allowing any command grants the system roots read and
        // execute, so `/etc` is inside the sandbox on purpose. The parent of
        // the project is not.
        let outcome = run(&sandbox, json!({"command": "echo", "cwd": ".."})).await;
        assert!(!outcome.verdict.allowed, "{outcome:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn the_child_is_held_by_the_kernel_or_it_does_not_run() {
        let dir = scratch("required");
        let sandbox = sandbox_allowing(&dir, &["echo"], Enforcement::Kernel);
        let outcome = run(&sandbox, json!({"command": "echo", "args": ["hola"]})).await;

        match outcome.verdict.allowed {
            // Where both mechanisms are there, the verdict names them.
            true => assert!(matches!(
                outcome.verdict.enforced_by,
                Applied::Kernel { .. }
            )),
            // Where they are not, the answer is a denial that says so — the
            // default never degrades quietly.
            false => assert!(
                outcome.verdict.rule.contains("best-effort"),
                "{:?}",
                outcome.verdict
            ),
        }
        let _ = std::fs::remove_dir_all(&dir);
    }
}
