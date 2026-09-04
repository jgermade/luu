# Sessions in SQLite, derived from the record

The oldest blocker with nothing in front of it. `serve` loses the conversation on
restart, so there is no long task to resume and nothing to transfer.

## The rule it has to obey

From [`luu-design.md`](../../luu-design.md) §Persistence, and it is the whole
design of this item:

> Whatever the store holds must be reproducible by folding the record.

So the store is a **cache of a fold, not a second log**. The JSON-lines stream
stays the account of what happened; SQLite holds what `api::SessionView` already
computes from it, so that resuming does not mean recomputing context from scratch.

## The acceptance test, which is the interesting part

The parity that already exists between the live server and the static mirror,
applied to the store:

```
fold(record)            -> SessionView   ==   load(store, session_id) -> SessionView
```

Same structure, same fold, asserted for every fixture in `site/fixtures`. A store
that can drift is a store that will, and this is the assertion that makes the
drift a test failure instead of a support question.

## What it must not accumulate

Anything the events cannot regenerate. Concretely, and each of these is a
temptation:

- A turn's fused prompt. `code_context` stays separate from the prompt per the
  fusion rule; store the rendering and a resumed session either recomputes
  everything or sums two counters into one bar.
- A token count without its `Counter`.
- The floor as a number with no `evicted` line behind it. Forgetting is an event,
  and it is on the wire since protocol v3.

## Open

Where the database lives (`~/.luu/` in the draft, against `luu.toml` sitting in
the project — they answer different questions and probably both exist);
compression; whether the record file is the write-ahead log or a sibling artifact;
and what happens to a session whose record was truncated or lost while the store
still has the fold.
