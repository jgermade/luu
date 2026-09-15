#!/bin/sh
# Drives the fifteen prompts of scripts/tasks/tool-call-probe.txt against a
# real model, one independent chat session per prompt (no shared history —
# the corpus is fifteen one-shot questions, not a conversation). Machine 4,
# qwen2.5-coder-14b over llama-server (--backend openai), same --ctx-size
# 8192 / --n-gpu-layers 99 / --flash-attn on as the baseline arm of
# RECORD/2026-09-08.the-14b-context-ceiling.completed.md, --temperature 0
# --seed 7 same as every other tool-call-probe run for comparability.
#
# Run from the repository root, with llama-server already serving the 14B
# on :8080. .jsonl recordings are gitignored project-wide (*.jsonl); the
# .txt transcripts (full stdout, tool markers included) are the committed
# evidence.
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.14b-tool-call-probe
TOKENIZER="$HOME/models/qwen2.5-coder-14b/tokenizer.json"
n=0
grep -v '^#' scripts/tasks/tool-call-probe.txt | grep -v '^\s*$' | while IFS= read -r prompt; do
  n=$((n + 1))
  printf '[%02d] %s\n' "$n" "$prompt" >&2
  "$BIN" chat "$prompt" \
    --backend openai --openai-url http://127.0.0.1:8080/v1 --model qwen2.5-coder-14b \
    --context-limit 8192 --temperature 0 --seed 7 --tokenizer "$TOKENIZER" \
    --record "$OUT/$(printf '%02d' "$n").jsonl" \
    > "$OUT/$(printf '%02d' "$n").txt"
done
