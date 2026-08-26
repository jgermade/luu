# Loude (CLI alias: `luu`)

Local AI agent written in Rust that orchestrates calls to models, optimized for local inference (limited context, tight token budget).

This file is the design as it stands now, and is rewritten as decisions change. The dated
reasoning behind them — including the answers that were wrong first — lives in [`RECORD/`](RECORD/),
which is append-only. See [AGENTS.md](AGENTS.md).

## Goals

- **Optimize the context window** for local models (7B–32B, 8K–32K context) — the key differentiator vs. existing solutions that don't do this well.
- **Constrain resource access**: configurable folders and commands the agent can reach (application-level sandbox).
- **Container mode**: option to run inside a container to isolate system access.
- **CLI mode**: direct terminal usage.
- **VSCode integration**: a chat tab similar to Copilot Chat.

## Operating modes

- **plan**: the model only produces a plan (list of steps), without executing tools. Structured output (JSON schema) validated with `serde`.
- **run**: executes an already-approved plan, tool call by tool call.
- **auto**: plan → run in a loop, with an iteration limit and re-planning if a tool result diverges from what was expected.

Modeled in Rust as a state enum + a `Mode` trait with `next_action(&self, ctx: &Context) -> Action`.

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
                  │  modes, context,   │
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
- **Every count carries its counter.** A stored turn records which counter produced
  its tokens, and a budget names the counter that produced it. Two runs measured
  differently are not comparable, and nothing else would say so.
- **The turn is the unit.** Eviction drops whole turns, oldest first, so the retained
  window can only ever start on a user message. Half a turn leaves an answer to a
  question nobody asked; a window starting on an assistant message makes several chat
  templates continue instead of answering.
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

- **Hierarchical compaction**: rolling summary of older turns (generated by the local model itself in the background, without blocking the main loop) + full text only for the last 1–2 turns. Not built: it is a strategy, and a strategy has to beat a recorded baseline.
- **Relevance over recency**: index the repo (local embeddings or grep/AST via `tree-sitter`) and inject only fragments relevant to the current turn, instead of sending the full history.
- **Active pruning of tool results**: summarize or drop old tool outputs (e.g. a `cat` of 2000 lines shouldn't stick around in context turns later). Waits on tools existing at all.

## Tool calling: how actions actually get executed

The model never executes anything directly — it only emits a structured request that the program interprets and executes.

- **Native function calling**: define tools via JSON Schema; the model emits a `tool_call` with name + arguments. With llama.cpp, use **GBNF grammars / constrained decoding** to force valid JSON against the schema — key for reliability with small models.
- **Agent loop**:
  1. Prompt + history + tools → model
  2. If `tool_call`: parse → validate against `SandboxPolicy` → execute (real Rust code) → append result to history → back to 1
  3. If plain text: end of turn
- **File editing via diff/patch** (not full rewrites): a tool like `edit_file(path, old_string, new_string)` that finds an exact, unique `old_string` and replaces it. Saves output tokens (the slow part of local generation) and reduces errors.
- **Streaming**: you can stream the preceding reasoning text, but you need to buffer until the full `tool_call` block is closed before parsing and executing.

## Sandbox / security

- **Declarative config (TOML)** per project/session: `allowed_paths`, `denied_paths`, `allowed_commands` (whitelist/regex), `network: bool`.
- Each `Tool` receives an injected `SandboxPolicy` and self-validates before executing — canonicalize paths (`std::fs::canonicalize`) before comparing, to prevent symlink bypass.
- Permission validation lives in the program's code, not in the model "behaving well."

## Container mode

- Compile to a static binary (`musl`) → minimal image (`scratch`/distroless).
- Bind-mount only allowed folders, network disabled by default (`--network none`).
- If the inference backend runs on the host (to use GPU/Metal), the container isolates only tool execution (fs, bash) and talks to the backend over an explicitly exposed socket.
- First version: rootless Podman or Docker with `--cap-drop=ALL` + non-root user.

## VSCode integration

- Use the **VSCode Chat API** (`vscode.chat.createChatParticipant`) — requires a lightweight TypeScript extension.
- The TS extension acts as a bridge: renders the UI (messages, plan approval) and talks to the Rust binary (`agent-core --serve`) over stdio or a socket, using JSON-lines or JSON-RPC (reusable for the CLI too).
- The `plan` mode maps well to this UX: an editable plan before execution (similar to how Copilot shows changes before applying them); `run` as execution with streaming of which tool is being used.

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
| client → server | `prompt`, `approve_plan`, `edit_plan`, `cancel`, `set_mode` |
| server → client | `token`, `plan`, `tool_call`, `tool_result`, `context_snapshot`, `usage`, `error`, `turn_end` |

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
2. **Raw prompt viewer with a diff against the previous turn** — shows how much of the stable prefix
   survived, i.e. the real prompt-cache / KV-reuse hit rate.
3. **Tool call timeline** — arguments, sandbox verdict (allowed/denied and *which* rule matched),
   duration, and result size before/after pruning.
4. **Compaction log** — when a rolling summary was generated, what it replaced, tokens saved.

### Record and replay

`luu serve --record <file>` dumps the JSON-lines stream to disk, and the UI can load such a file
instead of a live socket. Sessions become replayable offline — useful for bug reports and for
comparing context strategies across runs without re-running inference.

`luu chat --script <file>` runs a file of prompts, one per line, against one shared history. That
is what makes a multi-turn run repeatable: a baseline typed into a browser cannot be re-run, and
two recordings are only comparable when the same task list produced both. The record's header
carries the model, the window and the counter for exactly that reason. Note that the **mock backend
cannot validate a context strategy** — it does not read the context, so every strategy "wins"
against it; baselines need a real model.

## Inference backend

Decide between:
- Talking to Ollama/llama.cpp via its local HTTP API (simpler).
- Binding directly to `llama-cpp-rs` (FFI bindings) for fine-grained control over the KV cache across calls and avoiding the overhead of an intermediate HTTP server — recommended given the performance goal.

## Persistence

Nothing is persisted yet: `--record` is opt-in and per-run, and a session's history
lives in memory for the life of the process.

- Sessions in SQLite (`rusqlite`) with compressed state, to resume long tasks without recomputing context from scratch.
- **Whatever the store holds must be reproducible by folding the record.** The
  JSON-lines stream is the account of what happened, and `api::SessionView` already
  folds it; a store that accumulates state the events cannot regenerate is a second
  truth, which is how the static mirror and the live server start disagreeing.
- A stored turn keeps `code_context` separate from the prompt (per the fusion rule
  above) and its token count together with the counter that produced it. Store the
  fused rendering instead and a resumed session either recomputes everything or sums
  two different units into one bar.

## Suggested work order

1. `agent-core`: base types (`Mode`, `Context`, `Tool`, `SandboxPolicy`) + inference backend (Ollama/llama.cpp). *`Context` and the Ollama/mock backends exist; `Mode`, `Tool` and `SandboxPolicy` do not.*
2. Context manager (the differentiating piece) working in plain CLI, without container or VSCode — to measure and iterate on performance quickly. *History, the budget and whole-turn eviction exist and are measured; compaction and relevance selection are still ahead, and each has to beat a recorded baseline.*
3. Agent protocol + `luu serve` + debug web client — early, because it is the instrument used to
   measure step 2. *Done.*
4. Application-level path/command sandbox.
5. Container packaging.
6. VSCode extension last, once the core is stable — it reuses the protocol from step 3.

## Naming

- Project name: **Loude** (echoes "Claude", free on npm).
- CLI command alias: **`luu`** — shorter and nicer to type daily.
- In Rust: define two `[[bin]]` entries in `Cargo.toml` pointing to the same `main.rs`, or a symlink/alias in the install script (`loude` ↔ `luu`).

## Open questions / next steps

- Finalize the remaining base types for `agent-core` (`Mode`, `Tool`, `SandboxPolicy`).
- Design the concrete GBNF grammar to force valid tool calls with the target model (Qwen2.5-Coder).
- Define the initial tool set: `read_file`, `edit_file`, `list_dir`, `run_command`, etc.
- Freeze v1 of the agent protocol message enums (shared by stdio, WebSocket and the record format).
