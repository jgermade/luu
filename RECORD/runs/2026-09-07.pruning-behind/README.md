# `--prune-behind`, four arms

**What this is.** Twenty turns of `scripts/tasks/grounded.txt` at
`--select-tokens 1024 --context-limit 8192`, run four times: nothing, rule A,
rule B, and both. Everything else identical.

**What it is not.** `--mock-reply "ok"`, so **no answer here was written by a
model**, and nothing in this run says what either rule costs in accuracy. The
counter is `approximate` (chars/4). What is real is everything the rules touch:
the selector over the real tree, the window, the buckets, and which turns were
still in the prompt at the end.

## Totals over the twenty turns

| | `off` | `--repeat-once` (A) | `--prune-behind` (B) | both |
| --- | ---: | ---: | ---: | ---: |
| prompt tokens | 125 408 | 119 607 | **126 943** | 121 671 |
| `history` | 95 388 | 93 543 | 96 923 | 95 607 |
| `code` | 20 105 | 16 149 | 20 105 | 16 149 |
| cuts | 12 | 8 | **0** | **0** |
| turns evicted | **13** | **13** | **0** | **0** |
| turns that stopped quoting | 0 | 0 | 15 | 15 |
| mean prefix reuse | 34.0% | 52.1% | 38.9% | **54.5%** |
| worst window (limit 8 192) | 8 192 | 8 021 | 8 063 | 8 068 |

## The row that matters is not the token row

**Rule B costs 1.2% more prompt tokens and evicts nothing at all.** Turns 1–13
are gone from the window in both arms that do not prune, and still in it in both
that do. That is the finding, and it is not the one the proposal expected: it
argued that ~90% of the history block being quoted code made B *the token win*.
It is not. Taking the code out does not shrink the history — it makes room for
the conversation that was being evicted instead, and the conversation is not
free either. B trades a small token cost for keeping every turn.

Read the last turn of `off` against the last turn of `b`: **6 124 tokens of
history against 6 121**, the same block to within 3 tokens — holding **7 turns**
in the first and **20** in the second. That is the proposal's own opening
sentence, measured: the same window holds either the conversation or the quotes.

**Prefix reuse rose under B (34.0% → 38.9%) although pruning rewrites the
history block.** It rewrites it 13 times; eviction was rewriting it from the
front on 12 cuts *and* dropping the turns. The rewrite pruning pays for is
cheaper than the one it avoided.

**A and B compose, and the pair is the best arm on every axis but one.** 3.0%
fewer tokens than the baseline, nothing evicted, and 54.5% reuse against 34.0%.
A pays for B: pruning alone costs 1.2%, and A's saving more than covers it.

## What no arm here can say

Whether a 7B answers as well from `[pruned] path:1-40 — 312 tokens, no longer
quoted` as from the forty lines it replaces. That needs a model, it is the
protocol in the record, and it is not in this directory.

## Files

- `off.jsonl`, `a.jsonl`, `b.jsonl`, `ab.jsonl` — the four recordings.
- `cost.json` — per turn: buckets, prefix reuse, and what each cut and each
  prune took.
