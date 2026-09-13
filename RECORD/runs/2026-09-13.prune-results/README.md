# `--prune-results` on a corpus that calls a tool every turn

**What this is.** The same twenty turns of `scripts/tasks/grounded.txt` the
rule-B run used, at `--select-tokens 1024 --context-limit 8192`, with one thing
added: **every turn now calls `read_file` and gets a real result back**. Six
arms — the two eviction policies, rule B alone and rule B with rule C, plus the
arm that carries rule C on its own to see whether it is as inert as its tests
say.

```sh
luu chat --script scripts/tasks/grounded.txt \
  --mock-script scripts/mock/grounded-tools.txt \
  --select-tokens 1024 --context-limit 8192 --mock-delay-ms 0 \
  [--evict block --low-water 0.5] [--prune-behind] [--prune-results] \
  --record <arm>.jsonl
```

The corpus is the instrument of
[`a-mock-that-calls-a-tool`](../../2026-09-13.a-mock-that-calls-a-tool.completed.md):
eight reply blocks, cycling, two per turn — a call, then the reply that reads
its result. The phase held in all six arms, which is the first number to read
here and the one everything else depends on: **20 turns, 20 tool calls, in every
arm**.

**What it is not.** The replies are the mock's, so no answer here was written by
a model and nothing says whether a citation where 8 KiB of a file stood costs
accuracy. What is real is the call, the sandbox check, the bytes that came back
and everything the window then did with them. The counter is `approximate`
(chars/4), the same for every arm.

## What one result costs

| | |
| --- | ---: |
| results in the run | 20 |
| mean result | **1 736 tokens** (6 946 bytes) |
| smallest / largest | 990 / 2 043 |
| all results together | 34 733 |

Against `--select-tokens 1024`. The record's claim — *the largest single block a
turn can put in the window is not the code the selector chose, it is the output
of one command* — is 1.7× at the mean and 2× at the cap.

## Totals over the twenty turns

| | `off` | `--prune-results` | `--prune-behind` | **both** |
| --- | ---: | ---: | ---: | ---: |
| prompt tokens | 122 626 | 122 626 | 126 888 | **119 384** |
| `history` bucket | 92 534 | 92 534 | 96 796 | 89 292 |
| `code` bucket | 20 177 | 20 177 | 20 177 | 20 177 |
| **cuts** | 15 | 15 | 12 | **0** |
| **turns dropped** | 17 | 17 | 16 | **0** |
| prunes | 0 | 0 | 17 | 16 |
| turns pruned | 0 | 0 | 18 | 18 |
| tokens freed by pruning | 0 | 0 | 17 687 | **48 434** |
| mean prefix reuse | 23.7% | 23.7% | 25.7% | **41.2%** |

And under `--evict block --low-water 0.5`:

| | `block --prune-behind` | `block` + both |
| --- | ---: | ---: |
| prompt tokens | 107 448 | **92 616** (−13.8%) |
| cuts / turns dropped | 6 / 16 | **0 / 0** |
| prunes | 11 | 9 |
| tokens freed by pruning | 17 687 | 50 406 |
| mean prefix reuse | 46.7% | **70.4%** |

## What the shape of it says

**Rule B alone does not hold a tooled session, and this is the run that says
so.** On the untooled corpus of 2026-09-08 `--prune-behind` ended with *nothing
evicted* — 0 turns dropped against 13. Here it drops **16 of 20**, three fewer
than the default and no more than that, because what it takes from an old turn
is its quoted spans and the expensive thing in these turns is not the span. It
even sends *more* than `off` (126 888 against 122 626) for the reason the last
run named: the turns it manages to keep cost tokens the default had already
thrown away.

**Rule C is what the corpus needed, and the two together are not additive.**
17 687 tokens freed by rule B alone; 48 434 by the pair — 2.7×, on a corpus
where every turn holds ~1 700 tokens of output that only rule C can reach. The
session ends with **all twenty turns still in the window** under both eviction
policies, against 16 and 17 dropped.

**The prompt is smaller, and that is still the smaller half of the claim.**
−5.9% against rule B alone on the default policy and −13.8% on the block
policy, which read like rounding next to what the same arms did to the
conversation: 0 cuts against 15. The rule the tests describe — *the prompt is
smaller and the conversation is intact* — arrives with the second half doing
almost all the work, exactly as rule B's did.

**Prefix reuse nearly doubles, and it is the number to watch next.** 23.7% →
41.2% on the default policy, 46.7% → 70.4% on the block policy. A prune rewrites
the history block from the pruned turn forward, which is a *cost* to reuse — but
an eviction rewrites it too, and here rule C buys 15 evictions' worth of
rewrites for 16 prunes that mostly land on the same old turns. Under the block
policy, where the line moves rarely by design, the effect compounds: 70.4% is
the highest reuse any arm of any run in this repository has recorded on this
corpus.

**`--prune-results` alone is inert, byte for byte.** Not "inert in a test": the
`results` arm's recording is identical to `off`'s message for message, every
field but the clock. The flag is a rule about what a turn behind the line gives
up, and without `--prune-behind` there is no line.

## What it does not say

- **Nothing about a model.** The open question of
  [`what-a-result-costs`](../../2026-09-09.what-a-result-costs.completed.md) is
  untouched: a citation where `cargo test` output stood is a stronger claim than
  one where a quoted span stood, and the only thing that can answer it is a box
  with a model on it.
- **The corpus is a cycle**, four files in the same order for twenty turns. Rule
  C takes bytes regardless of what they are, so the regularity does not flatter
  it — but the same run under rule A would be flattered, and an arm carrying
  `--repeat-once` was left out for that reason rather than by omission.
- **The results are all `read_file`.** A `run_command` result is the case the
  design doc's 8 KiB sentence was written about, and it is not in this run:
  `luu.toml` allows five programs, and none of them produces stable bytes.

## Files

- `cost.json` — per arm, per turn: buckets, prefix reuse, every cut and every
  prune with the turns it named, plus `tool_calls` and `turns_with_a_tool_call`,
  which is where the phase is checked. The six recordings are not committed; the
  command above reproduces them.
