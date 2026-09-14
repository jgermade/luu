#!/bin/sh
# Three arms of the tool-call probe, through the real CLI end to end
# (`luu chat --constrain …`), against a standalone llama-server — the same
# binary and weights RECORD/2026-09-14.the-bare-grammar-field.completed.md
# used. Run from the repository root.
#
# `off` and `grammar` were run against `llama-server -c 4096` and finished
# inside that window on every prompt. `schema` was tried there first and
# failed at prompt 1 with a 400 (context exceeded) — `--constrain schema`
# has no "just answer" branch, so the model calls a tool, gets a result,
# and is *still* not allowed to answer: it calls again, and again, until
# either the window or `--max-tool-steps` (default 8) stops it. Restarted
# at `-c 16384` (4x) to separate that from the ordinary tool-limit path —
# it did not help; every prompt still exhausts the 8-call budget having
# answered nothing. See stats.json and the individual .txt transcripts.
set -eu
cd "$(dirname "$0")/../../.."
BIN=target/release/luu
OUT=RECORD/runs/2026-09-14.constrain
URL=http://127.0.0.1:8090/v1

run_arm() {
  arm="$1"
  limit="$2"
  shift 2
  n=0
  grep -v '^#' scripts/tasks/tool-call-probe.txt | grep -v '^\s*$' | while IFS= read -r prompt; do
    n=$((n + 1))
    printf '[%s %02d] %s\n' "$arm" "$n" "$prompt" >&2
    "$BIN" chat "$prompt" \
      --backend openai --openai-url "$URL" --model qwen2.5-coder:7b \
      --context-limit "$limit" --temperature 0 --seed 7 \
      --record "$OUT/$arm.$(printf '%02d' "$n").jsonl" \
      "$@" \
      > "$OUT/$arm.$(printf '%02d' "$n").txt" 2>&1
  done
}

# llama-server -m <the qwen2.5-coder:7b blob> --port 8090 -c 4096 --no-webui
run_arm off 4096
run_arm grammar 4096 --constrain grammar

# restart: llama-server ... -c 16384 --no-webui
run_arm schema 16384 --constrain schema
