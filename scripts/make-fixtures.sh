#!/usr/bin/env bash
# Records the sessions the static deploy replays.
#
# They are real runs of the real protocol against the mock backend, not
# hand-written JSON — a fixture that drifts from the format is worse than none,
# and this one cannot: it is produced by the same code that serves it.
set -euo pipefail

luu="${1:?usage: make-fixtures.sh <path-to-luu> <out-dir>}"
out="${2:?usage: make-fixtures.sh <path-to-luu> <out-dir>}"
mkdir -p "$out"

"$luu" chat "What does the context manager do?" \
  --mock-delay-ms 45 --context-limit 8192 --record "$out/completed.jsonl" >/dev/null

"$luu" chat "Explain the sandbox policy" \
  --mock-delay-ms 45 --cancel-after-ms 700 --context-limit 8192 \
  --record "$out/cancelled.jsonl" >/dev/null

# A backend that is not there: the failure path, without depending on one.
"$luu" chat "Anything" --backend ollama --ollama-url http://127.0.0.1:1 \
  --record "$out/failed.jsonl" >/dev/null 2>&1 || true

cat > "$out/index.json" <<'JSON'
[
  { "name": "Completed turn",  "file": "./fixtures/completed.jsonl", "about": "A turn that ran to a stop, with usage reported." },
  { "name": "Cancelled turn",  "file": "./fixtures/cancelled.jsonl", "about": "Cancelled mid-generation: partial text, and no usage to plot." },
  { "name": "Backend failure", "file": "./fixtures/failed.jsonl",    "about": "The backend was unreachable." }
]
JSON

echo "fixtures written to $out:"
wc -l "$out"/*.jsonl
