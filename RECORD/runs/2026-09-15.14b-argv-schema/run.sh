#!/bin/sh
# Re-runs the fifteen prompts of scripts/tasks/tool-call-probe.txt against
# the same model, machine and settings as
# RECORD/runs/2026-09-15.14b-tool-call-probe/run.sh, after run_command's
# schema changed from {command, args, cwd, timeout_ms} to {argv, cwd,
# timeout_ms} — see the 2026-09-15 "a schema restructuring, tried" section of
# RECORD/2026-09-06.a-grammar-for-tool-calls.WIP.md. Same llama-server
# process, same --ctx-size 8192 / --n-gpu-layers 99 / --flash-attn on,
# --temperature 0 --seed 7. One flag apart from the prior run: the binary,
# rebuilt with the new schema, and nothing else.
#
# Run from the repository root, with llama-server already serving the 14B on
# :8080. .jsonl recordings are gitignored project-wide (*.jsonl); the .txt
# transcripts are the committed evidence.
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.14b-argv-schema
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
