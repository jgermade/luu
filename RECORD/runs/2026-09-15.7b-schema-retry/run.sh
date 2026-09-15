#!/bin/sh
# The same probe as ../2026-09-15.14b-schema-retry/run.sh, against
# qwen2.5-coder-7b over llama-server instead — the model the retry actually
# has something to rescue on: the 14B's unconstrained first attempt already
# parses 12/15 clean, so `--constrain schema` never has a Drifted reply to
# retry there. The 7B's 6/15 Drifted (runs/2026-09-15.7b-argv-schema/) is
# the corpus this run tests the retry against.
#
# Run from the repository root, with llama-server already serving the 7B
# on :8080 (no --tokenizer: this project has never had one for the 7B GGUF
# machine 4 uses, same as every other 7B run here). .jsonl recordings are
# gitignored project-wide (*.jsonl); the .txt transcripts are the committed
# evidence.
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.7b-schema-retry
n=0
grep -v '^#' scripts/tasks/tool-call-probe.txt | grep -v '^\s*$' | while IFS= read -r prompt; do
  n=$((n + 1))
  printf '[%02d] %s\n' "$n" "$prompt" >&2
  "$BIN" chat "$prompt" \
    --backend openai --openai-url http://127.0.0.1:8080/v1 --model qwen2.5-coder-7b \
    --context-limit 8192 --temperature 0 --seed 7 \
    --constrain schema \
    --record "$OUT/$(printf '%02d' "$n").jsonl" \
    > "$OUT/$(printf '%02d' "$n").txt" 2>&1
done
