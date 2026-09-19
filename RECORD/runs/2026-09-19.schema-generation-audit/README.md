# Evidence: which tool schema each recorded run was made against

The tables behind
[`../../2026-09-19.the-7b-numbers-audited.completed.md`](../../2026-09-19.the-7b-numbers-audited.completed.md),
which answers item 8 of [`ROADMAP/2026-09-17`](../../../ROADMAP/2026-09-17/README.md).

**Nothing was run against a model for this.** It reads the runs already on
disk, which is the whole of what makes it cheap and the whole of what limits
what it can say.

- **`audit.sh`** — classifies every run directory by the `run_command` envelope
  its committed transcripts carry: `{command, args, cwd, timeout_ms}` before
  `82229cc` (2026-09-15 11:38:17 +0200), one required `argv` array after.
  Writes `schema-generation.json`.
- **`pair.py`** — the two pre/post pairs the committed evidence supports, prompt
  by prompt. The **14B** pair
  ([`../2026-09-15.14b-tool-call-probe/`](../2026-09-15.14b-tool-call-probe/README.md)
  against [`../2026-09-15.14b-local-postfix/`](../2026-09-15.14b-local-postfix/README.md))
  is one flag apart — same file, machine, backend, flags and day, two binaries —
  so it measures the schema. The **7B** pair
  ([`../2026-09-14.tool-call-probe/`](../2026-09-14.tool-call-probe/README.md)
  against [`../2026-09-15.7b-ollama-registry/`](../2026-09-15.7b-ollama-registry/README.md))
  holds the weights, the engine, the corpus and the flags and still moves the
  machine, the Ollama version and the day, so it compares. Writes
  `verdicts.json`.

```sh
./RECORD/runs/2026-09-19.schema-generation-audit/audit.sh   # from the repository root
./RECORD/runs/2026-09-19.schema-generation-audit/pair.py
```

Both are committed rather than regenerated per run — unlike the scratch
scorers [`../2026-09-14.tool-call-probe/`](../2026-09-14.tool-call-probe/README.md)
and the schema-retry runs used — because the instrument *is* the finding here.
Re-running either after a new run lands re-answers the question for that run;
a scratch script would have had to be re-derived to do it.

## What the classifier reads, and what it therefore cannot see

It reads **what the model emitted**, not what the schema said, because no run
directory records the binary that made it. That is sound only while a run is
unanimous, and all fifteen classified runs are: every one is 100% one envelope
shape, none mixed. A run whose model ignored the schema on every call would be
misread, and there is none here.

Eleven run directories commit no `run_command` call at all and are listed under
`no_run_command_call_committed`. None of them is a tool-call-format run on the
7B, so item 8's question is covered; `82229cc` touched `run_command`'s schema
and no other tool's, so a run that never called it could not have carried the
confound either way.
