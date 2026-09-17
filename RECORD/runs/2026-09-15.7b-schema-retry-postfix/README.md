# Evidence: the 7B schema-retry probe, re-run where the instrument can see the retry

The transcripts and mechanically-scored verdicts behind the new section
appended to
[`../../2026-09-06.a-grammar-for-tool-calls.completed.md`](../../2026-09-06.a-grammar-for-tool-calls.completed.md)
that closes "the two real-model schema-retry runs made earlier today predate
[the instrument] fix". Same fifteen prompts, same model
(`qwen2.5-coder-7b`), same machine, backend and flags as
[`../2026-09-15.7b-schema-retry/`](../2026-09-15.7b-schema-retry/)
(`llama-server`, `--ctx-size 8192 --n-gpu-layers 99 --flash-attn on
--temperature 0 --seed 7 --constrain schema`) — only the binary changed, to
one built after "the tool-call probe's own instrument can see a retry now"
(`TraceMessage::StepCall` reaches the `--record` file for a schema retry's
own call, `step == 1 && retry`).

```sh
llama-server -m ~/models/qwen2.5-coder-7b/qwen2.5-coder-7b-instruct-q4_k_m.gguf \
  --ctx-size 8192 --n-gpu-layers 99 --flash-attn on --host 127.0.0.1 --port 8080 \
  --alias qwen2.5-coder-7b
./run.sh   # from the repository root; writes NN.txt and NN.jsonl per prompt
```

## Extraction: using the step_call boundary this run exists to test

`.jsonl` records every streamed reply as a run of `{"channel":"protocol",
"message":{"type":"token",...}}` lines with nothing between one model call's
tokens and the next — the "concatenated reply" problem "the retry shape,
built" section found by hand. What changed: a schema retry's own call now
emits `{"channel":"trace","message":{"type":"step_call","step":1,...}}`
immediately before its tokens start, and `step == 1` there is unambiguous —
the ordinary tool-loop proxy (`step > 1`) cannot produce it any other way, so
any `step_call` seen at `step == 1` marks exactly one thing: a retry began.

A scratch script (not committed, regenerated per run like
`.tmp/extract_first_reply.py` before it) walks each prompt's `.jsonl` in
order: `token` text accumulates into a "first attempt" buffer; the first
`step == 1` `step_call` switches accumulation to a "retry" buffer; a
`tool_call`/`tool_result`/`ended` line closes whichever buffer is open (so
neither one absorbs a later tool-loop step's own continuation reply, a bug
an earlier version of this script had and this run's own `02.retry.reply.txt`
would have been visibly wrong under — it ends cleanly at the closing `}` of
the retry's own JSON, not run on into "The top-level entries of ... are:").
Writes `NN.reply.txt` (first attempt, same convention as every prior
tool-call-probe run) and, only where a retry actually fired,
`NN.retry.reply.txt`.

## The result

Six prompts retried — **02, 04, 05, 07, 09, 12, the identical set** "the
retry shape, built" section named by hand-reading the old concatenated
`.jsonl`. Scored with the real `agent_core::tools::score_call` on each
bucket separately (reproduction: same throwaway `cargo run --example`
pattern as
[`../2026-09-15.14b-tool-call-probe/README.md`](../2026-09-15.14b-tool-call-probe/README.md),
pointed at both `NN.reply.txt` and, where present, `NN.retry.reply.txt`).

| prompt | first attempt | retry | final |
| --- | --- | --- | --- |
| 01 | NoCall | — (declined, not retried) | NoCall |
| 02 | Drifted | Parsed | Parsed |
| 03 | ContinuedPastFence | — (not `Drifted`, not retried) | ContinuedPastFence |
| 04 | Drifted | Parsed | Parsed |
| 05 | Drifted | Parsed | Parsed |
| 06 | Parsed | — | Parsed |
| 07 | Drifted | Parsed | Parsed |
| 08 | Parsed | — | Parsed |
| 09 | Drifted | Parsed | Parsed |
| 10 | NoCall | — (declined, not retried) | NoCall |
| 11 | Parsed | — | Parsed |
| 12 | Drifted | Parsed | Parsed |
| 13 | Parsed | — | Parsed |
| 14 | Parsed | — | Parsed |
| 15 | Parsed | — | Parsed |

**6/6 `Drifted` retries recovered to `Parsed`, zero retries that themselves
failed to parse** — matching "the retry shape, built" section's hand-read
6/6 exactly, now produced mechanically instead of by reading transcripts.
**Final: 12/15 `Parsed`, 2/15 `NoCall` (both genuine declines, correctly
untouched — a schema forces a call and neither `01` nor `10` was forced
into one), 1/15 `ContinuedPastFence`.**

Prompt 03's `ContinuedPastFence` (correct `read_file` call, one trailing
sentence after the fence) is not a retry candidate by design — `score_call`
only calls a reply `Drifted`, and only `Drifted` retries — but `03.txt`
shows the tool ran anyway (`parse_call` tolerates the trailing text the same
way it did for the 14B's prompts 4 and 14 elsewhere in this record) and the
turn ended `[stop]`, not `[tool limit reached]`. **13/15 prompts actually
executed a tool this run** — the same 13/15 "the retry shape, built" section
reported from execution logs by hand, now cross-checked against the
mechanical per-call verdict instead of standing alone.

Raw artifacts: `stats.json` (final verdict per prompt, same shape as every
other tool-call-probe run) plus the `.reply.txt` / `.retry.reply.txt` pairs
above; `.jsonl` recordings are gitignored project-wide as usual.
