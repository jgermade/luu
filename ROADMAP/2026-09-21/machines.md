# The machines, and what each one can uniquely answer

Allocation and order only. *What* each run measures and *how to read it* lives in
the record each row links to — this file exists because eight platforms arrived at
once and the question stopped being "what should we measure" and became "on which
box, in what order, and what does that box tell us that no other one does".

**The organising rule:** one machine, one question nothing else can answer.
Running the same corpus everywhere produces numbers that cannot be compared —
different silicon, different quantisation, different serving stack — and the
project's whole measurement discipline rests on *one flag apart*. A run that
changes the machine changes more than a flag.

**What changed in this revision: nothing here, and that is the finding.** The
last revision moved this file for the first time since 2026-09-04 — machine 6's
9.4 GiB turned out to be a setting rather than a carve-out, machine 2 came off
Reserve for two runs, and machine 4 answered a question nobody ordered. Five
days later **not one of the runs those changes were made for has happened**, and
the inventory is therefore carried verbatim below, with only the status column's
date moved.

What did change is on the other side of the table: **the queue behind it grew
from one claim to four.**
[`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) is
still the only `WIP` record in the tree, and it is now joined by three records
that each close with some version of *this is Section A work* —
[`a-corpus-that-edits`](../../RECORD/2026-09-19.a-corpus-that-edits.completed.md),
[`the-newest-body-wins`](../../RECORD/2026-09-20.the-newest-body-wins.completed.md)
and [`the-drafts-floor`](../../RECORD/2026-09-21.the-drafts-floor.completed.md).
Four days of work built the instruments; none of them can be read without one of
these boxes. Roadmap row 27 is that bill, and it lands on machine 1 or machine 4
— whichever has a hand on it first, because unlike rows 1–4 it does not care
which silicon answers.

## The inventory

Bandwidth figures are from specification, not measured here, and they are in the
table because for a memory-bound 7B they decide the speed and the ceiling decides
what fits at all.

| # | Machine | Memory for weights | ~Bandwidth | Ceiling at Q4 | Status as of 2026-09-21 |
| --- | --- | --- | --- | --- | --- |
| 1 | **M1 Pro, 16 GB** — macOS | 16 GB unified | ~200 GB/s | 7B comfortable, 14b tight | Gate probe, container baseline, and four model-in-the-loop runs (coverage, precision, repeat-once, the tool-call probe). The Apple answer for anything that is not confinement |
| 2 | **Mac mini M4, 16 GB** | 16 GB unified | ~120 GB/s | same, slower | **Allocated on 2026-09-17 and not yet run**: Metal as the control for RADV's doubt, and the bandwidth floor — [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §6–7. Cannot answer confinement at all: `enforcement = "kernel"` denies `run_command` on macOS, so `closes_on` is unreachable there |
| 3 | **MacBook M4 Pro, 48 GB** | 48 GB unified | ~273 GB/s | **32b comfortable** | Size sweep completed; **now also roadmap item 2**, 2026-09-22 — the article's own `Qwen3.8-27B-GSQ-RCO-IQ3_S.gguf`, not the RTX, answered *8K against 96K* first — [`the-size-sweep`](../../RECORD/2026-09-03.the-size-sweep.completed.md), [`8k-against-96k-on-the-m4-pro`](../../RECORD/2026-09-22.8k-against-96k-on-the-m4-pro.completed.md) |
| 4 | **Ryzen 5 3600 + RTX 5060 Ti** | 16 GB VRAM | ~448 GB/s | 14b comfortable — measured, 11.1/16.3 GB at ctx 8192; **27B IQ3_S is the open question** | Native Linux confinement, the 14B ceiling and KV precision, all closed. **The only box that can answer this revision's remaining crit row** — a model inside a container (item 3). Item 2 closed on machine 3 instead; the 27B fit test it used to gate is row 35, still owed here — [`landlock-holds-natively`](../../RECORD/2026-09-08.landlock-holds-natively.completed.md), [`the-rtx-holds-14b`](../../RECORD/2026-09-08.the-rtx-holds-14b.completed.md), [`the-14b-context-ceiling`](../../RECORD/2026-09-08.the-14b-context-ceiling.completed.md) |
| 5 | **i5 9400F + GTX 1660 Super** | 6 GB VRAM | ~336 GB/s | 7B at the edge, degrades rather than breaks | Hardware floor measured — [`the-floor-on-6gb`](../../RECORD/2026-09-08.the-floor-on-6gb.completed.md) |
| 6 | **BC-250** — Zen 2 + RDNA 2 | 16 GB GDDR6, **split by `amdgpu.gttsize` and not by the board** (~9.4 GiB addressable as configured today) | ~224 GB/s | 7b comfortable; **14b and 27B both a function of where the line is drawn** | Vulkan serving stack measured. Roadmap item 1: raise GTT to 12 GiB, reboot, and find out whether `ggml-vulkan` will use what it is given — [`the-bc250-run`](../../RECORD/2026-09-04.the-bc250-run.completed.md), [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §1–2 |
| 7 | **A GitHub runner** — `ubuntu-latest`, Linux 6.17 | — | — | no model | **Level 3 on Linux, on every push**: the image builds, the handshake holds, and Landlock ABI v7 + seccomp hold `run_command` inside stock Docker — [`the-container-on-a-runner`](../../RECORD/2026-09-08.the-container-on-a-runner.completed.md) |
| P1 | **BytePlus ModelArk** | — | — | whatever it serves | External judge candidate. Needs a design argument, not an endpoint |
| P2 | **build.nvidia.com**, free tier | — | — | whatever it serves | Remote 32b/70b ceiling. Unblocked by the OpenAI backend and unscheduled |

## The two sixteens, because the table's third column hides it

Machines 1, 2, 4 and 6 all read "16 GB" and **four different numbers are meant**:
16 GB unified and shared with the OS (1, 2), 16 GB of dedicated VRAM behind PCIe
(4), and 16 GB of GDDR6 partitioned by a kernel parameter into a device-local
half and a window onto the rest (6). What a 11.77 GB model needs from each of
them is a different question, and the column that would flatten them into one is
exactly the column this file refuses to add.

## What is left

1. **Whether the BC-250's line moves** (machine 6) — roadmap item 1, and the
   trap it exists for: RADV exposes both heaps, but the backend may insist on
   `DEVICE_LOCAL` for tensors and fail rather than spill. *A bigger number from
   `--list-devices` is not the same as a bigger model loading.* Two failures are
   expected and are findings rather than problems — IQ3_S on RADV has had uneven
   support, and `-fa on` with a quantized V-cache is CUDA-tested here and
   Vulkan-untested. The firmware knob is the *other* lever and is deliberately
   not touched first, because it is not revertible by a reboot.

2. **Whether the 27B fits machine 4 at all** — roadmap row 35, no longer item 2's
   precondition: item 2 closed on machine 3 on 2026-09-22 without this fit test
   ever running, because the M4 Pro was at hand and the question it answers —
   window as `luu`'s own flag — does not care which box does it.
   [`the-14b-context-ceiling`](../../RECORD/2026-09-08.the-14b-context-ceiling.completed.md)
   closed with *"Not Qwen3.8 anything"*, and that was right about a ~17 GB Q4
   build. A 11.77 GB non-uniform packing is a different file, which is why the
   run is a fit test before it is a comparison, and it is still worth running on
   its own terms: a 16 GB card holding what a 48 GB Mac holds is a different
   finding than the one machine 3 already made.

3. **A judge that is not the model under test** (P1) — roadmap item 13. Not a
   measurement to run: an argument to write. Every number this repository has was
   scored against a key on disk or by a person reading replies, and the tool-call
   probe is already past what that comfortably covers — four verdicts per reply,
   two models, five corpora.

4. **What the week's instruments owe a model** (machine 1 or machine 4) —
   roadmap row 27, and the only entry in this list that does not care which box
   answers it. Three claims share one corpus family and one afternoon: how often
   a model edits a file it has quoted and then asks about it, whether it notices
   the citation that replaces the stale bytes, and whether it behaves
   differently while drafting now that drafting cannot write. Each protocol is
   the two commands in its own record's header. It is here rather than in the
   numbered rows above because it is the first Section A run in this project's
   history whose blocker is an afternoon and not a machine.

5. **A ceiling** (P2) — the same open-weight family at 70B+, served remotely, to
   say what the local numbers are a fraction of. Unblocked since the OpenAI
   backend landed and never ordered, because nothing in the tree depends on the
   answer. It stays here so that "nobody scheduled it" does not become "nobody
   remembered it".

**Not in this list, deliberately:** speed. It is the standing rule — this project
scores answers, not seconds — and it survives one narrow exception that
[`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) takes
and states: a tokens-per-second *floor* is a question about whether a box is
usable at all, which is not a comparison between boxes. The exception does not
license a speed column here, and machine 2's run 7 is the only place it applies.
