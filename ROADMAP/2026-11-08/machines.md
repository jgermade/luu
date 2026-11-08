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

**What changed in this revision: no measurement ran.** Between 2026-09-23 and
2026-11-08, models did run on these boxes, but only to build things: engines
luu starts ([`a-model-server-luu-starts`](../../RECORD/2026-09-28.a-model-server-luu-starts.completed.md))
and native tool calls ([`native-tool-calls`](../../RECORD/2026-09-30.native-tool-calls.completed.md)).
None of them was a run this file allocates. The rest of the work went to the
public luu, and the boxes it needed were the GitHub runner and a container. That is the calibration finding in
[`README.md`](README.md), seen from this side.

What did change is that **machine 3's queue no longer waits on anything in the
repository**. Row 38's corpus is built and proved on the mock
([`a-corpus-that-overflows-with-mixed-output`](../../RECORD/2026-11-08.a-corpus-that-overflows-with-mixed-output.completed.md)).
The run needs `python3` on the box, which macOS's Command Line Tools provide,
3.9 or later. The traceback format differs from 3.11+, and the probe's literals
are in both.

The one-slot condition stands: every Section A run is on a server started with
`-np 1`.

[`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) is
still `WIP`, and what it schedules on machines 2, 4 and 6 has not moved in
seven weeks.

## The inventory

Bandwidth figures are from specification, not measured here, and they are in the
table because for a memory-bound 7B they decide the speed and the ceiling decides
what fits at all.

| # | Machine | Memory for weights | ~Bandwidth | Ceiling at Q4 | Status as of 2026-11-08 |
| --- | --- | --- | --- | --- | --- |
| 1 | **M1 Pro, 16 GB** — macOS | 16 GB unified | ~200 GB/s | 7B comfortable, 14b tight | Gate probe, container baseline, four model-in-the-loop runs, and the container start cost (item 11, measured 2026-09-15, landed 2026-09-21 — [`what-a-container-start-costs`](../../RECORD/2026-09-15.what-a-container-start-costs.completed.md)). The Apple answer for anything that is not confinement |
| 2 | **Mac mini M4, 16 GB** | 16 GB unified | ~120 GB/s | same, slower | **Allocated on 2026-09-17 and not yet run**: Metal as the control for RADV's doubt, and the bandwidth floor — [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §6–7. Cannot answer confinement at all: `enforcement = "kernel"` denies `run_command` on macOS, so `closes_on` is unreachable there |
| 3 | **MacBook M4 Pro, 48 GB** | 48 GB unified | ~273 GB/s | **32b comfortable; the 27B at `-c 98304` measured** | Items 2 and 5 and row 27, fifteen arms on 2026-09-22–23 — [`8k-against-96k-on-the-m4-pro`](../../RECORD/2026-09-22.8k-against-96k-on-the-m4-pro.completed.md), [`what-rule-c-is-worth-with-a-model`](../../RECORD/2026-09-22.what-rule-c-is-worth-with-a-model.completed.md), [`edit-reread-on-a-model`](../../RECORD/2026-09-22.edit-reread-on-a-model.completed.md), [`an-authority-a-model-is-told`](../../RECORD/2026-09-22.an-authority-a-model-is-told.completed.md). **Allocated rows 36, 38–41**: why the 8K arm lost, now on a second corpus as well (`mixed-session.txt`, built 2026-11-08), and what the design says about it |
| 4 | **Ryzen 5 3600 + RTX 5060 Ti** | 16 GB VRAM | ~448 GB/s | 14b comfortable — measured, 11.1/16.3 GB at ctx 8192; **27B IQ3_S is the open question** | Native Linux confinement, the 14B ceiling and KV precision, all closed. **The only box that can answer item 3**, a model inside a container, and row 35, the 27B fit test — [`landlock-holds-natively`](../../RECORD/2026-09-08.landlock-holds-natively.completed.md), [`the-rtx-holds-14b`](../../RECORD/2026-09-08.the-rtx-holds-14b.completed.md), [`the-14b-context-ceiling`](../../RECORD/2026-09-08.the-14b-context-ceiling.completed.md) |
| 5 | **i5 9400F + GTX 1660 Super** | 6 GB VRAM | ~336 GB/s | 7B at the edge, degrades rather than breaks | Hardware floor measured — [`the-floor-on-6gb`](../../RECORD/2026-09-08.the-floor-on-6gb.completed.md) |
| 6 | **BC-250** — Zen 2 + RDNA 2 | 16 GB GDDR6, **split by `amdgpu.gttsize` and not by the board** (~9.4 GiB addressable as configured today) | ~224 GB/s | 7b comfortable; **14b and 27B both a function of where the line is drawn** | Vulkan serving stack measured. Roadmap item 1: raise GTT to 12 GiB, reboot, and find out whether `ggml-vulkan` will use what it is given — [`the-bc250-run`](../../RECORD/2026-09-04.the-bc250-run.completed.md), [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §1–2 |
| 7 | **A GitHub runner** — `ubuntu-latest`, Linux 6.17 | — | — | no model | **Level 3 on Linux, on every push**: the image builds, the handshake holds, and Landlock ABI v7 + seccomp hold `run_command` inside stock Docker — [`the-container-on-a-runner`](../../RECORD/2026-09-08.the-container-on-a-runner.completed.md). Since 2026-11-08 it also runs the mixed corpus's probe, `python3` and all, on the mock |
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

1. **Why the 8K arm lost** (machine 3): roadmap rows 38 and 36 in one
   afternoon, on `mixed-session.txt` and `long-session.txt`. Then 39 and 40.
   Then the decision the design owes (41), which the next revision makes on
   whatever is on disk, if nothing else is.

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
   measurement to run: an argument to write. Row 38's corpus is built, and its key
   splits literals, which a test checks, from conclusions, which a person
   scores. A judge would take the second half and nothing else.

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
