#!/bin/sh
# The same fifteen prompts as ../2026-09-15.14b-argv-schema/run.sh, against
# qwen2.5-coder-7b instead of the 14b — the model the envelope collapse was
# first found on (the-tool-call-probe-run.completed.md, machine 1, Ollama).
# Not a repeat of that exact combination: this is machine 4, llama-server,
# not machine 1/Ollama, so a clean result here does not by itself confirm
# the original 7B/Ollama pairing is fixed. What it answers is narrower and
# still real: whether the argv schema holds on the smaller model at all,
# same machine and backend as every argv-schema run so far.
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.7b-argv-schema
TOKENIZER="$HOME/models/qwen2.5-coder-7b/tokenizer.json"
n=0
grep -v '^#' scripts/tasks/tool-call-probe.txt | grep -v '^\s*$' | while IFS= read -r prompt; do
  n=$((n + 1))
  printf '[%02d] %s\n' "$n" "$prompt" >&2
  "$BIN" chat "$prompt" \
    --backend openai --openai-url http://127.0.0.1:8080/v1 --model qwen2.5-coder-7b \
    --context-limit 8192 --temperature 0 --seed 7 --tokenizer "$TOKENIZER" \
    --record "$OUT/$(printf '%02d' "$n").jsonl" \
    > "$OUT/$(printf '%02d' "$n").txt"
done
