# `--repeat-once` on the grounded corpus, one flag apart

**What this is.** Twenty turns of `scripts/tasks/grounded.txt` at
`--select-tokens 1024 --context-limit 8192`, run twice, the second with
`--repeat-once`. Everything else is identical, including the order the
selector scored the tree in.

**What it is not.** The backend is `--mock-reply "ok"`, so **no answer here
was written by a model** and nothing in this run says whether the rule costs
accuracy. What is real is everything the rule touches: the selection is the
real selector over the real tree, and the buckets are what the prompt was.
The counter is `approximate` (chars/4) — the mock has no tokenizer, and the
comparison is between two runs counted the same way.

The corpus itself was unrunnable until the same day: both grounded scripts
pointed at `crates/agent-core/src/task.rs`, renamed in `d70e6d9`. See the
record.

## Totals over the twenty turns

| | `off` | `--repeat-once` | |
| --- | ---: | ---: | --- |
| prompt tokens | 125 300 | **120 980** | −3.4% |
| `code` bucket | 20 274 | **16 318** | **−19.5%** |
| `history` bucket | 95 111 | 94 747 | −0.4% |
| cuts | 12 | **8** | a third fewer |
| turns dropped | 13 | 13 | the same window, in fewer cuts |
| mean prefix reuse | 34.0% | **52.0%** | +18 points |

## What the shape of it says

**The saving lands in `code`, not in `history`, and that is the rule.** A
span is rendered by the *oldest* turn of the window that carries it, so the
history keeps what it had and the turn being asked is the one that gives a
span up. That is the whole reason rule A is prefix-safe: the block above the
newest message does not move, so the only message that changes is the one
that changes anyway.

**−19.5% of the code bucket, against the 9.2% the replay predicted.** The
replay was computed on `map-order-probe`, thirty-eight questions about
thirty-eight different files, built to exclude repetition; this corpus asks
five consecutive questions about one file. Both numbers are right about
their own corpus, and the gap between them is the reason
`what-leaves-the-history` said a conversation corpus was a precondition
rather than a follow-up.

**Prefix reuse went up, which nothing predicted.** Fewer tokens in the turn
means the window fills more slowly, which means fewer cuts — 8 against 12 —
and a cut is what rewrites the history block from its front. The same 13
turns leave the window either way; they leave in fewer, deeper cuts. This is
`Eviction::Block`'s argument arriving through a different door, and it is a
consequence, not a claim: it is one corpus, at one window size, with one
eviction policy.

## Files

- `off.jsonl`, `on.jsonl` — the two recordings, as `luu chat --record` wrote
  them.
- `cost.json` — per turn: buckets, prefix reuse, and what each cut freed.
