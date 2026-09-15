# Evidence: the schema-retry loop, against the model that has drift to rescue

The transcripts behind the 2026-09-15 "the retry shape, built" section
appended to
[`../../2026-09-06.a-grammar-for-tool-calls.WIP.md`](../../2026-09-06.a-grammar-for-tool-calls.WIP.md).
`qwen2.5-coder-7b` over `llama-server`, machine 4, `--constrain schema` —
now `agent_core::agent::SchemaRetry`: an unconstrained first attempt, and a
second, schema-forced one only when the first scores `CallVerdict::Drifted`
(fenced under a tool's own name, not `` ```tool ``). Same corpus, machine
and settings as
[`../2026-09-15.7b-argv-schema/`](../2026-09-15.7b-argv-schema/), the
unconstrained baseline this run is measured against.

```sh
./run.sh   # from the repository root, with llama-server already serving the
           # 7B on :8080
```

## The instrument doesn't see the retry, and says so wrong

`score_call`, run against `.tmp/extract_first_reply.py`'s output the way
every prior probe in this record has been scored, reports:

| verdict | count |
| --- | ---: |
| Parsed | 6 / 15 |
| Drifted | 6 / 15 |
| Continued past the fence | 1 / 15 |
| No call | 2 / 15 |

**Identical to the unconstrained baseline's own 6/6/1/2 split** — which
looks, at a glance, like the retry did nothing. It isn't that. The
extraction script concatenates every `token` event for the turn until the
first `tool_call` or `ended`, and `TurnEvent::Token` carries no marker
between one model call and the next — the retry's own `ModelCall` event
exists (`agent.rs` announces it before making the call, same as the first
attempt) but is deliberately not written into the recording
(`TurnEvent::ModelCall { .. } => {}` in the printer, unchanged by this
work). So a reply that drifted and then got a real, successful retry reads
as one string: the drifted fence, immediately followed by the retry's bare
JSON. `score_call`'s own `Drifted` check (a fence tagged with a tool's own
name) matches the *first half* of that string and returns `Drifted` — true
of the string, false of what happened.

Prompt 2's full extracted text makes this legible on its own:

````text
```list_dir
{"path": "crates/agent-core/src"}
```{
  "arguments": {
    "path": "crates/agent-core/src"
  },
  "name": "list_dir"
}
````

The first line is the unconstrained drift; everything after the closing
` ``` ` is the schema-forced retry — bare JSON, no fence, because
`response_format` output is a document, not a chat reply that happens to
contain one. `score_call` sees only the shape of the first half.

## What actually happened: read from execution, not from text shape

Every `.txt` transcript records whether a call was executed
(`  [1] → tool_name {...}`) and how the turn ended. That is retry-blind by
construction — it is downstream of whatever the loop decided to do — and is
the honest measurement here.

| prompt | unconstrained: first attempt executed a call? | `--constrain schema`: first attempt executed a call? |
| --- | :---: | :---: |
| 01 | no | no |
| 02 | no | **yes** |
| 03 | yes | yes |
| 04 | no | **yes** |
| 05 | no | **yes** |
| 06 | yes | yes |
| 07 | no | **yes** |
| 08 | yes | yes |
| 09 | no | **yes** |
| 10 | no | no |
| 11 | yes | yes |
| 12 | no | **yes** |
| 13 | yes | yes |
| 14 | yes | yes |
| 15 | yes | yes |

**7/15 → 13/15.** Every one of the six prompts `score_call` calls `Drifted`
(02, 04, 05, 07, 09, 12) went from *no tool ever touched* — the unconstrained
turn returns the malformed fence as its final answer, because `parse_call`
can't read it and the loop has no way to try again — to a correctly-shaped
call executed on the very first real attempt, `argv` and all. 6/6 recovered,
0 retries that failed to produce a parseable call. The two genuine declines
(01, 10 — the model answers in prose without reading anything, right or
wrong on the merits, but not a format failure) are untouched, exactly as
designed: `score_call`'s `NoCall` never triggers a retry, because a schema
cannot tell "chose not to call" from "forgot to," and forcing 01 or 10 into
a call would be the "a constrained model cannot say no" cost
[`what-constrain-does`](../../2026-09-14.what-constrain-does.completed.md)
already named.

**No new failure mode.** Four prompts (04, 08, 09, 14) end in
`[tool limit reached]` — the same "correct call, wrong troubleshooting
after a real failure" pattern the same-corpus 14B run already named for 04
and 14 (guessing at `cargo` flag orderings after a genuine `exit 1`), not a
retry defect: three of those four (04, 09, 14) are *retried* prompts that
would previously have made zero calls at all, so reaching the tool step
limit by actually working the problem is strictly more progress than
returning a fenced string as if it were the answer.

**No regression for `--constrain schema`'s old failure mode.** The
always-on schema arm measured in
[`what-constrain-does`](../../2026-09-14.what-constrain-does.completed.md)
never answered a single one of fifteen prompts — every run ended
`[tool limit reached]` after eight identical calls, because a model that
cannot say no just keeps calling once a result is in hand. None of that
here: 11/15 end `[stop]`, and the four that don't end there for the reason
named above, not because the schema trapped them in a loop.

Raw artifacts: [`stats.json`](stats.json) carries both the (misleading,
kept for the record) `score_call` verdict and the
`first_attempt_executed_a_call` / `turn_end_reason` fields the table above
is built from.
