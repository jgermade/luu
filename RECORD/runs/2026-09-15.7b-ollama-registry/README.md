# Evidence: Ollama's own qwen2.5-coder:7b, pulled from the registry

The transcripts behind
[`../../2026-09-15.the-drift-was-never-the-backend.completed.md`](../../2026-09-15.the-drift-was-never-the-backend.completed.md).
`ollama pull qwen2.5-coder:7b` — the exact model name
[`the-tool-call-probe-run`](../../2026-09-14.the-tool-call-probe-run.completed.md)
used on machine 1 — run here on machine 4 instead, same fifteen prompts,
same `--context-limit 8192 --temperature 0 --seed 7`.

`sha256sum` of the blob `ollama show --modelfile` points `FROM` at:
`60e05f2100071479f596b964f89f510f057ce397ea22f2833a0cfe029bfc2463` — not the
same file as
[`../2026-09-15.7b-ollama-same-gguf/`](../2026-09-15.7b-ollama-same-gguf/)'s
`509287f78cb4d4cf6b3843734733b914b2c158e43e22a7f4bf5e963800894d3c`, despite
both being labeled Q4_K_M at 4.7 GB. `ollama show qwen2.5-coder:7b
--template` is Ollama's own hand-authored Go template, not the GGUF's
embedded Jinja.

```sh
./run.sh   # from the repository root, with `ollama serve` reachable at
           # 127.0.0.1:11434 and `qwen2.5-coder:7b` pulled
```

Same extraction and scoring method as every other 2026-09-15 run.

| prompt | verdict |
| --- | --- |
| 01 | NoCall |
| 02 | Drifted |
| 03 | ContinuedPastFence |
| 04 | Drifted |
| 05 | NoCall |
| 06 | NoCall |
| 07 | NoCall |
| 08 | Parsed |
| 09 | Drifted |
| 10 | NoCall |
| 11 | Parsed |
| 12 | Drifted |
| 13 | ContinuedPastFence |
| 14 | ContinuedPastFence |
| 15 | Parsed |

3/15 Parsed, 4/15 Drifted, 3/15 ContinuedPastFence, 5/15 NoCall — 20%
clean-call, matching
[`the-tool-call-probe-run`](../../2026-09-14.the-tool-call-probe-run.completed.md)'s
3/15 = 20% on machine 1, a different day and a different Ollama version.
