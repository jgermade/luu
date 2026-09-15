# Evidence: the tool-call probe, re-run after run_command's argv schema change

The transcripts and verdicts behind the second 2026-09-15 section appended to
[`../../2026-09-06.a-grammar-for-tool-calls.WIP.md`](../../2026-09-06.a-grammar-for-tool-calls.WIP.md),
comparing directly against
[`../2026-09-15.14b-tool-call-probe/`](../2026-09-15.14b-tool-call-probe/).
Same fifteen prompts, same model
(`qwen2.5-coder-14b-instruct-q4_k_m.gguf`), same `llama-server` settings
(`--ctx-size 8192 --n-gpu-layers 99 --flash-attn on`, `--temperature 0 --seed
7`), same machine (4, `ryzen-cachy`). One flag apart: the binary, rebuilt
after `run_command`'s schema changed from `{command, args, cwd, timeout_ms}`
to `{argv, cwd, timeout_ms}`.

```sh
./run.sh                                    # writes NN.txt and NN.jsonl per prompt
python3 ../../../.tmp/extract_first_reply.py .   # writes NN.reply.txt from the jsonl
```

- **`NN.txt`** / **`NN.jsonl`** / **`NN.reply.txt`** — same shapes and same
  extraction rule (every `token` event of turn 1, concatenated, stopping
  before the first `tool_call` or at `ended`) as
  [`../2026-09-15.14b-tool-call-probe/README.md`](../2026-09-15.14b-tool-call-probe/README.md)
  used. The extraction script itself is disposable
  (`.tmp/extract_first_reply.py`, gitignored) rather than committed, per the
  same convention that run's README followed by describing the method
  instead of shipping a script.
- **`stats.json`** — not written as a file this time; the verdicts are the
  table below, produced the same way (a throwaway
  `cargo run -p agent-core --example score_run` against the real
  `agent_core::tools::score_call`, deleted after use).

```sh
# reproduce the verdicts (after ./run.sh and the extraction step above):
cat > crates/agent-core/examples/score_run.rs <<'EOF'
use agent_core::tools::{score_call, Tools};
fn main() {
    let tool_names: Vec<&str> = Tools::standard().names().collect();
    for n in 1..=15 {
        let path = format!("RECORD/runs/2026-09-15.14b-argv-schema/{n:02}.reply.txt");
        let text = std::fs::read_to_string(&path).unwrap();
        println!("{n:02} {:?}", score_call(&text, &tool_names));
    }
}
EOF
cargo run -q -p agent-core --example score_run
rm crates/agent-core/examples/score_run.rs
```

| prompt | verdict |
| --- | --- |
| 01 | Parsed |
| 02 | Parsed |
| 03 | Parsed |
| 04 | ContinuedPastFence |
| 05 | Parsed |
| 06 | Parsed |
| 07 | Parsed |
| 08 | Parsed |
| 09 | ContinuedPastFence |
| 10 | Parsed |
| 11 | Parsed |
| 12 | Parsed |
| 13 | Parsed |
| 14 | ContinuedPastFence |
| 15 | Parsed |

12/15 Parsed, 3/15 ContinuedPastFence, 0/15 Drifted, 0/15 NoCall.
