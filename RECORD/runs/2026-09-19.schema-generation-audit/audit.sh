#!/bin/sh
# Which tool schema was each recorded run made against?
#
# `82229cc` (2026-09-15 11:38) replaced `run_command`'s
# {command, args, cwd, timeout_ms} with a single required `argv` array. No run
# directory records the binary it was made with, so the schema generation is
# read out of what the model actually emitted: every committed transcript that
# contains a `run_command` call carries one envelope shape or the other.
#
# Writes schema-generation.json. Run from the repository root.
set -eu
cd "$(dirname "$0")/../../.."
OUT=RECORD/runs/2026-09-19.schema-generation-audit/schema-generation.json
UNSEEN=$(mktemp)
trap 'rm -f "$UNSEEN"' EXIT

printf '{\n  "fix": "82229cc 2026-09-15 11:38:17 +0200",\n  "runs": [\n' > "$OUT"
first=1
for dir in RECORD/runs/*/; do
  name=$(basename "$dir")
  # this run directory holds the classifier, not a transcript
  [ "$name" = "2026-09-19.schema-generation-audit" ] && continue
  calls=$(grep -rh -A3 'run_command' "$dir" 2>/dev/null || true)
  pre=$(printf '%s' "$calls" | grep -c '"command"' || true)
  post=$(printf '%s' "$calls" | grep -c '"argv"' || true)
  [ "$pre" = "0" ] && [ "$post" = "0" ] && { echo "$name" >> "$UNSEEN"; continue; }
  if [ "$pre" != "0" ] && [ "$post" != "0" ]; then gen=mixed
  elif [ "$pre" != "0" ]; then gen=pre-argv
  else gen=post-argv
  fi
  [ "$first" = "1" ] || printf ',\n' >> "$OUT"
  first=0
  printf '    { "run": "%s", "schema": "%s", "command_args": %s, "argv": %s }' \
    "$name" "$gen" "$pre" "$post" >> "$OUT"
done
printf '\n  ],\n  "no_run_command_call_committed": [\n' >> "$OUT"
sed -e 's/.*/    "&"/' -e '$!s/$/,/' "$UNSEEN" >> "$OUT"
printf '  ]\n}\n' >> "$OUT"
cat "$OUT"
