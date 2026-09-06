# Evidence: the precision run of 2026-09-06

The scored answers behind
[`../../2026-09-06.does-the-model-read-it.completed.md`](../../2026-09-06.does-the-model-read-it.completed.md).
One file per arm, 38 questions each: the question, its target from
[`map-order-probe.key`](../../../scripts/tasks/map-order-probe.key), whether that
target was structurally in the arm's `code` bucket, the verdict, every file the
answer named, and the answer itself.

**Why this is committed when `*.jsonl` is not.** Coverage never needs evidence on
disk — `cargo test -p luu --test select_probe` recomputes it on every commit.
**Precision cannot be recomputed**: it needs a model, a box and an hour, and part
of its score is a hand read. A precision number in prose with nothing behind it is
unfalsifiable, and this repository has just found one of its own coverage tables
had gone stale unnoticed. So the answers stay and the recordings do not: the three
`.jsonl` files were 4.0 MB, of which 98% is the tool prefix repeated 38 times and
one JSON object per streamed token. What is here is the 96 KB that is evidence.

`cost.json` is the other half, and it is what makes the record's cost table
falsifiable rather than quoted: per arm and per turn, the budget's buckets, the
usage the backend reported, the shared-prefix trace, every eviction with the
turns it threw away, and the totals the record prints. Without it "reuse fell to
17.7%" is a sentence; with it, it is a division anyone can redo.

Produced by, on `b55b6c9` against `qwen2.5-coder:7b` on an M1 Pro:

```sh
for arm in "off:" "select:--select-tokens 1024" "map:--map-tokens 1024"; do
  target/release/luu chat --script scripts/tasks/map-order-probe.txt \
    --backend ollama --model qwen2.5-coder:7b \
    --context-limit 8192 --reserve 512 \
    --tokenizer ~/models/qwen2.5-coder-7b/tokenizer.json \
    --temperature 0 --seed 7 --sandbox luu.toml \
    --record precision-${arm%%:*}.jsonl ${arm#*:}
done
```

`fragments` (selection arm) is the exact spans the selector chose for that
question — `crates/luu/src/serve.rs:50-79` — recomputed from the same commit, so
what the model was handed can be read without the recording it was handed in.

`verdict` is `right` when the answer names the target, `wrong` when it names some
other file in the tree, `declines` when it names none. A file is "named" by the
shortest path suffix unique in the tree — `agent.rs`, but `backend/mod.rs` —
matched on a non-identifier boundary so `select.rs` does not match inside
`select_probe.rs`.
