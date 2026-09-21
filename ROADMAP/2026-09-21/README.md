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

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 1 | **The BC-250's heap is a setting** — `amdgpu.gttsize=12288` and a reboot, then whether `ggml-vulkan` will *use* the heap it was given rather than insisting on `DEVICE_LOCAL`. A bigger number from `--list-devices` is not the same as a bigger model loading | machine 6, and a reboot | [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §1–2 |
| 2 | **8K against 96K, one flag apart** — the differentiator's first local trial, and the three outcomes are named before looking | machine 4, after the 27B fit test | [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §3–5 |
| 3 | **A model inside a container** — every contained run in this repository is the mock, and the trigger `luu.container.toml` names for narrowing its network has never fired. **Carried unchanged for a third revision** | machine 4 | [`a-session-picks-its-executor`](../../RECORD/2026-09-08.a-session-picks-its-executor.completed.md) §Still open |
| 4 | **Machine 2 earns its first question** — a Metal control for RADV's doubt, and the bandwidth floor at 120 GB/s | machine 2 | [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §6–7 |
| 27 | **What this week's instruments owe a model** — three claims, one afternoon, one corpus family, and the protocol for each is written in the record that built it: (a) how often a *model* edits a file it has quoted and then asks about it — 8 of 13 is a fact about a scripted corpus; (b) whether a model notices the divergence or its repair — a citation where the bytes stood; (c) whether a model behaves differently when it knows it is drafting, which the floor gave teeth, because a draft that does not know it may not write finds out by being refused and a refusal is a turn | a machine with a model | [`a-corpus-that-edits`](../../RECORD/2026-09-19.a-corpus-that-edits.completed.md), [`the-newest-body-wins`](../../RECORD/2026-09-20.the-newest-body-wins.completed.md), [`the-drafts-floor`](../../RECORD/2026-09-21.the-drafts-floor.completed.md) §Still open |

**Section B — the window.** Rule A is on and measured on the axis that justifies
it; B and C are built, off, and still waiting on item 5. Nothing in this section
changed that.

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 5 | **Whether a model minds losing tool output to a citation** — narrowed to `cited` by `results = "cited_reads"`, which keeps what a command found and gives up a file's bytes. Still the gate on rules B and C, and the same question it was on 2026-09-14 | a model; the corpus and the flag exist | [`what-rule-c-is-worth`](../../RECORD/2026-09-14.what-rule-c-is-worth.completed.md) §Still open |
| 7 | **Drift under a tool's own name** — `` ```list_dir ``, `` ```run_command ``: two of fifteen prompts, identical in the `off` and `grammar` arms, and a different trigger from the one the grammar closed. Wants `avoid_until_forced` once per tool name | nothing | [`what-constrain-does`](../../RECORD/2026-09-14.what-constrain-does.completed.md) §Still open |
| 21 | **A window that no longer fits is absorbed in silence** — `TurnView::dropped` carries the `Evicted`, the panel reads it, and nothing treats it as something a person is *told*. The answer is to make it visible, not configurable | needs a design argument | [`the-window-rules-are-a-session-fact`](../../RECORD/2026-09-18.the-window-rules-are-a-session-fact.completed.md) §6th |
| 28 | **The two keys by content, and the migration they cost** — item 18 landed the table under `repeat`, `prune`, `results` and its own record had settled `file-read` and `cmd-output`, the cut by *what the rule is about*. Taken knowingly: those keys would name a capability nothing implements. The rename is owed the moment rule A reaches a tool's result, and it is a migration of a file on somebody's machine | rule A reaching more than spans | [`the-window-rules-are-a-session-fact`](../../RECORD/2026-09-18.the-window-rules-are-a-session-fact.completed.md) §sixth, §eleventh; `crates/luu/src/provider.rs` |
| 29 | **The tally has no denominator and no opinion** — `repeated` says what rule A dropped and nothing counts the spans it did *not* collapse, so no share can be computed; and a count per session is not a count across sessions, which is what *how often does a model edit a file it has quoted* actually asks. Both are arithmetic on a surface that now exists | row 27 for the meaning; nothing for the arithmetic | [`one-counting-surface`](../../RECORD/2026-09-19.one-counting-surface.completed.md) §Still open |
| 30 | **Whether a draft's fold is worth its summary** — the zero-turn draft case is dead by construction, and a three-turn draft that concluded nothing is the same objection with a smaller number. Nothing says where the floor is | needs a design argument | [`every-turn-belongs-to-a-job`](../../RECORD/2026-09-20.every-turn-belongs-to-a-job.completed.md) §Still open |

**Section C — the gate, and the surfaces that show it.** Three of its four new
rows are *the page does not show what the last four days changed*, which is the
third revision running in which driving a surface is where the defects were.

| # | Item | Blocked on | Argued in |
| --- | --- | --- | --- |
| 34 | ~**A test whose bound is wall-clock time** — `a_recording_that_spans_sessions_carries_a_header_for_each` asserts `at_ms < 400` where 400 is also a `sleep`, so a loaded machine fails it. Twice in one afternoon and not in ten runs since. First in this section because a load-flaky test is what makes every other row's *all green* worth less~ **closed 2026-09-21: the number did not want raising, it wanted removing** — a line is checked against how long the session had actually been alive, on the recorder's own clock, so load moves the bound and the lines it bounds together. **And the row was half a row**: the same test held a second, unnamed race — it reads a recording out from under a live server, alone in this suite, and the recorder writes on its own task. Old bound under 48 busy loops on four cores: 6 runs, 6 failures; both fixes under the same load: 6 green | nothing | [`a-bound-that-is-not-a-clock`](../../RECORD/2026-09-21.a-bound-that-is-not-a-clock.completed.md) |
| 31 | ~**The gate panel does not show the floor** — it shows what a plan asks for and never what an unapproved turn may reach, which is now a different and smaller thing than the policy file. Same shape as item 10, one field along~ **closed 2026-09-21**: `/api/settings` carries the session's floor beside its posture and the gate prints it under the asks, so approving is visibly a decision about what to *add*. Resolved in Rust and never re-derived in the page — a second resolution of a policy is a second sandbox — and with no `writes` field, because the floor grants none by construction and a sentence says that better than an always-empty list | nothing | [`what-an-unapproved-turn-may-reach`](../../RECORD/2026-09-21.what-an-unapproved-turn-may-reach.completed.md) |
| 33 | **What the debug UI does with an alternation** — approving now closes a draft and opens a plan in one action, and the folded history above it is twice as long and half as uniform. Unargued, and the page is the one place the alternation is visible to a person | needs a design argument | [`every-turn-belongs-to-a-job`](../../RECORD/2026-09-20.every-turn-belongs-to-a-job.completed.md) §Still open |
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
  not depend on a date.
- **Row 27 is one afternoon and three claims, which is why it is a row and not
  three.** The corpus, the flags and the counting surface all exist; what does
  not exist is a single session of a model doing the thing. Splitting it would
  produce three rows blocked on the same hour.
- **Item 2 stays the one to protect if the afternoon runs out**, unchanged from
  the last revision and for the same reason: the instrument — a server started
  once at `-c 98304` — only exists while somebody is at machine 4.
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
  and it is the one that needs an argument rather than an afternoon.
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
  model; both are worth less than they look until row 27 says what the numbers
  are a share *of*.
- **Item 28 is ordered but not scheduled, and it is the one row here that gets
  cheaper by waiting.** The rename becomes correct exactly when rule A reaches
  a tool's result, and doing it before that names a capability the tree does not
  have. It is in the list so that *knowingly deferred* does not decay into
  *forgotten*, which is the same job row 25 did for counting.
- **Nothing in section D blocks anything in A, B or C** — fourth revision
  running, and it stays worth repeating because D is the section that grows
  without being worked.
