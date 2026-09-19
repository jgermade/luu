#!/bin/sh
# Drives scripts/tasks/edit-reread.txt against the mock backend, twice: once
# with the corpus's own `## fragment:` directives alone, and once with
# `--select-tokens 1024` on top, which is one flag apart.
#
# The mock and not a model, deliberately: what this run measures is the
# *corpus*, and the edits are scripted so that the tally is reproducible. What
# it cannot say is how often a model edits a file it has quoted — that is the
# run this instrument exists for, and it needs a machine with a model on it.
# See RECORD/2026-09-19.a-corpus-that-edits.completed.md.
#
# Every prompt in the corpus that changes a file changes it for the rest of the
# session, so each arm runs against a fresh copy of the committed template.
# Run from the repository root with `cargo build` already done. .jsonl
# recordings are gitignored project-wide; the .txt files beside this script are
# the committed evidence.
set -eu
cd "$(dirname "$0")/../../.."
BIN=$(pwd)/target/debug/luu
OUT=$(pwd)/RECORD/runs/2026-09-19.edit-reread-mock
CORPUS=$(pwd)/scripts/tasks/edit-reread.txt
TEMPLATE=$(pwd)/scripts/tasks/edit-reread

# One reply per *model call*, not per turn: an editing turn makes two, and the
# list has to line up with that or an edit lands on the wrong turn. The same
# seventeen `crates/luu/tests/edit_reread_probe.rs` builds, which is what keeps
# this run and that test the same run.
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

for arm in fragments selected; do
  SCRATCH=$(mktemp -d)
  cp -r "$TEMPLATE/src" "$SCRATCH/src"
  case $arm in
    fragments) select="" ;;
    selected)  select="--select-tokens 1024" ;;
  esac
  # shellcheck disable=SC2086
  (cd "$SCRATCH" && "$BIN" chat --script "$CORPUS" --allow-write . \
    --context-limit 8192 --mock-delay-ms 0 $select \
    --record "$SCRATCH/$arm.jsonl" "$@") > "$OUT/$arm.txt" 2>&1
  grep '"type":"diverged"' "$SCRATCH/$arm.jsonl" > "$OUT/$arm.diverged.txt" || true
  printf '%s: %s diverged line(s), tree left as:\n' "$arm" "$(wc -l < "$OUT/$arm.diverged.txt")"
  grep -h 'Hey\|luu 0.2\|= 32\|WIDTH: usize' "$SCRATCH"/src/*.rs
  rm -rf "$SCRATCH"
done
