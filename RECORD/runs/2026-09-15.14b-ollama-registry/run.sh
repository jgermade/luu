#!/bin/sh
# The same fifteen prompts, against Ollama's own registry pull of
# `qwen2.5-coder:14b` — mirrors
# RECORD/runs/2026-09-15.7b-ollama-registry/run.sh, but for the 14B, to
# check whether the two-GGUF problem that run found for the 7B (Ollama's
# registry build is a different file from the local HuggingFace GGUF,
# despite the same nominal name and quant label) also holds one size up.
# Compare against
# RECORD/runs/2026-09-15.14b-tool-call-probe/ (local GGUF, llama-server,
# same machine).
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.14b-ollama-registry
n=0
grep -v '^#' scripts/tasks/tool-call-probe.txt | grep -v '^\s*$' | while IFS= read -r prompt; do
  n=$((n + 1))
  printf '[%02d] %s\n' "$n" "$prompt" >&2
  "$BIN" chat "$prompt" \
    --backend ollama --ollama-url http://127.0.0.1:11434 --model qwen2.5-coder:14b \
    --context-limit 8192 --temperature 0 --seed 7 \
    --record "$OUT/$(printf '%02d' "$n").jsonl" \
    > "$OUT/$(printf '%02d' "$n").txt"
done
