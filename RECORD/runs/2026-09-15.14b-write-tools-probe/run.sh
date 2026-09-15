#!/bin/sh
# Drives the eight prompts of scripts/tasks/tool-call-probe-writes.txt
# against qwen2.5-coder-14b over llama-server, machine 4, the same settings
# as every other 2026-09-15 probe run (--ctx-size 8192 --n-gpu-layers 99
# --flash-attn on, --temperature 0 --seed 7). Each prompt runs against a
# fresh copy of the scratch project restored from a template, so a mutation
# one prompt makes cannot be read by another — the write-tool analogue of
# "no shared history" the read-only corpus gets for free.
#
# Run from the repository root, with llama-server already serving the 14B
# on :8080. .jsonl recordings are gitignored project-wide; .txt transcripts
# are the committed evidence.
set -eu
cd "$(dirname "$0")/../../.."
BIN=$(pwd)/target/release/luu
OUT=$(pwd)/RECORD/runs/2026-09-15.14b-write-tools-probe
TOKENIZER="$HOME/models/qwen2.5-coder-14b/tokenizer.json"
TEMPLATE=/tmp/luu-write-probe-template
SCRATCH=/tmp/luu-write-probe
n=0
grep -v '^#' scripts/tasks/tool-call-probe-writes.txt | grep -v '^\s*$' | while IFS= read -r prompt; do
  n=$((n + 1))
  rm -rf "$SCRATCH"
  cp -r "$TEMPLATE" "$SCRATCH"
  printf '[%02d] %s\n' "$n" "$prompt" >&2
  (cd "$SCRATCH" && "$BIN" chat "$prompt" \
    --allow-write . \
    --backend openai --openai-url http://127.0.0.1:8080/v1 --model qwen2.5-coder-14b \
    --context-limit 8192 --temperature 0 --seed 7 --tokenizer "$TOKENIZER" \
    --record "$OUT/$(printf '%02d' "$n").jsonl") \
    > "$OUT/$(printf '%02d' "$n").txt"
done
