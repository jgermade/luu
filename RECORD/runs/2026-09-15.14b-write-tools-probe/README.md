# Evidence: edit_file and write_file, probed for tool-call format for the first time

The transcripts and verdicts behind
[`../../2026-09-15.does-the-envelope-collapse-generalize.completed.md`](../../2026-09-15.does-the-envelope-collapse-generalize.completed.md).
Eight independent one-shot sessions — one per line of
[`scripts/tasks/tool-call-probe-writes.txt`](../../../scripts/tasks/tool-call-probe-writes.txt)
— against `qwen2.5-coder-14b` over `llama-server` (`--backend openai`),
machine 4, `--ctx-size 8192 --n-gpu-layers 99 --flash-attn on --temperature 0
--seed 7`, same settings as every other 2026-09-15 probe run.

Unlike the read-only corpus, each prompt runs against its own fresh copy of
a scratch project (`/tmp/luu-write-probe-template`, not committed — three
small files: `greeting.py`, `notes.md`, `config.json`), restored before
every invocation, so no prompt can read a mutation an earlier one made.
Never this repository: both tools under test write.

```sh
./run.sh   # from the repository root, with llama-server already serving
           # the 14B on :8080; writes NN.txt and NN.jsonl per prompt
```

- **`NN.txt`** — the full stdout `luu chat` printed for prompt `NN`.
- **`NN.jsonl`** — the full recording (`--record`), not committed
  (`*.jsonl` is gitignored project-wide).
- **`NN.reply.txt`** — the first model call's raw text, extracted the same
  way as [`../2026-09-15.14b-argv-schema/README.md`](../2026-09-15.14b-argv-schema/README.md)
  (every `token` event of turn 1, stopping before the first `tool_call` or
  at `ended`); not committed, reproducible from `NN.jsonl`.

```sh
# reproduce the verdicts (after ./run.sh and extracting NN.reply.txt, see
# ../2026-09-15.14b-argv-schema/README.md for the extraction script):
cat > crates/agent-core/examples/score_run.rs <<'EOF'
use agent_core::tools::{score_call, Tools};
fn main() {
    let tool_names: Vec<&str> = Tools::standard().names().collect();
    for n in 1..=8 {
        let path = format!("RECORD/runs/2026-09-15.14b-write-tools-probe/{n:02}.reply.txt");
        let text = std::fs::read_to_string(&path).unwrap();
        println!("{n:02} {:?}", score_call(&text, &tool_names));
    }
}
EOF
cargo run -q -p agent-core --example score_run
rm crates/agent-core/examples/score_run.rs
```

| prompt | tool | verdict |
| --- | --- | --- |
| 01 | edit_file | Parsed |
| 02 | edit_file | Parsed |
| 03 | edit_file | Parsed |
| 04 | edit_file | Parsed |
| 05 | write_file | Parsed |
| 06 | write_file | Parsed |
| 07 | write_file | Parsed |
| 08 | write_file | Parsed |

8/8 Parsed, 0/8 Drifted, 0/8 ContinuedPastFence, 0/8 NoCall.
