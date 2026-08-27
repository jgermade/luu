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
# is meant to be read against each other. A small window so the session
# overflows: the recordings then carry a real split, a whole turn evicted, and
# the prefix surviving — or not surviving — the eviction.
#
# **The window has to clear the tool definitions first.** They are 465 tokens of
# the prefix, so at 512 the fixed part alone exceeds the window, no history is
# ever selected, and both policies record an empty `history` bucket and an
# identical 98% reuse — a pair that shows nothing, which is what this was for a
# while. 1024 leaves ~480 tokens of history, eight or nine turns, which is what
# makes the eviction visible. See the correction appended to
# RECORD/2026-08-27.prefix-reuse-and-block-eviction.md.
#
# Same script, same window, same counter, one flag apart. That is what makes
# them comparable, and comparing is the whole reason the debug client exists.
for policy in turn block; do
  "$luu" chat --script "$(dirname "$0")/tasks/steady-state.txt" \
    --mock-delay-ms 8 --context-limit 1024 --reserve 64 --evict "$policy" \
    --record "$out/eviction-$policy.jsonl" >/dev/null
done

# The same twenty prompts again, this time grouped into four tasks that close
# every five turns — the third recording of that pair, and the one that shows
# what a fold does to a history the other two only evict.
"$luu" chat --script "$(dirname "$0")/tasks/steady-state-tasks.txt" \
  --mock-delay-ms 8 --context-limit 1024 --reserve 64 --evict turn \
  --record "$out/eviction-tasks.jsonl" >/dev/null

# The tool loop, with the sandbox answering both ways: one call allowed and one
# denied. Scripted replies rather than a model, so the recording is the same
# every time — and the point of the pair is that the panel has to show *who*
# enforced each verdict, not just that something was refused.
"$luu" chat "What is in AGENTS.md, and what is in /etc/hostname?" \
  --mock-delay-ms 20 --context-limit 8192 \
  --sandbox "$(dirname "$0")/../luu.toml" \
  --mock-reply 'Let me read the file.
```tool
{"name":"read_file","arguments":{"path":"AGENTS.md","max_lines":3}}
```' \
  --mock-reply 'Now the other one.
```tool
{"name":"read_file","arguments":{"path":"/etc/hostname"}}
```' \
  --mock-reply 'AGENTS.md is the shared instruction file. The second path is outside the sandbox, so I cannot read it.' \
  --record "$out/tool-calls.jsonl" >/dev/null

# A task, start to finish. The one recording where the history is rewritten:
# the plan and the turns it ran are replaced by one summary at the close, and
# the budget panel shows what that cost — `tasks` against `history`, before and
# after. Scripted replies again, so the plan is the same plan every time.
"$luu" chat --script "$(dirname "$0")/tasks/one-task.txt" \
  --mock-delay-ms 20 --context-limit 8192 \
  --sandbox "$(dirname "$0")/../luu.toml" \
  --mock-reply '```plan
{"steps":["read AGENTS.md","say what it is for"],"paths":["AGENTS.md"],"commands":[]}
```' \
  --mock-reply 'Let me read it.
```tool
{"name":"read_file","arguments":{"path":"AGENTS.md","max_lines":4}}
```' \
  --mock-reply 'It is the instruction file every agent on this project reads.' \
  --record "$out/one-task.jsonl" >/dev/null

# A backend that is not there: the failure path, without depending on one.
"$luu" chat "Anything" --backend ollama --ollama-url http://127.0.0.1:1 \
  --record "$out/backend-failure.jsonl" >/dev/null 2>&1 || true

# The static twin of the read API, folded from these same recordings by the
# same code the live server uses. `$out` is <site>/fixtures, so the API tree
# goes beside it and the recordings are reached as ./fixtures/<name>.jsonl.
# Named in order, not globbed: the first one is what the page plays on load,
# and a glob would lead with whichever name sorts first. The default policy
# leads — a demo that silently plays a non-default configuration is a demo of
# something the tool does not do — and its counterpart is one click away in the
# picker, which is the point of recording both.
"$luu" export \
  "$out/eviction-turn.jsonl" \
  "$out/eviction-block.jsonl" \
  "$out/eviction-tasks.jsonl" \
  "$out/tool-calls.jsonl" \
  "$out/one-task.jsonl" \
  "$out/completed-turn.jsonl" \
  "$out/cancelled-turn.jsonl" \
  "$out/backend-failure.jsonl" \
  --out "$(dirname "$out")/api" --record-base ./fixtures

echo "fixtures written to $out:"
wc -l "$out"/*.jsonl
