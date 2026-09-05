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

use super::{CommandResult, Tool, ToolFuture, ToolOutcome, clamp};

/// Long enough for a test run, short enough that a hung command does not hold a
/// session open forever. A default, not a law — `timeout_ms` overrides it.
pub const DEFAULT_TIMEOUT_MS: u64 = 30_000;

/// The longest `timeout_ms` may ask for, because it is **the model's number**.
///
/// Everything else a tool call carries is checked against the sandbox before it
/// runs; this one was checked against nothing, so `timeout_ms: 36000000` was a
/// ten-hour hang the gate never saw — the gate approves commands, and this is an
/// argument. Ten minutes is measured against the longest thing this repository
/// actually runs (`cargo test --workspace` on the slowest box in
/// `ROADMAP/*/machines.md`), and it is clamped rather than refused so that the
/// command still runs and the verdict still says who did the killing.
///
/// See `RECORD/2026-09-05.a-clock-at-the-seam.completed.md`.
pub const MAX_TIMEOUT_MS: u64 = 600_000;

/// What the far side was told this call may take, in the host's own words.
///
/// The seam derives its deadline from this rather than from a second knob: it is
/// the same number, read from the same request, so the seam cannot fire before
/// the tool's own clock has had its chance. A tool with no clock answers zero,
/// which is the honest answer — `read_file` has no timeout to inherit.
pub fn clock_of(call: &crate::tools::ToolCall) -> Duration {
    match call.name.as_str() {
        "run_command" => requested_timeout(&call.arguments),
        _ => Duration::ZERO,
    }
}

fn requested_timeout(arguments: &serde_json::Value) -> Duration {
    let asked = arguments
        .get("timeout_ms")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    Duration::from_millis(asked.min(MAX_TIMEOUT_MS))
}

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
                "timeout_ms": {"type": "integer", "description": "Default 30000, capped at 600000."},
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
            if let Some(proxy) = sandbox.proxy() {
                command.env("HTTP_PROXY", proxy);
                command.env("HTTPS_PROXY", proxy);
                command.env("ALL_PROXY", proxy);
                command.env("http_proxy", proxy);
                command.env("https_proxy", proxy);
                command.env("all_proxy", proxy);
            }
            restrictions.install(&mut command);

            let timeout = requested_timeout(arguments);

            let mut command = tokio::process::Command::from(command);
            command.kill_on_drop(true);
            // Around the child and nothing else: the checks and the spawn are
            // the step's time, not the command's, and a number that quietly
            // includes them is the kind that gets compared against itself later.
            let started = std::time::Instant::now();
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

            let duration_ms = started.elapsed().as_millis() as u64;

            let mut text = String::new();
            for (stream, bytes) in [("stdout", &output.stdout), ("stderr", &output.stderr)] {
                if !bytes.is_empty() {
                    text.push_str(&format!("--- {stream}\n{}", String::from_utf8_lossy(bytes)));
                    if !text.ends_with('\n') {
                        text.push('\n');
                    }
                }
            }

            let result = CommandResult {
                exit_code: output.status.code(),
                signal: exit_signal(&output.status),
                // Capped the same way the rendered blob is, and for the same
                // reason: this is what a recording carries.
                stdout: clamp(String::from_utf8_lossy(&output.stdout).into_owned()).0,
                stderr: clamp(String::from_utf8_lossy(&output.stderr).into_owned()).0,
                duration_ms,
            };

            match output.status.success() {
                true => ToolOutcome::from_command(verdict, text, result),
                // A non-zero exit is the answer to the question, not a failure
                // of the tool — but the model has to be able to tell, so it is
                // an error with the output kept.
                false => {
                    let how = match (result.exit_code, result.signal) {
                        (Some(code), _) => format!("exited with {code}"),
                        // The sentence that says which limit stopped it, now
                        // that there are limits that can.
                        (None, Some(signal)) => {
                            format!("was killed ({})", CommandResult::signal_name(signal))
                        }
                        (None, None) => "did not exit normally".to_string(),
                    };
                    let mut outcome = ToolOutcome::from_command(verdict, text, result);
                    outcome.error = Some(format!("{program} {how}"));
                    outcome
                }
            }
        })
    }
}

/// The signal that ended a child, where the platform has such a thing.
#[cfg(unix)]
fn exit_signal(status: &std::process::ExitStatus) -> Option<i32> {
    use std::os::unix::process::ExitStatusExt;
    status.signal()
}

#[cfg(not(unix))]
fn exit_signal(_status: &std::process::ExitStatus) -> Option<i32> {
    None
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
    use crate::sandbox::{Applied, Enforcement, Limits, PathRule, SandboxPolicy};
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

    fn sandbox_limiting(dir: &Path, commands: &[&str], limits: Limits) -> Sandbox {
        Sandbox::new(
            &SandboxPolicy {
                paths: vec![PathRule::new(".", Access::ReadWrite)],
                commands: commands.iter().map(|name| (*name).to_string()).collect(),
                network: false,
                enforcement: Enforcement::BestEffort,
                limits,
                ..SandboxPolicy::default()
            },
            dir,
        )
        .unwrap()
    }

    fn sandbox_allowing(dir: &Path, commands: &[&str], enforcement: Enforcement) -> Sandbox {
        Sandbox::new(
            &SandboxPolicy {
                paths: vec![PathRule::new(".", Access::ReadWrite)],
                commands: commands.iter().map(|name| (*name).to_string()).collect(),
                network: false,
                enforcement,
                limits: Default::default(),
                ..SandboxPolicy::default()
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

    /// The exit code, the two streams and the duration are **fields**, and the
    /// rendering is still the same short text.
    ///
    /// The distinction is the whole of this change: a task cannot be closed on
    /// an exit code that only ever existed inside a sentence, and a 7B must not
    /// pay for a JSON wrapper it does not read.
    #[tokio::test]
    async fn what_the_child_did_survives_as_fields_and_not_only_as_a_sentence() {
        let dir = scratch("structured");
        let sandbox = sandbox_allowing(&dir, &["sh"], Enforcement::BestEffort);
        let outcome = run(
            &sandbox,
            json!({"command": "sh", "args": ["-c", "echo out; echo err >&2; exit 3"]}),
        )
        .await;

        let result = outcome.command.clone().expect("a subprocess ran");
        assert_eq!(result.exit_code, Some(3));
        assert_eq!(result.signal, None);
        // Unmixed: a judge that has to re-split one blob on `--- stdout` is
        // parsing a rendering.
        assert_eq!(result.stdout.trim(), "out");
        assert_eq!(result.stderr.trim(), "err");

        let rendered = outcome.render("run_command");
        assert!(rendered.contains("exited with 3"), "{rendered}");
        assert!(
            !rendered.contains("exit_code"),
            "the rendering stayed frugal: {rendered}"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// And the payoff for the limits: the outcome says *which* one killed it.
    ///
    /// Before this, a child that hit `cpu-seconds` came back as "sh exited with
    /// a signal" — indistinguishable from a crash, which is a different bug
    /// with a different fix.
    #[cfg(unix)]
    #[tokio::test]
    async fn a_child_killed_by_a_limit_says_which_limit() {
        if !Path::new("/bin/sh").exists() {
            return;
        }
        let dir = scratch("killed");
        let sandbox = sandbox_limiting(
            &dir,
            &["sh"],
            Limits {
                cpu_seconds: Some(1),
                ..Limits::NONE
            },
        );
        let outcome = run(
            &sandbox,
            json!({"command": "sh", "args": ["-c", "while :; do :; done"]}),
        )
        .await;

        let result = outcome.command.clone().expect("a subprocess ran");
        assert_eq!(result.exit_code, None, "a signal is not an exit code");
        assert_eq!(result.signal, Some(libc::SIGXCPU));
        let error = outcome.error.clone().expect("a child that was killed");
        assert!(error.contains("SIGXCPU"), "{error}");
        assert!(error.contains("cpu-seconds limit"), "{error}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn a_models_timeout_is_clamped_rather_than_believed() {
        // Everything else in a call is checked against the sandbox before it
        // runs. This one was checked against nothing, so `timeout_ms` was a
        // ten-hour hang the gate never saw — the gate approves commands, and
        // this is an argument.
        assert_eq!(
            requested_timeout(&json!({"timeout_ms": 36_000_000u64})),
            Duration::from_millis(MAX_TIMEOUT_MS),
        );
        assert_eq!(
            requested_timeout(&json!({})),
            Duration::from_millis(DEFAULT_TIMEOUT_MS),
            "a call that asks for nothing gets the default, as it always did",
        );
        assert_eq!(
            requested_timeout(&json!({"timeout_ms": 200})),
            Duration::from_millis(200)
        );
    }

    #[test]
    fn only_the_tool_with_a_clock_of_its_own_answers_with_one() {
        // What the seam reads to build its deadline. `read_file` has no timeout
        // to inherit, and zero is the honest answer rather than a borrowed
        // default.
        use crate::tools::ToolCall;
        let clock = |name: &str, arguments| {
            clock_of(&ToolCall {
                name: name.to_string(),
                arguments,
            })
        };
        assert_eq!(clock("read_file", json!({"path": "x"})), Duration::ZERO);
        assert_eq!(
            clock("run_command", json!({"command": "ls"})),
            Duration::from_millis(DEFAULT_TIMEOUT_MS),
        );
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
