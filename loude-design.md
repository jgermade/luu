# Loude (CLI alias: `luu`)

Local-first AI agent written in Rust that orchestrates calls to models: optimized for local inference (limited context, tight token budget), and free to reach a remote model when the user names one — never as a fallback. See [`RECORD/2026-09-01.local-first.completed.md`](RECORD/2026-09-01.local-first.completed.md) for what "local-first" obligates and what it costs.

This file is the design as it stands now, and is rewritten as decisions change. The dated
reasoning behind them — including the answers that were wrong first — lives in [`RECORD/`](RECORD/),
which is append-only. What is *planned and not yet true* lives in [`ROADMAP/`](ROADMAP/), one
directory per revision; nothing there is an answer to what this file answers. See [AGENTS.md](AGENTS.md).

## Goals

- **Optimize the context window** for local models (7B–32B, 8K–32K context) — the key differentiator vs. existing solutions that don't do this well.
- **Constrain resource access**: configurable folders and commands the agent can reach, checked in our own code *and* held by the kernel (Landlock + seccomp) for anything that runs as a subprocess.
- **Container mode**: option to run inside a container to isolate system access.
- **CLI mode**: direct terminal usage.
- **VSCode integration**: a chat tab similar to Copilot Chat.

## Jobs (and Model Tasks)

A session is a sequence of **jobs** (historically called tasks), not a mode the agent is in. To avoid confusion with model checklist items, what was previously called a task is now a **job** (the bounded operational container: objective, gated approval, sandbox boundary, and compaction fold), while **tasks** designates the checklist items / steps emitted by models inside a plan (`plan.tasks`). One job:

```
the user asks for something
  → the agent proposes a plan: tasks (checklist), files it will touch, commands it will run
  → CONFIRMATION                  ← nothing runs before this
  → loop: act · check · ask when unsure
  → closing question: "shall I call this done?"
  → close: the job is summarised; its turns stop being sent verbatim
```

Every job is confirmed before anything runs, so there is no autonomy setting to
remember and no mode that can be left open. `plan`/`run`/`auto` used to live here;
they were global where approval is per piece of work, and — the reason they went —
a mode cannot tell the context manager what to compact. See
[`RECORD/2026-08-27.tasks-instead-of-modes.completed.md`](RECORD/2026-08-27.tasks-instead-of-modes.completed.md)
and [`RECORD/2026-09-04.from-tasks-to-jobs.completed.md`](RECORD/2026-09-04.from-tasks-to-jobs.completed.md).

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

**What exists**: the type, the lifecycle and the fold. A turn carries the task it
was asked inside; closing one is an *event* — its turns stay in the history and
stop being rendered, replaced by a deterministic summary of the approved plan,
what the tool results reported, and the fragments the task's turns were shown —
quoted from the file's own bytes, newest first, under a per-summary token cap,
with no model prose in it. The quote is there because the grounded probe measured
its absence losing answers: see
[`RECORD/2026-08-30.the-fold-probe-run.completed.md`](RECORD/2026-08-30.the-fold-probe-run.completed.md)
for the measurement and
[`RECORD/2026-08-30.what-a-summary-should-carry.completed.md`](RECORD/2026-08-30.what-a-summary-should-carry.completed.md)
for why verbatim rather than a digest. Reopening is therefore
not an undo: the fold stops applying. Eviction runs over items rather than turns,
so a folded task is kept or dropped whole and the two ways history gives way
compose. The budget gained a `summaries` bucket, because a fold nobody can see in
the panel is a claim rather than a measurement.

**How a task is approved**, in the two places there is something to approve:

- **In `luu serve`**, by a person. A prompt arriving with no task open buys one
  planning call — the agent is asked what it is about to do, and answers with a
  fenced ```` ```plan ```` block, parsed the same way a tool call is. The prompt is
  then **held on the server, unrun**, until someone approves or refuses it: no
  turn, no tool, no model call happens behind the gate. A model that answers in
  prose instead does not cost the gate — the proposal becomes the ask itself,
  declaring nothing, and the panel says so.
- **In a script**, which is where a repeatable run comes from. `## task:` names
  the objective, `## step:` / `## file:` / `## command:` give the plan, `## close`
  closes it. The written plan *is* the approval.

Either way approving runs a real check: every file the plan names must be
reachable in the sandbox and every command allowed by it. **And the approved plan
then becomes the sandbox for its task**: every turn inside it is checked against
what the plan named, not against everything `luu.toml` grants, so the task
boundary is the scope permission is granted at rather than a sentence saying it
is. The policy file is the outer bound and the plan the inner one — a plan
cannot grant what the file does not, and a plan that names nothing grants
nothing. A denial says which of the two refused.

Narrowing cuts the level as well as the extent. A plan has two path lists:
`files` is what the task may **read** and `writes` what it may also **change**,
so a file declared only as read is read-only inside the task even where the
policy grants both — and a plan that says it will change a file under a
read-only root is refused *at the gate* rather than four turns in, which is the
one case the check exists for and used to miss. A write to a path that does not
exist yet grants the directory that will hold it, because creating a file is a
write to its directory and at the kernel rung a grant is a directory anyway.
Approving carries an amendment — the reads, writes and commands the person adds
at the gate, checked against the policy file exactly as the model's plan was —
which is what stops an under-specified plan from being a dead run. It carries one
more thing the plan never had: `closes_on`, below, checked against the plan as it
will *be* rather than against the amendment alone, since the command it names is
usually one the model already declared.
See [`RECORD/2026-08-30.the-gate.completed.md`](RECORD/2026-08-30.the-gate.completed.md) and
[`RECORD/2026-08-30.narrowing.completed.md`](RECORD/2026-08-30.narrowing.completed.md).

A refusal is kept, not erased: `rejected` is a state a task stays in, with the
plan that was turned down. Nothing in a session is deleted — a closed task is
folded, a reopened one unfolded, a refused one recorded. The lifecycle is a
state machine and the messages that drive it come off a socket, so every
transition is guarded: only a proposal is approved or rejected, only an open
task closes, only a closed one reopens. A refused plan that could be *reopened*
would be a plan a person turned down becoming the live task, with the gate
behind it.

**A proposal says who wrote it.** `task_proposed` carries `source` — `model` when
the planning call emitted a parseable plan block, `prose` when it answered
without one, `written` for a script's directives. The gate's headline number is
how often a small model plans at all, and emptiness cannot answer it: a model
that ignored the format and one that declared empty lists arrive as the same
empty plan, and the two want different fixes (a grammar, or a sentence). The read
side keeps the plan as proposed beside the plan as approved, because the
difference between them is what a person had to add — the cost of the gate. How
to measure it is [`RECORD/2026-08-31.the-gate-probe.completed.md`](RECORD/2026-08-31.the-gate-probe.completed.md).

**And the server says no out loud.** A `refused` message carries the request it
answers, a reason (`busy`, `pending`, `task`, `not_granted`) and a sentence for
a person. Before it, a client could not tell a refusal from a message that never
arrived, and the debug UI had to guess by disabling its own composer — which is
not a permission model and not an interface either.

**Who declares a task done** is a ladder, not a single answer: deterministic checks
(exit codes, tests) first, then a judge, then the user's final question, which is the
only authority. The judge is the same model given a short context of **evidence, not
narrative** — the objective as typed, the diff, the commands and their exit codes —
because a judge fed the model's own account of its work inherits its hallucinations.
It runs in shadow mode until measured: every task the user closes is a label, so the
accuracy figure arrives for free and gates nothing until it exists.

**The exit-code rung exists.** A plan carries `closes_on`, one command line, and
the task folds itself the moment a `run_command` step of its own runs exactly
that line and comes back `exit_code: 0` — matched whole, so `cargo test` is not
`cargo test --no-run`, and on the code rather than on the absence of an error, so
a child killed by `SIGXCPU` is the limit reporting an unfinished command rather
than a success. A plan without one closes only when a person says so, which is
every plan a model writes.

**The model is never asked for `closes_on`; the person at the gate types it.**
`PLANNING` keeps its five keys, so the cached prefix does not move and every run
recorded before it stays comparable. The reason is not economy: *what would
convince me this is finished* is the judgement the gate exists for, and a 7B
asked for its own success criterion restates the plan it was about to run. It is
the first thing the gate can add that the model was never asked to propose.

**Every close says which authority folded it.** `task_closed` carries `by` —
`user` or `exit_code` — because a ladder is only worth climbing if each rung can
be counted: how often the exit code closed a task a person would have left open,
and how often a person closed one it missed. Absent means `user`, which is what
every close before the field existed was. See
[`RECORD/2026-09-02.closing-on-an-exit-code.completed.md`](RECORD/2026-09-02.closing-on-an-exit-code.completed.md) —
including the finding that the rung is **unreachable wherever the kernel cannot
hold a child**, macOS included, because it sits on top of `run_command`. The
container is what makes it real.

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

The core knows nothing about CLI, VSCode or the browser — it exposes an internal API (Rust) plus a local server speaking a single JSON message schema over swappable transports. This is also what makes container isolation a packaging question: the container wraps tool execution, not the whole core — see [Container mode](#container-mode) and [`RECORD/2026-09-01.the-container-decided.WIP.md`](RECORD/2026-09-01.the-container-decided.WIP.md).

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
  once the window is full, and the reusable prefix collapses to the system block —
  measured at 3–4% from the first eviction onwards, and it never recovers. Cutting
  down to a low-water mark (`--low-water`, 0.5) instead pays for that once every
  four or five turns and holds still in between: mean reuse over the same 20 turns
  goes from 33% to 67%. It costs 21% of the history, which is not free and is not
  yet known to be harmless. Numbers in
  [`RECORD/2026-08-27.prefix-reuse-and-block-eviction.completed.md`](RECORD/2026-08-27.prefix-reuse-and-block-eviction.completed.md).
- **What leaves the window stays out.** Eviction is monotone: `Context` keeps a floor
  that only moves forward. Recomputing the retained window from scratch each turn
  lets a dropped turn return the moment a shorter prompt leaves room, which moves the
  front of the history and is the one thing a prefix cache cannot survive — and it
  makes block eviction impossible, because the next fill walks straight back past the
  cut.
- **And it says so: a cut is a message.** The selection that moves the floor emits
  `evicted` — the turn that cut, the turns that left, what they were worth, the
  counter that counted them and which policy did it. Without it a recording shows the
  history bucket shrinking and cannot say whether that was the policy or the
  arithmetic, which is how a set of degenerate fixtures survived a whole commit. The
  turns are named rather than counted, because a reader months later cannot recover
  *which* without re-implementing the floor. See
  [`RECORD/2026-08-31.eviction-tombstones.completed.md`](RECORD/2026-08-31.eviction-tombstones.completed.md).
- **An unknown window is not an unlimited one**: `--context-limit 0` means unknown, so
  nothing is budgeted and nothing is evicted, and the panel says so rather than
  drawing a bar against nothing.
- **Stable prompt cache / prefix**: keep the fixed part (system + tool definitions)
  byte-identical across calls to take advantage of llama.cpp's prompt caching / KV
  cache reuse.

### The overhead we cannot see

Talking to Ollama's `/api/chat` — or to any OpenAI-compatible `/chat/completions`
— means the chat template is applied on its side: the `<|im_start|>` markers and
per-message separators are tokens we never count. Owning that number would mean
owning the prompt string (`/api/generate` with `raw: true`, or the completions
endpoint rather than the chat one), which is deliberately not where this sits
today.

So the gap is **accepted and reported**: the trace carries our count per bucket,
measured before the call, and the backend's own `usage.prompt_tokens` afterwards, and
the panel shows the difference as its own labelled quantity. A stable difference is
template overhead; a moving one means the template changed, which is worth more than
the precision given up.

For the difference to mean that, the two sides have to count the same calls. A
turn that uses a tool is several model calls — the first ends in a tool block,
the next reads the result — and `usage.prompt_tokens` is summed over all of them.
So the agent loop announces every call it makes before making it, and the trace
measures the ones after the first into the same chain as the turns. Measure only
the first and a 2-token template gap reads as 1 962, with nothing failing and
nothing saying so; see [`RECORD/2026-08-27.the-m4-pro-run.completed.md`](RECORD/2026-08-27.the-m4-pro-run.completed.md).

### Still ahead

- **Hierarchical compaction**: built, and measured once. A closed task is replaced by its deterministic summary — the approved plan plus the evidence: paths, commands and exit codes — and full text is kept only for the live one. The token threshold stays as the fallback for a task that overflows alone. Against the recorded baseline (twenty prompts, 1024-token window, mock backend): mean prefix reuse 69.9% under per-turn eviction and 89.9% under block eviction becomes **91.3%** with four task boundaries, and the prompts sent shrink from 16 715 to **13 417 tokens** — with eviction never firing at all, because the folded history never grew enough to need it. What the fold *loses* is the conversation itself — the turns' own prose — and, until it quoted them, the files the turns were shown: that second loss was measured against a real model (two of three closed-task questions came back wrong, one of them a refusal) and closed by quoting the fragments verbatim into the summary. On the mock pair that quote costs 16 927 → 24 356 tokens over the twenty prompts, against 30 682 unfolded, and leaves reuse where it was (89.6% → 89.3%). On the real model (`qwen2.5-coder:7b`, sampling pinned) both closed-task questions that broke before the fix now answer correctly, matching the unfolded run almost word for word, and the fold still wins on tokens but by less than estimated: 29 135 tokens against 36 288 unfolded, a 19.7% win rather than the ~50% win before the fix paid for the quote — see [`RECORD/2026-08-30.the-fold-fix-verified.completed.md`](RECORD/2026-08-30.the-fold-fix-verified.completed.md). Model prose in the summary stays rejected — it would enter the write-once region every later turn is built on, which is exactly why the quote is the file's bytes and not a digest of them.
- **Grounding a turn with a real file**: built. `--fragment PATH[:START-END]`, and `## fragment:` in a script, read a file **through the sandbox** and fuse it into one turn's user message — the `code` bucket, which existed from the start and until now was always zero. A path the sandbox would refuse to `read_file` is refused here too, and a denial is an error rather than a warning: a run that quietly dropped its grounding answers out of the model's training and looks like it worked, which is exactly what a real 7B did to twenty ungrounded turns. It is attached to one turn and then gone, because which turns a file belongs in is the next item's question and attaching it to all of them answers it wrongly, at every turn's expense. `scripts/tasks/grounded{,-tasks}.txt` are the pair this makes possible: the same twenty prompts, one grouped into tasks, with a last group that attaches nothing and asks the first fifteen again — the first corpus in which *does the fold lose what the task needed* is a question with an answer. The protocol for asking it is [`RECORD/2026-08-27.grounded-fold-probe.completed.md`](RECORD/2026-08-27.grounded-fold-probe.completed.md).
- **The repository map**: built, and ranked **behind a flag that is off**. Every `.rs` file's definitions with their signatures, bodies elided, from `tree-sitter`'s own `TAGS_QUERY`, in the cached prefix under the tool definitions — where blocks are ordered by how often they are rewritten, and the map changes only when the repository does. `--map-tokens N`, off by default so that every recording made before it stays comparable, and `luu map` prints the exact bytes. Files are outlined in path order until one does not fit; the map then says how many it left out. **`--map-rank` orders them by what the rest of the tree depends on instead** — a reference graph over the same query's `@reference.*` captures, PageRank with a *uniform* teleport, which is what keeps the map a prefix: Aider seeds its ranking with the files in the conversation, and that is the half that would rewrite the block every turn. It is off because it was measured and lost, twice, on two differently-built corpora: at 1024 tokens path order holds five files and the ranking holds two, because rank order puts the big central files first and the fill rule stops at the first file that does not fit — and on a corpus of 38 questions chosen one per file before either order was checked (so the loss cannot be the corpus favouring path order's own holdings), path order named the right file for **100%** of the questions it covered against rank order's **12.5%**, and rank order's denser, more self-referential files pushed a 7B into paragraphs of fabricated Rust that evicted the session's own history 24 times in 38 turns, where path order evicted nothing. `luu map --explain` prints the ranking under either order — each file's score and who references it — so the order the map did not take can be read beside the one it did. To address the oversized file trap and leaf-sink penalty, **`--map-in-degree`** weights inbound references by caller diversity rather than random-walk damping, and **`--map-non-greedy`** skips over files that exceed the remaining token budget to continue packing smaller ones (packing 5 files and 1026/1024 tokens at 1024 tokens, vs 2 files and 664 tokens under greedy fill). This repository is still the argument for doing this properly: its whole outline is **6 327 tokens, 77% of an 8K window**, so at any affordable budget most of it is missing and the alphabet chose which part. What it costs on the grounded script: 870 tokens a turn at `--map-tokens 1024`, +56% on the run's total prompt tokens — and a prefix-reuse number that rises from 93.9% to 96.3% for purely arithmetic reasons, which is why a map-on run is not comparable on reuse to a map-off one. See [`RECORD/2026-08-31.the-repo-map.completed.md`](RECORD/2026-08-31.the-repo-map.completed.md), [`RECORD/2026-09-02.ranking-the-map.completed.md`](RECORD/2026-09-02.ranking-the-map.completed.md), [`RECORD/2026-09-03.the-map-order-probe.completed.md`](RECORD/2026-09-03.the-map-order-probe.completed.md) and [`RECORD/2026-09-04.in-degree-and-fill.completed.md`](RECORD/2026-09-04.in-degree-and-fill.completed.md).
- **Relevance over recency**: inject only the fragments the current turn points at, instead of the full history. **The mechanism is `tree-sitter` tags plus a reference graph, not embeddings** — a graph can say *why* a file was included, staleness is `mtime`, and there is no second copy of the user's code to ship or govern. Decided against Aider's implementation; see [`RECORD/2026-08-27.aider-repo-map.completed.md`](RECORD/2026-08-27.aider-repo-map.completed.md). The graph now exists and ranks the map's files; what it does **not** yet do is choose fragments, which is this item.
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

**The outcome is structured and the rendering is not.** A `run_command` outcome
carries `exit_code`, `signal`, `stdout`, `stderr` and `duration_ms` as *fields*,
on the protocol and in the record; `ToolOutcome::render` still produces the same
short plain text it always did, because a 7B pays for every token of a wrapper it
does not read. The distinction is load-bearing rather than tidy: the exit code
used to live for one moment inside the string `"{program} exited with {code}"`,
and **a task cannot be closed on an exit code that was never a field** — the
ladder above the user (exit codes and tests, then a judge in shadow mode) was
blocked on this and on nothing else. The two streams are separate for the same
reason: a judge that has to re-split one blob on `--- stdout` is parsing a
rendering. `signal` is how a run says *which* limit stopped a child — `SIGXCPU`
and `SIGXFSZ` are `[sandbox.limits]` arriving, and nothing else in the outcome
could tell them from a crash. Absent for every in-process tool, where there is no
exit code and a zero would be a lie; additive on the wire, so the record format
did not move. See
[`RECORD/2026-09-01.what-the-audit-left.completed.md`](RECORD/2026-09-01.what-the-audit-left.completed.md).

## Sandbox / security

Three rungs, and the middle one is what makes the first worth having. Built: 1
and 2, and 3 in its development posture. See
[`RECORD/2026-08-27.tools-and-sandbox.completed.md`](RECORD/2026-08-27.tools-and-sandbox.completed.md).

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
   Landlock survives `exec` and cannot be dropped. On a Mac it is not a
   duplicate boundary, it is **the only one there is** — which is why it comes
   before the measurements that need it rather than after them.

**Declarative config (TOML)** per project — `luu.toml`, and `luu tools` prints the
resolved result:

```toml
[sandbox]
enforcement = "kernel"          # or "best-effort"
network = false
commands = ["cargo", "git"]     # program names, never a shell string

[sandbox.limits]                # what a child may spend, not what it may reach
cpu-seconds = 300               # RLIMIT_CPU, per process
file-size-mb = 1024             # RLIMIT_FSIZE, per file
# memory-mb = 4096              # RLIMIT_AS — off by default, see below
# processes = 512               # RLIMIT_NPROC — off by default, see below

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
- **The paths say what a child may reach and `limits` says what it may spend.**
  Nothing said the second thing until the limits existed, and the gap was not
  academic: a fork bomb, a disk bomb and a memory bomb all ran to completion
  inside `run_command`'s 30-second clock, whose only other companion was an 8 KiB
  cap on what got *reported* about them. `setrlimit` is POSIX rather than Linux,
  so the limits sit **above** the `linux.rs`/`fallback.rs` split and are the
  first rung that holds a child on macOS at all — where `how` reads `rlimits (…)`
  with the same `missing` as before, instead of "in-process check only".
  `cpu-seconds` and `file-size-mb` are on by default; `memory-mb` (`RLIMIT_AS`)
  is off because a Rust toolchain reserves address space far above what it
  commits, and `processes` (`RLIMIT_NPROC`) is off because the kernel counts it
  **per real uid, not per process tree** — a default there would deny `fork`
  over processes this agent never started, and the right mechanism for it is a
  pids cgroup with the container. A limit is applied soft and hard together — a
  child that can raise its own soft limit back is not limited — with one
  measured exception: `RLIMIT_CPU` is two-stage by design (soft sends `SIGXCPU`,
  hard sends `SIGKILL`), so equal limits collapse into a bare signal 9 that
  cannot be told from a crash. It gets one second of hard grace, which buys the
  signal that names the limit. See
  [`RECORD/2026-09-01.what-the-audit-left.completed.md`](RECORD/2026-09-01.what-the-audit-left.completed.md).

### Who enforced it is reported, never assumed

Level 2 is Linux-only, so `enforcement` decides what happens where it is not
available — the one place in this design where a security property is a setting:

- `"kernel"` (default) — a subprocess runs only if the kernel took the ruleset and
  the filter. On macOS, or a kernel without Landlock, `run_command` is **denied**,
  and the denial names what is missing and the flag that lowers the bar.
- `"best-effort"` — apply what this kernel has and report the gap.

Either way every verdict carries `Applied` — `Process`, `Kernel { how }`, or
`Partial { how, missing }` — and `how` names every mechanism holding the child,
the rlimits and their numbers included, *and its version*,
because Landlock's older ABIs mediate less. Nothing here may say "sandboxed"
without saying by what: a run whose subprocesses the kernel held and a run whose
subprocesses nothing held are not the same run, and afterwards the recording is
the only thing that could tell them apart.

What level 2 does *not* claim: it is not a network namespace (blocking the
internet address families stops a program opening a connection, not one that
inherited a socket), and canonicalize-then-open still has a TOCTOU window for the
in-process tools — `openat2(RESOLVE_BENEATH)` is the answer there and is not built.

## Container mode

Level 3. **The development posture is built**: `loude-worker` in a long-lived
container, wide open, with the narrowing kept as a separate item whose trigger is
a fact rather than a date. Level 2 stays applied inside it — inside a Linux
container Landlock works, is free, and is the one part of the sandbox the whole
exercise exists to reach, so the loosening widens `commands` and `network` and
leaves `enforcement` alone. See
[`RECORD/2026-09-01.the-container-decided.WIP.md`](RECORD/2026-09-01.the-container-decided.WIP.md)
for the decision and
[`RECORD/2026-09-02.the-worker-and-the-seam.completed.md`](RECORD/2026-09-02.the-worker-and-the-seam.completed.md)
for where it cuts in the code.

**The seam is one function.** The tool loop touches the tool set in exactly one
place, so the container arrives as an `Executor` behind it: `Tools` runs a call
here, `Worker` writes it down a pipe. The tool *definitions* stay on the host,
because they are the second half of the cached prefix and a prefix assembled
inside the image is one that moves every time the image is rebuilt. If adding
the container had had to touch the loop, the loop was wrong.

**The container's only process is the worker.** `<runtime> run --rm -i … luu
worker`, spoken to over stdio — the same transport the VSCode extension uses,
pointed the other way. So the container's lifetime *is* the worker's: no name to
allocate, no `docker rm` to forget, and no way to leave one running after the
session that owned it died. One per session, not one per command, because a
container per command is a start — on some runtimes a VM boot — per command.

**What crosses the pipe is the policy, not the sandbox.** A resolved `Sandbox` is
canonical paths on a filesystem the worker does not have; a `SandboxPolicy` is
portable, and the far side runs the same `Sandbox::new` the host would.
`Sandbox::to_policy` is the inverse, and a test asserts the round trip grants
what the original granted. The task's `Authority` crosses too, so a denial from
inside still says *the approved plan for task 7* rather than *the sandbox
policy*.

**The base is mounted at its own absolute path**, not `/workspace`. Paths appear
in verdicts, prompts, tool results and the record; translating them would make a
contained run and a host run of the same script differ in their bytes, and "one
flag apart" is what every probe in `scripts/tasks/` depends on. `[[worker.paths]]`
is for the other direction — trees that exist only inside the image, added to the
policy the worker resolves and never resolved on the host, because a granted path
that is not there is a load error and `/usr/local/cargo` is not a directory on a
Mac.

```toml
[worker]
runtime = "docker"          # host | direct | docker | podman | nerdctl | container
image = "loude-worker:dev"

[[worker.paths]]            # the image's toolchain, resolved only on its side
path = "/usr/local/cargo"
access = "execute"
```

- **A runtime is a name, not an integration.** This layer builds an argv, which
  is the same shape `commands` already has, so Docker, Podman, `nerdctl` and
  Apple's `container` substitute for each other and the dependency becomes a
  choice. Where the flags are *not* uniform it says so rather than assuming:
  `--user uid:gid` against `--uid`/`--gid`, and Apple's runtime having no
  `--network none` at all — under which the container stays attached, the
  per-command seccomp filter is doing all the denying, and the verdict reports
  it.
- **`runtime = "host"` is the default** and is every run this repository has
  measured. `direct` runs the worker as a plain child with no container: it
  isolates nothing, says so in every line that reports it, and exists so the seam
  is testable where no runtime is installed and so a failure can be attributed to
  the IPC or to the container in one flag.
- **The image is declared, not generated** — `Containerfile` in the tree, and
  `luu.container.toml` is the wide-open posture, a separate file so that in three
  weeks nobody has to tell a default from a leftover. `scratch`/distroless is
  wrong here: the container's whole job is running `cargo`, `rustc`, `git`, `rg`
  and `ls`, and a minimal image has none of them.
- **`commands = [...]` is the image's manifest.** The worker is the only process
  standing on the image's `PATH`, so its handshake answers which allowed commands
  the image actually has, and `luu tools` prints the gap under `absent` —
  "granted by the policy, absent from the image", a third failure mode distinct
  from *denied by the policy* and *the kernel will not hold it*. The handshake
  also carries the protocol number and the worker's own `luu` version, because an
  image is the easiest thing in this design to leave stale.
- **The container isolates tool execution only**; the context manager and the
  model client stay on the host. The GPU is the obvious reason and not the strong
  one: the topology does not justify `network = false`, it is what makes it
  *affordable*. A model call born inside the container could not be made with the
  container's network off, and the build-script protection would be lost at the
  moment isolation was raised. Under local-first this weighs more, not less.
- **The network is decided once, at creation, and never changed.** `--network
  none` when the session's policy denies it, attached when it allows it. Nothing
  toggles a running container's network: the filter is built at every spawn, so a
  *task's* grant is scoped to exactly the task that declared it, with no startup
  cost and no window that outlives it. Connecting and disconnecting by hand would
  move what the kernel already decides per call.
- **Egress through a host-side proxy (`EgressProxy`)** filters outbound destinations when
  `network: true`. A plan carries `egress: ["crates.io", "*.github.com"]` (or script directive
  `## egress: ...`), and an in-process HTTP CONNECT proxy inspects target hostnames, tunneling
  matching traffic via `copy_bidirectional` and returning 403 Forbidden for unapproved destinations.
  Standard proxy environment variables (`HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` and lowercase)
  are injected into `run_command` subprocesses. See
  [`RECORD/2026-09-04.egress-through-the-host.completed.md`](RECORD/2026-09-04.egress-through-the-host.completed.md).
- Still ahead: `--cap-drop=ALL`, a pids cgroup in place of `RLIMIT_NPROC`.

## VSCode integration

- Use the **VSCode Chat API** (`vscode.chat.createChatParticipant`) — requires a lightweight TypeScript extension.
- The TS extension acts as a bridge: renders the UI (messages, plan approval) and talks to a **spawned `luu stdio` subprocess** — one process per window, no port, no bind, no token, speaking line-oriented NDJSON with errors on stderr. Not a socket: the moment there is one, `approve_task` is reachable on it, and signed approvals become a precondition rather than a later step. The socket transport stays what `serve` already offers and becomes interesting for the editor only once multi-session exists. See [`RECORD/2026-09-01.how-a-surface-reaches-the-engine.completed.md`](RECORD/2026-09-01.how-a-surface-reaches-the-engine.completed.md), [`RECORD/2026-09-04.protocol-over-stdio.completed.md`](RECORD/2026-09-04.protocol-over-stdio.completed.md), and [`RECORD/2026-09-04.vscode-extension.completed.md`](RECORD/2026-09-04.vscode-extension.completed.md). Surface #3 lives in `editors/vscode/`.
- The task's confirmation step maps well to this UX: an editable plan before execution (similar to how Copilot shows changes before applying them), then execution with streaming of which tool is being used, and a closed task collapsing to its summary in the thread.

## Debug web client (agent protocol)

A local web UI (chat, session browser, context inspector) is the fastest way to see what the
context manager is actually doing — the CLI can't show a token budget or a prompt diff.

**v1 of the message enums is frozen.** `ClientMessage` is `prompt`, `cancel`,
`approve_task`, `reject_task`, `close_task`, `reopen_task`; `ServerMessage` is
the eleven the server emits, the task lifecycle included. It was frozen only once
every one of them had been watched being sent and answered rather than imagined
— the record format moved to 3 with it, and a change an older reader could not
make sense of bumps both from here.

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

`luu serve --bind 127.0.0.1:7878` — loopback by default and unauthenticated; **any other address
requires a bearer token, and without one the bind is refused rather than warned about.** The check
runs before the listener exists, so an unauthenticated non-loopback server is not a state the
program can be in. The token comes from `--auth-token-file <PATH>`, whose mode is checked (a flag
is greppable in `ps`, an env var is inherited by every child `run_command` spawns). It gates `/ws`,
which carries task approval, and `/api/*`, which carries this session's prompts and source; it does
not gate the embedded page, which is the same bytes in every copy of a public binary and which a
browser cannot request with a header. `Authorization: Bearer <token>` everywhere, plus `?token=` on
`/ws` alone, because the browser's `WebSocket` constructor cannot set a header. See
[`RECORD/2026-09-01.what-the-audit-left.completed.md`](RECORD/2026-09-01.what-the-audit-left.completed.md).

The UI is embedded in the binary with `rust-embed`, so there is one command, one URL, and no node
process in the loop.

Live channel — `WS /ws`:

| Direction | Messages |
| --- | --- |
| client → server | `prompt`, `approve_job`, `reject_job`, `close_job`, `reopen_job`, `cancel` (with `*_task` aliases) |
| server → client | `hello`, `turn_started`, `token`, `tool_call`, `tool_result`, `ended`, `failed`, `job_proposed`, `job_approved`, `job_rejected`, `job_closed`, `job_reopened`, `refused`, `evicted` — all built, protocol v4 (record format 6); `context_snapshot` is still ahead |

Closing a job is an event, not a mutation: reopening one is folding the log
differently, never undoing a deletion. Freezing v1 of these enums waits on the job
lifecycle for that reason — it is the last cheap moment to add it.

Read side — plain GETs, browsable and curl-able. Every path answers for the live
session, under `live` or under its own stored id, **and for any session the store
has** — so a second `serve` pointed at the same database can be asked what the
first one did:

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
   i.e. the prompt-cache / KV-reuse hit rate, as a share of the prompt. Built, and
   measured over *every* call rather than the first of each turn: a turn's tool
   round trips are in the same chain, so the panel says how many calls the turn
   made and what they sent instead of adding them to the template gap. The
   span-level diff of the two prompt strings is not built, and is a separate thing:
   the number says how much was reused, a diff would say what changed. `similar` is
   still right for the second and was the wrong tool for the first, which is a
   prefix.
3. **Tool call timeline** — arguments, sandbox verdict (allowed/denied and *which*
   rule matched), **who enforced it**, duration, and result size. Built. A call is
   listed when it is made and filled in when it returns, so one that is running or
   was denied reads as itself rather than as nothing happening. Result size
   *before/after pruning* waits on pruning existing.
4. **Compaction log** — when a rolling summary was generated, what it replaced, tokens saved.

### Record and replay

`luu serve --record <file>` dumps the JSON-lines stream to disk, and the UI can load such a file
instead of a live socket. Sessions become replayable offline — useful for bug reports and for
comparing context strategies across runs without re-running inference. What the run *forgot* is in
there too: the recorded fixtures make the two policies visible as what they are — ten small cuts
under `turn` against two deep ones under `block`, over the same twenty prompts, and none at all in
the tasks run, where the fold kept the history under the limit and eviction never fired.

`luu chat --script <file>` runs a file of prompts, one per line, against one shared history. That
is what makes a multi-turn run repeatable: a baseline typed into a browser cannot be re-run, and
two recordings are only comparable when the same task list produced both. The record's header
carries the model, the window, the counter and the eviction policy for exactly that reason.
`scripts/tasks/steady-state.txt` is twenty turns of uniform size, long enough to show what a
policy does *after* the first eviction, which is where they stop agreeing. Note that the **mock backend
cannot validate a context strategy** — it does not read the context, so every strategy "wins"
against it; baselines need a real model.

## Inference backend

**Two are built, behind one trait**: `ollama` (`POST /api/chat`, NDJSON) and
`openai` (`POST /chat/completions`, SSE) — the second is not a hosted-API feature,
it is how `llama-server`, vLLM and LM Studio are reached, which is five of the six
machines in [`ROADMAP/2026-09-01/machines.md`](ROADMAP/2026-09-01/machines.md) plus
two hosted endpoints. Binding `llama-cpp-rs` directly, for KV-cache control across
calls without an HTTP server in the way, is still the eventual answer and still
deferred until there is something to measure.

**The window is sent to one of them and cannot be sent to the other**, and the
difference is worth stating where someone will read it before running a
comparison. Ollama takes `options.num_ctx` and truncates silently without it —
the rule AGENTS.md prints in bold. The OpenAI chat-completions API has **no field
for the window at all**: `max_tokens` caps the output, and on `llama-server`,
vLLM and LM Studio the window is what the server was *started* with. So
`--context-limit` is budgeted against and not sent, the CLI says so once before
the run, and the check moves to the response — `usage.prompt_tokens` is already
compared against our own count per turn, and a server serving a smaller window
than we budgeted shows up there.

Which is why **`Chunk::Done` carries `Option<Usage>`**: these servers report no
usage at all unless `stream_options.include_usage` is on the request, and some
report none even then. `None` is *not reported*; zero would claim the server saw
an empty prompt, in exactly the number the budget panel plots against ours. See
[`RECORD/2026-09-01.an-openai-compatible-backend.completed.md`](RECORD/2026-09-01.an-openai-compatible-backend.completed.md).

## Persistence

**Sessions are stored**, as SQLite (`rusqlite`, with SQLite compiled in so the
store does not depend on the host's system packages). `luu serve` caches its fold
into `~/.loude/sessions.db` by default — `--store <path>` names another,
`--no-store` keeps the session in memory the way every run did before. The store
is deliberately not beside `luu.toml`: the policy file describes *this project*
and is committed with it, and a session store that travelled with a checkout
would put one project's conversation into every clone of it.

**A row is the fold, whole, and nothing else.** `SessionView` serialised into one
column, with the listing columns derived from `SessionView::summary()` beside it
so a listing does not parse every blob. The normalised alternative — a table per
turn, task and tool call — is a *second definition of the fold*, and the first
time DDL and `api.rs` are changed apart the store and the live server start
disagreeing about a session. `GET /api/sessions` lists the live session and the
stored ones; every read path answers for a stored id as well as for `live`.

**Resuming and multi-session switching are supported.** `SessionStore::resume`
reconstructs the write-side `AgentContext` and turn counter from stored turns and
jobs, validated in `store_parity.rs`. In `serve`, `POST /api/sessions` starts a
clean session checkpointing the active one, `POST /api/sessions/:id/resume`
restores a stored session into the live engine, and `DELETE /api/sessions/:id`
removes past sessions. The web UI header displays stored sessions in a switcher
dropdown alongside a `+ New` button. See
[`RECORD/2026-09-04.session-resume.completed.md`](RECORD/2026-09-04.session-resume.completed.md) and
[`RECORD/2026-09-04.multi-session-in-serve.completed.md`](RECORD/2026-09-04.multi-session-in-serve.completed.md).

- **Whatever the store holds must be reproducible by folding the record.** The
  JSON-lines stream is the account of what happened, and `api::SessionView` already
  folds it; a store that accumulates state the events cannot regenerate is a second
  truth, which is how the static mirror and the live server start disagreeing.
- **Forgetting is an event too.** Built for eviction: an `evicted` line names the
  turns that left, what they were worth, who counted them and which policy cut —
  so a recording says what a session forgot instead of only showing the history
  bucket shrink. The transcript keeps those turns and marks them, because a view
  that agreed with the prompt could no longer show the difference between them.
  The shape is OpenHands' condensation tombstones, read in
  [`RECORD/2026-08-27.cline-openhands.completed.md`](RECORD/2026-08-27.cline-openhands.completed.md);
  the reasoning is in
  [`RECORD/2026-08-31.eviction-tombstones.completed.md`](RECORD/2026-08-31.eviction-tombstones.completed.md).
  Compaction's own tombstone already exists as `task_closed`; pruning tool results
  out of a live turn would need a third, and deliberately has none until it does.
- A stored turn keeps `code_context` separate from the prompt (per the fusion rule
  above) and its token count together with the counter that produced it. Store the
  fused rendering instead and a resumed session either recomputes everything or sums
  two different units into one bar.
- **The parity is a test, not a promise.** `fold(record) == load(store, id)`, over
  recordings produced by running the binary rather than hand-written, so a store
  that drifted from the fold is a red test instead of a support question. Writes
  happen at checkpoints — a turn ending, a task changing state — so the store is
  allowed to *lag* the record and never to contradict it.
- Compression is deferred until a real session has been measured and is not tens
  of kilobytes of JSON: `zstd` is a dependency, a format decision, and a thing to
  get wrong.

## Suggested work order

1. `agent-core`: base types (`Task`, `Context`, `Tool`, `SandboxPolicy`) + inference backend (Ollama/llama.cpp). *Done.*
2. Context manager (the differentiating piece) working in plain CLI, without container or VSCode — to measure and iterate on performance quickly. *History, the budget, whole-turn eviction, block eviction, compaction on task boundaries and the repository map exist; eviction is recorded rather than only applied, and prefix reuse is measured per turn. The reference graph and the ranking exist and are **off by default**: measured against the path-order baseline on this tree they hold two files where the alphabet holds five, because rank order leads with the big central files and the fill rule stops at the first that does not fit. So what is left of the step is the same item with a sharper question — ranking and the fill rule are one decision, not two, and the corpus that would settle them has to be built first, because the existing one defines its control group as "the files a path-ordered map holds". See [`RECORD/2026-09-02.ranking-the-map.completed.md`](RECORD/2026-09-02.ranking-the-map.completed.md).*
3. Agent protocol + `luu serve` + debug web client — early, because it is the instrument used to
   measure step 2. *Done.*
4. Path/command sandbox — in-process checks, then the kernel holding subprocesses. *Done; see the section above. What is still open is per-task policy, which waits on tasks.*
5. Container packaging (level 3), with the level-2 restrictions still applied inside it.
   *The development posture is built and verified live — the worker, the seam,
   the runtime layer, the image, and Landlock ABI v8 active inside Docker
   Desktop on macOS ([`RECORD/2026-09-03.the-container-observed.completed.md`](RECORD/2026-09-03.the-container-observed.completed.md)).
   What is left of the step is the narrowing: egress through the host, and
   `network` per plan. It no longer blocks the exit-code rung or the gate probe's
   command prompts on macOS.*
6. VSCode extension last, once the core is stable — it reuses the protocol from step 3.

The six steps are the *shape* of the work and have not changed. What is being
built next, in what order, and what blocks what is a separate question with a
separate answer: [`ROADMAP/`](ROADMAP/), latest revision. Federation — sovereign
hosts, a coordinating portal, sessions moved between machines — is proposed and
not decided; the argument is
[`RECORD/2026-08-31.the-portal-and-the-gate.completed.md`](RECORD/2026-08-31.the-portal-and-the-gate.completed.md).

## Naming

- Project name: **Loude** (echoes "Claude", free on npm).
- CLI command alias: **`luu`** — shorter and nicer to type daily.
- In Rust: define two `[[bin]]` entries in `Cargo.toml` pointing to the same `main.rs`, or a symlink/alias in the install script (`loude` ↔ `luu`).

## Open questions / next steps

- Narrowing `enforcement` with the rest of the plan. `network` and `egress` are
  now per-job and filtered by host-side proxy; enforcement level remains session-wide.
- **A tool call has no timeout at the seam.** `run_command` has its own clock
  inside the worker, and a worker that dies mid-call surfaces as EOF — an error
  rather than a hang. A worker that is alive and stuck is not covered, and the
  honest place for that clock is the seam rather than each tool.
- Whether `writes` should also bound `run_command`: a child can write whatever
  the task's roots allow, and a plan's `commands` list says nothing about paths.
  Narrower than it was — the child is held to the *task's* roots now — but a
  command is still the widest thing a plan can ask for.
- Who else may close a task: **exit codes are in** — a plan's `closes_on`, matched
  whole, on an exit of 0, and every close says which authority folded it. Tests
  as a distinct rung, and then the judge in shadow mode, are still ahead. Nothing
  counts the two authorities against each other yet, which wants closed tasks in
  quantity and therefore the store.
- Design the concrete GBNF grammar to force valid tool calls with the target model
  (Qwen2.5-Coder), replacing the text parse.
- `openat2(RESOLVE_BENEATH)` for the in-process tools, closing the TOCTOU window
  that canonicalize-then-open leaves.
- The CLI has no gate: `luu chat "prompt"` runs one turn with the policy file as
  the standing approval, because a one-shot has no human loop to gate. Whether it
  should grow one, or stay the scripted/one-shot surface it is, is open.
