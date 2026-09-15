#!/bin/sh
# The same fifteen prompts as ../2026-09-15.7b-argv-schema/run.sh, same
# weights (qwen2.5-coder-7b-instruct-q4_k_m.gguf, imported into Ollama with
# a bare `FROM <path>` Modelfile — no custom TEMPLATE, so Ollama reads the
# GGUF's own embedded chat_template, confirmed byte-identical to what
# llama-server reported at /props for the same file). One flag apart from
# that run: --backend ollama instead of --backend openai against
# llama-server. Everything else — model weights, chat template, machine,
# temperature, seed — held constant, to isolate the serving stack itself as
# the only remaining variable in the Drifted-rate gap.
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.7b-ollama-same-gguf
TOKENIZER="$HOME/models/qwen2.5-coder-7b/tokenizer.json"
n=0
grep -v '^#' scripts/tasks/tool-call-probe.txt | grep -v '^\s*$' | while IFS= read -r prompt; do
  n=$((n + 1))
  printf '[%02d] %s\n' "$n" "$prompt" >&2
  "$BIN" chat "$prompt" \
    --backend ollama --ollama-url http://127.0.0.1:11434 --model qwen2.5-coder-7b-local \
    --context-limit 8192 --temperature 0 --seed 7 --tokenizer "$TOKENIZER" \
    --record "$OUT/$(printf '%02d' "$n").jsonl" \
    > "$OUT/$(printf '%02d' "$n").txt"
done
