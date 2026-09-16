# Roadmap — revision 2026-09-09

**What this is:** the order of work as it stands on this date, and what blocks
what. Not a decision and not a description of the tree — for *what is true today*
read [`luu-design.md`](../../luu-design.md), and for *why* read the dated file
in [`RECORD/`](../../RECORD/) each item links to.

Supersedes [`ROADMAP/2026-09-08`](../2026-09-08/) wholesale, and it is the
shortest supersession this project has had: that revision was reordered inside
its own day around the surfaces a person works in, **all five rows of its
section A closed within the day**, and what is left of it is section B, untouched
and unreordered. This revision is that section, made into an order of its own,
plus the six things one night of driving those surfaces left open — gathered in
[`RECORD/2026-09-09.state-of-play.completed.md`](../../RECORD/2026-09-09.state-of-play.completed.md),
which is the file to read before this one.

**The finding this revision inherits, and the reason its first row is where it
is.** Three bugs were found on 2026-09-08 and none of them was found by a test:
a plan that granted a file could not read it, the page could not open a session
at all, and a posture's name vanished when it was `null`. All three were found by
*driving the thing* — and the two rows below that need a model are the same
shape of unbought answer, one afternoon each, waiting on a box rather than on a
design.

**What has not moved is what hurts.** Item 1 was called the cheapest unbought
answer in the project on 2026-09-06, was ordered behind four rows of UI work on
2026-09-08, and is still unbought on 2026-09-09. `--repeat-once` is off by
default for two reasons; one of them was measured and the other has never been
asked. It stays at the top of this order until a machine answers it. **It was:
2026-09-14, machine 1, struck through below.**

## What landed since 2026-09-08

| | |
| --- | --- |
| **A granted file is not a directory** | `PathCheck::beneath` resolves a root asked for by its own name as its last component — a regression in the tree since 2026-09-05, on every platform, at the gate — [`the-surfaces-first`](../../RECORD/2026-09-08.the-surfaces-first.completed.md) |
| **A browser test of the gate** | `tests/smoke/gate.spec.js` drives one whole job on the mock in a second, in CI; it found that `record::FORMAT` had drifted to 8 in Rust and stayed 7 in the browser, so the page could not open a session at all — [`a-test-that-clicks-approve`](../../RECORD/2026-09-08.a-test-that-clicks-approve.completed.md) |
| **The panel keeps the turn** | One shape per turn, a picker in the inspector, history from the API on connect and appended as each turn ends — [`the-panel-keeps-the-turn`](../../RECORD/2026-09-08.the-panel-keeps-the-turn.completed.md) |
| **The compaction log** | `job_closed` carries `replaced {turns, tokens, summary_tokens}`, counted at the close with the counter that counted the summary. Record format 9 — [`what-a-fold-writes-down`](../../RECORD/2026-09-08.what-a-fold-writes-down.completed.md) |
| **The container, on Linux and from the surface** | `scripts/container-check.sh` and the `container` job hold Landlock ABI v7 + seccomp inside stock Docker on a runner; `[posture.<name>]` in `config.toml`, a picker in the starter dialog, record format 10 — [`the-container-on-a-runner`](../../RECORD/2026-09-08.the-container-on-a-runner.completed.md), [`a-session-picks-its-executor`](../../RECORD/2026-09-08.a-session-picks-its-executor.completed.md) |
| **Prune behind (rule B)** | Landed 2026-09-08, off by default. The saving is not the one predicted: −0.3% of prompt tokens and **nothing evicted** — 0 turns dropped against 13. What the flag buys is what the same prompt is made of — [`prune-behind`](../../RECORD/2026-09-08.prune-behind.completed.md) |

## The order

**Section A — the two afternoons that need a model.** Neither is a design
question; both are a box, a flag and a corpus that already exist.

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 1 | ~**Whether a 7B misses the repetition** — `--repeat-once` one flag apart against a model. Rule A is off for two reasons and this closes the second: position inside the *bucket* does not matter, position across the *window* has never been asked. **One revision late**~ **measured 2026-09-14 against `qwen2.5-coder:7b` on machine 1: verdict-for-verdict identical between `off` and `--repeat-once` on all 20 questions of `grounded.txt` (11 right / 3 partial / 6 wrong, unchanged), while the `code` bucket fell 17.5% — replicating the mock's −19.5% almost exactly. The measurement is closed; whether the default flips is a separate, unmade decision** | nothing further; the decision, if anyone wants to make it | [`the-7b-does-not-miss-it`](../../RECORD/2026-09-14.the-7b-does-not-miss-it.completed.md), from [`a-span-is-rendered-once`](../../RECORD/2026-09-07.a-span-is-rendered-once.completed.md) §Still open |
| 2 | **A model inside a container** — every contained run in this repository, on the Mac and on the runner, is the mock. The trigger `luu.container.toml` names for narrowing its network is the first `run_command` inside one with a model in the loop, and nothing has ever exercised it | machine 4, which is the only box that is both Linux and a model | [`a-session-picks-its-executor`](../../RECORD/2026-09-08.a-session-picks-its-executor.completed.md) §Still open |

**Section B — the window, which is where the code is.**

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 3 | ~**Tool results are only capped** — an 8 KiB `cat` and a 1 000-token fragment are the same object to the window, and neither `Repeat` nor rule B reaches `ToolStep` today. The shape rule B settled on — a ratchet, a citation, and the eviction policy's own target — is the one a tool result reuses, so this is the cheapest row in the revision~ **landed the day this revision was written, as rule C: `--prune-results`, off, and inert without `--prune-behind`. It reaches the case rule B cannot — a turn that ran a tool and selected nothing has no spans to give — and it has no number: nothing has been run one flag apart, which is now the row below.** Found under it: `job_closed`'s `replaced.tokens` had been over-reporting a pruned turn since rule B landed on 2026-09-08, counting what its turns were *stored* at rather than what the prompt they left was made of. Fixed in the same commit | nothing | [`what-a-result-costs`](../../RECORD/2026-09-09.what-a-result-costs.completed.md), from [`what-leaves-the-history`](../../RECORD/2026-09-06.what-leaves-the-history.completed.md) §Still open |
| 3b | ~**What rule C is worth** — `--prune-results` one flag apart, the way `--repeat-once` and `--prune-behind` were both measured. The rule the tests describe is *the prompt is smaller and the conversation is intact*; how much smaller is unmeasured, and a rule built without a run is the thing item 1 is a warning about.~ **measured 2026-09-14 on the mock, using `--mock-cycle`: `--prune-behind` alone changes nothing on a tool-only corpus (0 of 15 prune moves freed a token — it walks `code_context`, which this corpus has none of); adding `--prune-results` freed 32 752 tokens over 20 turns and, sharper than rule B's own finding, eliminated eviction entirely — 16/20 turns evicted whole without it, 0 with it, all 20 exchanges still in the prompt at turn 20. Whether a model minds losing tool output to a citation is still unmeasured** | nothing further for the mechanics; a model, for the accuracy question | [`what-rule-c-is-worth`](../../RECORD/2026-09-14.what-rule-c-is-worth.completed.md), from [`what-a-result-costs`](../../RECORD/2026-09-09.what-a-result-costs.completed.md) §Still open |
| 4 | ~**A probe for tool calls** — N prompts that each require exactly one call, scored *parsed* / *drifted* / *no call* / **continued past the fence**, the fourth being what the precision run found by accident and no existing probe counts. **The harness is written** (2026-09-14): `agent_core::tools::score_call`, a fifteen-prompt corpus (`scripts/tasks/tool-call-probe.txt`), and `crates/luu/tests/tool_call_probe.rs` proving the scorer against `--mock-cycle` recovers all four verdicts — [`the-tool-call-probe`](../../RECORD/2026-09-14.the-tool-call-probe.completed.md). Section A's kind of work is what is left: running it against a model~ **run 2026-09-14 against `qwen2.5-coder:7b` on machine 1, fifteen independent one-shot prompts: 3 Parsed, 2 Drifted, 2 Continued past the fence, 8 No call — a 20% clean-call rate. Two of the eight No calls are a shape `score_call` does not name (a correctly-tagged `\`\`\`tool` fence around the wrong JSON shape; a fence tagged with neither `tool` nor a tool's own name), both falling through to No call by the algorithm as specified. Six of the eight are not refusals but confidently wrong answers the model gave instead of calling anything** | nothing further; item 5's first move, next | [`the-tool-call-probe-run`](../../RECORD/2026-09-14.the-tool-call-probe-run.completed.md), from [`the-tool-call-probe`](../../RECORD/2026-09-14.the-tool-call-probe.completed.md) |
| 5 | ~**A grammar for tool calls** — the parse reads the block correctly; what fails is that generation does not stop. Narrowed to tool calls alone, the plan block stays as it is~ **landed 2026-09-14: `Constraint`, `--constrain grammar\|schema` on `chat`, both arms measured through the real CLI against a standalone `llama-server`. `grammar` (`agent_core::grammar::compile`, fixed after an intermediate version measured to be a no-op — see `the-alternation-that-was-not-one`): 7/15 clean calls against 3/15 unconstrained, fence-continuation eliminated outright, drift-by-tool's-own-name untouched (different trigger), and one new cost found — a corrected shape can still drop the content a malformed one carried. `schema` (`Tools::call_schema`, `response_format`): applied to every call in a turn rather than as the one-off retry it was designed to be, it answered **0 of 15** prompts — every run exhausts `--max-tool-steps` calling a tool it is never allowed to stop calling. Both off by default** | nothing further for either arm as built; the retry loop `schema` needs, or drift-by-tool-name's own `avoid_until_forced` pass, are separate, unbuilt work | [`a-grammar-for-tool-calls`](../../RECORD/2026-09-06.a-grammar-for-tool-calls.WIP.md), [`the-bare-grammar-field`](../../RECORD/2026-09-14.the-bare-grammar-field.completed.md), [`the-alternation-that-was-not-one`](../../RECORD/2026-09-14.the-alternation-that-was-not-one.completed.md), [`what-constrain-does`](../../RECORD/2026-09-14.what-constrain-does.completed.md) |

**Section C — what the night left open.** Five rows, none of them ordered
before, all of them from
[`state-of-play`](../../RECORD/2026-09-09.state-of-play.completed.md) §Still
open. They are small, and being small is exactly why they need a line in a plan
rather than a sentence in a record nobody re-reads.

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 6 | **Nothing in the page reads the trace's `pruned` lines** — `/ws/trace` has a consumer for `prompt`, `prefix_reuse`, `budget` and `step_call`, and none for the lines rule B added. The panel exists to watch the window give way, and the one rule that makes it give way without losing a turn is invisible in it | nothing | [`prune-behind`](../../RECORD/2026-09-08.prune-behind.completed.md) |
| 7 | **What a container start costs** — *the worker starts with the session* was chosen on the shape of the thing and not on a number. The `container` job already makes the same call on both sides and is where the number comes from | nothing | [`a-session-picks-its-executor`](../../RECORD/2026-09-08.a-session-picks-its-executor.completed.md) §Still open |
| 8 | **A session resumed under a different posture is refused by nothing** — it is visible in the header and enforced nowhere. The store keeps jobs and their approved plans; it does not keep what they were approved *under*, so an approval granted inside a container can be replayed on the host | nothing; wants a record first | [`a-session-picks-its-executor`](../../RECORD/2026-09-08.a-session-picks-its-executor.completed.md) §Still open |
| 9 | ~**Enforcement per job** — `network` and `egress` narrow per job; `enforcement` is still session-wide. **Ordered since 2026-09-05 and still unargued**: the next move on it is a record, not a diff. The honest guess is unchanged — a plan may only ever narrow, and "narrow" is not obviously defined on a scale~ **landed 2026-09-16, and the guess held in a way this row could not see: the scale runs backwards. Every other field a plan declares is a grant, where less is narrower and a plan naming nothing gets nothing; `enforcement` is a strictness whose permissive value is the reassuring one, so a plan asking for `kernel` needs no permission and one asking for `best-effort` inside a `kernel` session is refused at the gate like any other thing the policy does not grant. The record was not the next move after all — it and the diff are the same age, both written 2026-09-06 on a branch that was never merged, and ported here unchanged. `limits` is now the last field a plan cannot narrow** | nothing | [`enforcement-per-job`](../../RECORD/2026-09-06.enforcement-per-job.completed.md) |
| 10 | **Nothing counts how often a plan grants a file rather than a directory**, which is the number that would say how ordinary the 2026-09-08 regression was — and therefore how much of the gate's surface has no test | nothing | [`the-surfaces-first`](../../RECORD/2026-09-08.the-surfaces-first.completed.md) |

**Section D — arguments, boxes and one thing that is not designed.**

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 11 | **A judge that is not the model under test** — every probe in this repository is scored by a key on disk or by a person reading replies. Scoring at corpus scale wants a judge, and a judge wants an argument before it wants an endpoint | needs a design argument | [`machines.md`](machines.md) P1 |
| 12 | **Concurrency** — `serve` runs one live session at a time; sessions are switched, not run side by side. A per-session `Agency` is the first half of what it would need and the rest is not designed. **Newly ordered, and the only row here that is a shape rather than a task** | needs a design argument | [`state-of-play`](../../RECORD/2026-09-09.state-of-play.completed.md) §Still open |
| 13 | **The BC-250's 14B ceiling** — ~9.4 GiB usable against a 14B Q4; the last unconfirmed ceiling in the inventory, and the only row in this revision that needs a box nobody has a hand on. The interesting half is whether the `-ctk`/`-ctv` trick that moved machine 4's ceiling pays on RADV/Vulkan | hardware and a hand on it | [`machines.md`](machines.md), from [`the-bc250-run`](../../RECORD/2026-09-04.the-bc250-run.completed.md) |
| 14 | **Rotating and revoking an approval key** — a compromised key is removed by editing `luu.toml` and restarting. Also: nothing signs a *recording*, so a reader that dropped lines is not detected | waits on a fleet being more than the boxes in one room | [`signed-approvals`](../../RECORD/2026-09-04.signed-approvals.completed.md) §Still open |

```mermaid
gantt
    title The order as of 2026-09-09
    dateFormat YYYY-MM-DD
    axisFormat %b %d
    section Needs a model
    Whether a 7B misses the repetition     :done, rep, 2026-09-09, 5d
    A model inside a container             :crit, ctr, after rep, 2d
    section The window
    Tool results are only capped           :done, steps, 2026-09-09, 1d
    What rule C is worth                   :done, worth, after steps, 5d
    A probe for tool calls                 :done, probe, after worth, 3d
    A grammar for tool calls               :done, gbnf, after probe, 4d
    section What the night left open
    The panel reads the pruned lines       :trace, after steps, 1d
    What a container start costs           :cost, after trace, 1d
    A posture on resume is refused         :posture, after cost, 2d
    Enforcement per job                    :done, enf, after posture, 2d
    section Arguments and boxes
    A judge that is not the model          :judge, after enf, 3d
    Concurrency                            :conc, after judge, 4d
    The BC-250 14B ceiling                 :bc250, 2026-09-09, 14d
    Rotating and revoking an approval key  :keys, after conc, 3d
```

## What actually blocks what

- **Item 1 blocked nothing and was first anyway, and is now closed.** Rule A is
  built, off, and stayed off until somebody ran it — machine 1, 2026-09-14. What
  it blocked was the *decision*, not any code, and the decision is still
  unmade: the run cleared the bar (no verdict moved across 20 questions) but
  one conversation is not the same claim as a changed default. See
  [`the-7b-does-not-miss-it`](../../RECORD/2026-09-14.the-7b-does-not-miss-it.completed.md).
- **Item 3 landed the day this revision was written, item 3b is what it cost,
  and item 3b is now closed too.** Measured 2026-09-14: `--prune-behind` alone
  is a no-op on a tool-heavy corpus and `--prune-results` is not, freeing
  32 752 tokens and eliminating eviction outright over 20 turns. What is not
  closed is the shape item 1 was a warning about either way — this is a mock
  measurement of mechanics, and whether a model minds losing tool output to a
  citation is unasked.
- **Item 3 is the last row of the window's original argument.**
  [`what-leaves-the-history`](../../RECORD/2026-09-06.what-leaves-the-history.completed.md)
  proposed two rules and left a third case open in its §Still open — *tool
  results and fragments may not want the same rule* — and this is that case,
  answered rather than assumed: they want the same *line* and a different
  citation. It needs no model, no box and no format bump, which is why it is
  ahead of everything in sections C and D.
- **Items 3b and 4 wanted the same thing built first; it is built, and 3b is
  now closed with it.** `--mock-cycle` (2026-09-14) wraps the mock's reply
  queue back to its first element instead of sticking on its last, so a
  corpus where a tool is called every turn no longer needs a model or a box —
  see [`a-call-every-turn`](../../RECORD/2026-09-14.a-call-every-turn.completed.md).
  3b's run is done ([`what-rule-c-is-worth`](../../RECORD/2026-09-14.what-rule-c-is-worth.completed.md));
  4's run is done too, the same day, against `qwen2.5-coder:7b` on machine 1:
  20% of fifteen one-shot prompts parsed cleanly, and more than half the
  no-calls were the model answering confidently wrong rather than declining —
  see [`the-tool-call-probe-run`](../../RECORD/2026-09-14.the-tool-call-probe-run.completed.md).
- **Item 4 closed the way item 1 did — the measurement, not the decision.**
  What the reordering of 2026-09-08 changed is that the probe's *harness* was
  section B work and its *run* was section A work, splitting them is what
  stopped the pair waiting on a box, and both halves are now done. What item 4
  does not decide is item 5: a 20% clean-call rate is the number a grammar
  would exist to fix, and whether to build one is still the unmade call this
  row was ordered ahead of.
- **Item 5 is built, both arms, the same day.** The grammar arm answered its
  own question yes, at a real cost (fence-continuation gone, drift-by-name
  untouched, content can still drop under a corrected shape); the schema
  arm answered no — applied to a whole turn rather than the one-shot retry
  it was designed as, it answers zero of fifteen probe prompts. See
  [`what-constrain-does`](../../RECORD/2026-09-14.what-constrain-does.completed.md).
- **Item 6 is the smallest row here and the one that would have found the most.**
  Three of the three bugs of 2026-09-08 were found by watching the surface act.
  A rule whose whole effect is invisible in the panel is a rule nobody will
  notice going wrong.
- **Items 8 and 9 are both a record before they are a diff, and they are the
  same record's neighbourhood**: what an approval was granted under, and what a
  job may narrow. Whoever writes one should read the other first — that is a
  suggestion about sequencing and not a claim that they are one item.
- **Item 12 is ordered here for the first time and is deliberately last of the
  arguments.** `serve` running one session at a time has not cost anything yet
  because nothing has asked for two. The row exists so that the first thing to
  ask for two does not discover that the shape was never designed.
- **Item 13 is the only row this project can still call blocked by geography**,
  and the only one in the revision that no amount of work here can advance.
