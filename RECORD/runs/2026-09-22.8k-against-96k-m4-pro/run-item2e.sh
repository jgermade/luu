#!/bin/sh
cd /Users/JG31772/dev/joshua/luu
run() { name=$1; shift
  target/debug/luu chat --script scripts/tasks/long-session.txt \
    --backend openai --openai-url http://127.0.0.1:8100/v1 \
    --model qwen3.8-27b --context-limit 8192 --reserve 512 \
    --temperature 0 --seed 1 "$@" \
    --tokenizer "$HOME/models/qwen3.8-27b/tokenizer.json" \
    --record .tmp/long-8192-$name.jsonl > .tmp/long-8192-$name.txt 2>&1
  echo "done $name $(date)" >> .tmp/run-item2e.progress
}
run prune-behind --prune-behind
run prune-behind-results --prune-behind --prune-results
