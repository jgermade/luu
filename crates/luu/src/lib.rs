//! `luu` — the Loude CLI.
//!
//! The binary is installed under two names (`luu` and `loude`); both are thin
//! wrappers over [`run`].

use std::time::Duration;

use agent_core::agent::{DEFAULT_MAX_STEPS, run_agent_turn};
use agent_core::backend::{Backend, CompletionRequest, mock::Mock, ollama::Ollama};
use agent_core::context::{Budget, Context as AgentContext, Eviction};
use agent_core::protocol::ServerMessage;
use agent_core::sandbox::{Access, Enforcement, Sandbox, SandboxPolicy};
use agent_core::task::{TaskId, Tasks};
use agent_core::tools::Tools;
use agent_core::trace::TraceMessage;
use agent_core::turn::{EndReason, TurnEvent};

use crate::session::{
    Agency, DEFAULT_RESERVE, Event, PrefixTracker, Recorder, SYSTEM, counter_for, now_ms,
    propose_plan, rendered,
};
use anyhow::{Context, Result};

pub mod export;
pub mod serve;
pub mod session;
use clap::{Parser, Subcommand, ValueEnum};
use tokio::io::{AsyncWriteExt, stdout};
use tokio::sync::{mpsc, watch};

#[derive(Parser)]
#[command(
    name = "luu",
    version,
    about = "Local AI agent, built for small models"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Turn recordings into the static twin of the read API.
    ///
    /// The same JSON the live server serves, as files a static host can
    /// answer with — which is what makes the GitHub Pages deploy more than
    /// a screenshot.
    Export {
        /// Recorded `.jsonl` sessions. Each file's stem becomes its session id.
        #[arg(required = true)]
        records: Vec<std::path::PathBuf>,

        /// Where to write the tree (`sessions.json`, `sessions/<id>/…`).
        #[arg(long, short)]
        out: std::path::PathBuf,

        /// How the page reaches the recordings, relative to the site root.
        #[arg(long, default_value = "./fixtures")]
        record_base: String,
    },

    /// Print the resolved sandbox and the exact tool definitions that go into
    /// the prompt.
    ///
    /// The definitions are the second half of the cached prefix, so being able
    /// to look at the bytes is the difference between a cache miss you can see
    /// and one you cannot.
    Tools {
        #[command(flatten)]
        sandbox: SandboxArgs,
    },

    /// Serve the debug UI and the agent protocol over HTTP.
    Serve {
        #[command(flatten)]
        sandbox_args: SandboxArgs,

        #[arg(long, default_value = "127.0.0.1:7878")]
        bind: std::net::SocketAddr,

        #[arg(long, value_enum, default_value_t = BackendKind::Mock)]
        backend: BackendKind,

        #[arg(long, default_value = "qwen2.5-coder:7b")]
        model: String,

        #[arg(long, default_value = "http://127.0.0.1:11434")]
        ollama_url: String,

        #[arg(long, default_value_t = 25)]
        mock_delay_ms: u64,

        /// Write every protocol and trace message to a replayable JSON-lines file.
        #[arg(long)]
        record: Option<std::path::PathBuf>,

        /// The model's context window. 0 means unknown: no budget, no eviction.
        #[arg(long, default_value_t = 0)]
        context_limit: u32,

        /// The model's `tokenizer.json`. Without it, tokens are counted
        /// approximately and every number says so.
        #[arg(long)]
        tokenizer: Option<std::path::PathBuf>,

        /// Tokens held back for the answer before history is considered.
        #[arg(long, default_value_t = DEFAULT_RESERVE)]
        reserve: u32,

        /// How the history gives way when a turn no longer fits. `turn` drops
        /// the minimum and rewrites the history on every call once the window
        /// is full; `block` cuts down to --low-water and then holds still.
        #[arg(long, value_enum, default_value_t = EvictionKind::Turn)]
        evict: EvictionKind,

        /// The fraction of the history budget `--evict block` cuts down to.
        /// Ignored by `--evict turn`. A guess, and a flag so that it can stop
        /// being one.
        #[arg(long, default_value_t = 0.5)]
        low_water: f32,
    },

    /// Run a turn — or a scripted sequence of them — streaming to stdout.
    Chat {
        /// The prompt. Reads stdin when omitted, and ignored with --script.
        prompt: Option<String>,

        #[command(flatten)]
        sandbox_args: SandboxArgs,

        /// A file of prompts, one per line, run in order against one shared
        /// history. `#` comments and blank lines are skipped. This is how a
        /// multi-turn baseline gets recorded the same way twice.
        #[arg(long)]
        script: Option<std::path::PathBuf>,

        #[arg(long, value_enum, default_value_t = BackendKind::Mock)]
        backend: BackendKind,

        #[arg(long, default_value = "qwen2.5-coder:7b")]
        model: String,

        #[arg(long, default_value = "http://127.0.0.1:11434")]
        ollama_url: String,

        /// Milliseconds between mock tokens, for exercising slow generation.
        #[arg(long, default_value_t = 25)]
        mock_delay_ms: u64,

        /// What the mock backend answers, one per model call. Repeatable, and
        /// the last one repeats — which is how the tool loop is exercised end
        /// to end without a model: a reply containing a ```tool block, then the
        /// reply that reads its result.
        #[arg(long = "mock-reply", value_name = "TEXT")]
        mock_replies: Vec<String>,

        /// Stop the turn after this many milliseconds, to exercise cancelling.
        #[arg(long)]
        cancel_after_ms: Option<u64>,

        /// Write the turn to a replayable JSON-lines file. The same format
        /// `luu serve --record` writes, so the UI can load either.
        #[arg(long)]
        record: Option<std::path::PathBuf>,

        /// The model's context window. 0 means unknown: no budget, no eviction.
        #[arg(long, default_value_t = 0)]
        context_limit: u32,

        /// The model's `tokenizer.json`. Without it, tokens are counted
        /// approximately and every number says so.
        #[arg(long)]
        tokenizer: Option<std::path::PathBuf>,

        /// Tokens held back for the answer before history is considered.
        #[arg(long, default_value_t = DEFAULT_RESERVE)]
        reserve: u32,

        /// How the history gives way when a turn no longer fits. `turn` drops
        /// the minimum and rewrites the history on every call once the window
        /// is full; `block` cuts down to --low-water and then holds still.
        #[arg(long, value_enum, default_value_t = EvictionKind::Turn)]
        evict: EvictionKind,

        /// The fraction of the history budget `--evict block` cuts down to.
        /// Ignored by `--evict turn`. A guess, and a flag so that it can stop
        /// being one.
        #[arg(long, default_value_t = 0.5)]
        low_water: f32,
    },
}

/// The sandbox flags, shared by every subcommand that can act.
///
/// They *add* to whatever the policy file said rather than replacing it, so
/// widening a project's sandbox for one run is a flag and narrowing it is an
/// edit to the file — which is the direction that should be the harder one.
#[derive(Clone, clap::Args)]
struct SandboxArgs {
    /// Sandbox policy (TOML). Defaults to ./luu.toml when it is there.
    #[arg(long, value_name = "FILE")]
    sandbox: Option<std::path::PathBuf>,

    /// Grant read access to a path. Repeatable.
    #[arg(long = "allow-read", value_name = "PATH")]
    allow_read: Vec<std::path::PathBuf>,

    /// Grant read and write. Repeatable.
    #[arg(long = "allow-write", value_name = "PATH")]
    allow_write: Vec<std::path::PathBuf>,

    /// Grant read and the right to run what is in the tree. Repeatable.
    #[arg(long = "allow-exec", value_name = "PATH")]
    allow_exec: Vec<std::path::PathBuf>,

    /// Let run_command run this program. Repeatable, and a program name — never
    /// a shell string, which would make the list it is checked against
    /// meaningless.
    #[arg(long = "allow-command", value_name = "NAME")]
    allow_command: Vec<String>,

    /// Let subprocesses reach the network.
    #[arg(long = "allow-network")]
    allow_network: bool,

    /// What to do where the kernel cannot hold a subprocess: `kernel` denies
    /// the call and says what is missing, `best-effort` runs it and reports the
    /// gap in every verdict.
    #[arg(long = "sandbox-enforcement", value_enum)]
    enforcement: Option<EnforcementKind>,

    /// Tool calls one turn may make before it has to answer.
    #[arg(long, default_value_t = DEFAULT_MAX_STEPS)]
    max_tool_steps: u32,

    /// Run without tools. The prefix loses the definitions block, so this is
    /// not the same prompt with the tools ignored — it is a different one.
    #[arg(long)]
    no_tools: bool,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum EnforcementKind {
    Kernel,
    BestEffort,
}

impl SandboxArgs {
    /// Resolves the policy: the file, then the flags, then the working
    /// directory as the base every relative path is taken against.
    fn resolve(&self) -> Result<Agency> {
        let base = std::env::current_dir().context("the working directory")?;

        let explicit = self.sandbox.is_some();
        let path = self
            .sandbox
            .clone()
            .unwrap_or_else(|| base.join("luu.toml"));
        let mut policy = match (explicit, path.exists()) {
            // An explicit --sandbox that is not there is an error: it was asked
            // for, and falling back to the default would be a wider sandbox
            // than the one the user named.
            (true, false) => anyhow::bail!("--sandbox {}: no such file", path.display()),
            (_, true) => SandboxPolicy::from_file(&path)
                .with_context(|| format!("reading {}", path.display()))?,
            (false, false) => SandboxPolicy::default(),
        };

        for granted in &self.allow_read {
            policy.allow(granted, Access::Read);
        }
        for granted in &self.allow_exec {
            policy.allow(granted, Access::Execute);
        }
        for granted in &self.allow_write {
            policy.allow(granted, Access::ReadWrite);
        }
        for command in &self.allow_command {
            policy.allow_command(command);
        }
        policy.network |= self.allow_network;
        if let Some(enforcement) = self.enforcement {
            policy.enforcement = match enforcement {
                EnforcementKind::Kernel => Enforcement::Kernel,
                EnforcementKind::BestEffort => Enforcement::BestEffort,
            };
        }

        Ok(Agency {
            tools: std::sync::Arc::new(match self.no_tools {
                true => Tools::new(Vec::new()),
                false => Tools::standard(),
            }),
            sandbox: std::sync::Arc::new(Sandbox::new(&policy, &base)?),
            max_steps: self.max_tool_steps,
        })
    }
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum EvictionKind {
    Turn,
    Block,
}

impl EvictionKind {
    fn policy(self, low_water: f32) -> Eviction {
        match self {
            Self::Turn => Eviction::Turn,
            Self::Block => Eviction::Block { low_water },
        }
    }
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum BackendKind {
    Mock,
    Ollama,
}

fn build_backend(
    kind: BackendKind,
    ollama_url: &str,
    mock_delay_ms: u64,
    mock_replies: Vec<String>,
) -> Box<dyn Backend> {
    match kind {
        BackendKind::Mock => Box::new(
            match mock_replies.is_empty() {
                true => Mock::default(),
                false => Mock::replies(mock_replies),
            }
            .delay(Duration::from_millis(mock_delay_ms)),
        ),
        BackendKind::Ollama => Box::new(Ollama::new(ollama_url)),
    }
}

/// The mock ignores the model name; passing the Ollama default through would
/// just put a model nobody loaded into the record file's header.
fn model_for(backend: &dyn Backend, model: String) -> String {
    match backend.name() {
        "mock" => "mock".to_string(),
        _ => model,
    }
}

/// One line of a `chat` run: something to ask, or something to do to the task
/// around it.
///
/// Directives are in the script rather than in flags because approval is the
/// one thing that must be typed. A `--approve-everything` flag would make the
/// gate a setting again, which is the shape this whole design replaced.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Step {
    Prompt(String),
    /// `:task <objective>` — asks the model for a plan. Runs nothing.
    Task(String),
    /// `:approve` — the confirmation.
    Approve,
    /// `:discard` — the plan was read and refused.
    Discard,
    /// `:close` — summarise the task and fold its turns.
    Close,
}

impl Step {
    fn parse(line: &str) -> Result<Self> {
        let Some(directive) = line.strip_prefix(':') else {
            return Ok(Self::Prompt(line.to_string()));
        };
        let (word, rest) = directive
            .split_once(char::is_whitespace)
            .unwrap_or((directive, ""));
        match (word, rest.trim()) {
            ("task", "") => anyhow::bail!(":task needs an objective"),
            ("task", objective) => Ok(Self::Task(objective.to_string())),
            ("approve", _) => Ok(Self::Approve),
            ("discard", _) => Ok(Self::Discard),
            ("close", _) => Ok(Self::Close),
            (unknown, _) => anyhow::bail!(
                ":{unknown} is not a directive (:task <objective>, :approve, :discard, :close)"
            ),
        }
    }
}

/// Runs one lifecycle directive. `Err` is a refusal to print and stop on: a
/// script that carried on past a refused gate would record a session that could
/// not have happened.
#[allow(clippy::too_many_arguments)]
async fn run_directive(
    step: &Step,
    tasks: &mut Tasks,
    context: &mut AgentContext,
    prefix: &mut PrefixTracker,
    backend: &dyn Backend,
    model: &str,
    budget: Budget,
    counter: &dyn agent_core::context::TokenCounter,
    recorder: Option<&Recorder>,
) -> std::result::Result<(), String> {
    let publish = |message: ServerMessage| {
        if let Some(recorder) = recorder {
            recorder.write(&Event::Protocol(message));
        }
    };

    match step {
        Step::Prompt(_) => Ok(()),
        Step::Task(objective) => {
            tasks.may_propose()?;
            println!("\n[task] {objective}\n  planning…");
            let (plan, sent) =
                propose_plan(backend, model, context, budget, counter, objective).await;
            let id = tasks.propose(plan.clone());
            println!("{}", indent(&plan.render()));
            if !plan.parsed {
                println!("  (the model did not answer in a plan block; these are its own lines)");
            }
            println!("  → :approve or :discard");
            publish(ServerMessage::TaskProposed { task: id, plan });
            // A planning call is a call: it is what the cache holds afterwards,
            // and the claim that proposing costs nothing is checkable only if
            // the prompt it sent is written down.
            if let Some(recorder) = recorder {
                recorder.write(&Event::Trace(prefix.measure_plan(id, &sent, counter)));
            }
            Ok(())
        }
        Step::Approve => {
            let id = pending(tasks)?;
            tasks
                .approve(id, context, counter)
                .map_err(|e| e.to_string())?;
            println!("[task {id}] approved");
            publish(ServerMessage::TaskApproved { task: id });
            Ok(())
        }
        Step::Discard => {
            let id = pending(tasks)?;
            tasks.discard(id).map_err(|e| e.to_string())?;
            println!("[task {id}] discarded");
            publish(ServerMessage::TaskDiscarded { task: id });
            Ok(())
        }
        Step::Close => {
            let id = tasks
                .active()
                .map(|task| task.id)
                .ok_or("no task is open")?;
            let closed = tasks
                .close(id, context, counter)
                .map_err(|e| e.to_string())?;
            println!("\n{}", indent(&closed.summary));
            // The one rewrite in a session, so the numbers travel with it.
            println!(
                "  [folded] {} turn(s), {} → {} tokens",
                closed.fold.turns, closed.fold.tokens_before, closed.fold.tokens_after,
            );
            publish(ServerMessage::TaskClosed {
                task: id,
                summary: closed.summary,
            });
            if let Some(recorder) = recorder {
                recorder.write(&Event::Trace(TraceMessage::folded(id, closed.fold)));
            }
            Ok(())
        }
    }
}

fn pending(tasks: &Tasks) -> std::result::Result<TaskId, String> {
    tasks
        .pending()
        .map(|task| task.id)
        .ok_or_else(|| "no plan is waiting for an answer".to_string())
}

/// Two spaces in front of every line, so a plan and a summary read as blocks
/// rather than as more of the transcript.
fn indent(text: &str) -> String {
    text.lines()
        .map(|line| format!("  {line}"))
        .collect::<Vec<_>>()
        .join("\n")
}

pub async fn run() -> Result<()> {
    let Cli { command } = Cli::parse();

    if let Command::Export {
        records,
        out,
        record_base,
    } = &command
    {
        let sessions = records
            .iter()
            .map(|path| {
                let id = path
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .context("a record file needs a name")?
                    .to_string();
                let url = format!(
                    "{}/{}",
                    record_base.trim_end_matches('/'),
                    path.file_name()
                        .and_then(|s| s.to_str())
                        .unwrap_or_default()
                );
                Ok(export::Session {
                    id,
                    record: export::RecordSource { url },
                    lines: export::read_record(path)?,
                })
            })
            .collect::<Result<Vec<_>>>()?;

        let summaries = export::export(&sessions, out)?;
        for summary in &summaries {
            println!("{} — {} turn(s)", summary.id, summary.turns);
        }
        println!("written to {}", out.display());
        return Ok(());
    }

    if let Command::Tools { sandbox } = &command {
        let agency = sandbox.resolve()?;
        print!("{}", agency.describe());
        let definitions = agency.definitions();
        if !definitions.is_empty() {
            println!("\n--- the prefix block, verbatim ---\n{definitions}");
        }
        return Ok(());
    }

    if let Command::Serve {
        bind,
        sandbox_args,
        backend,
        model,
        ollama_url,
        mock_delay_ms,
        record,
        context_limit,
        tokenizer,
        reserve,
        evict,
        low_water,
    } = command
    {
        let backend = build_backend(backend, &ollama_url, mock_delay_ms, Vec::new());
        let model = model_for(backend.as_ref(), model);
        let (counter, warning) = counter_for(&model, tokenizer.as_deref())?;
        if let Some(warning) = &warning {
            eprintln!("warning: {warning}");
        }
        let agency = sandbox_args.resolve()?;
        eprint!("{}", agency.describe());
        return serve::serve(serve::ServeOptions {
            address: bind,
            backend: backend.into(),
            model,
            record,
            budget: Budget::new(context_limit, reserve, evict.policy(low_water)),
            counter,
            agency,
        })
        .await;
    }

    let Command::Chat {
        prompt,
        sandbox_args,
        script,
        backend,
        model,
        ollama_url,
        mock_delay_ms,
        mock_replies,
        cancel_after_ms,
        record,
        context_limit,
        tokenizer,
        reserve,
        evict,
        low_water,
    } = command
    else {
        unreachable!("serve and tools are handled above");
    };

    let agency = sandbox_args.resolve()?;

    // One prompt, or a file of them: a script is what makes a multi-turn run
    // repeatable, and a baseline that cannot be re-run is not a baseline.
    let steps = match (&script, prompt) {
        (Some(path), _) => {
            let text = std::fs::read_to_string(path)
                .with_context(|| format!("reading {}", path.display()))?;
            let steps: Vec<Step> = text
                .lines()
                .map(str::trim)
                .filter(|line| !line.is_empty() && !line.starts_with('#'))
                .map(Step::parse)
                .collect::<Result<_>>()?;
            if !steps.iter().any(|step| matches!(step, Step::Prompt(_))) {
                anyhow::bail!("{} has no prompts in it", path.display());
            }
            steps
        }
        (None, Some(prompt)) => vec![Step::Prompt(prompt)],
        (None, None) => vec![Step::Prompt(std::io::read_to_string(std::io::stdin())?)],
    };

    let backend = build_backend(backend, &ollama_url, mock_delay_ms, mock_replies);
    let model = model_for(backend.as_ref(), model);
    let (counter, warning) = counter_for(&model, tokenizer.as_deref())?;
    if let Some(warning) = &warning {
        eprintln!("warning: {warning}");
    }

    let budget = Budget::new(context_limit, reserve, evict.policy(low_water));
    let started_at = now_ms();
    let recorder = match &record {
        Some(path) => Some(std::sync::Arc::new(
            Recorder::create(
                path,
                backend.name(),
                &model,
                budget,
                counter.id(),
                started_at,
            )
            .await?,
        )),
        None => None,
    };

    // The history the turns accumulate into. One turn and it stays empty; the
    // selection still runs either way, because a `chat` that assembled its
    // prompt differently from `serve` would be measuring something the server
    // never sends.
    let mut context = AgentContext::new(SYSTEM).with_tools(agency.definitions());
    let mut prefix = PrefixTracker::default();
    let mut tasks = Tasks::default();
    let mut failed = false;
    let mut turn: agent_core::protocol::TurnId = 0;
    let multi = steps.len() > 1;

    for step in &steps {
        let prompt = match step {
            Step::Prompt(text) => text,
            // The lifecycle, run from the script so that a task session is as
            // repeatable as a plain one. Approval is typed, never implied: a
            // run that approved its own plans would be measuring a gate that
            // is not there.
            directive => {
                match run_directive(
                    directive,
                    &mut tasks,
                    &mut context,
                    &mut prefix,
                    backend.as_ref(),
                    &model,
                    budget,
                    counter.as_ref(),
                    recorder.as_deref(),
                )
                .await
                {
                    Ok(()) => continue,
                    Err(refusal) => {
                        eprintln!("refused: {refusal}");
                        failed = true;
                        break;
                    }
                }
            }
        };

        // Nothing runs while a plan is waiting to be read.
        if let Err(refusal) = tasks.gate() {
            if let Some(recorder) = &recorder {
                recorder.write(&Event::Protocol(ServerMessage::Refused {
                    reason: refusal.clone(),
                }));
            }
            eprintln!("refused: {refusal}");
            failed = true;
            break;
        }

        turn += 1;
        let task = tasks.active().map(|task| task.id);
        tasks.record_turn(turn);
        let selection = context.select(prompt, &[], budget, counter.as_ref());

        if let Some(recorder) = &recorder {
            recorder.write(&Event::Protocol(ServerMessage::TurnStarted {
                turn,
                prompt: prompt.clone(),
                task,
            }));
            let text = rendered(&selection.messages);
            let reuse = prefix.measure(turn, &text, counter.as_ref());
            recorder.write(&Event::Trace(TraceMessage::Prompt { turn, text }));
            if let Some(reuse) = reuse {
                recorder.write(&Event::Trace(reuse));
            }
            // Before the call, not after: this is what we decided to send, and
            // a cancelled turn has it too.
            recorder.write(&Event::Trace(TraceMessage::Budget {
                turn,
                limit: selection.limit,
                counter: selection.counter.clone(),
                buckets: selection.buckets.clone(),
            }));
        }

        let request = CompletionRequest {
            model: model.clone(),
            messages: selection.messages,
            context_limit: selection.limit,
        };

        let (stop, cancel) = watch::channel(false);
        if let Some(ms) = cancel_after_ms {
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_millis(ms)).await;
                let _ = stop.send(true);
            });
        }

        let (tx, mut rx) = mpsc::channel::<TurnEvent>(256);
        let printer = {
            let recorder = recorder.clone();
            let prompt = prompt.clone();
            tokio::spawn(async move {
                let mut out = stdout();
                if multi {
                    // A transcript of a scripted run is unreadable without the
                    // questions in it.
                    let _ = out.write_all(format!("\n> {prompt}\n\n").as_bytes()).await;
                }
                while let Some(event) = rx.recv().await {
                    if let Some(recorder) = recorder.as_ref() {
                        let message = ServerMessage::from_turn_event(turn, event.clone());
                        recorder.write(&Event::Protocol(message));
                    }
                    match event {
                        // Written and flushed per token on purpose: this is the
                        // CLI's whole job at this stage — showing that
                        // generation streams.
                        TurnEvent::Token(text) => {
                            let _ = out.write_all(text.as_bytes()).await;
                            let _ = out.flush().await;
                        }
                        TurnEvent::ToolCall { step, call } => {
                            let arguments = serde_json::to_string(&call.arguments)
                                .unwrap_or_else(|_| "{}".into());
                            let _ = out
                                .write_all(
                                    format!("\n\n  [{step}] → {} {arguments}\n", call.name)
                                        .as_bytes(),
                                )
                                .await;
                            let _ = out.flush().await;
                        }
                        TurnEvent::ToolResult { step, outcome } => {
                            // The verdict is on the line, always. "The agent
                            // ran a command" and "the kernel held the command
                            // it ran" are different facts.
                            let verdict = &outcome.outcome.verdict;
                            // The rule is printed once. On a denial the error
                            // is the rule with a word in front of it, and
                            // saying it twice reads as two findings.
                            let status = match (verdict.allowed, &outcome.outcome.error) {
                                (false, _) => "denied",
                                (true, Some(error)) => error.as_str(),
                                (true, None) => "ok",
                            };
                            let _ = out
                                .write_all(
                                    format!(
                                        "  [{step}] ← {status} · {} · held by {} · {} ms\n\n",
                                        verdict.rule, verdict.enforced_by, outcome.duration_ms,
                                    )
                                    .as_bytes(),
                                )
                                .await;
                            let _ = out.flush().await;
                        }
                        TurnEvent::Ended { reason, usage } => {
                            let _ = out.write_all(b"\n").await;
                            let counts = match usage {
                                Some(u) => format!(
                                    "{} prompt / {} completion",
                                    u.prompt_tokens, u.completion_tokens
                                ),
                                None => "usage unknown".to_string(),
                            };
                            let reason = match reason {
                                EndReason::Stop => "stop",
                                EndReason::Length => "length",
                                EndReason::Other => "other",
                                EndReason::ToolLimit => "tool limit reached",
                                EndReason::Cancelled => "cancelled",
                            };
                            let _ = out
                                .write_all(format!("\n[{reason}] {counts}\n").as_bytes())
                                .await;
                            let _ = out.flush().await;
                        }
                        TurnEvent::Failed(error) => {
                            let _ = out
                                .write_all(format!("\n\n[failed] {error}\n").as_bytes())
                                .await;
                            let _ = out.flush().await;
                        }
                    }
                }
            })
        };

        let outcome = run_agent_turn(
            backend.as_ref(),
            request,
            agency.tools.as_ref(),
            agency.sandbox.as_ref(),
            agency.max_steps,
            tx,
            cancel,
        )
        .await;
        let _ = printer.await;

        // A cancelled turn keeps its partial answer: it happened, and the next
        // turn was asked in its light. A turn that produced nothing is not
        // remembered — an empty assistant message is not a thing that happened.
        if !outcome.text.is_empty() || !outcome.steps.is_empty() {
            context.push_turn_with_steps(
                prompt.clone(),
                outcome.text,
                vec![],
                outcome.steps,
                counter.as_ref(),
            );
        }

        // A script does not push on through a broken backend: the remaining
        // turns would all fail the same way and bury the first, real error.
        if outcome.error.is_some() {
            failed = true;
            break;
        }
    }

    // Let the recorder task drain before the process exits.
    tokio::time::sleep(Duration::from_millis(50)).await;

    if failed {
        std::process::exit(1);
    }
    Ok(())
}
