# Evidence: Ollama's own qwen2.5-coder:14b, pulled from the registry

The transcripts behind the 2026-09-15 section appended to
[`../../2026-09-06.a-grammar-for-tool-calls.completed.md`](../../2026-09-06.a-grammar-for-tool-calls.completed.md)
that closes "whether the 14B has the same two-GGUF problem" — mirrors
[`../2026-09-15.7b-ollama-registry/`](../2026-09-15.7b-ollama-registry/), one
size up. `ollama pull qwen2.5-coder:14b`, machine 4, the same fifteen
prompts, the same `--context-limit 8192 --temperature 0 --seed 7` as every
other tool-call-probe run.

`sha256sum` of the blob `ollama show --modelfile` points `FROM` at:
`ac9bc7a69dab38da1c790838955f1293420b55ab555ef6b4615efa1c1507b1ed` — not the
same file as
[`~/models/qwen2.5-coder-14b/qwen2.5-coder-14b-instruct-q4_k_m.gguf`](../2026-09-15.14b-tool-call-probe/README.md)'s
`c1e659736d89ac1065fb495330fb824d94001974a4bfa78e7270e43476a8d940`, despite
both being labeled Q4_K_M at 9.0 GB — the same two-files-one-name pattern the
7B showed.

```sh
./run.sh   # from the repository root, with `ollama serve` reachable at
           # 127.0.0.1:11434 and `qwen2.5-coder:14b` pulled
```

Same extraction and scoring method as every other 2026-09-15 run (see
[`../2026-09-15.14b-tool-call-probe/README.md`](../2026-09-15.14b-tool-call-probe/README.md)
for the reproduction steps this run reused verbatim, `2026-09-15.14b-ollama-registry`
substituted for the output directory).

| prompt | verdict |
| --- | --- |
| 01 | Parsed |
| 02 | Parsed |
| 03 | Parsed |
| 04 | Parsed |
| 05 | Parsed |
| 06 | Parsed |
| 07 | Parsed |
| 08 | Parsed |
| 09 | Parsed |
| 10 | Parsed |
| 11 | Parsed |
| 12 | Parsed |
| 13 | Parsed |
| 14 | ContinuedPastFence |
| 15 | Parsed |

14/15 Parsed, 0/15 Drifted, 1/15 ContinuedPastFence, 0/15 NoCall — 93%
clean-call, **higher** than
[`the-same-corpus-same-scorer 14B number`](../../2026-09-06.a-grammar-for-tool-calls.completed.md)'s
12/15 = 80% on the local HuggingFace GGUF over `llama-server`, same machine,
same corpus, same scorer. The one miss (prompt 9's `run_command` envelope,
`NoCall`) that appeared on the local GGUF did not reproduce here — prompt 9
parsed cleanly, `argv` and all — and no `run_command` call in this run
collapsed the envelope.
