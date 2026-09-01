# Loude (CLI alias: `luu`)

A local-first AI agent in Rust that orchestrates calls to models, built for local
inference — 7B–32B models with 8K–32K of context — and free to reach a remote
model when you name one, never as a fallback. The differentiator is context
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
- [`ROADMAP/`](ROADMAP/) — what is **planned and not yet true**, one directory
  per revision (`ROADMAP/YYYY-MM-DD/`). Rewritable *within* a revision and
  superseded wholesale by a later one, so the roadmap can be corrected without
  the correction erasing what was believed before it.

The three overlap on purpose and must not be merged. If you only update the
design doc, the reasoning is lost; if you only write a record, nobody can tell
what is true today; if you only keep a roadmap, nobody can tell the plan from the
tree. A decision that changes touches the first two: rewrite the design doc,
append to the record.

The rule that keeps the third honest, because it is the one that rots: **nothing
in `ROADMAP/` is ever the answer to "what is true today."** An item that lands
moves into `loude-design.md` and gets a record; the roadmap entry it came from is
struck through in place, not deleted, so a revision reads as *what we set out to
do and how much of it happened*. A roadmap that quietly loses its misses is a
roadmap nobody can calibrate.

## Plans

Design work that isn't code yet lives in `RECORD/`, one file per proposal, named
`YYYY-MM-DD.<slug>.<state>.md` after the day it was written, where `<state>` is
`WIP` or `completed`.

**The suffix is the record's own state, not the project's.** A record is `WIP`
while it is still waiting on the thing it exists to produce — code it plans, a run
it specifies, a decision it asks for — and becomes `completed` once that has
happened. A *Still open* section does not make a record `WIP`: every record ends
with one by design, and if open threads counted, nothing would ever be complete.
A study, a stock-taking or an argument is `completed` the day it is written,
because writing it is the whole of what it was for.

Renaming is the **only** change a finished record may receive; the content stays
append-only, and the state moves in the filename rather than in an edited line. A
rename breaks every link that points at the file — including the ones in Rust doc
comments — so it updates all of them in the same commit, and `RECORD/` is checked
for dangling links before the commit lands. Write the plan there *before*
touching code — the reasoning is the point, and it's what a future reader (or the
next agent) needs in order to argue with the decision rather than rediscover it. A
plan states the problem, the proposal, what it costs, what was rejected and why,
and what's still open.

**A record argues; a roadmap orders.** They answer different questions and the
split is worth keeping: *why is this the right thing to build* belongs in a dated
record and never changes afterwards, while *what are we building next and in what
order* belongs in `ROADMAP/<revision>/` and is expected to change. So a roadmap
entry is a few lines and a link to the record that argued for it, never a second
copy of the argument — the moment it restates the reasoning it starts drifting
from it, which is the same failure the design doc and the record are split to
avoid. Sequencing, what blocks what, and the Gantt are the roadmap's own content
and belong nowhere else.

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

# the gate, without a model: the planning call answers with a plan block, then
# the turn answers. Type a prompt and the UI holds it until you approve it.
cargo run --bin luu -- serve --mock-reply '```plan
{"objective": "explain the budget", "steps": ["read the design"], "files": ["loude-design.md"]}
```' --mock-reply 'The budget is split into buckets.'
cargo run --bin luu -- tools                          # the resolved sandbox and the exact prefix block
cargo run --bin luu -- map --map-tokens 1024          # the repository outline that budget resolves to

# the tool loop end to end without a model: one reply per model call
cargo run --bin luu -- chat "what is in AGENTS.md?" --mock-delay-ms 0 \
  --mock-reply 'looking
```tool
{"name":"read_file","arguments":{"path":"AGENTS.md","max_lines":3}}
```' --mock-reply 'It is the shared instruction file.'

# a real file fused into the turn, read through the sandbox
cargo run --bin luu -- chat "which two commitments does this open with?" \
  --fragment crates/agent-core/src/context.rs:1-15

# a repeatable multi-turn run, which is the only kind worth comparing
cargo run --bin luu -- chat --script scripts/tasks/long-session.txt \
  --context-limit 8192 --tokenizer path/to/tokenizer.json --record before.jsonl

# the same twenty prompts grouped into tasks: the history folds at each `## close`
cargo run --bin luu -- chat --script scripts/tasks/steady-state-tasks.txt \
  --context-limit 1024 --reserve 64 --record tasks.jsonl

# the same run under the other eviction policy — one flag apart, so comparable
cargo run --bin luu -- chat --script scripts/tasks/steady-state.txt \
  --context-limit 512 --evict block --low-water 0.5 --record after.jsonl

# the grounded pair: twenty prompts that are about this repository, with files
# attached, one grouped into tasks. The last five questions attach nothing and
# ask the first fifteen again — in the tasks run those have been folded.
for script in grounded grounded-tasks; do
  cargo run --bin luu -- chat --script scripts/tasks/$script.txt \
    --context-limit 8192 --reserve 512 --record $script.jsonl
done

./scripts/make-fixtures.sh ./target/debug/luu site/fixtures   # record the replay fixtures
```

`--mock-delay-ms` paces the mock backend, `--mock-reply` scripts what it answers
(repeatable, one per model call, the last repeating), `--cancel-after-ms` exercises
cancelling, and `--record <file>` writes a replayable session from either subcommand.

In `serve`, a prompt with no task open buys a planning call and is then **held,
unrun**, until it is approved or refused in the UI — nothing runs behind the
gate, and a client that sends a prompt anyway gets a `refused` message saying
why: a turn is running, a proposal is pending, the task is not in that state, or
the policy file does not grant part of what was asked. That message is what took
the protocol to v2 (and the record format to 4) — a new variant of a tagged enum
is a change an older reader cannot parse; `evicted` took it to **v3** and the
format to **5** under the same rule. A proposal says **who wrote it**: `source` is
`model` when the planning call emitted a parseable plan block, `prose` when it
answered without one (the ordinary case for a 7B — the proposal is then the ask
itself, declaring nothing), and `written` for a script's `## task:`, where no
planning call happened. Inferring that from an empty plan, which is what the
panel used to do, cannot tell a model that ignored the format from one that
declared an empty list — and which of the two it is decides whether the fix is a
grammar or a sentence. The read side keeps the plan **as proposed** beside the
plan as approved for the same reason: the difference between them is what a
person had to add, which is the cost of the gate. The task lifecycle is a state
machine and every transition is guarded: a proposal cannot be closed, a rejected
plan cannot be reopened. See
[`RECORD/2026-08-30.a-refusal-is-a-message.completed.md`](RECORD/2026-08-30.a-refusal-is-a-message.completed.md). A closed task collapses
in the transcript to the summary the model now gets, expandable to what it no
longer sees.

`serve` binds loopback and answers without authentication, and **any other
address needs `--auth-token-file <PATH>`**: `/ws` carries task approval, so off
loopback that authority is one request away from anyone who can reach the port.
The bind is *refused* without one, before the listener exists, rather than
warned about above a port that is already serving. The token gates `/ws` and
`/api/*` — `Authorization: Bearer <token>`, or `?token=` on `/ws` alone,
because that is all a browser's `WebSocket` constructor can send — and not the
embedded page, so a guarded server is opened at `http://host:7878/?token=…` and
the UI carries it from there. The file's mode is checked: a flag is greppable in
`ps` and an env var is inherited by every child `run_command` spawns.

A script is prompts one per line, `#` comments, and `##` directives for the task
lifecycle — `## task: <objective>`, then `## step:` / `## file:` / `## write:` /
`## command:` for its plan, and `## close`. `## file:` is what the task may
**read** and `## write:` what it may also **change**; a plan that declares no
writes may not write, and a write into a read-only root is refused before the
first turn. `## fragment: <path>[:start-end]` is the other
directive, and it is **not** `## file:`: the plan's files are what the task is
allowed to touch, a fragment is text put into the next prompt.

**The written plan is the approval**: every file it names has to be reachable in
the resolved sandbox and every command allowed by it, or the run stops before the
first turn. **It is also the sandbox for its task** — from `## task:` to
`## close`, and from an approval to a close in `serve`, a turn may touch what the
plan named and nothing else, fragments included, at the level it named: `files`
are granted read, `writes` read-write. The policy file is the outer
bound (a plan cannot grant what it does not) and a plan that names nothing grants
nothing; a denial says which of the two refused. In `serve`, `approve_task`
carries the files and commands the person adds at the gate, checked against the
policy file the same way, which is what keeps an under-specified plan from being
a dead run. See
[`RECORD/2026-08-30.narrowing.completed.md`](RECORD/2026-08-30.narrowing.completed.md). Closing folds the task's turns into a
deterministic summary (the plan, what the tool results reported, and the
fragments the turns were shown, quoted verbatim under a token cap), which is
what the `summaries` bucket in the budget panel plots. A directive it does not
recognise is an error, never a prompt.

The sandbox comes from `luu.toml` — `[sandbox]`, with `paths`, `commands`,
`network` and `enforcement` — and the `--allow-read/-write/-exec`,
`--allow-command`, `--allow-network` and `--sandbox-enforcement` flags **add** to
it for one run. `--max-tool-steps` caps the tool calls one turn may make and
`--no-tools` runs without them. `luu tools` prints what all of that resolved to,
implicit grants included.

`--map-tokens N` puts a **repository map** in the prefix: every `.rs` file's
definitions with their signatures, bodies elided, from `tree-sitter`'s own
`TAGS_QUERY`. It goes under the tool definitions and above the history, because
blocks are ordered by how often they are *rewritten* and the map changes only
when the repository does. It is **0 by default** — a map that arrived switched on
would silently change every number in every recording made so far. `luu map`
prints the exact bytes and what they cost.

Files are outlined in **path order** until one does not fit, and the map says how
many it left out. That is not relevance and does not pretend to be: it is the
baseline the reference graph has to beat, and this repository makes the case by
itself — the whole outline is 6 327 tokens, **77% of an 8K window**, so at any
budget a real run can afford, most of the repository is missing and the alphabet
picked which part. Two readings to keep straight: the map is paid on *every*
call (870 tokens × 20 turns is +56% on the grounded script's total), and it
**inflates prefix reuse as pure arithmetic** — a bigger constant block raises
shared and total together, so 93.9% → 96.3% is not an improvement and a reuse
number from a run with the map is not comparable to one without it. Numbers and
the two bugs that running it found are in
[`RECORD/2026-08-31.the-repo-map.completed.md`](RECORD/2026-08-31.the-repo-map.completed.md).

`--fragment PATH[:START-END]` on `chat` fuses a real file into the next prompt —
repeatable, 1-based inclusive lines, read **through the sandbox**, and attached
to one turn only. Which turns a file belongs in is what relevance selection
exists to decide later, and attaching it to all of them would answer that now,
wrongly, and pay for it every turn. A path the sandbox refuses is an error, not
a warning: a run that quietly dropped its grounding answers out of the model's
training and looks like it worked. This is what fills the `code` bucket, which
was zero in every recording before the surface existed — and why every script in
`scripts/tasks/` except the grounded pair is ungrounded Q&A that a 7B will answer
from training. See `RECORD/2026-08-27.grounded-fold-probe.completed.md`.

`--context-limit` is the model's window (`0` means unknown: no budget, no
eviction), `--reserve` is what is held back for the answer, `--evict` is how the
history gives way (`turn` drops the minimum, `block` cuts to `--low-water` and
then holds still), and `--tokenizer` points at the model's `tokenizer.json`.
A cut says so: the run prints `== evicted turn N` and the recording carries an
`evicted` line naming the turns that left, what they were worth, who counted
them and which policy did it. Over the same twenty prompts at 1024 tokens that
is ten small cuts under `turn`, two deep ones under `block`, and none at all in
the tasks run — the fold kept it under the limit. See
[`RECORD/2026-08-31.eviction-tombstones.completed.md`](RECORD/2026-08-31.eviction-tombstones.completed.md). **Without `--tokenizer` the counts are
`chars/4`**, labelled approximate everywhere they appear — fine for a smoke run,
useless for a comparison, and the numbers say so themselves.

Two of the tests are not unit tests and are the only ones that run the thing:
`crates/agent-core/tests/ollama_wire.rs` puts a stub HTTP server on an ephemeral
port and asserts what `Ollama::stream` actually sends — the window included,
which is the bug that shipped once — and `crates/luu/tests/serve_ws.rs` binds the
server, drives the gate over `/ws`, and checks the read API agrees with what the
socket said. Both run under a plain `cargo test --workspace` in under a second.

The third one is the page, and it is the only thing here that wants node —
`cargo build` still must not:

```sh
cp -r crates/luu/ui/. site/ && ./scripts/make-fixtures.sh ./target/debug/luu site/fixtures
cd tests/smoke && npm ci && npx playwright install chromium && npx playwright test
```

It loads the assembled site, replays a recording, clicks through every fixture
in the picker, and fails on any console error other than the socket that cannot
connect on a static host. It runs in the `web` job, after the site is assembled.
It has already earned itself: it found a double replay that had been showing a
ghost turn to every visitor of the deployed page. See
[`RECORD/2026-08-30.tests-that-run-it.completed.md`](RECORD/2026-08-30.tests-that-run-it.completed.md).

CI is `.github/workflows/build.yml` (fmt, clippy, tests, both binaries, the site) on
every push and pull request. `release.yml` is manual: it takes `patch`/`minor`/`major`,
raises the workspace version, tags it, calls `build.yml` at the new commit, publishes
the release and deploys Pages. It has a `dry-run` input — use it before the first
real release.

**Do not edit the UI's `dist`-like output, because there isn't one.** `crates/luu/ui/`
is served as it is: `rust-embed` reads it from disk in debug builds and bakes it into
the binary for release. Editing a component costs a reload, not a `cargo build`.

## Running against a real model (Ollama)

Everything measured so far comes from the mock backend and `chars/4`. That is
legitimate for reuse and token accounting — both are properties of the string we
assemble, decided before the call — and it is **not** legitimate for the question
that decides whether a context strategy is any good: does the task still succeed.
That needs a model. This is the setup for one.

This has been run: `qwen2.5-coder:7b/14b/32b` pulled, tokenizer fetched, the
setup below confirmed correct as written — `num_ctx` reached the server, the
macOS `run_command` denial read exactly as described. What running it actually
found is in [`RECORD/2026-08-27.the-m4-pro-run.completed.md`](RECORD/2026-08-27.the-m4-pro-run.completed.md):
`steady-state`/`steady-state-tasks` carry no grounding, so they cannot answer
whether an answer survives a fold; a tool call inside a turn was invisible to
`prefix_reuse` the same way a planning call had been; and folding's token result
flips with the window — losing narrowly at `--context-limit 1024` (chosen to
force eviction), winning by more than half at the `8192` this recipe uses,
because the baseline never evicts at 8192 and its history compounds unchecked.
The first two are fixed; the grounded pair below is what the first one bought.

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
# the pair that asks the question the mock cannot: `grounded.txt` attaches real
# files from this repository and its twin groups the same prompts into tasks, so
# the last five turns — which attach nothing and ask about all of it again — rest
# on the full history in one run and on three summaries in the other.
for script in grounded grounded-tasks; do
  cargo run --release --bin luu -- chat --script scripts/tasks/$script.txt \
    --backend ollama --model qwen2.5-coder:7b \
    --context-limit 8192 --reserve 512 \
    --tokenizer ~/models/qwen2.5-coder-7b/tokenizer.json \
    --record ~/records/$script.jsonl
done

# and the ungrounded pair, which is what the eviction and fold numbers came from
for script in steady-state steady-state-tasks; do
  cargo run --release --bin luu -- chat --script scripts/tasks/$script.txt \
    --backend ollama --model qwen2.5-coder:7b \
    --context-limit 8192 --reserve 512 \
    --tokenizer ~/models/qwen2.5-coder-7b/tokenizer.json \
    --record ~/records/$script.jsonl
done

cargo run --release --bin luu -- serve --record ~/records/live.jsonl   # and look at them
```

**The protocol for that run — the commands, what a right answer to each of the
last five turns contains, how to tell "the summary dropped it" from "the model
ignored it", and the sampling precondition — is
[`RECORD/2026-08-27.grounded-fold-probe.completed.md`](RECORD/2026-08-27.grounded-fold-probe.completed.md).
Read it before running, and append what it says to append.**

**The gate has its own, and it has never been run:**
[`RECORD/2026-08-31.the-gate-probe.WIP.md`](RECORD/2026-08-31.the-gate-probe.WIP.md) —
fifteen prompts typed through `serve`, what a plan worth approving names for
each, the four ways a denial can happen and how to tell them apart, and the five
numbers to write down. Everything verified for the gate, for narrowing and for
`writes` so far was mock-backed or driven by hand; **no model has ever proposed a
plan that this tree then held it to.** Read it before running that, too — and
approve with the least that will run, because the count of what had to be added
is the measurement.

What to look at, in order: **the last five turns of the grounded pair**, where
the same four questions are answered from a full history in one run and from
three summaries in the other — that comparison is the only thing that says
whether folding loses what the task needed; the gap between our count and
`usage.prompt_tokens` per turn (stable is the chat template; a tooled turn's
round trips are now counted, so they no longer masquerade as it); and whether
reuse behaves as the mock said it would.

## Commits

Keep `Co-Authored-By` when an agent wrote the change — it is conventional
attribution, and readable by anyone.

**Do not add a `Claude-Session:` trailer.** Some agent harnesses append one by
default; this repository is public, and a backlink into a tool nobody reading the
log can open is not worth a permanent line in shared history. Three of the
squashed commits carry one because it was added before anyone weighed that, and
they are left alone rather than rewritten.

This convention was itself written on a branch that was never merged, which is
why it is arriving after the commits it describes rather than before them.

## Design commitments that are easy to erode

These are not style preferences. Each one is load-bearing, and each looks like an
implementation detail from close up:

- **The model never executes anything.** It emits a structured request; the program
  parses it, validates it against the `SandboxPolicy`, and executes real Rust code.
  Any path where model output reaches a shell or the filesystem without passing
  that validation is a bug, however convenient.
- **A plan says what it reads and what it writes, and is held to both.** The
  distinction is the difference between a check that can say no and one that
  only looks like it does: without it every task ran with write access to
  everything it named.
- **The approved plan is the sandbox for its task.** A task boundary is the
  scope permission is granted at, and that is only true if a turn inside a task
  is held to what the task was approved for. Checking the plan against the
  policy and then running with the policy makes the plan a comment.
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
- **The map is prefix, so it is stable, and it is not ranked yet.** The
  repository outline sits under the tool definitions and does not move for the
  life of a run — that is what makes it cheap on a prompt cache, and it is why
  ranking it (which personalizes it per turn, per task) is a trade to *measure*
  rather than a patch to apply. `#[cfg(test)]` modules are skipped: a module the
  compiler only builds for tests is not the interface the map describes, and in
  this repository the test names were most of the budget. The first file that
  does not fit stops the map rather than being skipped over, so a wider budget
  always shows everything a tighter one did — a map whose contents do not nest
  cannot be compared against itself.
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
- **Forgetting is an event, and it names names.** The selection that moves the
  floor emits `evicted`, on the protocol beside `task_closed` rather than behind
  `--trace`: a fold and an eviction are the two ways history stops being sent,
  and a recording that could show one and not the other says the history bucket
  shrank without saying whether that was the policy or the arithmetic. It names
  the *turns*, which is why `Turn` carries the id the session handed it — the
  position in `Context::turns` is a different number, since a turn that produced
  nothing is never pushed and a planning call is never remembered. The transcript
  keeps an evicted turn and marks it; removing it would make the view agree with
  the prompt and lose the difference between them, which is what the debug client
  is for.
- **Every token count carries which counter produced it.** Two runs measured by
  different counters are not comparable, and nothing else in the system would
  ever say so.
- **Every model call a turn makes is measured, not just the first.** A turn that
  uses a tool is several calls, each carrying the previous result, and the
  backend's `usage.prompt_tokens` is summed over all of them. Measure only the
  first and the two numbers count different things — a 2-token chat-template gap
  reads as 1 962 — and nothing fails. The agent loop announces every call it
  makes before making it; the trace channel measures the ones after the first
  into the same chain. See `RECORD/2026-08-27.the-m4-pro-run.completed.md`.
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
