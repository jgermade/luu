# Evidence: the argv schema against the 7B, machine 4 / llama-server

The transcripts and verdicts behind the "does it hold on the 7B" section
appended to
[`../../2026-09-06.a-grammar-for-tool-calls.completed.md`](../../2026-09-06.a-grammar-for-tool-calls.completed.md).
The official fifteen prompts,
[`scripts/tasks/tool-call-probe.txt`](../../../scripts/tasks/tool-call-probe.txt),
against `qwen2.5-coder-7b` (`qwen2.5-coder-7b-instruct-q4_k_m.gguf`) over
`llama-server` (`--backend openai`), machine 4, `--ctx-size 8192
--n-gpu-layers 99 --flash-attn on --temperature 0 --seed 7` — the argv
schema's own settings, everything but the model unchanged from
[`../2026-09-15.14b-argv-schema/`](../2026-09-15.14b-argv-schema/).

**Not a repeat of the original 7B discovery run.**
[`the-tool-call-probe-run`](../../2026-09-14.the-tool-call-probe-run.completed.md)
ran this same 7B over **Ollama** on **machine 1**. This run changes both the
machine and the backend at once relative to that one, which the project's
own "one flag apart" discipline would normally rule out — done anyway
because machine 4 has no Ollama installed, and a same-machine,
same-backend 7B number (comparable to every other argv-schema run today)
was worth more than no 7B number at all. Read the comparison to the
original 3/15 accordingly.

```sh
./run.sh   # from the repository root, with llama-server already serving
           # the 7B on :8080; writes NN.txt and NN.jsonl per prompt
```

Same extraction and scoring method as every other 2026-09-15 run (see
[`../2026-09-15.14b-argv-schema/README.md`](../2026-09-15.14b-argv-schema/README.md)).

| prompt | verdict |
| --- | --- |
| 01 | NoCall |
| 02 | Drifted |
| 03 | ContinuedPastFence |
| 04 | Drifted |
| 05 | Drifted |
| 06 | Parsed |
| 07 | Drifted |
| 08 | Parsed |
| 09 | Drifted |
| 10 | NoCall |
| 11 | Parsed |
| 12 | Drifted |
| 13 | Parsed |
| 14 | Parsed |
| 15 | Parsed |

6/15 Parsed, 6/15 Drifted, 1/15 ContinuedPastFence, 2/15 NoCall.
**Zero envelope collapses** — every `run_command` prompt (4, 9, 14) that did
attempt a call sent a correctly-shaped `{"argv": [...]}` payload, whether or
not it was wrapped or fenced correctly (04 and 09 fenced it as
` ```run_command ` instead of ` ```tool ` — `Drifted`, not a collapse; 14
wrapped and fenced correctly — `Parsed`).
