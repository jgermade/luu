# The tree `edit-reread.txt` runs against

A fixture, not a crate: nothing in this workspace builds it, it has no
`Cargo.toml`, and the only thing that reads it is
[`../edit-reread.txt`](../edit-reread.txt) — the corpus that edits a file and
then asks about it again.

**It is a template and is never run in place.** Every prompt in that corpus
that changes a file changes it for the rest of the session, so a run copies
this directory to a scratch tree and works there; running it here would leave
the next run reading the last one's edits, and the corpus measures what a
*session* does to a window rather than what a week of runs does to a fixture.
[`RECORD/2026-09-15.does-the-envelope-collapse-generalize.completed.md`](../../../RECORD/2026-09-15.does-the-envelope-collapse-generalize.completed.md)'s
run did the same thing against a template in `/tmp` that was never committed,
which is why that run cannot be repeated. This one is in the tree.

## What each file is for

| file | the corpus does this to it |
| --- | --- |
| `src/greeting.rs` | reads it, edits it twice, asks again after each — the case item 19 is about |
| `src/format.rs` | reads it twice and never edits it — the control, which must *not* be reported |
| `src/banner.rs` | reads it once, edits it, never asks again — stale and invisible to the detector |
| `src/tally.rs` | reads `1-6`, edits it, reads `1-16` — two overlapping ranges, which the detector counts as two paths |

The four are separate files on purpose: one file playing four parts would make
every divergence depend on every edit, and the control is only a control while
nothing else in the session can touch it.

## Why there is no `lib.rs`

Four files and no module root, which is not tidiness. A fixture inside this
repository is inside **this repository's own repo map**: `walk_sources` skips
dot-directories and `target/` and nothing else, so these files are candidates
for `luu map` and `luu select` here as much as anywhere. Four leaf files cost
that nothing measurable — every verdict in `select_probe`'s 38 questions is
identical with them and without them. A fifth file carrying `pub mod banner;`
and three more like it cost **two first-place hits and four top-3 hits** on the
probe's graph-hop arm, because those lines are edges and the graph signal
follows them. The measurement is in
[`RECORD/2026-09-19.a-corpus-that-edits.completed.md`](../../../RECORD/2026-09-19.a-corpus-that-edits.completed.md)
§What it cost; `the_corpus_tree_is_a_fixture_and_not_a_crate` is what keeps the
file from coming back.

## Why Rust, and why it is small

`agent_core::repo_map` indexes `.rs` and nothing else, so a template in any
other language can be grounded by `## fragment:` and can never be *chosen* by
`luu select`. Relevance selection is the mechanism that re-reads a span without
anybody typing its name, which is the path a real session takes, so the
template has to be something the map can see.

Small because the corpus grounds it by line range: a range past the end of a
file is an error at run time, and a fixture nobody reads is a fixture that
rots. Four files, every one of them named in the corpus, every range checked by
`every_script_names_a_file_that_exists`.
