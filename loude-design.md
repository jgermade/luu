# Loude (CLI alias: `luu`)

Local AI agent written in Rust that orchestrates calls to models, optimized for local inference (limited context, tight token budget).

This file is the design as it stands now, and is rewritten as decisions change. The dated
reasoning behind them — including the answers that were wrong first — lives in [`RECORD/`](RECORD/),
which is append-only. See [AGENTS.md](AGENTS.md).

## Goals

- **Optimize the context window** for local models (7B–32B, 8K–32K context) — the key differentiator vs. existing solutions that don't do this well.
- **Constrain resource access**: configurable folders and commands the agent can reach, checked in our own code *and* held by the kernel (Landlock + seccomp) for anything that runs as a subprocess.
- **Container mode**: option to run inside a container to isolate system access.
- **CLI mode**: direct terminal usage.
- **VSCode integration**: a chat tab similar to Copilot Chat.

## Tasks

A session is a sequence of **tasks**, not a mode the agent is in. One task:

```
the user asks for something
  → the agent proposes a plan: steps, files it will touch, commands it will run
  → CONFIRMATION                  ← nothing runs before this
  → loop: act · check · ask when unsure
  → closing question: "shall I call this done?"
  → close: the task is summarised; its turns stop being sent verbatim
```

Every task is confirmed before anything runs, so there is no autonomy setting to
remember and no mode that can be left open. `plan`/`run`/`auto` used to live here;
they were global where approval is per piece of work, and — the reason they went —
a mode cannot tell the context manager what to compact. See
[`RECORD/2026-08-27.tasks-instead-of-modes.md`](RECORD/2026-08-27.tasks-instead-of-modes.md).

**Built**, and the shape it took is in
[`RECORD/2026-08-27.tasks-in-the-core.md`](RECORD/2026-08-27.tasks-in-the-core.md):

- **A task owns no turns.** It is bookkeeping — a plan, a state, and where in the
  history it began — and everything it produces enters the history as an ordinary
  turn tagged with a `TurnKind`: the approved plan is one (prompt = the objective as
  typed, answer = the plan), the closing summary is another. That is what keeps the
  strict alternation the prompt shape depends on, and what keeps both inside the
  budget instead of in a second, quieter window nobody measures. They are counted in
  their own `tasks` bucket, because "the scaffolding costs N tokens" is a number the
  strategy has to justify.
- **The gate is a comparison, not a mode.** A prompt while a plan is waiting is
  *refused, visibly* (`refused` on the wire) rather than dropped or queued: a queued
  prompt runs against a plan nobody answered.
- **The model writes the plan**, parsed out of a ```` ```plan ```` block the way a
  tool call is parsed. The planning call is measured like any other call — it
  carries its prompt, its size and its reuse on the trace channel, because a call
  nothing measures is a cost nothing accounts for. **`parsed: true` is a syntax
  check, not a grounding one**: a real 7B produced clean plan blocks naming
  Python files that do not exist in this Rust workspace, because nothing in the
  prompt had shown it the repository. When the block does not parse, the model's own lines become
  the steps and the plan says so — the user is about to approve this, and a plan
  with our sentences in it is a confirmation of something nobody proposed. The
  planning call is an ordinary user message on the unchanged prefix, so proposing
  costs no cache, and it is not remembered: the approved plan enters the history,
  the request that produced it does not.
- **The summary is evidence, not prose**: the objective, the approved steps, and
  what the turns' tool calls actually did — paths touched, commands and how they
  exited, calls the sandbox refused. No model call, so closing cannot fail because
  generation did, and the same session summarises the same way twice.
- **Closing folds the span** from the plan turn to the end of the history into that
  one summary. What it cost and bought (`turns`, tokens before and after) is a trace
  message, beside the other numbers.
- Tasks are **opt-in** for now: `luu chat "hola"` still runs a turn with no task
  around it, because that path is what every baseline recording uses. In `chat` the
  lifecycle is script directives — `:task <objective>`, `:approve`, `:discard`,
  `:close` — so a task session is as repeatable as a plain one and approval is
  always typed. In the debug UI it is `/task <objective>` and two buttons.

**What folding actually buys depends on the window**, and that is the finding.
The same twenty prompts, with and without tasks, measured twice
([`RECORD/2026-08-27.tasks-in-the-core.md`](RECORD/2026-08-27.tasks-in-the-core.md),
third and fourth passes):

*1024 tokens, mock backend — a window chosen so the baseline is forced to evict:*

|  | reuse, all calls | turn prompts | planning | total sent |
| --- | ---: | ---: | ---: | ---: |
| plain · `turn` (baseline) | 69.9% | 16 397 | — | 16 397 |
| plain · `block` | 89.9% | 15 294 | — | 15 294 |
| tasks · either policy | 91.1% | 13 954 | 2 559 | 16 513 |

*8192 tokens, `qwen2.5-coder:7b` on an M4 Pro — the regime this tool targets:*

|  | reuse, all calls | turn prompts | planning | total sent |
| --- | ---: | ---: | ---: | ---: |
| plain · `turn` (baseline) | 89.5% | 66 986 | — | 66 986 |
| tasks · `turn` | 80.7% | 31 879 | 3 177 | 35 056 |

- **Narrow window: folding buys reuse, not tokens.** Four planning calls at
  557–718 tokens put the total above both baselines, so the 15% saved on turn
  prompts is spent on proposing. The eviction policy stops mattering entirely —
  the history never grows enough for either to fire.
- **Comfortable window: folding buys tokens, not reuse.** The baseline never
  evicts, so its history compounds over all twenty turns and folding sends about
  half as much. Reuse goes the other way: an append-only history that is never
  rewritten is the best case a prefix cache has, and folding rewrites it four
  times.
- So the two quantities trade against each other and **which one you win is
  decided by whether the window forces the baseline to evict.** State the
  configuration beside the figure — a reuse percentage compares only against the
  same window, the same prefix and the same counter.
- The scaffolding is real either way: at 1024, 2 095 tokens of plans and
  summaries against 2 103 of history. A planning call is a per-task cost against
  a per-turn benefit, so the ratio is task length: 15% of the session at five
  turns a task, about 5% at fifteen. **A design that proposes a plan per task
  wants tasks worth planning.**
- A fold costs ~15 points of reuse, and it is the *planning call* that pays them,
  not a turn — visible only because a planning call is measured. Turn-to-turn
  reuse inside a tasks run is flat.

The reason to keep it is not any single figure, then: it is the two things a
threshold cannot give at any figure — the cut lands where the work ended, and the
transcript collapses exactly what the context collapses.

Still missing from the loop: reopening a closed task, the judge below, and the
task-scoped `SandboxPolicy` — see *Open questions*.

One boundary does three jobs, which is the argument for it:

- **Compaction gets a cut point the work chose** rather than one the counter fell
  on. A closed task is the only rewrite in an otherwise write-once session: plan,
  turns and summary are each written once and never edited.
- **Permission gets its scope**: the approved plan named the files and commands, so
  it *is* the `SandboxPolicy` for that task. One informed approval beats a prompt
  per tool call, which is what trains people to click yes without reading.
- **The transcript gets its grouping**: a closed task collapses to its summary in the
  UI — the view collapses exactly what the context collapses.

A task can still overflow the window on its own, so the boundary is the *preferred*
cut point and the token threshold remains the fallback.

**Who declares a task done** is a ladder, not a single answer: deterministic checks
(exit codes, tests) first, then a judge, then the user's final question, which is the
only authority. The judge is the same model given a short context of **evidence, not
narrative** — the objective as typed, the diff, the commands and their exit codes —
because a judge fed the model's own account of its work inherits its hallucinations.
It runs in shadow mode until measured: every task the user closes is a label, so the
accuracy figure arrives for free and gates nothing until it exists. **Not built** —
today the user closes the task with `:close` and nothing judges anything, which is
the right order: there is nothing to be accurate about until tasks have been closed
in anger.

## Overall architecture

```
┌─────────────┐   ┌──────────────────┐   ┌─────────────────┐
│  CLI (bin)  │   │  VSCode ext (TS) │   │  Web debug UI   │
└──────┬──────┘   └────────┬─────────┘   └────────┬────────┘
       │ in-process        │ stdio               │ WebSocket
       └───────────────────┴─────────────────────┘
                            │   one JSON message schema, N transports
                  ┌─────────▼──────────┐
                  │   agent-core (lib) │  ← all the brains here
                  │  tasks, context,   │
                  │  tools, sandbox    │
                  └─────────┬──────────┘
                            │
                  ┌─────────▼──────────┐
                  │  inference backend  │ (llama.cpp / Ollama)
                  └─────────────────────┘
```

The core knows nothing about CLI, VSCode or the browser — it exposes an internal API (Rust) plus a local server speaking a single JSON message schema over swappable transports. This gets container isolation for free: the container only wraps the core.

## Context management (the differentiating piece)

### The shape of the prompt: one rule

`system`, then strict `user`/`assistant` alternation. Anything that is not a turn —
selected code, a rolling summary — is **fused into the nearest turn's user message**,
never sent as a message of its own. Storage keeps them separate, so pruning can reach
them later without parsing back what was already written.

```
[system]      system text + tool definitions   ← never changes: the cached prefix
[user]        (fused summary) + oldest retained turn
[assistant]   …
[user] …      recent turns, verbatim           ← appended to, never rewritten
[user]        selected code + the new prompt   ← fused, as late as possible
```

What changes most often goes last. Bucket order in the rendered prompt is a cache
decision, not a readability one, and retrieved code never goes in the system block:
it changes every turn, and putting it there invalidates the whole prefix without
anything failing.

### Decide, then render

`Context::select` chooses what fits against the budget and *then* renders; the
rendering is a pure function of the selection, so every token sent is attributable
to a bucket. Rendering everything and trimming the string is the alternative, and it
forecloses the point: a rendered prompt no longer knows where each span came from,
and the cut lands wherever the limit happens to fall — sooner or later inside the
stable prefix.

- **Explicit token budget**: real tokenization with the loaded model's tokenizer
  (HuggingFace `tokenizers` crate, `--tokenizer`), split across system/tools, code
  context, history, and a reserve held back for the answer before any history is
  considered. Without a tokenizer file the count degrades to `chars/4` and is
  **labelled approximate everywhere it appears** — it is not a measurement.
- **A file gets into a turn by being attached to it** (`--fragment PATH[:A-B]`,
  `:file` in a script), read through the sandbox — a path `read_file` would
  refuse must not become readable by spelling it in a flag — and fused into that
  turn's user message only. It does not follow the conversation: which turns a
  file belongs in is the question relevance selection exists to answer, and
  attaching it to all of them answers it wrong, expensively. A planning call sees
  what is pending without consuming it, so a plan can be proposed in view of the
  file it is about. Until this existed the `code` bucket was always zero, every
  script was ungrounded Q&A, and a 7B asked "what does the context manager do?"
  answered about Python's `__enter__`/`__exit__` — correctly, to a question
  nobody had asked.
- **Every count carries its counter.** A stored turn records which counter produced
  its tokens, and a budget names the counter that produced it. Two runs measured
  differently are not comparable, and nothing else would say so.
- **A turn is not a call, and every call is measured.** A turn that uses a tool
  is several model calls — the first ends in a tool block, the next carries the
  result — and the backend's `usage.prompt_tokens` is summed over all of them.
  Measuring only the first made our count and the backend's count different
  things: 1 590 against 3 552 on a real run, on a turn where every other turn
  agreed within 3 tokens. So the agent loop announces **every** call it makes and
  the trace channel measures the ones after the first (`step_call`, beside
  `plan_call` for the same reason). The budget bar is still the first call, and
  the panel says how much the round trips added rather than folding it into the
  chat-template gap.
- **Prefix reuse is measured, not assumed.** Every turn after the first carries the
  longest common prefix between its prompt and the previous one, in bytes and in
  tokens. A cache stops at the first difference, so a prefix is the whole quantity —
  matching text after the divergence is reuse the cache never gets. It is measured
  against our own rendering rather than the templated string, which is a proxy and
  labelled as one; it holds because a chat template renders message by message, so an
  identical message prefix is an identical templated prefix. This is the number a
  context strategy is judged by, and it is measurable against the mock backend
  because it is decided before the call.
- **The turn is the unit.** Eviction drops whole turns, oldest first, so the retained
  window can only ever start on a user message. Half a turn leaves an answer to a
  question nobody asked; a window starting on an assistant message makes several chat
  templates continue instead of answering.
- **Order blocks by how often they are rewritten — growing at the end is not being
  rewritten.** A prefix cache reuses the longest common prefix, so a large block that
  changes rarely belongs *above* an append-only history, not below it.
- **Eviction cuts in blocks, not one turn at a time** (`--evict block`, default
  `turn`). Dropping the minimum rewrites the history from its front on every call
  once the window is full, and the reusable prefix collapses to the constant part
  of the prompt — and never recovers. Cutting down to a low-water mark
  (`--low-water`, 0.5) instead pays for that once every four or five turns and
  holds still in between: over 20 turns in a 1024-token window, mean reuse goes
  from **70% to 90%** and 7% fewer tokens are sent. It costs history, which is not
  free and is not yet known to be harmless.
  **A reuse percentage is not comparable across prefixes**: the same pair measured
  before tool definitions existed floored at 3% instead of 50%, because the floor
  *is* the constant share of the prompt. Numbers, and the correction, in
  [`RECORD/2026-08-27.prefix-reuse-and-block-eviction.md`](RECORD/2026-08-27.prefix-reuse-and-block-eviction.md).
  The window has to clear the tool definitions before any of this is visible at
  all — at 512 tokens they alone fill it, no history is selected, and both
  policies record the same run of nothing.
- **What leaves the window stays out.** Eviction is monotone: `Context` keeps a floor
  that only moves forward. Recomputing the retained window from scratch each turn
  lets a dropped turn return the moment a shorter prompt leaves room, which moves the
  front of the history and is the one thing a prefix cache cannot survive — and it
  makes block eviction impossible, because the next fill walks straight back past the
  cut.
- **A closed task is the one rewrite in a session.** Eviction drops the minimum and
  moves the front of the history every turn once the window is full; a fold cuts
  once, deeply, at a point the work chose, and the history is byte-identical in
  between. The floor moves back far enough to keep the summary inside the window
  when the fold spans turns that had already left it — the *index* moves, the
  content does not come back: the folded turns are gone from the history and what
  stands in their place is not one of them. A summary left below the floor would
  cost the tokens to write it and send none of them.
- **An unknown window is not an unlimited one**: `--context-limit 0` means unknown, so
  nothing is budgeted and nothing is evicted, and the panel says so rather than
  drawing a bar against nothing.
- **Stable prompt cache / prefix**: keep the fixed part (system + tool definitions)
  byte-identical across calls to take advantage of llama.cpp's prompt caching / KV
  cache reuse.

### The overhead we cannot see

Talking to Ollama's `/api/chat` means the chat template is applied on its side: the
`<|im_start|>` markers and per-message separators are tokens we never count. Owning
that number would mean owning the prompt string (`/api/generate` with `raw: true`),
which is deliberately not where this sits today.

So the gap is **accepted and reported**: the trace carries our count per bucket,
measured before the call, and the backend's own `usage.prompt_tokens` afterwards, and
the panel shows the difference as its own labelled quantity. A stable difference is
template overhead; a moving one means the template changed, which is worth more than
the precision given up.

### Still ahead

- **Hierarchical compaction**: built and measured. A closed task is replaced by its deterministic summary (the plan plus the evidence: paths, commands, exit codes, denials), and the token threshold stays as the fallback for a task that overflows alone. What the table says is narrower than the idea promised — see below. What is still unmeasured is the only question the mock cannot answer: whether the summary loses something the task needed — now askable, and the protocol for asking it is [`RECORD/2026-08-27.grounded-fold-probe.md`](RECORD/2026-08-27.grounded-fold-probe.md).
- **Relevance over recency**: inject only the fragments the current turn points at, instead of the full history. **The mechanism is `tree-sitter` tags plus a reference graph, not embeddings** — a graph can say *why* a file was included, staleness is `mtime`, and there is no second copy of the user's code to ship or govern. Decided against Aider's implementation; see [`RECORD/2026-08-27.aider-repo-map.md`](RECORD/2026-08-27.aider-repo-map.md). Tools and the sandbox now exist, so this is unblocked.
- **Active pruning of tool results**: summarize or drop old tool outputs (e.g. a `cat` of 2000 lines shouldn't stick around in context turns later). Now has results to prune and a bucket to watch shrink: a turn stores its steps, and each result is capped at 8 KiB but never shortened afterwards. The cap is not the strategy — it is what stops one `cat` blowing the window open while the strategy is still unmeasured.

## Tool calling: how actions actually get executed

The model never executes anything directly — it only emits a structured request that the program interprets and executes.

- **Agent loop**, built:
  1. Prompt + history + tools → model
  2. If a tool call: parse → validate against the `Sandbox` → execute (real Rust code) → append the call *and its result* to the turn → back to 1
  3. If plain text: end of turn

  Capped at `--max-tool-steps` (8), and exhausting it ends the turn with
  `EndReason::ToolLimit` rather than presenting an investigation cut short as a
  conclusion.
- **The steps are messages, and they alternate**: a turn renders as `user(prompt)`,
  then `assistant(call) · user(result)` per step, then `assistant(answer)`. The
  fusion rule applied to a new kind of content — and it means a turn with tool
  calls is still *one* turn, evicted whole, so the model never sees a result
  whose call has gone.
- **How the model expresses a call is a transport detail.** `ToolCall` is the type
  and the parser is one function. Today it reads a fenced ```` ```tool ```` block
  out of plain text, which works against any backend including a 7B that has
  never seen a tool API. **Native function calling** — JSON Schema definitions,
  a `tool_call` from the backend, and **GBNF grammars / constrained decoding**
  under llama.cpp to force valid JSON — replaces that function and nothing above
  it. Not built.
- **The definitions are the second half of the cached prefix**, so their rendering
  is a wire format: tools sorted by name, schemas serialized through
  `serde_json`'s sorted maps, nothing interpolated. `luu tools` prints the exact
  bytes. The budget has a `tools` bucket beside `system` for the same reason —
  "the system block grew" is not an answer to why the window is full.
- **File editing via diff/patch** (not full rewrites): `edit_file(path, old_string, new_string)`
  replaces an exact, *unique* occurrence and refuses an ambiguous one. Saves output
  tokens (the slow part of local generation) and reduces errors; the uniqueness rule
  is the safety half, because a replacement that matched twice would edit the one
  the model was not looking at.
- **Streaming**: the preceding reasoning text streams, but an unclosed block is a
  call still being generated, and half a call is not a call.

The set: `read_file`, `list_dir`, `edit_file`, `write_file`, `run_command`.
Output is capped at 8 KiB with a `truncated` flag — pruning old results out of
the history is a later, measured change; a cap is the part that is not a strategy.

## Sandbox / security

Three rungs, and the middle one is what makes the first worth having. Built: 1
and 2. See [`RECORD/2026-08-27.tools-and-sandbox.md`](RECORD/2026-08-27.tools-and-sandbox.md).

1. **In-process checks.** Canonicalize (`std::fs::canonicalize`) before comparing,
   or a symlink walks straight out. Everything an in-process tool can have — and
   nothing a subprocess gets: the check happens before the syscall, in a program
   that then makes the syscall itself. A child makes its own.
2. **The kernel, same process tree, no image and no daemon.** Landlock for the
   filesystem and seccomp for sockets, applied to the child between `fork` and
   `exec`. `run_command("cargo", …)` otherwise hands a build script the same
   authority the agent has, and checking the string `cargo` against an allowlist
   and calling that a sandbox is the part that would be a lie.
3. **A container.** Below, and *on top of* level 2 rather than instead of it:
   Landlock survives `exec` and cannot be dropped.

**Declarative config (TOML)** per project — `luu.toml`, and `luu tools` prints the
resolved result:

```toml
[sandbox]
enforcement = "kernel"          # or "best-effort"
network = false
commands = ["cargo", "git"]     # program names, never a shell string

[[sandbox.paths]]
path = "."
access = "read-write"           # read | execute | read-write
```

- **Longest match wins**, so a narrower rule under a broader one grants more.
- **There is no deny list**, deliberately, though an earlier version of this file
  had one. Landlock is allow-only: a subtraction could be honoured in-process and
  could not be honoured in a subprocess, so `denied = ["./.env"]` would stop
  `read_file` and would not stop `cat .env`, with nothing in the config saying so.
  The way to deny is to not grant.
- **The commands allowlist is a program name.** `run_command` takes `command` plus
  `args` and never a shell string, which would make the allowlist meaningless —
  `sh -c "cargo test; curl …"` passes any check that looks at the first word.
- **Allowing any command implies read+execute on the system roots** (`/usr`, `/bin`,
  `/sbin`, `/lib`, `/lib64`, `/etc`, `/opt`), because a program cannot run without
  reading its own interpreter. **For the child only** — an in-process tool sees
  only what was written down, or `commands = ["ls"]` would quietly grant the agent
  `/etc`.
- **Permission validation lives in the program's code**, not in the model behaving well.

### Who enforced it is reported, never assumed

Level 2 is Linux-only, so `enforcement` decides what happens where it is not
available — the one place in this design where a security property is a setting:

- `"kernel"` (default) — a subprocess runs only if the kernel took the ruleset and
  the filter. On macOS, or a kernel without Landlock, `run_command` is **denied**,
  and the denial names what is missing and the flag that lowers the bar.
- `"best-effort"` — apply what this kernel has and report the gap.

Either way every verdict carries `Applied` — `Process`, `Kernel { how }`, or
`Partial { how, missing }` — and `how` names the mechanism *and its version*,
because Landlock's older ABIs mediate less. Nothing here may say "sandboxed"
without saying by what: a run whose subprocesses the kernel held and a run whose
subprocesses nothing held are not the same run, and afterwards the recording is
the only thing that could tell them apart.

What level 2 does *not* claim: it is not a network namespace (blocking the
internet address families stops a program opening a connection, not one that
inherited a socket), and canonicalize-then-open still has a TOCTOU window for the
in-process tools — `openat2(RESOLVE_BENEATH)` is the answer there and is not built.

## Container mode

Level 3, and still ahead. The level-2 restrictions stay applied inside it.

- Compile to a static binary (`musl`) → minimal image (`scratch`/distroless).
- Bind-mount only allowed folders, network disabled by default (`--network none`).
- If the inference backend runs on the host (to use GPU/Metal), the container isolates only tool execution (fs, bash) and talks to the backend over an explicitly exposed socket.
- First version: rootless Podman or Docker with `--cap-drop=ALL` + non-root user.

## VSCode integration

- Use the **VSCode Chat API** (`vscode.chat.createChatParticipant`) — requires a lightweight TypeScript extension.
- The TS extension acts as a bridge: renders the UI (messages, plan approval) and talks to the Rust binary (`agent-core --serve`) over stdio or a socket, using JSON-lines or JSON-RPC (reusable for the CLI too).
- The task's confirmation step maps well to this UX: an editable plan before execution (similar to how Copilot shows changes before applying them), then execution with streaming of which tool is being used, and a closed task collapsing to its summary in the thread.

## Debug web client (agent protocol)

A local web UI (chat, session browser, context inspector) is the fastest way to see what the
context manager is actually doing — the CLI can't show a token budget or a prompt diff.

### Transport: HTTP + WebSocket, not REST-only and not gRPC

**Decision: one JSON message schema, several transports (stdio, WebSocket), served over plain HTTP.**

- **Not gRPC.** Browsers can't speak gRPC natively — it needs grpc-web plus a proxy (Envoy) or
  Connect. `tonic` also forces a protobuf IDL maintained in parallel with the `serde` types, with
  codegen in `build.rs`, and binary frames that are unreadable in devtools. gRPC pays off for
  cross-language, multi-service, high-throughput systems; this is one local process talking to one
  browser. The cost is all setup and the benefit is nil.
- **Not REST-only.** No streaming of tokens or tool events; polling makes a debug UI useless. Adding
  SSE on top gets you halfway to a WebSocket with more moving parts.
- **Reuse what stdio already needs.** The VSCode bridge already requires a JSON-lines protocol.
  Define the messages once as `serde` enums (`#[serde(tag = "type")]`), put the transport behind a
  trait, and the web client and the extension speak the same protocol — one schema to version, two
  clients debugged at once.

### Server shape

`luu serve --http 127.0.0.1:7878` (loopback by default, no auth; require a bearer token when bound
to any other address). The UI is embedded in the binary with `rust-embed`, so there is one command,
one URL, and no node process in the loop.

Live channel — `WS /ws`:

| Direction | Messages |
| --- | --- |
| client → server | `prompt`, `approve_plan`, `edit_plan`, `close_task`, `reopen_task`, `cancel` |
| server → client | `token`, `tool_call`, `tool_result`, `ended`, `failed` — built; `task_proposed`, `task_approved`, `task_closed`, `context_snapshot` wait on the task lifecycle |

Closing a task is an event, not a mutation: reopening one is folding the log
differently, never undoing a deletion. Freezing v1 of these enums waits on the task
lifecycle for that reason — it is the last cheap moment to add it.

Read side — plain GETs, browsable and curl-able:

- `GET /api/sessions` · `GET /api/sessions/:id` · `POST /api/sessions` · `DELETE /api/sessions/:id`
- `GET /api/sessions/:id/turns?from=&limit=`
- `GET /api/sessions/:id/turns/:n/prompt` — the exact string sent to the model
- `GET /api/sessions/:id/context` — current token budget breakdown
- `GET /events?session=` — SSE mirror of the WS stream, so a session can be followed with `curl -N`
  without a browser

Server stack: `axum` + `tokio`.

### UI stack

**[jq79](https://github.com/jgermade/jq79) — single-file, no compiler, zero dependencies.**

The constraint that decides this is the build pipeline, not the framework's ergonomics. A bundled
frontend (Vite + React or otherwise) forces one of two bad options: `npm` inside `build.rs`, so
`cargo build` requires node, or a committed `dist/` with the noise that brings. jq79 has no compiler
step — components are `.html` files loaded at runtime — so `rust-embed` ships one `jq79.js` plus a
handful of `.html` files and the build stays pure Cargo.

The rest follows from that:

- **Dev loop**: `jq79 dev` hot-reloads components with no bundler. Serve the directory from disk in
  debug builds (`#[cfg(debug_assertions)]`) and from `rust-embed` in release — the dev server applies
  no transforms, so both paths serve the same bytes. No Vite proxy to configure.
- **Rendering**: proxy-based fine-grained reactivity with no virtual DOM, and `:each` with `:key`
  keeps existing DOM when the transcript is appended to.
- **Virtualization**: no library, and none needed — bind `:each` to a computed window plus two spacer
  elements. `GET /api/sessions/:id/turns?from=&limit=` already puts pagination on the server.

Two consequences worth designing around rather than discovering:

- **Compute the prompt diff in Rust** (`similar`), and send resolved spans over the protocol. Pulling
  in CodeMirror or Monaco for it would reintroduce the bundler this choice just removed, and the
  server holds both prompt strings anyway — it is the right place for the work. Should the editable
  plan later need a real editor, `await $mounted()` + `$self(...)` mounts an imperative widget
  cleanly, so the option stays open without being paid for now.
- **No compile-time check of the protocol types**, which a TypeScript build would have given. Recover
  most of it by splitting the UI: the transport and store layer as a `// @ts-check`ed `.js` module
  validated against the `ts-rs`-generated `.d.ts`, templates untyped. `tsc --noEmit` stays an
  optional check task, never a build step.

### What this needs from jq79

Two gaps, neither expressible from userland, both general enough to belong upstream rather than in a
patched copy. Listed in the order loude hits them:

1. **A teardown hook** (`$onDestroy`, or a cleanup function returned from `$effect`). Effects, stores
   and injected styles are disposed with the component, but a resource the component itself owns has
   nowhere to be released. The first component written here — the one holding the WebSocket — needs
   it, as does any `requestAnimationFrame` loop or mounted imperative widget.
2. **Effect batching** (`batch(fn)`, microtask-coalesced flush). There is no scheduler; writes
   propagate synchronously, so a token stream costs one DOM write per token. Until it exists, buffer
   `token` messages and flush on `requestAnimationFrame` — worth doing under any framework.

**Promotion rule**, to keep a library whose value is "one file, no dependencies" from bending toward
its first consumer: a gap moves upstream only when it cannot be expressed in userland, or when the
same workaround has appeared twice. The two above pass the first test. List virtualization fails
both — it is ordinary component code, so it stays here until something else asks for it.

What flows back is a workload no tutorial or benchmark produces: long-lived sessions, sustained
high-frequency updates, thousand-row lists that grow at the end, and components owning external
resources.

### Debug panels that earn their place

Chat and session list are table stakes. The ones that justify building this at all:

1. **Token budget per turn** — stacked bar of system/tools · code context · history · reserve, with
   the underlying text of each block on hover.
2. **Prefix reuse against the previous turn** — how much of the stable prefix survived,
   i.e. the prompt-cache / KV-reuse hit rate, as a share of the prompt. Built. The
   span-level diff of the two prompt strings is not, and is a separate thing: the
   number says how much was reused, a diff would say what changed. `similar` is still
   right for the second and was the wrong tool for the first, which is a prefix.
3. **Tool call timeline** — arguments, sandbox verdict (allowed/denied and *which*
   rule matched), **who enforced it**, duration, and result size. Built. A call is
   listed when it is made and filled in when it returns, so one that is running or
   was denied reads as itself rather than as nothing happening. Result size
   *before/after pruning* waits on pruning existing.
4. **Compaction log** — when a rolling summary was generated, what it replaced, tokens saved.

### Record and replay

`luu serve --record <file>` dumps the JSON-lines stream to disk, and the UI can load such a file
instead of a live socket. Sessions become replayable offline — useful for bug reports and for
comparing context strategies across runs without re-running inference.

`luu chat --script <file>` runs a file of prompts, one per line, against one shared history. That
is what makes a multi-turn run repeatable: a baseline typed into a browser cannot be re-run, and
two recordings are only comparable when the same task list produced both. The record's header
carries the model, the window, the counter and the eviction policy for exactly that reason.
`scripts/tasks/steady-state.txt` is twenty turns of uniform size, long enough to show what a
policy does *after* the first eviction, which is where they stop agreeing. Note that the **mock backend
cannot validate a context strategy** — it does not read the context, so every strategy "wins"
against it; baselines need a real model.

## Inference backend

Decide between:
- Talking to Ollama/llama.cpp via its local HTTP API (simpler). *This is what
  exists: `POST /api/chat`, NDJSON stream.*
- Binding directly to `llama-cpp-rs` (FFI bindings) for fine-grained control over the KV cache across calls and avoiding the overhead of an intermediate HTTP server — recommended given the performance goal.

**The window is sent, not assumed.** The request carries `options.num_ctx` from
`--context-limit`, and omits it entirely when the window is unknown. Ollama's own
default is a couple of thousand tokens and it *truncates the prompt silently* to
it: a run that budgets 8k against a server serving 2k measures a prompt the model
never saw, and every bucket, reuse figure and usage count in it is a reading of
something else.

Two numbers the backend reports and we do not yet read: `prompt_eval_duration`
and `eval_duration`. They are what turns reuse into seconds, which is the
question the whole cache argument is a proxy for.

## Persistence

Nothing is persisted yet: `--record` is opt-in and per-run, and a session's history
lives in memory for the life of the process.

- Sessions in SQLite (`rusqlite`) with compressed state, to resume long tasks without recomputing context from scratch.
- **Whatever the store holds must be reproducible by folding the record.** The
  JSON-lines stream is the account of what happened, and `api::SessionView` already
  folds it; a store that accumulates state the events cannot regenerate is a second
  truth, which is how the static mirror and the live server start disagreeing.
- **Forgetting is an event too.** Eviction — and later compaction — is recorded, not
  just applied: a recording has to be able to say which turns left the window, when,
  and under which policy. Today the eviction floor is in-memory and a recording can
  only show the history bucket shrinking, which is the symptom without the cause.
  Decided, not built; the shape is OpenHands' condensation tombstones, read in
  [`RECORD/2026-08-27.cline-openhands.md`](RECORD/2026-08-27.cline-openhands.md).
- A stored turn keeps `code_context` separate from the prompt (per the fusion rule
  above) and its token count together with the counter that produced it. Store the
  fused rendering instead and a resumed session either recomputes everything or sums
  two different units into one bar.

## Suggested work order

1. `agent-core`: base types (`Task`, `Context`, `Tool`, `SandboxPolicy`) + inference backend (Ollama/llama.cpp). *All four exist, with the Ollama and mock backends.*
2. Context manager (the differentiating piece) working in plain CLI, without container or VSCode — to measure and iterate on performance quickly. *History, the budget, whole-turn eviction, block eviction and the fold at a task boundary exist, and prefix reuse is measured per turn. Whether folding on the boundary beats the threshold is unmeasured; relevance selection is still ahead.*
3. Agent protocol + `luu serve` + debug web client — early, because it is the instrument used to
   measure step 2. *Done.*
4. Path/command sandbox — in-process checks, then the kernel holding subprocesses. *Done; see the section above. Per-task policy is still open, and no longer for want of tasks — see below.*
5. Container packaging (level 3), with the level-2 restrictions still applied inside it.
6. VSCode extension last, once the core is stable — it reuses the protocol from step 3.

## Naming

- Project name: **Loude** (echoes "Claude", free on npm).
- CLI command alias: **`luu`** — shorter and nicer to type daily.
- In Rust: define two `[[bin]]` entries in `Cargo.toml` pointing to the same `main.rs`, or a symlink/alias in the install script (`loude` ↔ `luu`).

## Open questions / next steps

- **What a plan's `paths` and `commands` should grant.** The approved plan is meant
  to *be* the `SandboxPolicy` for its task, and it declares both — but nothing
  narrows the sandbox to them yet, so the policy file is still the standing
  approval for the whole session. The obstacle is concrete: `Sandbox::new` needs
  every granted path to exist, because Landlock takes a descriptor per root, and
  the most ordinary claim a plan makes is that it will create a file. Narrowing to
  the plan either denies the work the plan describes or silently widens to the
  nearest existing ancestor — a grant on `src/` wearing the label of a grant on one
  file. Neither is shippable; choosing between them needs its own record.
- Design the concrete GBNF grammar to force valid tool calls with the target model
  (Qwen2.5-Coder), replacing the text parse.
- `openat2(RESOLVE_BENEATH)` for the in-process tools, closing the TOCTOU window
  that canonicalize-then-open leaves.
- What reopening a closed task does to a prompt already built on its summary.
  Closing is an event and the fold is what the event does; reopening is a different
  fold, and inventing it before anything has been closed in anger is guessing.
- Whether the judge (below) earns its place: it needs about thirty closed tasks in
  shadow mode before its accuracy against the user's own verdict means anything.
- The protocol enums are at v2, which carries the task lifecycle. What is still
  unfrozen is `task_reopened`, for the reason above.
