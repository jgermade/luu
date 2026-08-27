# Loude (CLI alias: `luu`)

A local AI agent in Rust that orchestrates calls to models, built for local
inference — 7B–32B models with 8K–32K of context. The differentiator is context
management: what gets into the prompt, and what earns its place there.

Be concise in your answers. I prefer examples over long text.

[`loude-design.md`](loude-design.md) is the design as it stands. Read it before
proposing anything; a suggestion that contradicts a decision already recorded
there needs to say so and argue with it, not route around it.

## Where things live

- [`loude-design.md`](loude-design.md) — the design as it stands **now**. It is
  rewritten as decisions change, so it always reads as the current answer.
- [`RECORD/`](RECORD/) — dated proposals and the reasoning behind them,
  **append-only**. It is how the current answer was arrived at, including the
  answers that were wrong first.

The two overlap on purpose and must not be merged. If you only update the design
doc, the reasoning is lost; if you only write a record, nobody can tell what is
true today. A decision that changes touches both: rewrite the design doc, append
to the record.

## Plans

Design work that isn't code yet lives in `RECORD/`, one file per proposal, named
`YYYY-MM-DD.<slug>.md` after the day it was written. Write the plan there *before*
touching code — the reasoning is the point, and it's what a future reader (or the
next agent) needs in order to argue with the decision rather than rediscover it. A
plan states the problem, the proposal, what it costs, what was rejected and why,
and what's still open.

**`RECORD/` files are append-only.** Never edit or delete what's already in one,
even to fix something the file gets wrong. A plan is a record of what was believed
on a given day, and rewriting it erases the only evidence of how the thinking
moved. A superseded decision gets a new dated section appended to the bottom (or a
new file, if the whole proposal is being replaced) saying what changed and why. The
mistakes stay where they are, because the reason they were made is usually the most
useful thing in the file.

## Commands

```sh
cargo test --workspace
cargo clippy --workspace --all-targets     # CI runs this with RUSTFLAGS=-D warnings
cargo fmt --all --check

cargo run --bin luu -- chat "hola"                    # one turn, mock backend, to stdout
cargo run --bin luu -- chat "hola" --backend ollama   # against a local Ollama
cargo run --bin luu -- serve                          # the debug UI on 127.0.0.1:7878
cargo run --bin luu -- tools                          # the resolved sandbox and the exact prefix block

# a task, start to finish, against the mock backend: the first reply is the plan
cargo run --bin luu -- chat --script scripts/tasks/one-task.txt --mock-delay-ms 0 \
  --mock-reply '```plan
{"steps":["read AGENTS.md"],"paths":["AGENTS.md"],"commands":[]}
```' --mock-reply 'It is the shared instruction file.'

# the tool loop end to end without a model: one reply per model call
cargo run --bin luu -- chat "what is in AGENTS.md?" --mock-delay-ms 0 \
  --mock-reply 'looking
```tool
{"name":"read_file","arguments":{"path":"AGENTS.md","max_lines":3}}
```' --mock-reply 'It is the shared instruction file.'

# a repeatable multi-turn run, which is the only kind worth comparing
cargo run --bin luu -- chat --script scripts/tasks/long-session.txt \
  --context-limit 8192 --tokenizer path/to/tokenizer.json --record before.jsonl

# the same run under the other eviction policy — one flag apart, so comparable
cargo run --bin luu -- chat --script scripts/tasks/steady-state.txt \
  --context-limit 1024 --reserve 64 --evict block --low-water 0.5 --record after.jsonl

# and its twin: the same twenty prompts grouped into tasks that close every five
cargo run --bin luu -- chat --script scripts/tasks/steady-state-tasks.txt \
  --context-limit 1024 --reserve 64 --record folded.jsonl

./scripts/make-fixtures.sh ./target/debug/luu site/fixtures   # record the replay fixtures
```

A script line starting with `:` is a task directive: `:task <objective>` asks the
model for a plan and runs nothing, `:approve` is the confirmation, `:discard`
refuses it, and `:close` summarises the task and folds its turns into that summary.
A prompt while a plan is waiting is refused, not queued. Outside a task, `chat`
behaves exactly as it always did. In the debug UI the same lifecycle is
`/task <objective>` in the composer and two buttons on the card.

`--mock-delay-ms` paces the mock backend, `--mock-reply` scripts what it answers
(repeatable, one per model call, the last repeating), `--cancel-after-ms` exercises
cancelling, and `--record <file>` writes a replayable session from either subcommand.

## Running against a real model (Ollama)

Everything measured so far comes from the mock backend and `chars/4`. That is
legitimate for reuse and token accounting — both are properties of the string we
assemble, decided before the call — and it is **not** legitimate for the question
that decides whether a context strategy is any good: does the task still succeed.
That needs a model. This is the setup for one.

*Nothing in this section has been run yet. Whoever does it first should correct
it in place and append what they found to `RECORD/`.*

**No administrator rights are needed for any of this.** Ollama runs entirely out
of your home directory — the models live in `~/.ollama/models` (`OLLAMA_MODELS`
moves them) and the server is an ordinary user process. What the package manager
buys is a symlink in `/usr/local/bin`, which is the only part that wants `sudo`
and the only part nothing here uses: `luu` talks to a URL.

```sh
# however the binary got onto the machine — unzipped .app, tarball, ~/.local/bin
export OLLAMA_KEEP_ALIVE=30m                 # see below; set it *before* serving
nohup ollama serve > ~/ollama.log 2>&1 &     # detached, listening on 11434
ollama ps                                    # what is loaded, and until when

ollama pull qwen2.5-coder:7b                 # ~4.7 GB, the CLI's default model
ollama pull qwen2.5-coder:14b                # ~9 GB
ollama pull qwen2.5-coder:32b                # ~20 GB — fits 48 GB, much slower

# the tokenizer, which Ollama does not ship: GGUF carries its own vocab, and
# `--tokenizer` wants the HuggingFace file. It must be the tokenizer of the
# model actually being run, which is the entire point of `Counter::Model { id }`.
mkdir -p ~/models/qwen2.5-coder-7b
curl -L -o ~/models/qwen2.5-coder-7b/tokenizer.json \
  https://huggingface.co/Qwen/Qwen2.5-Coder-7B-Instruct/resolve/main/tokenizer.json

cargo run --release --bin luu -- chat "hola" --backend ollama \
  --model qwen2.5-coder:7b --context-limit 8192 \
  --tokenizer ~/models/qwen2.5-coder-7b/tokenizer.json
```

Four things that will otherwise waste a session:

- **The window has to be sent, and it is.** `--context-limit` becomes
  `options.num_ctx` on the request. Ollama's own default is a couple of thousand
  tokens and it truncates the prompt to it *silently*, so without this a run
  budgeting 8k measures a prompt the model never saw. `--context-limit 0` sends
  no option at all and the server keeps its default.
- **A larger window costs memory before it costs anything else.** The KV cache
  grows with `num_ctx` and with the model; 7B at 8–16K is the place to start on
  48 GB, and 32B at 32K is where it stops being comfortable.
- **The prompt cache lives with the loaded model.** If Ollama unloads between
  runs the next first call is cold, and a reuse comparison across runs is
  measuring the unload. `OLLAMA_KEEP_ALIVE` is read by the *server*, so it has to
  be set before `ollama serve` starts, not in the shell that runs `luu`; `ollama
  ps` says what is resident and for how long. Check it between the two halves of
  a comparison, not only before the first.
- **The sandbox is weaker on macOS, and says so.** Landlock and seccomp are
  Linux-only, so `run_command` is *denied* by default with a verdict naming what
  is missing. `--sandbox-enforcement best-effort` runs it anyway, and then the
  allowlist in our own process is all that holds the child — which is exactly
  what the verdict will report. In-process tools (`read_file`, `write_file`,
  `edit_file`, `list_dir`) are unaffected: they never had kernel enforcement.

The runs worth recording first, because they are the ones the mock cannot answer:

```sh
# the same pair the mock measured, now with a model that reads what it is sent
for script in steady-state steady-state-tasks; do
  cargo run --release --bin luu -- chat --script scripts/tasks/$script.txt \
    --backend ollama --model qwen2.5-coder:7b \
    --context-limit 8192 --reserve 512 \
    --tokenizer ~/models/qwen2.5-coder-7b/tokenizer.json \
    --record ~/records/$script.jsonl
done

cargo run --release --bin luu -- serve --record ~/records/live.jsonl   # and look at them
```

What to look at, in order: whether the answers after a fold still refer to what
the task did (the summary is the only trace of it left), the gap between our
count and `usage.prompt_tokens` on each turn (a stable gap is the chat template,
a moving one means the template changed), and whether reuse behaves as the mock
said it would.

The sandbox comes from `luu.toml` — `[sandbox]`, with `paths`, `commands`,
`network` and `enforcement` — and the `--allow-read/-write/-exec`,
`--allow-command`, `--allow-network` and `--sandbox-enforcement` flags **add** to
it for one run. `--max-tool-steps` caps the tool calls one turn may make and
`--no-tools` runs without them. `luu tools` prints what all of that resolved to,
implicit grants included.

`--context-limit` is the model's window (`0` means unknown: no budget, no
eviction), `--reserve` is what is held back for the answer, `--evict` is how the
history gives way (`turn` drops the minimum, `block` cuts to `--low-water` and
then holds still), and `--tokenizer` points at the model's `tokenizer.json`. **Without `--tokenizer` the counts are
`chars/4`**, labelled approximate everywhere they appear — fine for a smoke run,
useless for a comparison, and the numbers say so themselves.

CI is `.github/workflows/build.yml` (fmt, clippy, tests, both binaries, the site) on
every push and pull request. `release.yml` is manual: it takes `patch`/`minor`/`major`,
raises the workspace version, tags it, calls `build.yml` at the new commit, publishes
the release and deploys Pages. It has a `dry-run` input — use it before the first
real release.

**Do not edit the UI's `dist`-like output, because there isn't one.** `crates/luu/ui/`
is served as it is: `rust-embed` reads it from disk in debug builds and bakes it into
the binary for release. Editing a component costs a reload, not a `cargo build`.

## Design commitments that are easy to erode

These are not style preferences. Each one is load-bearing, and each looks like an
implementation detail from close up:

- **The model never executes anything.** It emits a structured request; the program
  parses it, validates it against the `SandboxPolicy`, and executes real Rust code.
  Any path where model output reaches a shell or the filesystem without passing
  that validation is a bug, however convenient.
- **Permission checks live in the code, not in the model behaving well.**
  Canonicalize paths (`std::fs::canonicalize`) before comparing, or a symlink walks
  straight out of the sandbox.
- **A subprocess is the kernel's to hold, not ours.** An in-process check happens
  before the syscall, in a program that then makes the syscall itself; a child
  makes its own, and nothing we wrote is in the way. So `run_command` builds a
  Landlock ruleset and a seccomp filter in the parent and applies them in
  `pre_exec` — and where the kernel cannot, the default is to deny rather than to
  run the child unheld.
- **Nothing may say "sandboxed" without saying by what.** Every verdict carries who
  enforced it, and a partial one carries what is missing. A run the kernel held and
  a run nothing held are not the same run, and afterwards the recording is the only
  thing that could tell them apart.
- **A grant that exists for the child does not answer for a tool.** Allowing a
  command implies read+execute on the system roots, because a program cannot run
  without reading libc — and that reasoning says nothing about `read_file`, so the
  in-process path check ignores those roots.
- **A task's plan and its summary are turns, and closing is the one rewrite.** They
  enter the history as ordinary turns so the user/assistant alternation holds and so
  they stay inside the budget — tagged, and counted in their own bucket, because
  scaffolding that hides inside `history` is scaffolding nobody can justify. The
  fold at the close replaces the span from the plan turn onward with the summary;
  it is the only place the history is rewritten, and the floor moves back exactly
  far enough to keep the summary in the window. What the fold cost travels with it.
- **Nothing runs before the confirmation, and a refusal is said out loud.** The gate
  is a comparison on the task's state, not a mode: a prompt while a plan is waiting
  is refused with a reason. Dropping it silently, or queueing it, runs work against
  a plan nobody answered.
- **A summary is evidence, not the model's account of itself.** It is derived from
  the turns' tool calls — paths, commands, exit lines, denials — with no model call
  in the path, because it lands in the write-once region every later turn is built
  on and a fallback must not need the thing that failed.
- **Decide what goes in, then render it.** `Context::select` chooses against a
  token budget and the rendering is a pure function of that choice, so every
  token sent is attributable to a bucket. Rendering first and trimming the
  string afterwards loses the attribution and cuts wherever the limit lands —
  sooner or later inside the stable prefix.
- **History is evicted in whole turns, and what leaves the window stays out.**
  Half a turn leaves an answer to a question nobody asked, and a window starting
  on an assistant message makes several chat templates continue instead of
  answering. Eviction is also monotone — `Context` keeps a floor that only moves
  forward — because a turn that comes back rewrites the history from its front,
  and that is the prefix cache's worst case.
- **Every token count carries which counter produced it.** Two runs measured by
  different counters are not comparable, and nothing else in the system would
  ever say so.
- **The stable prompt prefix stays byte-identical across calls**, and how much of
  it survived is reported per turn rather than assumed. System text and
  tool definitions are what llama.cpp's prompt cache reuses. The definitions are
  therefore a wire format: tools sorted by name, schemas through `serde_json`'s
  sorted maps, nothing interpolated. `luu tools` prints the exact bytes. Reordering tools,
  re-serializing a schema with different key order, or interpolating a timestamp
  into the system block silently destroys KV reuse — and nothing fails, it just
  gets slower. Treat the prefix as a wire format.
- **One JSON message schema, several transports.** stdio, WebSocket and the record
  format carry the same enums. A field that only makes sense to one client does not
  belong in the protocol; debug traces go behind `--trace`, on their own channel, so
  the protocol doesn't grow a debug half that stdio has to carry forever.
- **`agent-core` knows nothing about the CLI, VSCode or the browser.** That is what
  makes container isolation a packaging question rather than a rewrite.
- **`cargo build` must not require node.** The debug UI ships as
  [jq79](https://github.com/jgermade/jq79) — one runtime file plus `.html`
  components, embedded with `rust-embed`. Keep npm out of `build.rs`.

## Before you claim the context manager got better

Context strategies are measured, not argued. A change to compaction, relevance
selection or budget allocation needs numbers from the same model and the same task
set, before and after — token counts per bucket, prompt-cache hit rate, and whether
the task still succeeds. "Fewer tokens" is not a result on its own: dropping the
fragment the model needed is also fewer tokens.

The debug web client exists to produce those numbers — which is why it sits early
in the work order rather than at the end, and why `luu serve --record` writes a
replayable session file. Compare runs, don't recall them.

Two things that have already gone wrong here, both silently:

- **The window has to leave room for what is being measured.** The tool
  definitions are ~465 tokens of the prefix, so the eviction pair recorded at
  `--context-limit 512` selected no history at all and both policies produced an
  identical run of nothing — for several commits, with every panel drawing
  normally. If a bucket you expect to move is flat, check the fixed part first.
- **A reuse percentage is not comparable across prefixes.** The floor is the
  constant share of the prompt, so adding a large block to the prefix raises every
  number without improving anything. Same rule as "every count carries its
  counter", one level up: state the configuration beside the figure.
