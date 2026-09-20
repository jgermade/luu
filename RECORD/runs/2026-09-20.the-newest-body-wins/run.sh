#!/bin/sh
# The same corpus, the same replies, and the same two flags as
# RECORD/runs/2026-09-19.one-counting-surface — with a third axis that is the
# whole point of this run: the binary. `before` is the fix's parent commit and
# `after` is the fix, so what moves between them is rule A's repair and nothing
# else. That is this project's own *one flag apart*, with the flag being a
# commit, because the repair is not switchable: under `Repeat::Once` the newest
# body wins, and the way back is `--no-repeat-once`, which is the arm the
# corpus's original numbers now live in.
#
# The cost is a prefix number, so it comes out of the recording's own
# `prefix_reuse` lines rather than out of `luu count`, which has no opinion
# about prefixes. prefix-reuse.md beside this script is that table.
#
# Run from the repository root with both binaries built:
#
#   git worktree add /tmp/luu-before <the fix's parent>
#   (cd /tmp/luu-before && cargo build --bin luu)
#   cargo build --bin luu
#
# .jsonl recordings are gitignored project-wide; the .txt and .md files beside
# this script are the committed evidence.
set -eu
cd "$(dirname "$0")/../../.."
BIN=$(pwd)/target/debug/luu
BEFORE=${BEFORE:-/tmp/luu-before/target/debug/luu}
OUT=$(pwd)/RECORD/runs/2026-09-20.the-newest-body-wins
CORPUS=$(pwd)/scripts/tasks/edit-reread.txt
TEMPLATE=$(pwd)/scripts/tasks/edit-reread

# One reply per *model call*, not per turn — the same seventeen
# `crates/luu/tests/edit_reread_probe.rs` builds and the same
# `2026-09-19.edit-reread-mock/run.sh` used, which is what makes the two runs
# the same run.
edit() {
  printf 'Let me change it.\n```tool\n{"name":"edit_file","arguments":{"path":"%s","old_string":"%s","new_string":"%s"}}\n```' "$1" "$2" "$3"
}
set -- \
  --mock-reply 'greet returns a greeting.' \
  --mock-reply 'render pads to WIDTH.' \
  --mock-reply 'banner prints a line.' \
  --mock-reply 'THRESHOLD is 16.' \
  --mock-reply "$(edit src/greeting.rs 'Hello, {name}' 'Hi, {name}')" \
  --mock-reply 'Edited.' \
  --mock-reply 'greet returns a greeting.' \
  --mock-reply 'render pads to WIDTH.' \
  --mock-reply "$(edit src/banner.rs '\"luu\"' '\"luu 0.2\"')" \
  --mock-reply 'Edited.' \
  --mock-reply "$(edit src/tally.rs 'THRESHOLD: usize = 16' 'THRESHOLD: usize = 32')" \
  --mock-reply 'Edited.' \
  --mock-reply 'THRESHOLD is 32.' \
  --mock-reply "$(edit src/greeting.rs 'Hi, {name}' 'Hey, {name}')" \
  --mock-reply 'Edited.' \
  --mock-reply 'greet returns a greeting.' \
  --mock-reply 'greeting, banner and tally have changed.'

# Four arms. `after` and `after-sel` are the fix; `before` and `before-sel` are
# the same two on the parent commit, which is what the prefix table compares.
# `always` is the fix's binary with rule A off — the arm the 2026-09-19 numbers
# moved into, kept here so that a reader can see them produced rather than
# quoted.
for arm in before after before-sel after-sel always; do
  SCRATCH=$(mktemp -d)
  cp -r "$TEMPLATE/src" "$SCRATCH/src"
  case $arm in
    before)     bin=$BEFORE; select="" ;;
    after)      bin=$BIN;    select="" ;;
    before-sel) bin=$BEFORE; select="--select-tokens 1024" ;;
    after-sel)  bin=$BIN;    select="--select-tokens 1024" ;;
    always)     bin=$BIN;    select="--no-repeat-once" ;;
  esac
  # shellcheck disable=SC2086
  (cd "$SCRATCH" && "$bin" chat --script "$CORPUS" --allow-write . \
    --context-limit 8192 --mock-delay-ms 0 $select \
    --record "$SCRATCH/$arm.jsonl" "$@") > /dev/null 2>&1
  "$BIN" count "$SCRATCH/$arm.jsonl" > "$OUT/$arm.count.txt"
  cp "$SCRATCH/$arm.jsonl" "$OUT/$arm.jsonl"
  printf '%s:\n' "$arm"
  cat "$OUT/$arm.count.txt"
  rm -rf "$SCRATCH"
done

# The cost, out of the lines `luu count` does not read.
python3 "$OUT/prefix.py" "$OUT" > "$OUT/prefix-reuse.md"
rm -f "$OUT"/*.jsonl
