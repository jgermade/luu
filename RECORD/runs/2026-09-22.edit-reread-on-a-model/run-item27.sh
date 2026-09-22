#!/bin/sh
cd /Users/JG31772/dev/joshua/luu
OUT="$(pwd)/.tmp/runs27"
CORPUS=$(pwd)/scripts/tasks/edit-reread.txt
TEMPLATE=$(pwd)/scripts/tasks/edit-reread
TOKENIZER="$HOME/models/qwen3.8-27b/tokenizer.json"
for arm in fragments selected; do
  SCRATCH=$(mktemp -d)
  cp -r "$TEMPLATE/src" "$SCRATCH/src"
  case $arm in
    fragments) select="" ;;
    selected)  select="--select-tokens 1024" ;;
  esac
  (cd "$SCRATCH" && /Users/JG31772/dev/joshua/luu/target/debug/luu chat --script "$CORPUS" --allow-write . \
    --backend openai --openai-url http://127.0.0.1:8100/v1 --model qwen3.8-27b \
    --context-limit 8192 --reserve 512 --temperature 0 --seed 1 $select \
    --tokenizer "$TOKENIZER" \
    --record "$OUT/$arm.jsonl") > "$OUT/$arm.txt" 2>&1
  cp -r "$SCRATCH/src" "$OUT/$arm.tree"
  rm -rf "$SCRATCH"
  echo "done $arm $(date)" >> "$OUT/progress"
done
