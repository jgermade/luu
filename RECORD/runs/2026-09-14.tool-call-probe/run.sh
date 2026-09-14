#!/bin/sh
# Drives the fifteen prompts of scripts/tasks/tool-call-probe.txt against a
# real model, one independent chat session per prompt (no shared history —
# the corpus is fifteen one-shot questions, not a conversation). Machine 1,
# qwen2.5-coder:7b over Ollama, same context limit / temperature / seed as
# the other machine-1 runs recorded 2026-09-14 for comparability.
#
# Run from the repository root. .jsonl recordings are gitignored project-wide
# (*.jsonl); the .txt transcripts (full stdout, tool markers included) are
# the committed evidence.
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-14.tool-call-probe
n=0
grep -v '^#' scripts/tasks/tool-call-probe.txt | grep -v '^\s*$' | while IFS= read -r prompt; do
  n=$((n + 1))
  printf '[%02d] %s\n' "$n" "$prompt" >&2
  "$BIN" chat "$prompt" \
    --backend ollama --model qwen2.5-coder:7b \
    --context-limit 8192 --temperature 0 --seed 7 \
    --record "$OUT/$(printf '%02d' "$n").jsonl" \
    > "$OUT/$(printf '%02d' "$n").txt"
done
