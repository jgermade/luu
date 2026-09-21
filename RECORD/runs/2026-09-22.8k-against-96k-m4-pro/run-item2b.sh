#!/bin/sh
cd /Users/JG31772/dev/joshua/luu
for limit in 8192 98304; do
  target/debug/luu chat --script scripts/tasks/long-session.txt \
    --backend openai --openai-url http://127.0.0.1:8100/v1 \
    --model qwen3.8-27b --context-limit $limit --reserve 512 \
    --temperature 0 --seed 1 \
    --tokenizer "$HOME/models/qwen3.8-27b/tokenizer.json" \
    --record .tmp/long-$limit.jsonl > .tmp/long-$limit.txt 2>&1
  echo "done $limit $(date)" >> .tmp/run-item2b.progress
done
