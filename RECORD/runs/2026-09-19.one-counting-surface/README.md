# The tally, against a number somebody already had

`luu count` over [`scripts/tasks/edit-reread.txt`](../../../scripts/tasks/edit-reread.txt),
in the same two arms [`2026-09-19.edit-reread-mock`](../2026-09-19.edit-reread-mock/README.md)
drove one day earlier, with the same seventeen replies. **This run measures the
instrument and not the corpus**, which is why it is the same corpus: an
instrument is checked against a number somebody already has.

Argued in [`one-counting-surface`](../../2026-09-19.one-counting-surface.completed.md).

## It reproduces the table that was counted by eye

That run's README carries four columns. Three of them came out of a person
reading the recording; the fourth came out of `grep … | wc -l`. Here they are as
a command:

| arm | `diverged` lines | renders | asking | paths |
| --- | ---: | ---: | ---: | --- |
| fragments only | 8 | 8 of 13 | 2 | `src/greeting.rs:1-10` |
| `--select-tokens 1024` | 16 | 8 of 13 | 2 | `…:1-10`, `…:8-10` |

Cell for cell what the 18th's README says, off the same corpus and through a
different substrate — the tally moved from `&[RecordLine]` to the fold on its
way into `Counts`, and the numbers did not move with it. That is the whole of
what this arm of the run is for.

## And the line nothing could produce before format 14

```
                     renders   turns  asking   spans   tokens
  fragments only           7       6       1       7      854
  --select-tokens 1024    11      31       5      43    2 466
```

**Rule A's own saving, on a corpus built to ask about something else.** Until
`record::FORMAT` 14 this number was computed inside the render, subtracted from
the `history` bucket and dropped — so a run that collapsed forty-three spans and
one that collapsed none left the same evidence.

Two things in it that are worth more than the totals:

**The selected arm collapses six times as many spans as the typed one**, off
the same thirteen turns. Nobody typed those spans: the selector re-scores the
same prompt-relevant file every turn and re-chooses it, and rule A is what stops
each choice being sent again. That is
[`a-span-is-rendered-once`](../../2026-09-07.a-span-is-rendered-once.completed.md)'s
own justification — *9.2% of the code tokens in the precision corpus are a span
some earlier turn already put in the window* — showing up as the arm that has a
selector in it, rather than as a figure in a record.

**`repeated` is 7 renders where `diverged` is 8**, in the same session, and the
gap is item 19's mechanism as two numbers rather than as a paragraph. Rule A
collapses the spans it can see and cannot collapse the ones that moved: a
fragment's identity is its path *and* its bytes, so the one case where a span
genuinely needs re-reading is exactly the case the dedup is blind to. The
session where the rule fired hardest is the session that contradicted itself
most. Pinned in `crates/luu/tests/edit_reread_probe.rs` as
`rule_a_collapses_the_spans_it_can_see_and_not_the_ones_that_moved`.

## What the run does not say

Nothing here is about a model. The edits are scripted, so every number is a fact
about the corpus, exactly as the 18th's run said of its own — and the instrument
inheriting that limitation is the point: it counts, it does not judge. *Is 8 of
13 a lot* is still the question item 19's fix waits on, and it still needs a
machine with a model on it.

## Files

- [`run.sh`](run.sh) — both arms, against a fresh copy of the committed template
  each time.
- `fragments.count.txt`, `selected.count.txt` — `luu count`, as a person reads it.
- `fragments.counts.json`, `selected.counts.json` — the same tally as `luu
  export` writes it into the static twin, copied out of
  `sessions/<id>/counts.json`. Both are here because a mirror that disagrees
  with what it mirrors is the thing `luu export` exists to not be.
