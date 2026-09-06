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

# The three that show the context manager working, and the only set in here
# meant to be read against each other. A deliberately small window so the
# session overflows: the recordings then carry a real split, a whole turn
# evicted, and the prefix surviving — or not surviving — the eviction.
#
# 1024 and not 512: the tool definitions are 465 tokens of the prefix, so at 512
# the fixed part alone fills the window and no history is ever retained. The
# recordings were degenerate — two files showing an empty history bucket, read
# as a comparison of eviction policies — from the commit that added tools until
# this one.
#
# Same script, same window, same counter. The first two are one flag apart; the
# third is the same twenty prompts grouped into four tasks, which is a boundary
# apart rather than a flag apart. See `RECORD/2026-08-30.tasks-in-code.completed.md`.
for policy in turn block; do
  "$luu" chat --script "$(dirname "$0")/tasks/steady-state.txt" \
    --mock-delay-ms 8 --context-limit 1024 --reserve 64 --evict "$policy" \
    --record "$out/eviction-$policy.jsonl" >/dev/null
done

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

# One task, start to finish. The one recording where the history is rewritten
# rather than only evicted: the plan and the turns it ran are replaced by one
# summary at the close, and the budget panel shows what that cost — `summaries`
# against `history`, before and after.
"$luu" chat --script "$(dirname "$0")/tasks/one-task.txt" \
  --mock-delay-ms 20 --context-limit 8192 \
  --sandbox "$(dirname "$0")/../luu.toml" \
  --record "$out/one-task.jsonl" >/dev/null

# The repository map in the prefix, which is the one recording where the `map`
# bucket is not zero. 1024 tokens: five files of this repository outlined, and
# the map says how many it left out — at 8K the whole outline is 6 327 tokens,
# which is why a budget is a flag rather than a default. The same prompt as the
# grounded turn below, so the two are one flag apart.
"$luu" chat "which two commitments does the context manager open with?" \
  --mock-delay-ms 20 --context-limit 8192 --map-tokens 1024 \
  --sandbox "$(dirname "$0")/../luu.toml" \
  --record "$out/repo-map.jsonl" >/dev/null

# A real file fused into a turn — the first recording in which the `code` bucket
# is not zero, which until the fragment surface existed it always was.
"$luu" chat "which two commitments does this file open with?" \
  --fragment crates/agent-core/src/context.rs:1-15 \
  --mock-delay-ms 20 --context-limit 8192 \
  --sandbox "$(dirname "$0")/../luu.toml" \
  --record "$out/grounded-turn.jsonl" >/dev/null

# The other way the history gives way: the turns stay and their tool output
# becomes a digest. Twelve turns that each read a real file, under the watermark
# — the only recording in which `pruned` lines appear, and the one where the
# transcript has to show output the model can no longer see. Off is not recorded
# beside it here because the pair is a measurement rather than a demo, and
# `scripts/prune-probe.sh` is where it lives.
prune_replies=()
for file in crates/agent-core/src/tools/mod.rs crates/agent-core/src/sandbox/policy.rs \
            crates/agent-core/src/repo_map.rs crates/agent-core/src/select.rs \
            crates/agent-core/src/job.rs crates/agent-core/src/agent.rs \
            crates/agent-core/src/turn.rs crates/agent-core/src/protocol.rs \
            crates/agent-core/src/backend/ollama.rs crates/agent-core/src/tools/fs.rs \
            crates/agent-core/src/tools/command.rs crates/agent-core/src/worker/mod.rs; do
  prune_replies+=(--mock-reply "let me look
\`\`\`tool
{\"name\":\"read_file\",\"arguments\":{\"path\":\"$file\",\"max_lines\":120}}
\`\`\`" --mock-reply "That file answers it.")
done
"$luu" chat --script "$(dirname "$0")/tasks/tool-heavy.txt" \
  --mock-delay-ms 8 --context-limit 8192 --reserve 512 \
  --prune watermark --prune-keep 2 \
  --sandbox "$(dirname "$0")/../luu.toml" \
  "${prune_replies[@]}" \
  --record "$out/pruning.jsonl" >/dev/null

# A backend that is not there: the failure path, without depending on one.
"$luu" chat "Anything" --backend ollama --ollama-url http://127.0.0.1:1 \
  --record "$out/backend-failure.jsonl" >/dev/null 2>&1 || true

# The static twin of the read API, folded from these same recordings by the
# same code the live server uses. `$out` is <site>/fixtures, so the API tree
# goes beside it and the recordings are reached as ./fixtures/<name>.jsonl.
# Named in order, not globbed: the first one is what the page plays on load,
# and a glob would lead with whichever name sorts first. The default policy
# leads — a demo that silently plays a non-default configuration is a demo of
# something the tool does not do — and its counterparts are one click away in
# the picker, which is the point of recording all three.
"$luu" export \
  "$out/eviction-turn.jsonl" \
  "$out/eviction-block.jsonl" \
  "$out/eviction-tasks.jsonl" \
  "$out/tool-calls.jsonl" \
  "$out/one-task.jsonl" \
  "$out/grounded-turn.jsonl" \
  "$out/repo-map.jsonl" \
  "$out/pruning.jsonl" \
  "$out/completed-turn.jsonl" \
  "$out/cancelled-turn.jsonl" \
  "$out/backend-failure.jsonl" \
  --out "$(dirname "$out")/api" --record-base ./fixtures

echo "fixtures written to $out:"
wc -l "$out"/*.jsonl
