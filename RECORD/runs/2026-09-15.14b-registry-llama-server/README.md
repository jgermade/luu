# Evidence: the registry's 14B GGUF, over `llama-server` directly

The transcripts behind the new section appended to
[`../../2026-09-06.a-grammar-for-tool-calls.WIP.md`](../../2026-09-06.a-grammar-for-tool-calls.WIP.md)
that re-runs the tool-call-format numbers against the Ollama registry pull's
14B GGUF blob
(`sha256-ac9bc7a69dab38da1c790838955f1293420b55ab555ef6b4615efa1c1507b1ed`),
**loaded straight into `llama-server`** rather than through Ollama — isolates
the file from the backend, which
[`../2026-09-15.14b-ollama-registry/`](../2026-09-15.14b-ollama-registry/)
changed at the same time as the file. Same flags as
[`../2026-09-15.14b-tool-call-probe/`](../2026-09-15.14b-tool-call-probe/)
(the local HuggingFace GGUF, same backend): `--ctx-size 8192
--n-gpu-layers 99`, `--temperature 0 --seed 7`.

```sh
llama-server -m ~/.ollama/models/blobs/sha256-ac9bc7a69dab38da1c790838955f1293420b55ab555ef6b4615efa1c1507b1ed \
  --ctx-size 8192 --n-gpu-layers 99 --host 127.0.0.1 --port 8080 \
  --alias qwen2.5-coder-14b
./run.sh   # from the repository root
```

Same extraction and scoring method as every other tool-call-probe run (see
[`../2026-09-15.14b-tool-call-probe/README.md`](../2026-09-15.14b-tool-call-probe/README.md)
for the reproduction steps, `2026-09-15.14b-registry-llama-server` substituted
for the output directory).

| prompt | verdict |
| --- | --- |
| 01 | Parsed |
| 02 | Parsed |
| 03 | Parsed |
| 04 | Parsed |
| 05 | Parsed |
| 06 | Parsed |
| 07 | Parsed |
| 08 | Parsed |
| 09 | Parsed |
| 10 | Parsed |
| 11 | Parsed |
| 12 | Parsed |
| 13 | Parsed |
| 14 | ContinuedPastFence |
| 15 | Parsed |

14/15 Parsed, 0/15 Drifted, 1/15 ContinuedPastFence, 0/15 NoCall — 93%
clean-call, **identical count and failing prompt** to
[`../2026-09-15.14b-ollama-registry/`](../2026-09-15.14b-ollama-registry/)'s
number on the same file over Ollama. Backend does not move this number for
the registry file, same conclusion
[`the-drift-was-never-the-backend`](../../2026-09-15.the-drift-was-never-the-backend.completed.md)
reached for the 7B.

**This run's transcripts already show the post-argv-fix `run_command`
schema** (`{"name": "run_command", "arguments": {"argv": [...], "cwd": "."}}`,
e.g. `09.txt`) — because the binary that ran it postdates commit `82229cc`
("run_command's argv: one array instead of command+args closes the envelope
collapse", 11:38) same as
[`../2026-09-15.14b-ollama-registry/`](../2026-09-15.14b-ollama-registry/)'s
run did (`8280a0a`, 15:21). **The original local-file number this compares
against
([`../2026-09-15.14b-tool-call-probe/`](../2026-09-15.14b-tool-call-probe/),
12/15 = 80%) does not** — it was run at `714eca7` (11:11), before the fix.
See
[`../2026-09-15.14b-local-postfix/`](../2026-09-15.14b-local-postfix/) for
the local file re-run against the current binary, which separates that
confound from the file-provenance question this run was built to answer.
