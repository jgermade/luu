# Roadmap — revision 2026-09-21

**What this is:** the order of work as it stands on this date, and what blocks
what. Not a decision and not a description of the tree — for *what is true today*
read [`luu-design.md`](../../luu-design.md), and for *why* read the dated file
in [`RECORD/`](../../RECORD/) each item links to.

Supersedes [`ROADMAP/2026-09-17`](../2026-09-17/) wholesale. Four days, and
**thirteen of its twenty-six rows closed** — 6, 8, 9, 10, 15, 17, 19, 20, 22,
23, 24, 25 and 26 are struck through in place there, and item 18 is struck in
its parts but for one owed rename. Three of them (9, 10, 15) were recorded on
the revision's own day and merged the morning after, so the four days *since*
closed ten. What is left is 1, 2, 3, 4, 5, 7, 11, 12, 13, 14, 16 and 21, plus
what item 18 did not finish.

**Numbers are carried, not reassigned.** A row keeps the number it was given, new
rows continue from 27, and the only renumbering this project has done — section A
in the last revision — is why [`luu-design.md`](../../luu-design.md) had to name a
revision beside an item number. Carried rows below are compressed to their claim
and their link: the long form stays in the row they came from, and the argument
was never in either.

**The calibration finding, and it is the exact opposite of the last revision's.**
That one found a fortnight of debug-UI work that no row had ordered, and wrote
down that a roadmap listing only what it predicted flatters itself. This one has
the reverse problem. Of the fifteen commits since 2026-09-17, **eleven are
substantive and every one of them is a roadmap row** — 36 through 46, closing
items 6, 8, 9, 10, 15, 17, 19, 20, 22, 23, 24, 25 and 26 and most of 18. The
roadmap predicted all of it. It predicted all of it because **all of it was work
a checkout can do**,
and the four days contain **zero runs against a model**. Section A did not move.

So the failure changed shape: it was *work nobody ordered*, and it is now *orders
nobody can run*. The bill arrives as rows 27 and 29 below — three claims and one
denominator that instruments built this week cannot make on their own.
[`one-counting-surface`](../../RECORD/2026-09-19.one-counting-surface.completed.md)
§Still open named the habit while it was happening, and named it as a habit
rather than a surprise: *the instrument is again being built ahead of the run
that gives it meaning — third time this week.*

**How this file was built, because the last one said it should be.**
[`the-drafts-floor`](../../RECORD/2026-09-21.the-drafts-floor.completed.md) came
straight out of item 22's *Still open* and the 2026-09-17 revision wrote down
what that made visible: a record's closing section is where the next rows are
born, and nothing moves them into a roadmap but somebody reading it. This
revision was made by reading the *Still open* of all fourteen records written
since 2026-09-17. That produced **eight rows (27–34)**, none of them new work and
each of them between one and five days old as a sentence somebody had already
written. The lag is the defect, not the rows.

## What landed since 2026-09-17

| | |
| --- | --- |
| **The window rules are a session's fact** (items 17, 18, 6) | Process-wide flags became a session fact, named in `config.toml`, changed live through a `PUT`, shown in a third Settings section; `session::header` gained `repeat`, `prune` and `results`, format 10 → 11; and the default flipped — rule A on, B and C off, on n=1 and pinned by two tests — [`the-window-rules-are-a-session-fact`](../../RECORD/2026-09-18.the-window-rules-are-a-session-fact.completed.md) |
| **A resume keeps what a session was approved under** (item 9) | A grant no longer outlives its posture, and `retarget_header` stopped being silent when the posture moved rather than the destination — [`what-an-approval-was-granted-under`](../../RECORD/2026-09-17.what-an-approval-was-granted-under.completed.md) |
| **The gate panel narrows, and the page says the icon theme exists** (items 10, 15) | `network`, `egress` and `enforcement` reachable from the surface people actually use; a page change, a test and no Rust — [`the-gate-panel-narrows`](../../RECORD/2026-09-17.the-gate-panel-narrows.completed.md) |
| **A span is read once, and the newest body wins** (items 19, 24) | The defect measured on a corpus built for it — `edit-reread.txt`, 8 of 13 turns sending bytes a later turn had edited away — then repaired by superseding the stale span with a citation — [`a-corpus-that-edits`](../../RECORD/2026-09-19.a-corpus-that-edits.completed.md), [`the-newest-body-wins`](../../RECORD/2026-09-20.the-newest-body-wins.completed.md) |
| **Fragments across a resume, by reference** (item 20) | `from_view` stops rebuilding turns with an empty `code_context`: the turn and the file come back and are re-read through the live job's sandbox, and an unreadable one becomes a citation rather than a silence — [`fragments-by-reference`](../../RECORD/2026-09-19.fragments-by-reference.completed.md) |
| **One counting surface** (item 25) | `api::Counts` and `SessionView::counts()`, `luu count <record.jsonl>`, format 13 → 14 — the answer to a sentence that had been written five times in five records as somebody else's aside — [`one-counting-surface`](../../RECORD/2026-09-19.one-counting-surface.completed.md) |
| **A header per session in a recording** (item 23) | A `serve --record` file spanning two sessions no longer names the first one's model, posture and rules for both — [`a-header-per-session`](../../RECORD/2026-09-19.a-header-per-session.completed.md) |
| **The 7B's tool-call numbers, audited** (item 8) | The confound was real: every recorded run re-scored against the tool schema it was made on. Cheap, unglamorous, and it would have invalidated a table — [`the-7b-numbers-audited`](../../RECORD/2026-09-19.the-7b-numbers-audited.completed.md) |
| **Every turn belongs to a job** (item 22) | `Turn::job` stops being `Option`: a session opens with a draft, `approve` closes it and opens a plan, and the negative decisions keep you where you are — [`every-turn-belongs-to-a-job`](../../RECORD/2026-09-20.every-turn-belongs-to-a-job.completed.md) |
| **The draft's floor** (item 26) | Exploration reads and the gate unlocks the write bit — one expression changed, not a field added — and it found a resume that had been handing an approved job the policy file instead of its plan — [`the-drafts-floor`](../../RECORD/2026-09-21.the-drafts-floor.completed.md) |

**What nobody ordered this time: nothing.** Four commits are not on the list
(`settings.json` three times and a `pull_request` trigger removed from
`build.yml`) and they are repository plumbing, not work. That is the number this
revision hands forward, and it is only good news read beside the zero above it.

## The order

**Section A — the runs, and the section that has now expired.** Carried whole
from the last revision, where it was also carried whole from the one before.
[`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) is the
only `WIP` record in the tree and it was written so that *the morning of
2026-09-17* would not be spent deciding what to type. That morning is five days
gone. Nothing in the protocol rots; what expired is the schedule, and the row
that changes is row 27, which did not exist when the section was last written.
**Row 2 also changed, on 2026-09-22**, and not the way this paragraph predicted:
it closed on machine 3, not machine 4, and split off row 35 rather than waiting
on the RTX fit test it used to be blocked behind. **A second `WIP` record
joined it the same day and closed the day after**:
[`an-authority-a-model-is-told`](../../RECORD/2026-09-22.an-authority-a-model-is-told.completed.md),
built and run for row 27's claim (c) — see that row, below.
[`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) is
once again the only one standing.

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 1 | **The BC-250's heap is a setting** — `amdgpu.gttsize=12288` and a reboot, then whether `ggml-vulkan` will *use* the heap it was given rather than insisting on `DEVICE_LOCAL`. A bigger number from `--list-devices` is not the same as a bigger model loading | machine 6, and a reboot | [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §1–2 |
| 2 | ~**8K against 96K, one flag apart** — the differentiator's first local trial, and the three outcomes are named before looking~ **closed 2026-09-22, on machine 3 and not machine 4**: the 27B fit test was never run on the RTX — the M4 Pro was at hand and item 2 took the fallback route §*What is left* #2 named in advance. `grounded.txt` never fills 8K (largest prompt 3 106 of 8 192) so it ties by construction and says nothing; `long-session.txt` does fill it, and 96K answers 4 of 6 turns against 8K's 1 of 6, with or without rules A/B/C — rule A never fires on this corpus (nothing it attaches is a span), rule C prunes tool output instead of evicting turns but does not raise the answer count. First outcome of the three named, on one seed, one corpus, one box | nothing | [`8k-against-96k-on-the-m4-pro`](../../RECORD/2026-09-22.8k-against-96k-on-the-m4-pro.completed.md) |
| 35 | **The 27B's RTX fit test, orphaned by item 2 closing elsewhere** — §3 of `the-16gb-threshold` was never run: does `Qwen3.8-27B-GSQ-RCO-IQ3_S.gguf` load and hold a turn on 16 GB of VRAM, still the open question `machines.md` names for machine 4. No longer item 2's precondition, since item 2 answered its question on machine 3 instead | machine 4 | [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §3 |
| 3 | **A model inside a container** — every contained run in this repository is the mock, and the trigger `luu.container.toml` names for narrowing its network has never fired. **Carried unchanged for a third revision** | machine 4 | [`a-session-picks-its-executor`](../../RECORD/2026-09-08.a-session-picks-its-executor.completed.md) §Still open |
| 4 | **Machine 2 earns its first question** — a Metal control for RADV's doubt, and the bandwidth floor at 120 GB/s | machine 2 | [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §6–7 |
| 27 | ~**What this week's instruments owe a model** — three claims, one afternoon, one corpus family: (a) how often a *model* edits a file it has quoted and then asks about it; (b) whether a model notices the divergence or its repair; (c) whether a model behaves differently when it knows it is drafting~ **closed in full, 2026-09-22–23, on machine 3.** (a) and (b): the model made every requested edit in both arms and `diverged` was 0 — the repair fires `superseded` 8 times (fragments) and 16 (`--select-tokens 1024`, which re-reads a span nobody typed) — and every re-asked value, including the one answerable only from memory with no fragment attached, came back current. (c) needed its own instrument first — nothing told a model it was drafting, so `[authority.draft]`/`[authority.plan]` was proposed and built (session fact, live `PUT`, a Settings section, `record::FORMAT` 18) — and then the run: asked to fix a one-line bug with no plan open, the model tries `edit_file` then `write_file`, both denied by the floor, explores the tree in between, and only then gives up and states the fix in prose — 5 tool calls, 2 824 completion tokens. Told in advance that writes are refused until a plan is approved, it reads the file once and proposes a plan without trying to write at all — 517 tokens, a fifth of the other arm's. Neither plan was approved, so whether the fix lands either way is untested | nothing | [`edit-reread-on-a-model`](../../RECORD/2026-09-22.edit-reread-on-a-model.completed.md), [`an-authority-a-model-is-told`](../../RECORD/2026-09-22.an-authority-a-model-is-told.completed.md) |

**Section B — the window.** Rule A is on and measured on the axis that justifies
it; B and C are built and off. **Item 5 closed 2026-09-22, inconclusively**: it
answers the mechanical half — `cited-reads` really does spare `run_command` and
`cited` really does not — but not the half B and C's defaults were waiting on,
because the corpus at hand never made a model's answer depend on a command's
result. B and C stay off; a new corpus is what would move them.

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 5 | ~**Whether a model minds losing tool output to a citation** — narrowed to `cited` by `results = "cited_reads"`, which keeps what a command found and gives up a file's bytes. Still the gate on rules B and C, and the same question it was on 2026-09-14~ **closed 2026-09-22, and inconclusively**: `cited-reads` cites 0 of the session's `run_command` output against `cited`'s 77 lines — the mechanical split holds — but answered turns stay 1 of 6 under `kept`, `cited-reads` and `cited` alike, because `long-session.txt`'s commands are all empty greps and the citation that costs the answer is `read_file`'s, which `cited-reads` does not spare. The case the item was written for — a command's *result* mattering, a test's exit code — is still unmeasured | nothing for this corpus; a new one for the real case | [`what-rule-c-is-worth-with-a-model`](../../RECORD/2026-09-22.what-rule-c-is-worth-with-a-model.completed.md) |
| 7 | **Drift under a tool's own name** — `` ```list_dir ``, `` ```run_command ``: two of fifteen prompts, identical in the `off` and `grammar` arms, and a different trigger from the one the grammar closed. Wants `avoid_until_forced` once per tool name | nothing | [`what-constrain-does`](../../RECORD/2026-09-14.what-constrain-does.completed.md) §Still open |
| 21 | **A window that no longer fits is absorbed in silence** — `TurnView::dropped` carries the `Evicted`, the panel reads it, and nothing treats it as something a person is *told*. The answer is to make it visible, not configurable | needs a design argument | [`the-window-rules-are-a-session-fact`](../../RECORD/2026-09-18.the-window-rules-are-a-session-fact.completed.md) §6th |
| 28 | **The two keys by content, and the migration they cost** — item 18 landed the table under `repeat`, `prune`, `results` and its own record had settled `file-read` and `cmd-output`, the cut by *what the rule is about*. Taken knowingly: those keys would name a capability nothing implements. The rename is owed the moment rule A reaches a tool's result, and it is a migration of a file on somebody's machine | rule A reaching more than spans | [`the-window-rules-are-a-session-fact`](../../RECORD/2026-09-18.the-window-rules-are-a-session-fact.completed.md) §sixth, §eleventh; `crates/luu/src/provider.rs` |
| 29 | **The tally has no denominator and no opinion** — `repeated` says what rule A dropped and nothing counts the spans it did *not* collapse, so no share can be computed; and a count per session is not a count across sessions, which is what *how often does a model edit a file it has quoted* actually asks. Both are arithmetic on a surface that now exists. **Row 27 closed 2026-09-22–23** and gave the meaning: every edit was caught, so the denominator this row wants is a share of a number that came back 100% on this corpus | nothing for the arithmetic | [`one-counting-surface`](../../RECORD/2026-09-19.one-counting-surface.completed.md) §Still open |
| 30 | **Whether a draft's fold is worth its summary** — the zero-turn draft case is dead by construction, and a three-turn draft that concluded nothing is the same objection with a smaller number. Nothing says where the floor is | needs a design argument | [`every-turn-belongs-to-a-job`](../../RECORD/2026-09-20.every-turn-belongs-to-a-job.completed.md) §Still open |

**Section C — the gate, and the surfaces that show it.** Three of its four new
rows are *the page does not show what the last four days changed*, which is the
third revision running in which driving a surface is where the defects were.

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 34 | ~**A test whose bound is wall-clock time** — `a_recording_that_spans_sessions_carries_a_header_for_each` asserts `at_ms < 400` where 400 is also a `sleep`, so a loaded machine fails it. Twice in one afternoon and not in ten runs since. First in this section because a load-flaky test is what makes every other row's *all green* worth less~ **closed 2026-09-21: the number did not want raising, it wanted removing** — a line is checked against how long the session had actually been alive, on the recorder's own clock, so load moves the bound and the lines it bounds together. **And the row was half a row**: the same test held a second, unnamed race — it reads a recording out from under a live server, alone in this suite, and the recorder writes on its own task. Old bound under 48 busy loops on four cores: 6 runs, 6 failures; both fixes under the same load: 6 green | nothing | [`a-bound-that-is-not-a-clock`](../../RECORD/2026-09-21.a-bound-that-is-not-a-clock.completed.md) |
| 31 | ~**The gate panel does not show the floor** — it shows what a plan asks for and never what an unapproved turn may reach, which is now a different and smaller thing than the policy file. Same shape as item 10, one field along~ **closed 2026-09-21**: `/api/settings` carries the session's floor beside its posture and the gate prints it under the asks, so approving is visibly a decision about what to *add*. Resolved in Rust and never re-derived in the page — a second resolution of a policy is a second sandbox — and with no `writes` field, because the floor grants none by construction and a sentence says that better than an always-empty list | nothing | [`what-an-unapproved-turn-may-reach`](../../RECORD/2026-09-21.what-an-unapproved-turn-may-reach.completed.md) |
| 33 | ~**What the debug UI does with an alternation** — approving now closes a draft and opens a plan in one action, and the folded history above it is twice as long and half as uniform. Unargued, and the page is the one place the alternation is visible to a person~ **closed 2026-09-21, and four of its five findings were defects rather than the design question the row was filed as**: a fold said *job N* for a draft and an approved job alike; a draft closed by an approval was recorded as `ClosedBy::User`, the same as one a person folded (now `Approval`, protocol 8 and format 17); `reopen` was offered on every fold although the route takes only the last job — a window item 22 shrank to *before your next prompt* — and refused by saying the job was not closed, about one the person had just watched fold. **The fifth was found by the test written for the first**: `draft_opened` carries the turn it was opened to hold and the page dropped it, so a draft's turns were loose live and attributed after a reload. The decision the row actually contained — keep both folds and mark them, rather than merging a draft into the job it became — took a paragraph | nothing | [`the-alternation-on-the-page`](../../RECORD/2026-09-21.the-alternation-on-the-page.completed.md) |
| 32 | ~**`Authority::Draft` has no job** — a recording says a refusal came from the floor and not *which* draft was refused. Cheap the moment somebody asks the question~ **closed 2026-09-21, by contradicting the note that created it and keeping its reason**: the variant carries `Option<JobId>`, `None` on the first prompt of a session — where the sandbox really is chosen before any draft exists — and the draft's id on every prompt after it, stamped only where the live job is a draft. Building it found the floor is chosen at *two* sites and that stamping one would have made a selection denial and a tool denial in the same turn disagree | nothing | [`what-an-unapproved-turn-may-reach`](../../RECORD/2026-09-21.what-an-unapproved-turn-may-reach.completed.md) |
| 11 | **What a container start costs** — *the worker starts with the session* was chosen on the shape of the thing and not on a number, and the `container` job already makes the same call on both sides | nothing | [`a-session-picks-its-executor`](../../RECORD/2026-09-08.a-session-picks-its-executor.completed.md) §Still open |
| 12 | **Grants are counted as path strings and not as kinds** — the tally answers *how ordinary the 2026-09-08 regression was* in strings, and the field that would close it is named and not written. It is a format bump and a decision about what the gate writes down at approval, where the answer is known | the field | [`the-surfaces-first`](../../RECORD/2026-09-08.the-surfaces-first.completed.md), narrowed in [`one-counting-surface`](../../RECORD/2026-09-19.one-counting-surface.completed.md) |

**Section D — wants an argument before it wants a commit.** Unchanged, and
nothing in it blocks anything above.

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 13 | **A judge that is not the model under test** — every probe here is scored by a key on disk or by a person reading replies, and the tool-call probe is already past what that comfortably covers | needs a design argument | [`machines.md`](machines.md) P1 |
| 14 | **Concurrency** — `serve` runs one live session at a time; a per-session `Agency` is the first half and the rest is not designed | needs a design argument | [`state-of-play`](../../RECORD/2026-09-09.state-of-play.completed.md) §Still open |
| 16 | **Rotating and revoking an approval key**, and nothing signs a *recording*, so a reader that dropped lines is not detected | waits on a fleet being more than the boxes in one room | [`signed-approvals`](../../RECORD/2026-09-04.signed-approvals.completed.md) §Still open |

## What actually blocks what

- **Section A is no longer a section with a date on it; it is a section with a
  queue behind it.** When it was written it gated one claim — *8K against 96K*,
  the differentiator's first trial. It now gates four: that one, item 5's
  `cited`, and the three in row 27. Every one of the last four days' commits
  built an instrument whose meaning is on the other side of a machine nobody has
  had a hand on. That is the argument for running section A before writing
  another instrument, and it is the first time this file has had one that does
  not depend on a date. **The count dropped to zero on 2026-09-23**: item 2 is
  answered, its RTX fit test moved to row 35 rather than staying inside this
  gate, and row 27 closed in full — the section's own queue, the thing that
  made it more than a date, is empty for the first time since it was written.
- **Row 27's afternoon happened for two of its three claims on 2026-09-22, and
  the third took a day more.** (a) and (b) shared one corpus and one hour,
  which is why splitting them earlier would have been three rows blocked on
  the same thing. (c) did not share that hour — it needed the gate in the
  loop, which this corpus deliberately keeps out, and an instrument
  ([`an-authority-a-model-is-told`](../../RECORD/2026-09-22.an-authority-a-model-is-told.completed.md))
  that did not exist yet — so it closed a day later, on its own.
- **Item 2 closed 2026-09-22, and the thing that protected it was not being tied
  to machine 4.** The instrument — a server started once at `-c 98304` — turned
  out to only need *a* machine with 48 GB and the article's own build, not the
  RTX specifically; run there instead and the fit test it was waiting on stopped
  being its precondition. Row 35 carries that fit test forward on its own.
- **Rows 31, 32 and 33 are one neighbourhood and should be read together**, the
  way items 9 and 10 were. The alternation and the floor both landed as changes
  to *what runs*, and neither reached the surface a person watches. The last two
  revisions each found their bugs by driving a surface rather than by ordering
  work; this is the cheapest instance of that method available right now, and it
  is deliberately placed where the previous two findings say it belongs. **31
  and 32 closed the same day, together, and the adjacency paid the way it did
  for 9 and 10**: writing the panel's line is what made the missing name in the
  refusal obvious, and writing the name is what found the floor being chosen at
  two sites rather than one. **Row 33 is what is left of the neighbourhood**,
  and it is the one that needs an argument rather than an afternoon. **It
  closed the same day too, and the estimate was the thing that was wrong**: it
  wanted an afternoon and not an argument, because four of the five things in it
  were defects nobody had looked for and the decision it was filed as took a
  paragraph. Third revision running in which a row that reads as a design
  question about a surface turns out to be a row about what is broken on it.
- **Row 34 goes first in its section for a reason that is not its size.** A test
  that fails under load and passes ten times after is the thing that makes the
  next real failure arguable, and every row in section C ends in *the workspace
  is green*. Raising a bound written for somebody else's reason is a small diff
  and an argument to have on purpose rather than at 2am. **Closed the day this
  file was written, and it paid the way the last two revisions said small rows
  do**: the row named one race and the test had two, the second found only by
  putting the machine under load rather than by reading the code. The
  calibration note is that *twice in one afternoon and not in ten runs since*
  was describing two different failures the whole time, and nobody could have
  told them apart from the frequency.
- **Item 12 and row 29 are the same arithmetic complaint one field apart**, and
  both are downstream of the surface item 25 landed. Neither is blocked on a
  model; **row 27 said what the numbers are a share *of* on 2026-09-22**, and
  it was a share of a corpus this small catching everything, which is worth
  less as a rate than it looked before the run existed to compare it to.
- **Item 28 is ordered but not scheduled, and it is the one row here that gets
  cheaper by waiting.** The rename becomes correct exactly when rule A reaches
  a tool's result, and doing it before that names a capability the tree does not
  have. It is in the list so that *knowingly deferred* does not decay into
  *forgotten*, which is the same job row 25 did for counting.
- **Nothing in section D blocks anything in A, B or C** — fourth revision
  running, and it stays worth repeating because D is the section that grows
  without being worked.
