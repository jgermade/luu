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

**What changed in this revision: machine 3 went from the fallback to the box
that answers.** Until 2026-09-21 it was only the fallback route named for item 2;
its size sweep was done and it sat as *32b comfortable*. Then, on 2026-09-22–23,
it ran fifteen arms against `qwen3.8-27b` and closed item 2, item 5 and all
of row 27 — the whole queue the last revision said was waiting on machines
1 and 4. It did so because it was at hand and because 48 GB of unified memory
holds the 27B at `-c 98304`, which no other box here has shown it can. The rule
above still holds, with a new meaning: **machine 3's question is now *why the
8K arm lost*.** It is the only box that has run the 96K arm that result is
measured against. Rows 36–41 are all its.

**And the serving stack gains a condition.** Every Section A run on machine 3
is on a server started with one slot (`-np 1`). A four-slot `llama-server`
picks a slot by longest common prefix with whatever ran there before. That
made five `--temperature 0 --seed 1` arms differ at turn 1 —
[`8k-against-96k-on-the-m4-pro`](../../RECORD/2026-09-22.8k-against-96k-on-the-m4-pro.completed.md)
§third section. The qualitative result held across all five; *one flag apart*
did not, and it is the thing this file exists to protect.

[`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) is
still the only `WIP` record in the tree. What it was written to schedule on
machines 2, 4 and 6 has not moved in the six days since.

## The inventory

Bandwidth figures are from specification, not measured here, and they are in the
table because for a memory-bound 7B they decide the speed and the ceiling decides
what fits at all.

| # | Machine | Memory for weights | ~Bandwidth | Ceiling at Q4 | Status as of 2026-09-23 |
| --- | --- | --- | --- | --- | --- |
| 1 | **M1 Pro, 16 GB** — macOS | 16 GB unified | ~200 GB/s | 7B comfortable, 14b tight | Gate probe, container baseline, four model-in-the-loop runs, and the container start cost (item 11, measured 2026-09-15, landed 2026-09-21 — [`what-a-container-start-costs`](../../RECORD/2026-09-15.what-a-container-start-costs.completed.md)). The Apple answer for anything that is not confinement |
| 2 | **Mac mini M4, 16 GB** | 16 GB unified | ~120 GB/s | same, slower | **Allocated on 2026-09-17 and not yet run**: Metal as the control for RADV's doubt, and the bandwidth floor — [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §6–7. Cannot answer confinement at all: `enforcement = "kernel"` denies `run_command` on macOS, so `closes_on` is unreachable there |
| 3 | **MacBook M4 Pro, 48 GB** | 48 GB unified | ~273 GB/s | **32b comfortable; the 27B at `-c 98304` measured** | Items 2 and 5 and row 27, fifteen arms on 2026-09-22–23 — [`8k-against-96k-on-the-m4-pro`](../../RECORD/2026-09-22.8k-against-96k-on-the-m4-pro.completed.md), [`what-rule-c-is-worth-with-a-model`](../../RECORD/2026-09-22.what-rule-c-is-worth-with-a-model.completed.md), [`edit-reread-on-a-model`](../../RECORD/2026-09-22.edit-reread-on-a-model.completed.md), [`an-authority-a-model-is-told`](../../RECORD/2026-09-22.an-authority-a-model-is-told.completed.md). **Allocated rows 36–41**: why the 8K arm lost, and what the design says about it |
| 4 | **Ryzen 5 3600 + RTX 5060 Ti** | 16 GB VRAM | ~448 GB/s | 14b comfortable — measured, 11.1/16.3 GB at ctx 8192; **27B IQ3_S is the open question** | Native Linux confinement, the 14B ceiling and KV precision, all closed. **The only box that can answer item 3**, a model inside a container, and row 35, the 27B fit test — [`landlock-holds-natively`](../../RECORD/2026-09-08.landlock-holds-natively.completed.md), [`the-rtx-holds-14b`](../../RECORD/2026-09-08.the-rtx-holds-14b.completed.md), [`the-14b-context-ceiling`](../../RECORD/2026-09-08.the-14b-context-ceiling.completed.md) |
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

1. **Why the 8K arm lost** (machine 3) — roadmap rows 36–41, in that order.
   First the step limit (36), because 5 of 6 turns in every 8K arm ended at
   it; then the second corpus (38) and the approved-plan run (39); then the
   decision the design owes (41). All against the 96K arm already on disk, on a
   one-slot server.

2. **Whether the BC-250's line moves** (machine 6) — roadmap item 1, and the
   trap it exists for: RADV exposes both heaps, but the backend may insist on
   `DEVICE_LOCAL` for tensors and fail rather than spill. *A bigger number from
   `--list-devices` is not the same as a bigger model loading.* Carried
   unchanged.

3. **Whether the 27B fits machine 4 at all** — roadmap row 35. Item 2 no longer
   needs it. What it would add now is a second box for rows 36–41: a 16 GB card
   that holds what the 48 GB Mac holds would make machine 3's findings
   reproducible on different silicon, which is the one thing they cannot be
   today.

4. **A judge that is not the model under test** (P1) — roadmap item 13. Not a
   measurement to run: an argument to write. Row 38's corpus is the first one
   whose scoring may need it, because *did the model notice a failing test* is
   harder to put in a key file than *did it quote the right line*.

5. **A ceiling** (P2) — the same open-weight family at 70B+, served remotely, to
   say what the local numbers are a fraction of. Unblocked and never ordered. It
   has a reason to exist now that it did not have before: if row 41 ends up
   saying the window beats management, *how much* a bigger window buys is the
   next question, and P2 is where it gets asked.

**Not in this list, deliberately:** speed. It is the standing rule — this project
scores answers, not seconds — and it survives one narrow exception that
[`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) takes
and states: a tokens-per-second *floor* is a question about whether a box is
usable at all, which is not a comparison between boxes. The exception does not
license a speed column here, and machine 2's run 7 is the only place it applies.
