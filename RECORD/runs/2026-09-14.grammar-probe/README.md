# Evidence: the grammar probe of 2026-09-14

The transcripts behind
[`../../2026-09-14.the-alternation-that-was-not-one.completed.md`](../../2026-09-14.the-alternation-that-was-not-one.completed.md).

- **`tool.gbnf`** — `agent_core::grammar::compile(&Tools::standard())`'s exact
  output, dumped from a throwaway `cargo run --example` (not committed) the
  way [`the-tool-call-probe`](../2026-09-14.tool-call-probe/README.md)'s
  `score_run.rs` was.
- **`NN.reply.txt`** — the fifteen prompts of
  [`scripts/tasks/tool-call-probe.txt`](../../../scripts/tasks/tool-call-probe.txt),
  each sent as an *independent* request (fresh `system` + one `user`
  message, no shared history — same corpus discipline as the first run)
  straight to a standalone `llama-server` (the same binary and the same
  `qwen2.5-coder:7b` weights `the-bare-grammar-field` used, bypassing
  Ollama, whose own `/v1` does not honour the field), with `tool.gbnf`
  attached as `grammar`, `temperature: 0`, `seed: 7`, same as every other
  destination on this machine.
- **`stats.json`** — `score_call`'s verdict for each, plus whether the byte
  a reply is byte-identical to
  [`../2026-09-14.tool-call-probe/NN.reply.txt`](../2026-09-14.tool-call-probe/)
  — the same fifteen prompts, unconstrained, against Ollama. All fifteen are.

```sh
# reproduce (llama-server already running on :8090 with the same weights —
# see the-bare-grammar-field.completed.md for how that was started):
diff RECORD/runs/2026-09-14.grammar-probe/01.reply.txt \
     RECORD/runs/2026-09-14.tool-call-probe/01.reply.txt   # empty: identical
```
