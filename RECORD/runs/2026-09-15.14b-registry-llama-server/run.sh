#!/bin/sh
# Drives the fifteen prompts of scripts/tasks/tool-call-probe.txt against the
# Ollama registry's 14B GGUF blob (sha256-ac9bc7a69dab3...), loaded directly
# into llama-server rather than through Ollama -- isolates the file from the
# backend, which RECORD/runs/2026-09-15.14b-ollama-registry/ did not (that
# run changed both file and backend at once). Same flags as
# RECORD/runs/2026-09-15.14b-tool-call-probe/run.sh (local HuggingFace GGUF,
# same backend): --ctx-size 8192 --n-gpu-layers 99, --temperature 0 --seed 7.
#
# Run from the repository root, with llama-server already serving the
# registry blob on :8080, e.g.:
#   llama-server -m ~/.ollama/models/blobs/sha256-ac9bc7a69dab38da1c790838955f1293420b55ab555ef6b4615efa1c1507b1ed \
#     --ctx-size 8192 --n-gpu-layers 99 --host 127.0.0.1 --port 8080 \
#     --alias qwen2.5-coder-14b
#
# .jsonl recordings are gitignored project-wide (*.jsonl); the .txt
# transcripts (full stdout, tool markers included) are the committed
# evidence. Reuses the local file's tokenizer.json -- confirmed shared
# across Coder sizes/sources in the-rtx-holds-14b, and the registry blob
# carries the same tokenizer per its own GGUF metadata.
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.14b-registry-llama-server
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
