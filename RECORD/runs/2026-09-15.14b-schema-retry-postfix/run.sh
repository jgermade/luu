#!/bin/sh
# Re-runs ../2026-09-15.14b-schema-retry/'s probe against the current binary
# -- see ../2026-09-15.7b-schema-retry-postfix/run.sh's header for why. The
# 14B's unconstrained first attempt already parses 12/15 clean, so this is
# expected to reproduce the same genuine negative result (no Drifted replies
# to retry against), now confirmed on a binary that could actually see one
# if it happened. Same model, machine, backend, flags: qwen2.5-coder-14b
# over llama-server, --ctx-size 8192 --n-gpu-layers 99 --temperature 0
# --seed 7 --constrain schema.
#
# Run from the repository root, with llama-server already serving the local
# 14B GGUF on :8080.
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.14b-schema-retry-postfix
TOKENIZER="$HOME/models/qwen2.5-coder-14b/tokenizer.json"
n=0
grep -v '^#' scripts/tasks/tool-call-probe.txt | grep -v '^\s*$' | while IFS= read -r prompt; do
  n=$((n + 1))
  printf '[%02d] %s\n' "$n" "$prompt" >&2
  "$BIN" chat "$prompt" \
    --backend openai --openai-url http://127.0.0.1:8080/v1 --model qwen2.5-coder-14b \
    --context-limit 8192 --temperature 0 --seed 7 --tokenizer "$TOKENIZER" \
    --constrain schema \
    --record "$OUT/$(printf '%02d' "$n").jsonl" \
    > "$OUT/$(printf '%02d' "$n").txt" 2>&1
done
