#!/bin/sh
# The fifteen tool-call-probe prompts against qwen2.5-coder-14b over
# llama-server, `--constrain schema` — now the retry-only arm
# `RECORD/2026-09-06.a-grammar-for-tool-calls.completed.md` always meant it to be
# (agent_core::agent::SchemaRetry), not the always-on `response_format` that
# `RECORD/2026-09-14.what-constrain-does.completed.md` found never answered
# a single prompt. Same machine, model and settings as
# runs/2026-09-15.14b-argv-schema/ — one flag apart.
#
# Run from the repository root, with llama-server already serving the 14B
# on :8080. .jsonl recordings are gitignored project-wide (*.jsonl); the
# .txt transcripts are the committed evidence.
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.14b-schema-retry
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
