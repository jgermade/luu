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
  --mock-delay-ms 45 --context-limit 8192 --record "$out/completed-turn.jsonl" >/dev/null

"$luu" chat "Explain the sandbox policy" \
  --mock-delay-ms 45 --cancel-after-ms 700 --context-limit 8192 \
  --record "$out/cancelled-turn.jsonl" >/dev/null

# A backend that is not there: the failure path, without depending on one.
"$luu" chat "Anything" --backend ollama --ollama-url http://127.0.0.1:1 \
  --record "$out/backend-failure.jsonl" >/dev/null 2>&1 || true

# The static twin of the read API, folded from these same recordings by the
# same code the live server uses. `$out` is <site>/fixtures, so the API tree
# goes beside it and the recordings are reached as ./fixtures/<name>.jsonl.
# Named in order, not globbed: the first one is what the page plays on load,
# and a glob would lead with whichever name sorts first.
"$luu" export \
  "$out/completed-turn.jsonl" \
  "$out/cancelled-turn.jsonl" \
  "$out/backend-failure.jsonl" \
  --out "$(dirname "$out")/api" --record-base ./fixtures

echo "fixtures written to $out:"
wc -l "$out"/*.jsonl
