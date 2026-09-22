#!/bin/sh
cd /Users/JG31772/dev/joshua/luu
run() { # name, LUU_HOME (or -), extra flags...
  name=$1; home=$2; shift 2
  if [ "$home" = "-" ]; then unset LUU_HOME; else export LUU_HOME="$home"; fi
  target/debug/luu chat --script scripts/tasks/long-session.txt \
    --backend openai --openai-url http://127.0.0.1:8100/v1 \
    --model qwen3.8-27b --context-limit 8192 --reserve 512 \
    --temperature 0 --seed 1 --prune-behind "$@" \
    --tokenizer "$HOME/models/qwen3.8-27b/tokenizer.json" \
    --record .tmp/item5-$name.jsonl > .tmp/item5-$name.txt 2>&1
  echo "done $name $(date)" >> .tmp/run-item5.progress
}
run kept -
run cited-reads "$(pwd)/.tmp/luu-home-cited-reads"
run cited - --prune-results
