# The edit-and-re-read corpus, against the mock

Two arms of [`scripts/tasks/edit-reread.txt`](../../../scripts/tasks/edit-reread.txt),
one flag apart, driven by [`run.sh`](run.sh) against a fresh copy of the
committed template each time. **The mock, not a model** — the edits are
scripted, so what this measures is the corpus and not how often a model edits a
file it has quoted. That run is what the instrument exists for and it needs a
machine with a model on it.

Argued in
[`a-corpus-that-edits`](../../2026-09-19.a-corpus-that-edits.completed.md); the
defect it produces is
[`one-path-two-bodies`](../../2026-09-19.one-path-two-bodies.completed.md).

| arm | `diverged` lines | renders | paths | asking |
| --- | ---: | ---: | --- | ---: |
| fragments only | 8 | 8 of 13 | `src/greeting.rs:1-10` | 2 |
| `--select-tokens 1024` | 16 | 8 of 13 | `src/greeting.rs:1-10`, `src/greeting.rs:8-10` | 2 |

## The number, and it is not the one that was expected

**Two edits left 8 of 13 prompts contradicting themselves**, which is 62% of
the session. The corpus edits `src/greeting.rs` on turns 5 and 11 and asks
about it on 6 and 12, and it was built expecting those two asking turns to be
the finding. They are 2 of the 8. The other six are turns 7 to 11 and 13, which
attach nothing of that file and carry the contradiction anyway — because it is
**in the history**: turn 1's `code_context` was written when turn 1 closed and
every later prompt renders it beside turn 6's.

So a divergence is not an event at the turn that causes it. It is a state the
window enters and does not leave until eviction takes the stale turn out. The
frequency [`one-path-two-bodies`](../../2026-09-19.one-path-two-bodies.completed.md)
§What is built says the fix waits on is therefore not *how many edits a session
makes*; it is *what share of a session's prompts go out after the first one*.

`asking` is 2 and not 8, and the gap is what that field is for: only turns 6
and 12 disagreed with the disk as it was when they were sent. The other six
disagreed with a version of the file that no longer existed anywhere.

## What the second arm adds

`--select-tokens 1024` doubles the lines without changing the renders: the
selector re-reads `src/greeting.rs:8-10` — `greet` itself, **a span nobody
typed** — beside the `1-10` the corpus attaches, and that span diverges on
exactly the same eight turns. It is the case the arm exists for, because a
divergence produced by relevance selection is one a session produces on its
own, with no `## fragment:` in the script.

It is also the reason the template is Rust: `agent_core::repo_map` indexes
`.rs` and nothing else, so a scratch project in any other language can be
grounded by hand and never *chosen*.

## The three silences

None of `src/format.rs`, `src/banner.rs` or `src/tally.rs` appears in either
arm, for three different reasons, and the runs leave the tree where the edits
say they should: `greet` returns `"Hey, {name}"`, the banner is `"luu 0.2"`,
`THRESHOLD` is 32 and `WIDTH` is untouched at 12.

- **`format.rs` is the control.** Read twice, never edited, so the two renders
  are one body and rule A collapses them. A detector that named this file would
  be naming every file.
- **`banner.rs` is a blind spot.** Edited after its only read, so the window
  keeps sending bytes that are no longer on disk and nothing contradicts them.
  The worst case in the session and the one the detector cannot see.
- **`tally.rs` is the other blind spot.** Grounded `1-6` before the edit and
  `1-16` after; both ranges carry line 4 and disagree about it, and the
  detector's notion of a path is the whole spec string, so it counts two paths
  rather than one path with two bodies. Named in that record's §Still open, and
  reproducible here rather than only argued.

## Reading the evidence

- [`fragments.txt`](fragments.txt), [`selected.txt`](selected.txt) — the
  transcripts, 13 turns each.
- [`fragments.diverged.txt`](fragments.diverged.txt),
  [`selected.diverged.txt`](selected.diverged.txt) — every `diverged` line,
  which is what the table above counts.

`.jsonl` recordings are gitignored project-wide, so the lines are extracted
rather than kept. `crates/luu/tests/edit_reread_probe.rs` is the same run as an
assertion, so the numbers above are checked on every commit rather than only
here.
