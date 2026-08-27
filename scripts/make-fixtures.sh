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

# The two that show the context manager working, and the only pair in here that
# is meant to be read against each other. A deliberately small window so the
# session overflows: the recordings then carry a real split, a whole turn
# evicted, and the prefix surviving — or not surviving — the eviction.
#
# Same script, same window, same counter, one flag apart. That is what makes
# them comparable, and comparing is the whole reason the debug client exists.
for policy in turn block; do
  "$luu" chat --script "$(dirname "$0")/tasks/steady-state.txt" \
    --mock-delay-ms 8 --context-limit 512 --reserve 64 --evict "$policy" \
    --record "$out/eviction-$policy.jsonl" >/dev/null
done

# A backend that is not there: the failure path, without depending on one.
"$luu" chat "Anything" --backend ollama --ollama-url http://127.0.0.1:1 \
  --record "$out/backend-failure.jsonl" >/dev/null 2>&1 || true

# The static twin of the read API, folded from these same recordings by the
# same code the live server uses. `$out` is <site>/fixtures, so the API tree
# goes beside it and the recordings are reached as ./fixtures/<name>.jsonl.
# Named in order, not globbed: the first one is what the page plays on load,
# and a glob would lead with whichever name sorts first.
"$luu" export \
  "$out/eviction-block.jsonl" \
  "$out/eviction-turn.jsonl" \
  "$out/completed-turn.jsonl" \
  "$out/cancelled-turn.jsonl" \
  "$out/backend-failure.jsonl" \
  --out "$(dirname "$out")/api" --record-base ./fixtures

echo "fixtures written to $out:"
wc -l "$out"/*.jsonl
