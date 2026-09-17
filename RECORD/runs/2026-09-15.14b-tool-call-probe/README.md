# Evidence: the tool-call probe, run against the 14B on machine 4

The transcripts and verdicts behind the 2026-09-15 section appended to
[`../../2026-09-06.a-grammar-for-tool-calls.completed.md`](../../2026-09-06.a-grammar-for-tool-calls.completed.md).
Fifteen independent one-shot sessions — no shared history, one per line of
[`scripts/tasks/tool-call-probe.txt`](../../../scripts/tasks/tool-call-probe.txt)
— against `qwen2.5-coder-14b` (`qwen2.5-coder-14b-instruct-q4_k_m.gguf`) over
`llama-server` (`--backend openai`), machine 4, `--ctx-size 8192
--n-gpu-layers 99 --flash-attn on` (the baseline arm of
[`the-14b-context-ceiling`](../../2026-09-08.the-14b-context-ceiling.completed.md)),
`--temperature 0 --seed 7`, the same values every other tool-call-probe run
has used — for comparability against
[`the-tool-call-probe-run`](../../2026-09-14.the-tool-call-probe-run.completed.md)'s
3/15 on `qwen2.5-coder:7b`, machine 1, over Ollama.

```sh
./run.sh   # from the repository root, with llama-server already serving the
           # 14B on :8080; writes NN.txt and NN.jsonl per prompt
```

- **`NN.txt`** — the full stdout `luu chat` printed for prompt `NN`.
- **`NN.jsonl`** — the full recording (`--record`), not committed (`*.jsonl`
  is gitignored project-wide).
- **`NN.reply.txt`** — what was actually scored: every `token` protocol
  event for the prompt's first model call, concatenated, stopping at the
  first `tool_call` or `ended` event — the same extraction
  [`the-tool-call-probe-run`](../2026-09-14.tool-call-probe/README.md) used,
  for the same reason (`NN.txt` interleaves the model's text with the CLI's
  own rendering and, where a tool ran, a second reply).
- **`stats.json`** — one row per prompt: the verdict `agent_core::tools::score_call`
  gave `NN.reply.txt`, using the real function, called from a throwaway
  `cargo run --example` against `agent-core`, deleted after use.

```sh
# reproduce stats.json's verdicts (after ./run.sh has produced NN.reply.txt):
cat > crates/agent-core/examples/score_run.rs <<'EOF'
use agent_core::tools::{score_call, Tools};
fn main() {
    let tool_names: Vec<&str> = Tools::standard().names().collect();
    for n in 1..=15 {
        let path = format!("RECORD/runs/2026-09-15.14b-tool-call-probe/{n:02}.reply.txt");
        let text = std::fs::read_to_string(&path).unwrap();
        println!("{n:02} {:?}", score_call(&text, &tool_names));
    }
}
EOF
cargo run -q -p agent-core --example score_run
rm crates/agent-core/examples/score_run.rs
```
