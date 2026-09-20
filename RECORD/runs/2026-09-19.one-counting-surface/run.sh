#!/bin/sh
# `luu count` over the edit-and-re-read corpus, in the same two arms
# RECORD/runs/2026-09-19.edit-reread-mock drove one day earlier — so that the
# table that run's README carries can be compared against the one a command
# produces, rather than against a `grep` and a person's eye.
#
# It is the same corpus and the same replies deliberately: this run measures
# the *instrument*, not the corpus, and an instrument is checked against a
# number somebody already has. What it adds that nothing could produce before
# `record::FORMAT` 14 is rule A's own line.
#
# Run from the repository root with `cargo build` already done. .jsonl
# recordings are gitignored project-wide; the .txt files beside this script are
# the committed evidence.
set -eu
cd "$(dirname "$0")/../../.."
BIN=$(pwd)/target/debug/luu
OUT=$(pwd)/RECORD/runs/2026-09-19.one-counting-surface
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
    --record "$SCRATCH/$arm.jsonl" "$@") > /dev/null 2>&1
  "$BIN" count "$SCRATCH/$arm.jsonl" > "$OUT/$arm.count.txt"
  # The static twin answers the same question, and a mirror that disagrees with
  # what it mirrors is the thing `luu export` exists to not be.
  "$BIN" export "$SCRATCH/$arm.jsonl" --out "$SCRATCH/site" > /dev/null
  cp "$SCRATCH/site/sessions/$arm/counts.json" "$OUT/$arm.counts.json"
  printf '%s:\n' "$arm"
  cat "$OUT/$arm.count.txt"
  rm -rf "$SCRATCH"
done
