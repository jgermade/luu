#!/usr/bin/env sh
# The pruning probe: the same twelve tool-heavy turns under each policy, one
# flag apart, so the runs are comparable.
#
#   ./scripts/prune-probe.sh ./target/debug/luu /tmp/prune
#
# Writes off.jsonl, age.jsonl and watermark.jsonl into the output directory.
# The numbers to read out of them are in
# RECORD/2026-09-05.pruning-tool-results.completed.md.
set -eu

luu=${1:?usage: prune-probe.sh <path to luu> <output dir>}
out=${2:?usage: prune-probe.sh <path to luu> <output dir>}
mkdir -p "$out"

# One scripted call and one scripted answer per prompt: the mock consumes one
# reply per model call, so a turn that uses a tool costs two. The files are real
# and are read through the sandbox, which is what makes the token counts mean
# anything.
files='crates/agent-core/src/tools/mod.rs
crates/agent-core/src/sandbox/policy.rs
crates/agent-core/src/repo_map.rs
crates/agent-core/src/select.rs
crates/agent-core/src/job.rs
crates/agent-core/src/agent.rs
crates/agent-core/src/turn.rs
crates/agent-core/src/protocol.rs
crates/agent-core/src/backend/ollama.rs
crates/agent-core/src/tools/fs.rs
crates/agent-core/src/tools/command.rs
crates/agent-core/src/worker/mod.rs'

set --
for file in $files; do
  set -- "$@" --mock-reply \
    "let me look
\`\`\`tool
{\"name\":\"read_file\",\"arguments\":{\"path\":\"$file\",\"max_lines\":120}}
\`\`\`" \
    --mock-reply "That file answers it."
done

run() {
  name=$1
  shift
  echo "== $name"
  "$luu" chat --script scripts/tasks/tool-heavy.txt \
    --sandbox luu.toml --mock-delay-ms 0 \
    --context-limit 8192 --reserve 512 \
    --record "$out/$name.jsonl" \
    "$@" > "$out/$name.log"
}

run off "$@"
run age "$@" --prune age --prune-keep 2
# 0.35 is the share this was first written with, and on this corpus two results
# exceed it on their own — so the watermark fires every turn and is `age` with
# more flags. Kept as an arm because that is the finding.
run watermark-low "$@" --prune watermark --prune-keep 2 --prune-above 0.35
run watermark "$@" --prune watermark --prune-keep 2

python3 ./scripts/record-summary.py "$out"/off.jsonl "$out"/age.jsonl \
  "$out"/watermark-low.jsonl "$out"/watermark.jsonl
