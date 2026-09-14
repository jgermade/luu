# Evidence: the `--prune-results` run of 2026-09-14

The transcripts and derived numbers behind
[`../../2026-09-14.what-rule-c-is-worth.completed.md`](../../2026-09-14.what-rule-c-is-worth.completed.md).
Three arms of the same 20-turn script, one flag apart, against the **mock**
backend — this measures the window's mechanics, not a model's answers, the
same split `prune-behind`'s own first measurement made.

```sh
for arm in "off:" "prune-behind:--prune-behind" \
           "prune-behind-and-results:--prune-behind --prune-results"; do
  target/release/luu chat --script script.txt \
    --backend mock --mock-delay-ms 0 \
    --mock-reply 'looking
```tool
{"name": "read_file", "arguments": {"path": "crates/agent-core/src/context.rs"}}
```' --mock-reply 'Noted.' --mock-cycle \
    --context-limit 8192 --reserve 512 --sandbox luu.toml \
    --record ${arm%%:*}.jsonl ${arm#*:} > ${arm%%:*}.txt
done
```

`--mock-cycle` (2026-09-14,
[`a-call-every-turn`](../../2026-09-14.a-call-every-turn.completed.md)) is what
makes this corpus possible at all: every one of the 20 turns calls
`read_file` on `crates/agent-core/src/context.rs` (136 KB, so the 8 KiB
`MAX_OUTPUT_BYTES` cap always bites) and then answers in one word, so every
turn carries a maximal tool result and nothing else competes with it for the
window. `script.txt` is the twenty prompts; their text is inert against the
mock and chosen only to read sensibly in the transcript.

`.jsonl` recordings are not committed — gitignored project-wide, and 96% of
each is the tool-definition prefix and streamed tokens repeated 20 times.
`stats.json` is every number this run's numbers are read from: each arm's
`turn 20` budget buckets, eviction and prune event counts and the tokens each
one moved, and mean prefix reuse — computed with `jq` directly against the
(uncommitted) recording, reproducible by rerunning the command above. The
three `.txt` files are the full streamed transcripts.
