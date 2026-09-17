#!/bin/sh
# Re-runs the fifteen prompts of scripts/tasks/tool-call-probe.txt against the
# SAME local HuggingFace 14B GGUF as
# RECORD/runs/2026-09-15.14b-tool-call-probe/ (12/15 = 80%, prompt 9 NoCall
# on a run_command envelope collapse), but with the current binary --
# i.e. after commit 82229cc ("run_command's argv: one array instead of
# command+args closes the envelope collapse"), which that original run
# predates (714eca7, 11:11 vs 82229cc, 11:38, same day). Exists to separate
# two confounds that RECORD/2026-09-06.a-grammar-for-tool-calls.completed.md's
# "14B's two-GGUF check" section did not: the registry-vs-local file
# difference, and the pre/post argv-schema-fix difference, since the
# registry runs (both this one and 2026-09-15.14b-ollama-registry) were all
# run after the fix landed while the original local-file number was not.
#
# Same flags as every other tool-call-probe run: --ctx-size 8192
# --n-gpu-layers 99 (llama-server), --temperature 0 --seed 7.
#
# Run from the repository root, with llama-server already serving the local
# 14B GGUF on :8080:
#   llama-server -m ~/models/qwen2.5-coder-14b/qwen2.5-coder-14b-instruct-q4_k_m.gguf \
#     --ctx-size 8192 --n-gpu-layers 99 --host 127.0.0.1 --port 8080 \
#     --alias qwen2.5-coder-14b
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.14b-local-postfix
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
