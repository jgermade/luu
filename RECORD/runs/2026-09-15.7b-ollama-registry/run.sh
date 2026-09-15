#!/bin/sh
# The same fifteen prompts, against Ollama's own registry pull of
# `qwen2.5-coder:7b` — the exact model name and source
# the-tool-call-probe-run.completed.md used on machine 1 — but run here on
# machine 4, so weights+template match the original discovery and only the
# machine differs. Compare against ../2026-09-15.7b-ollama-same-gguf/ (same
# machine and backend, different weights+template) to separate "which
# weights/template" from "which machine" in the Drifted-rate gap.
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.7b-ollama-registry
n=0
grep -v '^#' scripts/tasks/tool-call-probe.txt | grep -v '^\s*$' | while IFS= read -r prompt; do
  n=$((n + 1))
  printf '[%02d] %s\n' "$n" "$prompt" >&2
  "$BIN" chat "$prompt" \
    --backend ollama --ollama-url http://127.0.0.1:11434 --model qwen2.5-coder:7b \
    --context-limit 8192 --temperature 0 --seed 7 \
    --record "$OUT/$(printf '%02d' "$n").jsonl" \
    > "$OUT/$(printf '%02d' "$n").txt"
done
