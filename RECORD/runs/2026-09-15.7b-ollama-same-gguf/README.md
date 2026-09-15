# Evidence: the local GGUF over Ollama instead of llama-server

The transcripts behind
[`../../2026-09-15.the-drift-was-never-the-backend.completed.md`](../../2026-09-15.the-drift-was-never-the-backend.completed.md).
Same file as
[`../2026-09-15.7b-argv-schema/`](../2026-09-15.7b-argv-schema/)
(`qwen2.5-coder-7b-instruct-q4_k_m.gguf`), imported into Ollama with:

```
FROM /home/joshua/models/qwen2.5-coder-7b/qwen2.5-coder-7b-instruct-q4_k_m.gguf
```

— no `TEMPLATE` directive, so Ollama reads the chat template embedded in
the GGUF itself. `ollama show qwen2.5-coder-7b-local --template` printed
byte-identical Jinja to what `llama-server`'s `/props` reports for the same
file.

```sh
./run.sh   # from the repository root, with `ollama serve` reachable at
           # 127.0.0.1:11434 and `qwen2.5-coder-7b-local` created
```

Same extraction and scoring method as every other 2026-09-15 run. Every one
of the fifteen `NN.reply.txt` files is byte-identical to
[`../2026-09-15.7b-argv-schema/`](../2026-09-15.7b-argv-schema/)'s — same
verdicts, same text, character for character:

6/15 Parsed, 6/15 Drifted, 1/15 ContinuedPastFence, 2/15 NoCall.
