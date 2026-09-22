#!/bin/sh
cd /Users/JG31772/dev/joshua/luu
target/debug/luu chat --script scripts/tasks/long-session.txt \
  --backend openai --openai-url http://127.0.0.1:8100/v1 \
  --model qwen3.8-27b --context-limit 8192 --reserve 512 \
  --temperature 0 --seed 1 \
  --repeat-once \
  --tokenizer "$HOME/models/qwen3.8-27b/tokenizer.json" \
  --record .tmp/long-8192-repeat-once.jsonl > .tmp/long-8192-repeat-once.txt 2>&1
echo "done repeat-once $(date)" > .tmp/run-item2d.progress
