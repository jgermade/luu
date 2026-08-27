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
  --context-limit 512 --evict block --low-water 0.5 --record after.jsonl

./scripts/make-fixtures.sh ./target/debug/luu site/fixtures   # record the replay fixtures
```

`--mock-delay-ms` paces the mock backend, `--mock-reply` scripts what it answers
(repeatable, one per model call, the last repeating), `--cancel-after-ms` exercises
cancelling, and `--record <file>` writes a replayable session from either subcommand.

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
