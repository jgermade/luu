# Evidence: the local 14B GGUF, re-run against the post-argv-fix binary

The transcripts behind the new section appended to
[`../../2026-09-06.a-grammar-for-tool-calls.completed.md`](../../2026-09-06.a-grammar-for-tool-calls.completed.md)
that separates a confound
[`../2026-09-15.14b-registry-llama-server/`](../2026-09-15.14b-registry-llama-server/)
found: the original local-file number
([`../2026-09-15.14b-tool-call-probe/`](../2026-09-15.14b-tool-call-probe/),
12/15 = 80%, one `NoCall` at prompt 9 from a `run_command` envelope
collapse) was run at commit `714eca7` (11:11), **before** `82229cc` (11:38,
"run_command's argv: one array instead of command+args closes the envelope
collapse") — while every registry-file run this record compares it against
postdates that fix. This re-runs the exact same local GGUF
(`~/models/qwen2.5-coder-14b/qwen2.5-coder-14b-instruct-q4_k_m.gguf`) against
the current binary, same flags: `--ctx-size 8192 --n-gpu-layers 99`
(`llama-server`), `--temperature 0 --seed 7`.

```sh
llama-server -m ~/models/qwen2.5-coder-14b/qwen2.5-coder-14b-instruct-q4_k_m.gguf \
  --ctx-size 8192 --n-gpu-layers 99 --host 127.0.0.1 --port 8080 \
  --alias qwen2.5-coder-14b
./run.sh   # from the repository root
```

Same extraction and scoring method as every other tool-call-probe run (see
[`../2026-09-15.14b-tool-call-probe/README.md`](../2026-09-15.14b-tool-call-probe/README.md)),
with one addition: `[tool limit reached]` (prompts that hit
`--max-tool-steps` without a `[stop]`, e.g. prompt 9 here) is treated as a
cut point exactly like `[stop]` — the extraction only needs the first model
call either way.

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

12/15 Parsed, 0/15 Drifted, 3/15 ContinuedPastFence, 0/15 NoCall — 80%
clean-call, the **same total** as the pre-fix run, but a different
composition: prompt 9's `NoCall` is gone (`09.txt` shows a correctly wrapped
`{"name": "run_command", "arguments": {"argv": [...]}}` on every one of its
eight attempts — the argv fix holds on the local file too, not just the
registry one), replaced by a new `ContinuedPastFence` (the first reply wraps
correctly, then adds one trailing sentence after the closing fence — the
same failure mode prompts 4 and 14 already showed, now also on 9).

**The registry-vs-local gap survives controlling for the schema fix.** With
both files now measured on the same post-fix binary, same backend
(`llama-server`), same flags: local 80% (this run) against registry 93%
([`../2026-09-15.14b-registry-llama-server/`](../2026-09-15.14b-registry-llama-server/)).
The original 80%-vs-93% comparison in the WIP record's "two-GGUF check"
section reached the right headline number by an accident of timing — it
never actually held the schema fixed — but the gap it reported was real, not
an artifact of the fix: a genuinely cleaner comparison reproduces it.
