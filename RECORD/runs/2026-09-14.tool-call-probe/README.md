# Evidence: the tool-call probe run of 2026-09-14

The transcripts and verdicts behind
[`../../2026-09-14.the-tool-call-probe-run.completed.md`](../../2026-09-14.the-tool-call-probe-run.completed.md).
Fifteen independent one-shot sessions — no shared history, one per line of
[`scripts/tasks/tool-call-probe.txt`](../../../scripts/tasks/tool-call-probe.txt)
— against `qwen2.5-coder:7b` over Ollama, machine 1, `--context-limit 8192
--temperature 0 --seed 7`, the same destination and settings
[`the-7b-does-not-miss-it`](../../2026-09-14.the-7b-does-not-miss-it.completed.md)
used.

```sh
./run.sh   # from the repository root; writes NN.txt and NN.jsonl per prompt
```

- **`NN.txt`** — the full stdout `luu chat` printed for prompt `NN`: the
  model's raw text, then (when one parsed) the CLI's own `[1] → …` / `[1] ←
  …` tool-call and tool-result lines, then a second model reply where a tool
  ran, then the `[stop] … prompt / … completion` footer.
- **`NN.jsonl`** — the full recording (`--record`), not committed
  (`*.jsonl` is gitignored project-wide, same as every other run here).
- **`NN.reply.txt`** — what was actually scored: every `token` protocol
  event for the prompt's single turn, concatenated, stopping at the first
  `tool_call` event or at `ended` if none fired — i.e. exactly the first
  model call's raw text, with the CLI's own markers and any second call
  stripped out. Extracted from `NN.jsonl` because `NN.txt` interleaves the
  model's text with the CLI's rendering and a second reply, and slicing that
  by string search is exactly the "two scanners drift" problem this
  project's own `fenced`/`fenced_span` split exists to avoid — the jsonl's
  event boundaries are unambiguous where the printed text is not.
- **`stats.json`** — one row per prompt: the verdict `agent_core::tools::score_call`
  gave `NN.reply.txt`, using the real function (not a reimplementation),
  called from a throwaway `cargo run --example` against `agent-core`,
  deleted after use — the same "jq against the uncommitted recording, not
  committed itself" shape `2026-09-14.prune-results/README.md` used for its
  own numbers.

```sh
# reproduce stats.json's verdicts (after ./run.sh has produced NN.reply.txt):
cat > crates/agent-core/examples/score_run.rs <<'EOF'
use agent_core::tools::{score_call, Tools};
fn main() {
    let tool_names: Vec<&str> = Tools::standard().names().collect();
    for n in 1..=15 {
        let path = format!("RECORD/runs/2026-09-14.tool-call-probe/{n:02}.reply.txt");
        let text = std::fs::read_to_string(&path).unwrap();
        println!("{n:02} {:?}", score_call(&text, &tool_names));
    }
}
EOF
cargo run -q -p agent-core --example score_run
rm crates/agent-core/examples/score_run.rs
```
