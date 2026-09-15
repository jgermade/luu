#!/bin/sh
# Re-runs ../2026-09-15.7b-schema-retry/'s probe against the current binary,
# which now carries "the tool-call probe's own instrument can see a retry
# now" (TraceMessage::StepCall reaches the record at a schema retry's own
# call, step > 1 || retry) -- the original run predates that fix and its
# .jsonl recordings have no boundary between a drifted first attempt and its
# retry, which the WIP record's own "still open" list named unresolved.
# Same model, machine, backend, flags: qwen2.5-coder-7b over llama-server,
# --ctx-size 8192 --n-gpu-layers 99 --flash-attn on --temperature 0 --seed 7
# --constrain schema.
#
# Run from the repository root, with llama-server already serving the 7B
# on :8080 (no --tokenizer, same as every other 7B run here). .jsonl
# recordings are gitignored project-wide but are what this run actually
# needs -- see extract.py, run after this script, which reads them for the
# step_call boundary and is not itself committed (scratch, like
# .tmp/extract_first_reply.py before it).
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-15.7b-schema-retry-postfix
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
