# Evidence: the repeat-once precision run of 2026-09-14

The scored transcripts behind
[`../../2026-09-14.the-7b-does-not-miss-it.completed.md`](../../2026-09-14.the-7b-does-not-miss-it.completed.md).
Two files, one per arm — `off.txt` and `repeat-once.txt` — each the full
streamed output of one `luu chat --script` run: `scripts/tasks/grounded.txt`,
20 turns, one shared history, against `qwen2.5-coder:7b` over Ollama on the
M1 Pro (machine 1).

```sh
for arm in "off:" "repeat-once:--repeat-once"; do
  target/release/luu chat --script scripts/tasks/grounded.txt \
    --backend ollama --model qwen2.5-coder:7b \
    --context-limit 8192 --select-tokens 1024 --reserve 512 \
    --tokenizer ~/models/qwen2.5-coder-7b/tokenizer.json \
    --temperature 0 --seed 7 --sandbox luu.toml \
    --record ${arm%%:*}.jsonl ${arm#*:} > ${arm%%:*}.txt
done
```

Same corpus, same flags, same model as the mock run in
[`a-span-is-rendered-once`](../../2026-09-07.a-span-is-rendered-once.completed.md#what-it-does-on-the-corpus-that-could-not-run-until-today),
run one flag apart. `.jsonl` recordings are not committed — `*.jsonl` is
gitignored project-wide, and here as there the recording is 98% the tool
prefix repeated 20 times; the two files above are the evidence, and every
number in the record is read off them or off the `usage`/`budget`/
`prefix_reuse` lines of the (uncommitted) recordings, reproducible by rerunning
the command above.

## Verdicts

`grounded.txt` gives each question's answer in the fragment it attaches — see
the three files it quotes: `crates/agent-core/src/context.rs:1-15`,
`crates/agent-core/src/job.rs:1-12`, `luu.toml:1-26`. `right` matches that
text; `wrong` contradicts or fabricates against it; `partial` is on-topic and
plausible but not what the fragment actually says, or not fully precise.
Group D repeats questions from groups A–C with no fragment attached, so the
window and eviction are all it has to answer from — exactly the case rule A
turns off if it happens to be discarding a needed repeat.

| # | question | off | `--repeat-once` |
| --: | --- | :-: | :-: |
| 1 | two commitments this file opens with | right | right |
| 2 | where selected code gets fused, and why | right | right |
| 3 | what "decide, then render" forbids | right | right |
| 4 | which record it points at | **wrong** | **wrong** |
| 5 | one change that breaks prefix reuse | partial | partial |
| 6 | three jobs one task boundary does | right | right |
| 7 | "closing is an event, not a mutation" | right | right |
| 8 | what reopening does to a task's turns | right | right |
| 9 | the two records it cites, and for what | right | right |
| 10 | session: a sequence of tasks, or a mode? | right | right |
| 11 | why there is no deny list | right | right |
| 12 | `enforcement = "kernel"`, Landlock missing | **wrong** | **wrong** |
| 13 | why commands are program names, not shell strings | partial | partial |
| 14 | which programs the policy allows | right | right |
| 15 | network access, and how a verdict reports it | **wrong** | **wrong** |
| 16 | *(review, no file)* the two commitments again | **wrong** | **wrong** |
| 17 | *(review, no file)* the three jobs again | **wrong** | **wrong** |
| 18 | *(review, no file)* which programs again | right | right |
| 19 | *(review, no file)* `enforcement = "kernel"` again | **wrong** | **wrong** |
| 20 | *(review)* which of the last four came from the text | partial | partial |

**11 right, 3 partial, 6 wrong — the same 11, the same 3, the same 6, in both
arms.** Not one question flipped verdict between `off` and `--repeat-once`.

## The aggregates behind the record's table

Read from `usage` on every `ended` line and from `budget`/`prefix_reuse` on
the trace channel of the (uncommitted) `.jsonl` recording, summed over the 20
turns of each arm:

```
                       off      --repeat-once     Δ
prompt tokens        129 970       124 321      −4.4%
code bucket            20 962        17 297     −17.5%
history bucket          98 720        96 666     −2.1%
cuts (evictions)            12            11
mean prefix reuse         33.4%         39.1%
completion tokens        1 549         1 874
```

`completion tokens` is not a saving — a smaller prompt did not produce shorter
answers here, it produced *slightly longer* ones in aggregate (1 549 → 1 874),
which is a data point against "a smaller prompt makes the model terser," not
for it.
