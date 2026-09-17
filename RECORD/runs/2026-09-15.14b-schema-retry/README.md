# Evidence: the schema-retry loop, against a model with nothing to rescue

The transcripts behind the same 2026-09-15 "the retry shape, built" section
as
[`../2026-09-15.7b-schema-retry/README.md`](../2026-09-15.7b-schema-retry/README.md).
`qwen2.5-coder-14b` over `llama-server`, machine 4, `--constrain schema`
(`agent_core::agent::SchemaRetry`). A negative result, and worth keeping:
the 14B's unconstrained first attempt
([`../2026-09-15.14b-argv-schema/`](../2026-09-15.14b-argv-schema/)) already
parses 12/15 clean with 0 `Drifted`, so there is nothing here for the retry
to fire on — every reply in `stats.json` carries `retry_engaged: false`,
confirmed the same way
[`../2026-09-15.7b-schema-retry/README.md`](../2026-09-15.7b-schema-retry/README.md)
confirms a retry did fire there: no reply's extracted text contains more
than one fenced block, and every prompt's `score_call` verdict here matches
its own unconstrained run exactly (12 Parsed, 0 Drifted, 3
`ContinuedPastFence`, 0 `NoCall` — see
[`../2026-09-15.14b-argv-schema/README.md`](../2026-09-15.14b-argv-schema/README.md)
for the unconstrained numbers this reproduces unchanged).

```sh
./run.sh   # from the repository root, with llama-server already serving the
           # 14B on :8080
```

**What this run is actually worth**: proof that `--constrain schema` no
longer breaks this model either. Before this work,
[`what-constrain-does`](../../2026-09-14.what-constrain-does.completed.md)
measured the always-on schema arm ending `[tool limit reached]` on every one
of fifteen prompts, on this same model, regardless of whether a call was
warranted. Here, 12/15 end `[stop]`, byte-for-byte the same twelve as the
unconstrained run — the three that end `[tool limit reached]` (04, 09, 14)
are the pre-existing `ContinuedPastFence` prompts where the call itself
parsed fine and the model then guessed at `cargo` flag orderings after a
real `exit 1`, the same pattern
[`the-same-corpus-same-scorer 14B number`](../../2026-09-06.a-grammar-for-tool-calls.completed.md)
already named — not a schema-retry defect, since no retry ever fired on
this run at all. The retry-only shape costs nothing when there is nothing
to retry, which is the property the design was for.
