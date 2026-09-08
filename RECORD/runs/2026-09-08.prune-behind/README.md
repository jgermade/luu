# `--prune-behind` on the grounded corpus, one flag apart, under both policies

**What this is.** Twenty turns of `scripts/tasks/grounded.txt` at
`--select-tokens 1024 --context-limit 8192`, run six times: the two eviction
policies, each with and without `--prune-behind`, plus the two arms that also
carry `--repeat-once` so rule A and rule B can be read against each other.
Everything else is identical, including the order the selector scored the tree
in.

```sh
luu chat --script scripts/tasks/grounded.txt --select-tokens 1024 \
  --context-limit 8192 --mock-delay-ms 0 --mock-reply "ok" \
  [--evict block --low-water 0.5] [--repeat-once] [--prune-behind] \
  --record <arm>.jsonl
```

**What it is not.** The backend is `--mock-reply "ok"`, so **no answer here was
written by a model** and nothing in this run says whether a citation where a
span used to be costs accuracy. What is real is everything the rule touches:
the selection is the real selector over the real tree, the buckets are what the
prompt was, and the prunes and cuts are what the window did. The counter is
`approximate` (chars/4) — the mock has no tokenizer, and every arm is counted
the same way.

## Totals over the twenty turns

| | `off` | `--prune-behind` | | `--evict block` | `block --prune-behind` |
| --- | ---: | ---: | --- | ---: | ---: |
| prompt tokens | 124 393 | 124 028 | −0.3% | 100 000 | 101 375 |
| `code` bucket | 20 057 | 20 057 | — | 20 057 | 20 057 |
| `history` bucket | 94 421 | 94 056 | −0.4% | 70 028 | 71 403 |
| **cuts** | 12 | **0** | | 4 | **0** |
| **turns dropped** | 13 | **0** | | 16 | **0** |
| prunes | 0 | 13 | | 0 | **4** |
| turns pruned | 0 | 14 | | 0 | 15 |
| tokens freed by pruning | 0 | 13 732 | | 0 | 14 712 |
| mean prefix reuse | 34.1% | 37.6% | | 66.5% | **69.8%** |

And the two arms that carry rule A as well, against `--repeat-once` alone:

| | `--repeat-once` | `--repeat-once --prune-behind` |
| --- | ---: | ---: |
| prompt tokens | 120 375 | 121 784 |
| `code` bucket | 16 101 | 16 101 |
| cuts / turns dropped | 7 / 12 | **0 / 0** |
| prunes | 0 | 9 |
| mean prefix reuse | 56.4% | 53.5% |

## What the shape of it says

**The headline is not a token count. Nothing was evicted.** Twenty turns, and
under every arm that carries the flag the session ends with the whole
conversation still in the window — against 13 turns dropped by the default and
16 by the block policy. The prompt is the same size it always was; **what it is
made of changed**: 13 732 tokens of quoted code left, and the questions and
answers that would have gone with them stayed.

That is the trade the record proposed, arriving exactly as proposed and paying
in a currency the table nearly failed to show. Read the first two columns for
tokens alone and rule B looks like a rounding error at −0.3%. Read the `turns
dropped` row and it is the difference between a session that remembers the last
twenty exchanges and one that remembers the last seven.

**The depth-against-frequency trade is the eviction policy's, and pruning
inherits it.** Under `Eviction::Turn` the line moves 13 times in 20 turns —
shallow and frequent, which is that policy's whole character — and prefix reuse
gains only 3.5 points. Under `Eviction::Block` it moves **4 times**, and prefix
reuse is 69.8% against the 66.5% of the arm that was throwing turns away. This
is the argument for reusing the policy's own target rather than inventing a
depth for pruning: the flag does not have a character of its own, it has the
one the run already chose.

**Rule A and rule B do not add up, and the record predicted why.** `both`
sends *more* than `once` (121 784 against 120 375) because it is keeping eight
turns that `once` had dropped. Its prefix reuse is lower for the same reason: a
prune rewrites the history block from the pruned turn forward, and `once`'s
history block was not being rewritten at all between cuts. Nothing here says
one flag is better than the other; they buy different things, and the run says
which is which.

**Pruning fires where rule A has nothing left to take.** The `both` arm prunes
9 times against `prune`'s 13: where A has already reduced a span to a single
copy, pruning the turn holding it hands the copy to a younger turn and adds a
citation, which is bigger — so `prune_to` declines. That interaction was found
by the code and not by the record; see the addendum to
[`prune-behind`](../../2026-09-08.prune-behind.completed.md).

## Files

- `cost.json` — per turn, per arm: buckets, prefix reuse, every cut and every
  prune with the turns it named. The six recordings are not committed; the
  command above reproduces them.
