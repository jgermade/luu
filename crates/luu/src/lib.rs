//! `luu` — the Loude CLI.
//!
//! The binary is installed under two names (`luu` and `loude`); both are thin
//! wrappers over [`run`].

use std::time::Duration;

use agent_core::agent::{DEFAULT_MAX_STEPS, run_agent_turn};
use agent_core::backend::{Backend, CompletionRequest, mock::Mock, ollama::Ollama};
use agent_core::context::{Budget, Context as AgentContext, Eviction, Fragment};
use agent_core::fragment;
use agent_core::protocol::ServerMessage;
use agent_core::sandbox::{Access, Enforcement, Sandbox, SandboxPolicy};
use agent_core::task::Plan;
use agent_core::tools::Tools;
use agent_core::trace::TraceMessage;
use agent_core::turn::{EndReason, TurnEvent};

use crate::session::{
    Agency, DEFAULT_RESERVE, Event, PrefixTracker, Recorder, SYSTEM, counter_for, now_ms, rendered,
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

        /// What the mock backend answers, one per model call, the last
        /// repeating. The gate needs two to be seen working without a model:
        /// the plan block the planning call returns, then the answer.
        #[arg(long = "mock-reply", value_name = "TEXT")]
        mock_replies: Vec<String>,

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

        /// Pin the sampler's temperature. Unset leaves it to the server's own
        /// default, which is not fixed across calls.
        #[arg(long)]
        temperature: Option<f32>,

        /// Pin the sampler's seed. Unset leaves it to the server's own choice.
        #[arg(long)]
        seed: Option<u32>,
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

        /// Fuse a file into the next prompt, as `PATH` or `PATH:START-END`.
        /// Repeatable, read through the sandbox, and attached to **one** turn —
        /// which turns a file belongs in is what relevance selection exists to
        /// decide later. In a script, `## fragment: <path>` does the same thing
        /// at the point it appears.
        #[arg(long = "fragment", value_name = "PATH[:START-END]")]
        fragments: Vec<String>,

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

        /// Pin the sampler's temperature, so two runs meant to be compared
        /// differ only by what they're testing. Unset leaves it to the
        /// server's own default.
        #[arg(long)]
        temperature: Option<f32>,

        /// Pin the sampler's seed. Unset leaves it to the server's own choice.
        #[arg(long)]
        seed: Option<u32>,
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

/// One instruction from a script, or the single prompt of a one-shot `chat`.
///
/// A script is the only harness that makes two runs comparable, and a task is
/// confirmed before anything runs. Both hold: **the script carries the approved
/// plan**, written down and reviewable in a diff, and approving it is a check
/// rather than a question. See `RECORD/2026-08-30.tasks-in-code.md` for why
/// this and not an `--auto-approve` flag.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    Prompt(String),
    /// `## fragment: <path>[:start-end]` — fuses a file into the next prompt.
    Fragment(String),
    OpenTask {
        objective: String,
        plan: Plan,
    },
    CloseTask,
}

/// Parses a script: prompts one per line, `#` comments, and `##` directives.
///
/// ```text
/// ## task: explain the context manager
/// ## step: read the design
/// ## file: loude-design.md
/// ## write: loude-design.md
/// ## fragment: loude-design.md:1-40
/// what does the context manager do?
/// ## close
/// ```
///
/// `## file:` and `## fragment:` are not the same thing and the names have to
/// stay apart: the first declares a path the *plan* is allowed to touch, the
/// second puts that file's text into the next prompt. A plan may name a file it
/// never reads, and a fragment may ground a turn no plan mentions.
///
/// A directive it does not know is an error rather than a prompt: a typo that
/// silently becomes a question would put a line of `## fille: x` into a
/// recorded baseline and nothing would ever say so.
fn parse_script(text: &str) -> Result<Vec<Step>> {
    let mut steps = Vec::new();
    let mut open = false;

    for (number, line) in text.lines().enumerate() {
        let line = line.trim();
        let number = number + 1;
        if line.is_empty() || (line.starts_with('#') && !line.starts_with("##")) {
            continue;
        }
        let Some(directive) = line.strip_prefix("##").map(str::trim) else {
            steps.push(Step::Prompt(line.to_string()));
            continue;
        };

        let (key, value) = match directive.split_once(':') {
            Some((key, value)) => (key.trim(), value.trim()),
            None => (directive, ""),
        };

        match key {
            "task" => {
                if open {
                    anyhow::bail!("line {number}: a task is still open; `## close` it first");
                }
                if value.is_empty() {
                    anyhow::bail!("line {number}: `## task:` needs an objective");
                }
                open = true;
                steps.push(Step::OpenTask {
                    objective: value.to_string(),
                    plan: Plan::default(),
                });
            }
            // The plan belongs to the task being opened and has to be the last
            // thing pushed: it is approved before its turns run, so it cannot
            // grow after one of them has.
            "step" | "file" | "write" | "command" => {
                let Some(Step::OpenTask { plan, .. }) = steps.last_mut() else {
                    anyhow::bail!(
                        "line {number}: `{line}` must follow a `## task:`, before its first prompt"
                    );
                };
                match key {
                    "step" => plan.steps.push(value.to_string()),
                    "file" => plan.files.push(value.to_string()),
                    // `## file:` is what the task may read; `## write:` what it
                    // may also change. A plan that declares no writes may not
                    // write — the check is worth having only if it can say no.
                    "write" => plan.writes.push(value.to_string()),
                    _ => plan.commands.push(value.to_string()),
                }
            }
            // Attached to the next prompt, wherever it appears — inside a task
            // or not. It is grounding for one turn, not part of the plan.
            "fragment" => {
                if value.is_empty() {
                    anyhow::bail!("line {number}: `## fragment:` needs a path");
                }
                steps.push(Step::Fragment(value.to_string()));
            }
            "close" => {
                if !open {
                    anyhow::bail!("line {number}: `## close` with no task open");
                }
                open = false;
                steps.push(Step::CloseTask);
            }
            _ => anyhow::bail!(
                "line {number}: `{line}` is not a directive \
                 (`## task:`, `## step:`, `## file:`, `## write:`, \
                 `## command:`, `## fragment:`, `## close`)"
            ),
        }
    }

    if !steps.iter().any(|step| matches!(step, Step::Prompt(_))) {
        anyhow::bail!("no prompts in it");
    }
    Ok(steps)
}

/// Reads one `--fragment` or `## fragment:`, through the sandbox.
///
/// The user typed the path, not the model, so this is not the rule about model
/// output reaching the filesystem. It is the other one: a path `read_file`
/// would refuse must not become readable by spelling it in a flag.
///
/// A denial is an error rather than a warning. A run that quietly dropped the
/// file it was told to ground itself with would answer out of the model's
/// training and look like it worked, which is the failure this surface exists
/// to remove.
fn load_fragment(sandbox: &Sandbox, spec: &str) -> Result<Fragment> {
    fragment::load(sandbox, &fragment::Spec::parse(spec))
        .with_context(|| format!("fragment {spec}"))
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
        mock_replies,
        record,
        context_limit,
        tokenizer,
        reserve,
        evict,
        low_water,
        temperature,
        seed,
    } = command
    {
        let backend = build_backend(backend, &ollama_url, mock_delay_ms, mock_replies);
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
            temperature,
            seed,
        })
        .await;
    }

    let Command::Chat {
        prompt,
        sandbox_args,
        script,
        fragments,
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
        temperature,
        seed,
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
            parse_script(&text).with_context(|| path.display().to_string())?
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
    // Shared with the printer task, because the tool round trips are announced
    // there and they belong in the same chain as the turns: two trackers would
    // measure one session against two different pasts.
    let prefix = std::sync::Arc::new(std::sync::Mutex::new(PrefixTracker::default()));
    let mut failed = false;

    let multi = steps
        .iter()
        .filter(|step| matches!(step, Step::Prompt(_)))
        .count()
        > 1;
    let mut turn: agent_core::protocol::TurnId = 0;

    // Waiting to be fused into the next prompt, then gone: a fragment belongs
    // to one turn and is stored with it. The flags are read before the first
    // step, so `--fragment` and a one-shot prompt behave like a script that
    // opens with `## fragment:`.
    let mut attached: Vec<Fragment> = fragments
        .iter()
        .map(|spec| load_fragment(agency.sandbox.as_ref(), spec))
        .collect::<Result<_>>()?;

    // The live task's own sandbox, from `## task:` to `## close`. A script's
    // written plan is its approval, so it narrows exactly as a plan approved at
    // the gate does: what the task may touch is what the plan named.
    let mut narrowed: Option<std::sync::Arc<agent_core::sandbox::Sandbox>> = None;

    for step in &steps {
        // The task lifecycle happens between turns, and every part of it is
        // recorded: a run that cannot say what it was allowed to do is not a
        // baseline either.
        let prompt = match step {
            Step::OpenTask { objective, plan } => {
                // The confirmation, as a check rather than a question. A plan
                // that asks for what the sandbox does not grant does not run —
                // the alternative is discovering it as a denial four turns in.
                let unmet = plan.unmet(agency.sandbox.as_ref());
                if !unmet.is_empty() {
                    anyhow::bail!(
                        "task `{objective}` asks for what the sandbox does not grant:\n  {}",
                        unmet.join("\n  "),
                    );
                }
                let id = context.propose_task(objective.clone(), plan.clone());
                context.approve_task(id);
                narrowed = Some(std::sync::Arc::new(
                    plan.narrow(agency.sandbox.as_ref(), id)
                        .with_context(|| format!("resolving the plan of task `{objective}`"))?,
                ));
                if let Some(recorder) = &recorder {
                    recorder.write(&Event::Protocol(ServerMessage::TaskProposed {
                        task: id,
                        objective: objective.clone(),
                        plan: plan.clone(),
                    }));
                    recorder.write(&Event::Protocol(ServerMessage::TaskApproved {
                        task: id,
                        plan: plan.clone(),
                    }));
                }
                println!("\n== task {id} approved: {objective}");
                print!("{}", plan.describe());
                continue;
            }
            Step::CloseTask => {
                // The parser guarantees a task is open here.
                let Some(id) = context.live_task() else {
                    unreachable!("`## close` with no task open is refused when the script is read")
                };
                let summary = context.close_task(id, counter.as_ref()).unwrap_or_default();
                // Outside a task the policy file is the whole answer again.
                narrowed = None;
                if let Some(recorder) = &recorder {
                    recorder.write(&Event::Protocol(ServerMessage::TaskClosed {
                        task: id,
                        summary: summary.clone(),
                    }));
                }
                println!("\n== task {id} closed; its turns are now sent as:");
                for line in summary.lines() {
                    println!("   {line}");
                }
                continue;
            }
            Step::Fragment(spec) => {
                // Through the sandbox that holds *now*: a path `read_file`
                // would refuse must not become readable by spelling it in a
                // directive, and inside a task it is the plan that refuses.
                let sandbox = narrowed.clone().unwrap_or_else(|| agency.sandbox.clone());
                let fragment = load_fragment(sandbox.as_ref(), spec)?;
                println!(
                    "\n== fragment {} — {} bytes into the next prompt",
                    fragment.path,
                    fragment.text.len()
                );
                attached.push(fragment);
                continue;
            }
            Step::Prompt(prompt) => prompt,
        };

        turn += 1;
        let task = context.live_task();
        // Taken, not copied: these fragments are this turn's, and the next turn
        // starts with none.
        let code = std::mem::take(&mut attached);
        let selection = context.select(prompt, &code, budget, counter.as_ref());

        if let Some(recorder) = &recorder {
            recorder.write(&Event::Protocol(ServerMessage::TurnStarted {
                turn,
                prompt: prompt.clone(),
                task,
            }));
            let text = rendered(&selection.messages);
            let reuse = prefix
                .lock()
                .expect("the prefix tracker is never held across an await")
                .measure(turn, &text, counter.as_ref());
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
            // The window we budgeted against, sent so the server serves it.
            // Budgeting 8k against a server truncating to 4k measures a prompt
            // the model never saw, and nothing in the recording would say so.
            context_limit: budget.limit,
            temperature,
            seed,
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
            let prefix = prefix.clone();
            let counter = counter.clone();
            tokio::spawn(async move {
                let mut out = stdout();
                if multi {
                    // A transcript of a scripted run is unreadable without the
                    // questions in it.
                    let _ = out.write_all(format!("\n> {prompt}\n\n").as_bytes()).await;
                }
                while let Some(event) = rx.recv().await {
                    // A model call is not a protocol message: it explains the
                    // agent rather than driving it. Measured into the same
                    // chain as the turns, from the second call on — the first
                    // is the turn's own prompt and is already in it.
                    if let TurnEvent::ModelCall { step, messages } = &event {
                        if let (Some(recorder), true) = (recorder.as_ref(), *step > 1) {
                            let text = rendered(messages);
                            let mut tracker = prefix
                                .lock()
                                .expect("the prefix tracker is never held across an await");
                            if let Some(TraceMessage::PrefixReuse {
                                shared_bytes,
                                shared_tokens,
                                prompt_tokens,
                                ..
                            }) = tracker.measure(turn, &text, counter.as_ref())
                            {
                                recorder.write(&Event::Trace(TraceMessage::StepCall {
                                    turn,
                                    step: *step,
                                    text,
                                    prompt_tokens,
                                    shared_bytes,
                                    shared_tokens,
                                }));
                            }
                        }
                        continue;
                    }
                    if let Some(recorder) = recorder.as_ref()
                        && let Some(message) = ServerMessage::from_turn_event(turn, event.clone())
                    {
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
                        // Handled above, before the protocol conversion: it is
                        // not a protocol message and it is not printed.
                        TurnEvent::ModelCall { .. } => {}
                    }
                }
            })
        };

        // Inside a task, the plan it was approved with; outside one, the policy
        // file. Never both.
        let sandbox = narrowed.clone().unwrap_or_else(|| agency.sandbox.clone());
        let outcome = run_agent_turn(
            backend.as_ref(),
            request,
            agency.tools.as_ref(),
            sandbox.as_ref(),
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
                code,
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_script_without_tasks_parses_the_way_it_always_did() {
        let steps = parse_script("# a comment\n\nfirst question\nsecond question\n").unwrap();
        assert_eq!(
            steps,
            vec![
                Step::Prompt("first question".into()),
                Step::Prompt("second question".into()),
            ]
        );
    }

    #[test]
    fn a_task_carries_its_approved_plan() {
        let steps = parse_script(
            "## task: explain the context manager\n\
             ## step: read the design\n\
             ## file: loude-design.md\n\
             ## command: cargo\n\
             what does it do?\n\
             ## close\n",
        )
        .unwrap();

        assert_eq!(steps.len(), 3);
        let Step::OpenTask { objective, plan } = &steps[0] else {
            panic!("{steps:?}")
        };
        assert_eq!(objective, "explain the context manager");
        assert_eq!(plan.steps, ["read the design"]);
        assert_eq!(plan.files, ["loude-design.md"]);
        assert_eq!(plan.commands, ["cargo"]);
        assert_eq!(steps[2], Step::CloseTask);
    }

    #[test]
    fn a_fragment_attaches_to_the_next_prompt_and_is_not_part_of_a_plan() {
        let steps =
            parse_script("## fragment: luu.toml:1-5\n## task: a\n## file: luu.toml\nq\n## close\n")
                .unwrap();
        assert_eq!(
            steps[0],
            Step::Fragment("luu.toml:1-5".into()),
            "it stands on its own: grounding for a turn, not a path the plan declares",
        );
        let Step::OpenTask { plan, .. } = &steps[1] else {
            panic!("expected the task after it, {steps:?}");
        };
        assert_eq!(plan.files, ["luu.toml"]);

        assert!(
            parse_script("## fragment:\nq\n")
                .unwrap_err()
                .to_string()
                .contains("needs a path")
        );
    }

    #[test]
    fn a_mistyped_directive_is_refused_rather_than_asked_as_a_question() {
        let error = parse_script("## task: x\n## fille: y\nq\n").unwrap_err();
        assert!(
            error.to_string().contains("is not a directive"),
            "a typo that becomes a prompt would sit in a baseline unnoticed: {error}",
        );
    }

    #[test]
    fn the_lifecycle_has_to_make_sense_in_the_file() {
        assert!(
            parse_script("## close\nq\n")
                .unwrap_err()
                .to_string()
                .contains("no task open")
        );
        assert!(
            parse_script("## task: a\nq\n## task: b\nq\n")
                .unwrap_err()
                .to_string()
                .contains("still open")
        );
        assert!(
            parse_script("## step: read it\nq\n")
                .unwrap_err()
                .to_string()
                .contains("must follow a `## task:`")
        );
        assert!(
            parse_script("## task: a\nq\n## step: late\n")
                .unwrap_err()
                .to_string()
                .contains("before its first prompt"),
            "a plan cannot grow after a turn it was supposed to authorise",
        );
    }

    #[test]
    fn a_script_of_directives_and_no_questions_is_not_a_script() {
        assert!(
            parse_script("## task: a\n## close\n")
                .unwrap_err()
                .to_string()
                .contains("no prompts")
        );
    }
}
