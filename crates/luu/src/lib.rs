//! `luu` — the CLI.
//!
//! One name, one binary: `src/bin/luu.rs` is a thin wrapper over [`run`]. The
//! second name the design used to install alongside it is gone; see
//! `RECORD/2026-09-04.the-name-and-the-config-dir.completed.md`.

use std::time::Duration;

use agent_core::agent::{DEFAULT_MAX_STEPS, run_agent_turn};
use agent_core::approval::{Approval, Approvers, Signer};
use agent_core::backend::{Backend, CompletionRequest, mock::Mock, ollama::Ollama, openai::OpenAi};
use agent_core::context::{Budget, Context as AgentContext, Eviction, Fragment};
use agent_core::fragment;
use agent_core::protocol::{ClientMessage as ServerBoundMessage, ServerMessage};
use agent_core::repo_map::{Order, RepoMap};
use agent_core::sandbox::{Access, Enforcement, Sandbox, SandboxPolicy};
use agent_core::task::{ApprovedBy, ClosedBy, Plan, PlanSource};
use agent_core::tools::Tools;
use agent_core::trace::TraceMessage;
use agent_core::turn::{EndReason, TurnEvent};
use agent_core::worker::{Runtime, Worker, WorkerConfig, WorkerSpec, serve_stdio};

use crate::session::{
    Agency, DEFAULT_RESERVE, Event, PrefixTracker, Recorder, SYSTEM, counter_for, now_ms, rendered,
};
use anyhow::{Context, Result};

pub mod auth;
pub mod config;
pub mod export;
pub mod serve;
pub mod session;
pub mod store;
pub mod transfer;
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

    /// The executor half of the worker IPC: read tool calls on stdin, run them,
    /// write outcomes on stdout.
    ///
    /// This is what runs *inside* the container — the container's only process,
    /// so its lifetime is the session's and `--rm` plus a closed stdin is the
    /// whole of the cleanup. It takes no sandbox flags on purpose: the sandbox
    /// arrives with every call, as the policy it is built from, because the
    /// paths a host resolved are not the paths an image has. See
    /// `RECORD/2026-09-02.the-worker-and-the-seam.completed.md`.
    Worker {
        /// A command the policy allows, so the handshake can report whether
        /// this side's `PATH` actually has it. Repeatable.
        ///
        /// It is the third failure mode — granted by the policy, absent from
        /// the image — answered by the only process that can see the image.
        #[arg(long = "command", value_name = "NAME")]
        commands: Vec<String>,
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

    /// Write a session out as a transfer bundle: its record stream, and an
    /// envelope naming where it came from.
    ///
    /// What crosses a border is the **stream**, never a snapshot of the fold —
    /// the fold is a cache of the stream, and a transfer built on the stream
    /// inherits the parity test that makes the fold trustworthy. See
    /// `RECORD/2026-09-04.the-border-and-the-gate.completed.md`.
    Transfer {
        /// The session, as `GET /api/sessions` and `luu serve` name it. Leave
        /// it out and pass `--record` instead to move a recording.
        session: Option<String>,

        /// Move a `.jsonl` recording rather than a stored session. The file
        /// stem names the session, the way `luu export` names one.
        #[arg(long, value_name = "FILE", conflicts_with = "session")]
        record: Option<std::path::PathBuf>,

        /// Where the store is. Defaults to the state directory's `sessions.db`.
        #[arg(long, value_name = "PATH")]
        store: Option<std::path::PathBuf>,

        /// The directory to write — `manifest.json` and `record.jsonl`.
        #[arg(long, short, value_name = "DIR")]
        out: std::path::PathBuf,

        /// The origin's own sandbox travels in the manifest, so the person
        /// approving on the far side can see what this job could reach here.
        #[command(flatten)]
        sandbox: SandboxArgs,
    },

    /// Read a transfer bundle into this host's store.
    ///
    /// Every job that is not closed **returns to this host's gate**: an
    /// approval is a statement about resolved paths on one tree, and the same
    /// words name different files here. A plan this `luu.toml` does not grant
    /// arrives refused, carrying the lines that refused it.
    Import {
        /// The bundle directory, as `luu transfer --out` wrote it.
        bundle: std::path::PathBuf,

        /// Where the store is. Defaults to the state directory's `sessions.db`.
        #[arg(long, value_name = "PATH")]
        store: Option<std::path::PathBuf>,

        /// What to call it here. Defaults to the name it had on the origin,
        /// and is refused if this host already has a session by that name.
        #[arg(long = "as", value_name = "ID")]
        rename: Option<String>,

        /// The policy file every arriving plan is re-checked against. This
        /// host's, which is the whole point.
        #[command(flatten)]
        sandbox: SandboxArgs,
    },

    /// Approval keys: make one, or sign an approval with it.
    ///
    /// A verification path with no way to produce a signature is a feature
    /// nobody can run, and the signing half belongs where a remote operator's
    /// `luu` can call it rather than inside one surface's UI. See
    /// `RECORD/2026-09-04.signed-approvals.completed.md`.
    Key {
        #[command(subcommand)]
        action: KeyAction,
    },

    /// Print the repository map that a budget resolves to, and what it cost.
    ///
    /// The map is the last block of the cached prefix, so being able to look at
    /// the bytes is the same difference `luu tools` exists for — and it is the
    /// only place the walk's two rules (no dot-directories, no `target`) are
    /// visible rather than inferred from a number that looks wrong.
    Map {
        #[command(flatten)]
        sandbox: SandboxArgs,

        /// Tokens to spend on the outline.
        #[arg(long, default_value_t = 1024)]
        map_tokens: u32,

        /// Order the map by what the rest of the tree depends on, instead of
        /// by path. Off, and the reason is a measurement rather than caution:
        /// at 1024 tokens the alphabet holds five files and the ranking holds
        /// two, because rank order puts the big central files first and the
        /// fill rule stops at the first that does not fit. `luu map --explain`
        /// prints the ranking either way. See
        /// `RECORD/2026-09-02.ranking-the-map.completed.md`.
        #[arg(long)]
        map_rank: bool,

        /// Order the map by direct weighted in-degree (breadth of inbound references).
        #[arg(long)]
        map_in_degree: bool,

        /// Pack the token budget non-greedily (skip oversized files to fit smaller ones).
        #[arg(long)]
        map_non_greedy: bool,

        /// The model's `tokenizer.json`. Without it the count is `chars/4` and
        /// says so, which is a different number from what a run would spend.
        #[arg(long)]
        tokenizer: Option<std::path::PathBuf>,

        /// Print the ranking that chose the order: every outlined file, its
        /// score, and the files that reference it.
        ///
        /// The order stopped being the alphabet's, so it stopped being legible
        /// from `ls` — and a selection nobody can interrogate is the thing
        /// embeddings were rejected for. It prints *after* the block and never
        /// inside it: a map that explained itself to the model would spend the
        /// budget on its own footnotes.
        #[arg(long)]
        explain: bool,
    },

    /// Serve the debug UI and the agent protocol over HTTP.
    Serve {
        #[command(flatten)]
        sandbox_args: SandboxArgs,

        #[arg(long, default_value = "127.0.0.1:7878")]
        bind: std::net::SocketAddr,

        /// A file holding the bearer token every `/ws` and `/api/*` request
        /// must carry. Required to bind anything but a loopback address:
        /// `/ws` approves plans, and off loopback that is one request away
        /// from anyone who can reach the port.
        #[arg(long, value_name = "PATH")]
        auth_token_file: Option<std::path::PathBuf>,

        /// Where sessions are cached between restarts, as SQLite. Defaults to
        /// `sessions.db` in the state directory the first run chose
        /// (`~/.luu` or `~/.config/luu`, or wherever `LUU_HOME` names);
        /// `--no-store` keeps the session in memory,
        /// which is what `serve` did before the store existed.
        ///
        /// Deliberately not beside `luu.toml`: the policy file describes *this
        /// project* and is meant to be committed with it, and a session store
        /// that travelled with a checkout would put one project's conversation
        /// into every clone of it. See
        /// `RECORD/2026-09-02.sessions-in-sqlite.completed.md`.
        #[arg(long, value_name = "PATH")]
        store: Option<std::path::PathBuf>,

        /// Keep the session in memory only.
        #[arg(long, conflicts_with = "store")]
        no_store: bool,

        #[arg(long, value_enum, default_value_t = BackendKind::Mock)]
        backend: BackendKind,

        #[arg(long, default_value = "qwen2.5-coder:7b")]
        model: String,

        #[arg(long, default_value = "http://127.0.0.1:11434")]
        ollama_url: String,

        /// Where the OpenAI-compatible server is. `llama-server`, vLLM, LM
        /// Studio and the hosted endpoints all answer here — and none of them
        /// takes the window on a request, so start the server with the window
        /// this run budgets against.
        #[arg(long, default_value = agent_core::backend::openai::DEFAULT_BASE_URL)]
        openai_url: String,

        /// A file holding the bearer token for `--backend openai`. Omitted
        /// means no `Authorization` header at all, which is what a local
        /// server wants. A file rather than a flag or an env var, for the
        /// reason `--auth-token-file` gives.
        #[arg(long, value_name = "PATH")]
        api_key_file: Option<std::path::PathBuf>,

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

        /// Tokens of repository outline to put in the prefix — definitions
        /// with their signatures, bodies elided, from tree-sitter. 0 is off,
        /// which is the default: a map that arrived switched on would change
        /// every number in every recording made so far. `luu map` prints what
        /// a budget resolves to.
        #[arg(long, default_value_t = 0)]
        map_tokens: u32,

        /// Order the map by what the rest of the tree depends on, instead of
        /// by path. Off, and the reason is a measurement rather than caution:
        /// at 1024 tokens the alphabet holds five files and the ranking holds
        /// two, because rank order puts the big central files first and the
        /// fill rule stops at the first that does not fit. `luu map --explain`
        /// prints the ranking either way. See
        /// `RECORD/2026-09-02.ranking-the-map.completed.md`.
        #[arg(long)]
        map_rank: bool,

        /// Order the map by direct weighted in-degree.
        #[arg(long)]
        map_in_degree: bool,

        /// Pack the token budget non-greedily.
        #[arg(long)]
        map_non_greedy: bool,
    },

    /// Serve the agent protocol over stdin/stdout as NDJSON.
    ///
    /// The editor surface: an IDE (VSCode, Neovim, Emacs) or language server
    /// spawns `luu stdio` as a subprocess and speaks the protocol directly over
    /// standard input and output. The gate, tasks, tools, and store behave
    /// identically to `luu serve` without needing a network port or authentication token.
    Stdio {
        #[command(flatten)]
        sandbox_args: SandboxArgs,

        /// Where sessions are cached between restarts, as SQLite. Defaults to
        /// `sessions.db` in the state directory the first run chose
        /// (`~/.luu` or `~/.config/luu`, or wherever `LUU_HOME` names);
        /// `--no-store` keeps the session in memory,
        /// which is what `serve` did before the store existed.
        #[arg(long, value_name = "PATH")]
        store: Option<std::path::PathBuf>,

        /// Keep the session in memory only.
        #[arg(long, conflicts_with = "store")]
        no_store: bool,

        #[arg(long, value_enum, default_value_t = BackendKind::Mock)]
        backend: BackendKind,

        #[arg(long, default_value = "qwen2.5-coder:7b")]
        model: String,

        #[arg(long, default_value = "http://127.0.0.1:11434")]
        ollama_url: String,

        /// Where the OpenAI-compatible server is. `llama-server`, vLLM, LM
        /// Studio and the hosted endpoints all answer here — and none of them
        /// takes the window on a request, so start the server with the window
        /// this run budgets against.
        #[arg(long, default_value = agent_core::backend::openai::DEFAULT_BASE_URL)]
        openai_url: String,

        /// A file holding the bearer token for `--backend openai`. Omitted
        /// means no `Authorization` header at all, which is what a local
        /// server wants.
        #[arg(long, value_name = "PATH")]
        api_key_file: Option<std::path::PathBuf>,

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

        /// Tokens of repository outline to put in the prefix — definitions
        /// with their signatures, bodies elided, from tree-sitter. 0 is off,
        /// which is the default: a map that arrived switched on would change
        /// every number in every recording made so far. `luu map` prints what
        /// a budget resolves to.
        #[arg(long, default_value_t = 0)]
        map_tokens: u32,

        /// Order the map by what the rest of the tree depends on, instead of
        /// by path. Off, and the reason is a measurement rather than caution:
        /// at 1024 tokens the alphabet holds five files and the ranking holds
        /// two, because rank order puts the big central files first and the
        /// fill rule stops at the first that does not fit. `luu map --explain`
        /// prints the ranking either way. See
        /// `RECORD/2026-09-02.ranking-the-map.completed.md`.
        #[arg(long)]
        map_rank: bool,

        /// Order the map by direct weighted in-degree.
        #[arg(long)]
        map_in_degree: bool,

        /// Pack the token budget non-greedily.
        #[arg(long)]
        map_non_greedy: bool,
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

        /// Where the OpenAI-compatible server is. `llama-server`, vLLM, LM
        /// Studio and the hosted endpoints all answer here — and none of them
        /// takes the window on a request, so start the server with the window
        /// this run budgets against.
        #[arg(long, default_value = agent_core::backend::openai::DEFAULT_BASE_URL)]
        openai_url: String,

        /// A file holding the bearer token for `--backend openai`. Omitted
        /// means no `Authorization` header at all, which is what a local
        /// server wants. A file rather than a flag or an env var, for the
        /// reason `--auth-token-file` gives.
        #[arg(long, value_name = "PATH")]
        api_key_file: Option<std::path::PathBuf>,

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

        /// Tokens of repository outline to put in the prefix — definitions
        /// with their signatures, bodies elided, from tree-sitter. 0 is off,
        /// which is the default: a map that arrived switched on would change
        /// every number in every recording made so far. `luu map` prints what
        /// a budget resolves to.
        #[arg(long, default_value_t = 0)]
        map_tokens: u32,

        /// Order the map by what the rest of the tree depends on, instead of
        /// by path. Off, and the reason is a measurement rather than caution:
        /// at 1024 tokens the alphabet holds five files and the ranking holds
        /// two, because rank order puts the big central files first and the
        /// fill rule stops at the first that does not fit. `luu map --explain`
        /// prints the ranking either way. See
        /// `RECORD/2026-09-02.ranking-the-map.completed.md`.
        #[arg(long)]
        map_rank: bool,

        /// Order the map by direct weighted in-degree.
        #[arg(long)]
        map_in_degree: bool,

        /// Pack the token budget non-greedily.
        #[arg(long)]
        map_non_greedy: bool,
    },
}

/// The flag, as the map's own type.
fn order_of(rank: bool, in_degree: bool) -> Order {
    if in_degree {
        Order::InDegree
    } else if rank {
        Order::Ranked
    } else {
        Order::Path
    }
}

fn fill_of(non_greedy: bool) -> agent_core::repo_map::Fill {
    if non_greedy {
        agent_core::repo_map::Fill::NonGreedy
    } else {
        agent_core::repo_map::Fill::Greedy
    }
}

#[derive(Subcommand)]
enum KeyAction {
    /// Make a key, write the private half, and print the block to paste into
    /// `luu.toml`.
    New {
        /// Where the private half goes. Written `0600`, for the reason
        /// `crate::auth` reads its token out of a file: a flag is greppable in
        /// `ps` and an environment variable is inherited by every child.
        #[arg(long, short, value_name = "FILE")]
        out: std::path::PathBuf,

        /// What the host's `luu.toml` will call this key. Names are per host.
        #[arg(long, default_value = "operator")]
        name: String,
    },

    /// Sign an `approve_job` message read on stdin, and write it back out with
    /// its signature attached.
    Sign {
        /// The private half, as `luu key new --out` wrote it.
        #[arg(long, short, value_name = "FILE")]
        key: std::path::PathBuf,

        /// The session the approval belongs to, as the host's `hello` names
        /// it. In the signed bytes, so a signature captured here does not
        /// replay against another host that numbers its jobs the same way.
        #[arg(long, value_name = "ID")]
        session: String,

        /// The name the host's `luu.toml` calls this key.
        #[arg(long = "as", default_value = "operator")]
        signer: String,
    },
}

/// `luu key`: the half of signed approvals that produces a signature.
fn run_key(action: &KeyAction) -> Result<()> {
    match action {
        KeyAction::New { out, name } => {
            let signer = Signer::generate()?;
            write_private(out, &signer.secret())?;
            eprintln!("wrote {} (mode 0600)", out.display());
            println!("[[approvals.key]]");
            println!("name = \"{name}\"");
            println!("public = \"{}\"", signer.public());
            Ok(())
        }
        KeyAction::Sign {
            key,
            session,
            signer,
        } => {
            let signing = Signer::from_file(key)?;
            let mut input = String::new();
            std::io::Read::read_to_string(&mut std::io::stdin(), &mut input)
                .context("reading the approval on stdin")?;
            let message: ServerBoundMessage =
                serde_json::from_str(input.trim()).context("parsing the approval on stdin")?;
            let ServerBoundMessage::ApproveJob {
                job,
                files,
                writes,
                commands,
                closes_on,
                network,
                egress,
                ..
            } = message
            else {
                anyhow::bail!("only an approve_job message can be signed");
            };
            let signature = signing.sign(
                &Approval {
                    session,
                    job,
                    files: &files,
                    writes: &writes,
                    commands: &commands,
                    closes_on: closes_on.as_ref(),
                    network,
                    egress: egress.as_ref(),
                },
                signer.clone(),
            )?;
            let signed = ServerBoundMessage::ApproveJob {
                job,
                files,
                writes,
                commands,
                closes_on,
                network,
                egress,
                signature: Some(signature),
            };
            println!("{}", serde_json::to_string(&signed)?);
            Ok(())
        }
    }
}

/// The private half, written where only its owner can read it — the same rule
/// `crate::auth` holds the bearer token to, and for the same reason.
fn write_private(path: &std::path::Path, text: &str) -> Result<()> {
    std::fs::write(path, format!("{text}\n"))
        .with_context(|| format!("writing {}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .with_context(|| format!("setting the mode of {}", path.display()))?;
    }
    Ok(())
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

    /// Where tool calls run: `host` (this process), `direct` (a `luu worker`
    /// child, no container), or a container runtime — `docker`, `podman`,
    /// `nerdctl`, `container`, `colima`. Overrides `[worker] runtime`.
    ///
    /// `direct` isolates nothing and says so in every line that reports it. It
    /// exists so the seam can be exercised where no runtime is installed.
    #[arg(long = "worker", value_name = "RUNTIME")]
    worker: Option<Runtime>,

    /// The image the worker runs in. Overrides `[worker] image`.
    #[arg(long = "worker-image", value_name = "REF")]
    worker_image: Option<String>,

    /// Where `luu` is, for `--worker direct`. Defaults to this binary.
    #[arg(long = "worker-binary", value_name = "PATH")]
    worker_binary: Option<std::path::PathBuf>,
}

#[derive(Copy, Clone, PartialEq, Eq, ValueEnum)]
enum EnforcementKind {
    Kernel,
    BestEffort,
}

impl SandboxArgs {
    /// Resolves the policy: the file, then the flags, then the working
    /// directory as the base every relative path is taken against.
    ///
    /// Async because the last step may be starting a worker, and a worker is
    /// not started until its handshake has come back: a session that announced
    /// a container and then could not reach one would be a session whose
    /// verdicts lie.
    /// The file `[sandbox]`, `[worker]` and `[approvals]` are all read from —
    /// three readers of one file, which is why the path is resolved in one
    /// place rather than three.
    fn policy_path(&self) -> Result<std::path::PathBuf> {
        let base = std::env::current_dir().context("the working directory")?;
        Ok(self
            .sandbox
            .clone()
            .unwrap_or_else(|| base.join("luu.toml")))
    }

    /// `[approvals]`: who may approve, and whether anyone must sign to. A file
    /// without the block is not an error — it is a `luu.toml` from before
    /// signatures existed, and its approvals are the operator's own.
    fn approvers(&self) -> Result<Approvers> {
        let path = self.policy_path()?;
        match path.exists() {
            true => {
                Approvers::from_file(&path).with_context(|| format!("reading {}", path.display()))
            }
            false => Ok(Approvers::default()),
        }
    }

    async fn resolve(&self) -> Result<Agency> {
        let base = std::env::current_dir().context("the working directory")?;

        let explicit = self.sandbox.is_some();
        let path = self.policy_path()?;
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

        // The `[worker]` block of the same file, and the flags over it. A
        // `luu.toml` from before this existed has no block and resolves to
        // `host`, which is the run every measurement in this repository was
        // made under.
        let mut worker_config = match path.exists() {
            true => WorkerConfig::from_file(&path)
                .with_context(|| format!("reading {}", path.display()))?,
            false => WorkerConfig::default(),
        };
        if let Some(runtime) = self.worker {
            worker_config.runtime = runtime;
        }
        if self.worker_image.is_some() {
            worker_config.image = self.worker_image.clone();
        }

        let sandbox = Sandbox::new(&policy, &base)?;
        let worker = match worker_config.runtime.is_worker() {
            false => None,
            true => {
                let spec = WorkerSpec::new(worker_config.runtime, sandbox.base())
                    .with_image(worker_config.image.clone())
                    // How the container is *created*, and never changed
                    // afterwards. A task that may not reach the network inside
                    // a session that may is the per-spawn seccomp filter's job.
                    .with_network(sandbox.network())
                    .with_binary(self.worker_binary.clone())
                    // The image's own trees, which the host must not try to
                    // resolve: `/usr/local/cargo` is the image's toolchain and
                    // is not a directory here.
                    .with_paths(worker_config.paths.clone());
                // What the runtime cannot express, said once, where a person
                // reads it — rather than silently dropped from the argv. Before
                // the start rather than after it, so a run that fails for an
                // unrelated reason still says what it would have been missing.
                if let Some(gap) = worker_config.runtime.cannot() {
                    eprintln!("warning: {}: {gap}", worker_config.runtime);
                }
                let worker = Worker::start(&spec, sandbox.commands())
                    .await
                    .with_context(|| format!("the {} worker", spec.label()))?;
                Some(std::sync::Arc::new(worker))
            }
        };

        Ok(Agency {
            tools: std::sync::Arc::new(match self.no_tools {
                true => Tools::new(Vec::new()),
                false => Tools::standard(),
            }),
            sandbox: std::sync::Arc::new(sandbox),
            max_steps: self.max_tool_steps,
            worker,
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
    /// Any OpenAI-compatible server: `llama-server`, vLLM, LM Studio, a hosted
    /// endpoint, or Ollama's own `/v1`.
    Openai,
}

/// Where a backend is, and what it needs to be reached. Grouped because two
/// subcommands build the same thing from the same flags, and a seventh
/// positional argument is how they drift apart.
struct BackendArgs<'a> {
    kind: BackendKind,
    ollama_url: &'a str,
    openai_url: &'a str,
    api_key_file: Option<&'a std::path::Path>,
    mock_delay_ms: u64,
    mock_replies: Vec<String>,
    /// What this run budgets against, only so the OpenAI backend can say that
    /// it cannot send it. Nothing else here reads it.
    context_limit: u32,
}

fn build_backend(args: BackendArgs<'_>) -> Result<Box<dyn Backend>> {
    Ok(match args.kind {
        BackendKind::Mock => Box::new(
            match args.mock_replies.is_empty() {
                true => Mock::default(),
                false => Mock::replies(args.mock_replies),
            }
            .delay(Duration::from_millis(args.mock_delay_ms)),
        ),
        BackendKind::Ollama => Box::new(Ollama::new(args.ollama_url)),
        BackendKind::Openai => {
            // Once, before anything is measured: this API has no field for the
            // window, so a run that budgets 8192 against a server started with
            // 4096 can only be told apart afterwards, by the prompt_tokens each
            // turn reports. Saying nothing here is how that becomes invisible.
            if let Some(caveat) = OpenAi::window_caveat(Some(args.context_limit)) {
                eprintln!("note: {caveat}");
            }
            let mut backend = OpenAi::new(args.openai_url);
            if let Some(path) = args.api_key_file {
                let key = std::fs::read_to_string(path)
                    .with_context(|| format!("reading the API key from {}", path.display()))?;
                let key = key.trim();
                if key.is_empty() {
                    anyhow::bail!("the API key file {} is empty", path.display());
                }
                backend = backend.with_api_key(key);
            }
            Box::new(backend)
        }
    })
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
/// rather than a question. See `RECORD/2026-08-30.tasks-in-code.completed.md` for why
/// this and not an `--auto-approve` flag.
#[derive(Debug, Clone, PartialEq)]
enum Step {
    Prompt(String),
    /// `## fragment: <path>[:start-end]` — fuses a file into the next prompt.
    Fragment(String),
    OpenJob {
        objective: String,
        plan: Plan,
    },
    CloseJob,
}

/// Parses a script: prompts one per line, `#` comments, and `##` directives.
///
/// ```text
/// ## job: explain the context manager
/// ## task: read the design
/// ## file: luu-design.md
/// ## write: luu-design.md
/// ## fragment: luu-design.md:1-40
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
            "job" => {
                if open {
                    anyhow::bail!("line {number}: a job is still open; `## close` it first");
                }
                if value.is_empty() {
                    anyhow::bail!("line {number}: `## job:` needs an objective");
                }
                open = true;
                steps.push(Step::OpenJob {
                    objective: value.to_string(),
                    plan: Plan::default(),
                });
            }
            "task" => {
                if !open {
                    if value.is_empty() {
                        anyhow::bail!("line {number}: `## task:` needs an objective");
                    }
                    open = true;
                    steps.push(Step::OpenJob {
                        objective: value.to_string(),
                        plan: Plan::default(),
                    });
                } else if let Some(Step::OpenJob { plan, .. }) = steps.last_mut() {
                    plan.tasks.push(value.to_string());
                } else {
                    anyhow::bail!("line {number}: a job is still open; `## close` it first");
                }
            }
            // The plan belongs to the job being opened and has to be the last
            // thing pushed: it is approved before its turns run, so it cannot
            // grow after one of them has.
            "step" | "file" | "write" | "command" | "network" | "egress" => {
                let Some(Step::OpenJob { plan, .. }) = steps.last_mut() else {
                    anyhow::bail!(
                        "line {number}: `{line}` must follow a `## job:` (or `## task:`), before its first prompt"
                    );
                };
                match key {
                    "step" => plan.tasks.push(value.to_string()),
                    "file" => plan.files.push(value.to_string()),
                    // `## file:` is what the job may read; `## write:` what it
                    // may also change. A plan that declares no writes may not
                    // write — the check is worth having only if it can say no.
                    "write" => plan.writes.push(value.to_string()),
                    "command" => plan.commands.push(value.to_string()),
                    "network" => {
                        plan.network = match value {
                            "" | "true" | "yes" | "on" => true,
                            "false" | "no" | "off" => false,
                            other => anyhow::bail!(
                                "line {number}: `## network:` expects true or false, got `{other}`"
                            ),
                        };
                    }
                    "egress" => {
                        for domain in value
                            .split([',', ' '])
                            .map(str::trim)
                            .filter(|d| !d.is_empty())
                        {
                            plan.egress.push(domain.to_string());
                        }
                        plan.network = true;
                    }
                    _ => unreachable!(),
                }
            }
            // Attached to the next prompt, wherever it appears — inside a job
            // or not. It is grounding for one turn, not part of the plan.
            "fragment" => {
                if value.is_empty() {
                    anyhow::bail!("line {number}: `## fragment:` needs a path");
                }
                steps.push(Step::Fragment(value.to_string()));
            }
            "close" => {
                if !open {
                    anyhow::bail!("line {number}: `## close` with no job open");
                }
                open = false;
                steps.push(Step::CloseJob);
            }
            _ => anyhow::bail!(
                "line {number}: `{line}` is not a directive \
                 (`## job:`, `## task:`, `## step:`, `## file:`, `## write:`, \
                 `## command:`, `## network:`, `## egress:`, `## fragment:`, `## close`)"
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

/// The store a transfer reads from or writes into: the one named, or the state
/// directory's. Undecidable without a home directory, and that is an error here
/// rather than a warning — `serve` can carry on in memory and a transfer cannot.
fn store_path(named: Option<std::path::PathBuf>) -> Result<std::path::PathBuf> {
    match named {
        Some(path) => Ok(path),
        None => crate::store::default_path().context(
            "no state directory (neither LUU_HOME nor HOME is set), so there is no default \
             session store: pass --store <path>",
        ),
    }
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

    // Before everything else, and reading no policy file: the worker is handed
    // its sandbox with every call, because the paths a host resolved are not
    // the paths this side has.
    if let Command::Worker { commands } = &command {
        return serve_stdio(
            std::sync::Arc::new(Tools::standard()),
            commands.clone(),
            tokio::io::stdin(),
            tokio::io::stdout(),
        )
        .await
        .context("the worker");
    }

    if let Command::Key { action } = &command {
        return run_key(action);
    }

    if let Command::Transfer {
        session,
        record,
        store,
        out,
        sandbox,
    } = &command
    {
        let agency = sandbox.resolve().await?;
        let source = match (session, record) {
            (Some(id), _) => transfer::Source::Store {
                path: store_path(store.clone())?,
                id: id.clone(),
            },
            (None, Some(path)) => transfer::Source::Record(path.clone()),
            (None, None) => anyhow::bail!(
                "name a session to transfer, or --record the file to move.                  `luu serve` lists the sessions this host has."
            ),
        };
        let (id, lines) = transfer::write(&source, agency.sandbox.as_ref(), out)?;
        println!("{id} — {lines} line(s) written to {}", out.display());
        println!(
            "the origin's sandbox travels with it:\n{}",
            agent_core::transfer::ResolvedSandbox::from(agency.sandbox.as_ref()).describe(),
        );
        println!(
            "move the directory to the other host and run: luu import {}",
            out.display(),
        );
        return Ok(());
    }

    if let Command::Import {
        bundle,
        store,
        rename,
        sandbox,
    } = &command
    {
        let agency = sandbox.resolve().await?;
        let store = store_path(store.clone())?;
        let read = transfer::read(bundle)?;
        let origin = read.manifest.origin.clone();
        let imported = transfer::import(&read, agency.sandbox.as_ref(), &store, rename.as_deref())?;

        println!(
            "{} — {} turn(s) from {} ({}), stored in {}",
            imported.id,
            imported.view.turns.len(),
            origin.session,
            origin.host,
            store.display(),
        );
        // The jobs first, because they are what somebody now has to answer.
        for job in &imported.jobs {
            let objective = imported
                .view
                .job(job.job)
                .map(|view| view.objective.as_str())
                .unwrap_or("");
            match job.unmet.is_empty() {
                true => println!("  job {} at the gate — {objective}", job.job),
                false => {
                    println!("  job {} refused — {objective}", job.job);
                    for line in &job.unmet {
                        println!("      {line}");
                    }
                }
            }
        }
        if imported.jobs.is_empty() {
            println!("  nothing to approve: every job arrived closed");
        } else {
            println!(
                "\nwhat the difference is between the two hosts:\n{}",
                transfer::difference(&origin.sandbox, agency.sandbox.as_ref()),
            );
            println!(
                "resume it to answer the gate: luu serve --store {}",
                store.display(),
            );
        }
        return Ok(());
    }

    if let Command::Tools { sandbox } = &command {
        let agency = sandbox.resolve().await?;
        print!("{}", agency.describe());
        let definitions = agency.definitions();
        if !definitions.is_empty() {
            println!("\n--- the prefix block, verbatim ---\n{definitions}");
        }
        return Ok(());
    }

    if let Command::Map {
        sandbox,
        map_tokens,
        map_rank,
        map_in_degree,
        map_non_greedy,
        tokenizer,
        explain,
    } = &command
    {
        let agency = sandbox.resolve().await?;
        // Named, because the warning that comes back says which model the
        // count belongs to — and a map counted by `chars/4` is a different
        // number from the one a run with a tokenizer would spend.
        let (counter, warning) = counter_for("the map", tokenizer.as_deref())?;
        if let Some(warning) = &warning {
            eprintln!("warning: {warning}");
        }
        let order = order_of(*map_rank, *map_in_degree);
        let fill = if *map_non_greedy {
            agent_core::repo_map::Fill::NonGreedy
        } else {
            agent_core::repo_map::Fill::Greedy
        };
        let map = RepoMap::build_with(
            agency.sandbox.as_ref(),
            *map_tokens,
            counter.as_ref(),
            order,
            fill,
        );
        print!("{}", map.render());
        if *explain {
            println!("\n--- the ranking, most depended-on first ---");
            for file in &map.ranked {
                let why = match file.referrers.is_empty() {
                    // Named rather than left blank: a file at the top with
                    // nothing pointing at it is the ranking admitting it had
                    // nothing to go on, which is worth seeing.
                    true => "referenced by nothing the sandbox can read".to_string(),
                    false => file
                        .referrers
                        .iter()
                        .take(3)
                        .map(|(path, weight)| format!("{path} ({weight:.2})"))
                        .collect::<Vec<_>>()
                        .join(", "),
                };
                println!(
                    "{} {:.5}  {}\n         {why}",
                    match file.in_map {
                        true => "in ",
                        false => "out",
                    },
                    file.score,
                    file.path,
                );
            }
        }
        println!(
            "\n--- {} file(s) outlined, {} left out, {} of {map_tokens} tokens{} ---",
            map.files.len(),
            map.left_out,
            map.tokens,
            match map.counted_by.is_approximate() {
                true => " (approximate: pass --tokenizer)",
                false => "",
            },
        );
        return Ok(());
    }

    if let Command::Serve {
        bind,
        auth_token_file,
        store,
        no_store,
        sandbox_args,
        backend,
        model,
        ollama_url,
        openai_url,
        api_key_file,
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
        map_tokens,
        map_rank,
        map_in_degree,
        map_non_greedy,
    } = command
    {
        let backend = build_backend(BackendArgs {
            kind: backend,
            ollama_url: &ollama_url,
            openai_url: &openai_url,
            api_key_file: api_key_file.as_deref(),
            mock_delay_ms,
            mock_replies,
            context_limit,
        })?;
        let model = model_for(backend.as_ref(), model);
        let (counter, warning) = counter_for(&model, tokenizer.as_deref())?;
        if let Some(warning) = &warning {
            eprintln!("warning: {warning}");
        }
        let approvers = sandbox_args.approvers()?;
        let agency = sandbox_args.resolve().await?;
        eprint!("{}", agency.describe());
        if approvers.required {
            eprintln!(
                "approvals — signed, {} key(s) named in the policy file",
                approvers.keys.len()
            );
        }
        // Named, or the default, or nothing at all. A missing `HOME` leaves
        // the default undecidable, and the run says so rather than picking a
        // directory of its own and writing history into it.
        let store = match (no_store, store) {
            (true, _) => None,
            (false, Some(path)) => Some(path),
            (false, None) => {
                let path = crate::store::default_path();
                if path.is_none() {
                    eprintln!(
                        "warning: no state directory (neither LUU_HOME nor HOME is set), so no \
                         default session store: this session stays in memory. \
                         Pass --store <path> to keep it."
                    );
                }
                path
            }
        };
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
            store,
            map_tokens,
            map_order: order_of(map_rank, map_in_degree),
            map_fill: fill_of(map_non_greedy),
            auth_token_file,
            approvers,
        })
        .await;
    }

    if let Command::Stdio {
        store,
        no_store,
        sandbox_args,
        backend,
        model,
        ollama_url,
        openai_url,
        api_key_file,
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
        map_tokens,
        map_rank,
        map_in_degree,
        map_non_greedy,
    } = command
    {
        let backend = build_backend(BackendArgs {
            kind: backend,
            ollama_url: &ollama_url,
            openai_url: &openai_url,
            api_key_file: api_key_file.as_deref(),
            mock_delay_ms,
            mock_replies,
            context_limit,
        })?;
        let model = model_for(backend.as_ref(), model);
        let (counter, warning) = counter_for(&model, tokenizer.as_deref())?;
        if let Some(warning) = &warning {
            eprintln!("warning: {warning}");
        }
        let approvers = sandbox_args.approvers()?;
        let agency = sandbox_args.resolve().await?;
        eprint!("{}", agency.describe());
        let store = match (no_store, store) {
            (true, _) => None,
            (false, Some(path)) => Some(path),
            (false, None) => {
                let path = crate::store::default_path();
                if path.is_none() {
                    eprintln!(
                        "warning: no state directory (neither LUU_HOME nor HOME is set), so no \
                         default session store: this session stays in memory. \
                         Pass --store <path> to keep it."
                    );
                }
                path
            }
        };
        return serve::stdio(serve::StdioOptions {
            backend: backend.into(),
            model,
            record,
            budget: Budget::new(context_limit, reserve, evict.policy(low_water)),
            counter,
            agency,
            temperature,
            seed,
            store,
            map_tokens,
            map_order: order_of(map_rank, map_in_degree),
            map_fill: fill_of(map_non_greedy),
            approvers,
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
        openai_url,
        api_key_file,
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
        map_tokens,
        map_rank,
        map_in_degree,
        map_non_greedy,
    } = command
    else {
        unreachable!("serve and tools are handled above");
    };

    let agency = sandbox_args.resolve().await?;

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

    let backend = build_backend(BackendArgs {
        kind: backend,
        ollama_url: &ollama_url,
        openai_url: &openai_url,
        api_key_file: api_key_file.as_deref(),
        mock_delay_ms,
        mock_replies,
        context_limit,
    })?;
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
    // The map is built once, before the first turn: it is the last block of the
    // cached prefix, and a block that is rebuilt mid-session is not a prefix.
    // What that costs — an agent that edits a file then carries the outline it
    // had — is named in `RECORD/2026-08-31.the-repo-map.completed.md`.
    let map = RepoMap::build_with(
        agency.sandbox.as_ref(),
        map_tokens,
        counter.as_ref(),
        order_of(map_rank, map_in_degree),
        fill_of(map_non_greedy),
    );
    if !map.is_empty() {
        println!(
            "== repository map: {} file(s), {} left out, {} of {map_tokens} tokens",
            map.files.len(),
            map.left_out,
            map.tokens,
        );
    }
    let mut context = AgentContext::new(SYSTEM)
        .with_tools(agency.definitions())
        .with_map(map.render());
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
            Step::OpenJob { objective, plan } => {
                // The confirmation, as a check rather than a question. A plan
                // that asks for what the sandbox does not grant does not run —
                // the alternative is discovering it as a denial four turns in.
                let unmet = plan.unmet(agency.sandbox.as_ref());
                if !unmet.is_empty() {
                    anyhow::bail!(
                        "job `{objective}` asks for what the sandbox does not grant:\n  {}",
                        unmet.join("\n  "),
                    );
                }
                let id = context.propose_job(objective.clone(), plan.clone());
                // `luu chat` has no gate, so it has no approver either: the
                // policy file is its standing approval and the operator who
                // ran the command is who that was. See the open question in
                // `RECORD/2026-09-04.signed-approvals.completed.md`.
                context.approve_job(id, ApprovedBy::Operator);
                narrowed = Some(std::sync::Arc::new(
                    plan.narrow(agency.sandbox.as_ref(), id)
                        .with_context(|| format!("resolving the plan of job `{objective}`"))?,
                ));
                if let Some(recorder) = &recorder {
                    recorder.write(&Event::Protocol(ServerMessage::JobProposed {
                        job: id,
                        objective: objective.clone(),
                        plan: plan.clone(),
                        // No planning call happened: a script carries its plan,
                        // which is an approval written down in advance.
                        source: Some(PlanSource::Written),
                    }));
                    recorder.write(&Event::Protocol(ServerMessage::JobApproved {
                        job: id,
                        plan: plan.clone(),
                        approved_by: Some(ApprovedBy::Operator),
                    }));
                }
                println!("\n== job {id} approved: {objective}");
                print!("{}", plan.describe());
                continue;
            }
            Step::CloseJob => {
                // The parser guarantees a job is open here.
                let Some(id) = context.live_job() else {
                    unreachable!("`## close` with no job open is refused when the script is read")
                };
                let summary = context.close_job(id, counter.as_ref()).unwrap_or_default();
                // Outside a job the policy file is the whole answer again.
                narrowed = None;
                if let Some(recorder) = &recorder {
                    recorder.write(&Event::Protocol(ServerMessage::JobClosed {
                        job: id,
                        summary: summary.clone(),
                        // `## close` is a person's instruction written down in
                        // advance, which is the same authority typed later.
                        by: Some(ClosedBy::User),
                    }));
                }
                println!("\n== job {id} closed; its turns are now sent as:");
                for line in summary.lines() {
                    println!("   {line}");
                }
                continue;
            }
            Step::Fragment(spec) => {
                // Through the sandbox that holds *now*: a path `read_file`
                // would refuse must not become readable by spelling it in a
                // directive, and inside a job it is the plan that refuses.
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
        let job = context.live_job();
        // Taken, not copied: these fragments are this turn's, and the next turn
        // starts with none.
        let code = std::mem::take(&mut attached);
        let selection = context.select(prompt, &code, budget, counter.as_ref());

        // Said out loud, not only into the recording: a run that quietly
        // forgets half its history looks exactly like one that answers from all
        // of it, and the difference is the whole subject.
        if let Some(evicted) = &selection.eviction {
            let turns = evicted
                .turns
                .iter()
                .map(|turn| turn.to_string())
                .collect::<Vec<_>>()
                .join(", ");
            println!(
                "\n== evicted turn{} {turns} — {} tokens{} out of the window, for good",
                match evicted.turns.len() {
                    1 => "",
                    _ => "s",
                },
                evicted.tokens,
                match evicted.counter.is_approximate() {
                    true => " (approximate)",
                    false => "",
                },
            );
        }

        if let Some(recorder) = &recorder {
            recorder.write(&Event::Protocol(ServerMessage::TurnStarted {
                turn,
                prompt: prompt.clone(),
                job,
            }));
            // Before the prompt it explains, so a file reads in the order the
            // session happened: the history was cut, then this is what was
            // sent.
            if let Some(evicted) = selection.eviction.clone() {
                recorder.write(&Event::Protocol(ServerMessage::Evicted {
                    turn,
                    turns: evicted.turns,
                    tokens: evicted.tokens,
                    counter: evicted.counter,
                    policy: evicted.policy,
                }));
            }
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
            agency.executor(),
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
                turn,
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
             ## file: luu-design.md\n\
             ## command: cargo\n\
             what does it do?\n\
             ## close\n",
        )
        .unwrap();

        assert_eq!(steps.len(), 3);
        let Step::OpenJob { objective, plan } = &steps[0] else {
            panic!("{steps:?}")
        };
        assert_eq!(objective, "explain the context manager");
        assert_eq!(plan.tasks, ["read the design"]);
        assert_eq!(plan.files, ["luu-design.md"]);
        assert_eq!(plan.commands, ["cargo"]);
        assert_eq!(steps[2], Step::CloseJob);
    }

    #[test]
    fn a_job_carries_its_approved_egress() {
        let steps = parse_script(
            "## job: fetch updates\n\
             ## egress: crates.io, *.github.com\n\
             fetch them\n\
             ## close\n",
        )
        .unwrap();

        assert_eq!(steps.len(), 3);
        let Step::OpenJob { plan, .. } = &steps[0] else {
            panic!("{steps:?}")
        };
        assert!(plan.network);
        assert_eq!(plan.egress, ["crates.io", "*.github.com"]);
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
        let Step::OpenJob { plan, .. } = &steps[1] else {
            panic!("expected the job after it, {steps:?}");
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
                .contains("no job open")
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
                .contains("must follow a `## job:`")
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
