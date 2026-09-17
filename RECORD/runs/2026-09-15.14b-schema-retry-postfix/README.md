# Evidence: the 14B schema-retry probe, re-run where the instrument can see the retry

The transcripts and verdicts behind the new section appended to
[`../../2026-09-06.a-grammar-for-tool-calls.completed.md`](../../2026-09-06.a-grammar-for-tool-calls.completed.md),
the 14B half of closing "the two real-model schema-retry runs made earlier
today predate [the instrument] fix" — see
[`../2026-09-15.7b-schema-retry-postfix/README.md`](../2026-09-15.7b-schema-retry-postfix/README.md)
for the fix itself and the extraction method. Same fifteen prompts, same
model (`qwen2.5-coder-14b`), same machine, backend and flags as
[`../2026-09-15.14b-schema-retry/`](../2026-09-15.14b-schema-retry/)
(`llama-server`, `--ctx-size 8192 --n-gpu-layers 99 --temperature 0
--seed 7 --constrain schema`) — only the binary changed.

```sh
llama-server -m ~/models/qwen2.5-coder-14b/qwen2.5-coder-14b-instruct-q4_k_m.gguf \
  --ctx-size 8192 --n-gpu-layers 99 --host 127.0.0.1 --port 8080 \
  --alias qwen2.5-coder-14b
./run.sh   # from the repository root
```

## The result

**Zero retries fired — same genuine negative result as the original run,
now confirmed on a binary that could actually record one if it happened.**
The extraction script (see the 7B run's README) found no `step_call` at
`step == 1` in any of the fifteen `.jsonl` files: the 14B's unconstrained
first attempt never scores `Drifted` on this corpus, so `SchemaRetry` never
has anything to retry.

| verdict (`score_call`'s own) | count |
| --- | ---: |
| Parsed | 12 / 15 |
| Drifted | 0 / 15 |
| ContinuedPastFence | 3 / 15 (prompts 04, 09, 14) |
| No call | 0 / 15 |

**Identical composition to
[`../2026-09-15.14b-local-postfix/`](../2026-09-15.14b-local-postfix/)**
(the same model, file, machine, binary and corpus, unconstrained) —
expected, since `--constrain schema` never activates here: with no retry to
spend, the flag costs nothing and changes nothing, the same reading
[`the retry shape, built`](../../2026-09-06.a-grammar-for-tool-calls.completed.md)
gave the original pre-fix 14B run.

Raw artifacts: `stats.json`; `.jsonl` recordings are gitignored
project-wide as usual.
