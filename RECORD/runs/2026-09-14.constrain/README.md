# Evidence: `--constrain grammar` and `--constrain schema`, real CLI, 2026-09-14

The transcripts behind
[`../../2026-09-14.what-constrain-does.completed.md`](../../2026-09-14.what-constrain-does.completed.md).
Three arms of the fifteen-prompt tool-call probe, this time through `luu
chat` itself (not a hand-built HTTP request) against a standalone
`llama-server` — see `run.sh` for the exact commands and why `schema`
needed a second, larger server.

- **`off.NN.txt` / `grammar.NN.txt` / `schema.NN.txt`** — full stdout per
  prompt: provider line, caveats, the model's own text, the CLI's `[k] →` /
  `[k] ←` tool markers, and the `[reason] … prompt / … completion` footer.
- **`off.NN.reply.txt` / `grammar.NN.reply.txt`** — the first model call's
  raw text only, extracted from the matching `.jsonl` recording the same
  way [`the-tool-call-probe-run`](../2026-09-14.tool-call-probe/README.md)
  did (`token` events up to the first `tool_call` or `ended`). **No
  `schema.NN.reply.txt`**: `--constrain schema` never produces a scoreable
  first-call text separate from an eighth, tool-limited call — see below.
- **`.jsonl`** — not committed, gitignored project-wide.
- **`stats.json`** — `score_call`'s verdict for `off` and `grammar`
  (identical, prompt for prompt, to the hand-built-request run this
  replicates: 3/2/2/8 unconstrained, 7/2/0/6 grammar-constrained), and the
  `schema` arm's own number, which isn't a verdict tally at all: **15 of 15
  prompts ended `[tool limit reached]`, having answered none of them.**

```sh
./run.sh   # from the repository root; llama-server already listening
```
