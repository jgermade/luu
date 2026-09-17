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

**What changed in this revision, and it is the first time this file has had an
answer to that question since 2026-09-04: the inventory moved.** Not by adding a
box — nothing new arrived — but because
[`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) found
that one of its rows was describing a **setting as if it were a property**:

- **Machine 6's 9.4 GiB is a configuration, not a carve-out.**
  [`the-bc250-run`](../../RECORD/2026-09-04.the-bc250-run.completed.md)'s numbers
  are right and its correction of the ceiling to "7B comfortable" stands; the
  word that had not earned its keep is *board-level*. `--list-devices`' 10 973
  MiB is the device-local split **plus the GTT window**, summed, and both sides
  are the same GDDR6 at ~224 GB/s because a PS5-derived APU has one pool. Moving
  the line is accounting, not bandwidth — which turns roadmap item 1 from a
  ceiling nobody can raise into a kernel parameter and a reboot.
- **Machine 2 comes off Reserve, for two runs and not in general.** It has never
  been allocated a question of its own since this file was written, on the
  correct grounds that machine 1 dominates it on everything they share. A
  *control* and a *floor* are precisely the two jobs a box earns by being
  redundant and slow, so it gets those and goes back.
- **Machine 4 answered a question this file never ordered.** Quantized KV cache
  costs no measurable precision on the 14B — identical 30/34 across `f16`,
  `q8_0` and `q4_0`, the same four questions wrong every time. Appended to
  [`the-14b-context-ceiling`](../../RECORD/2026-09-08.the-14b-context-ceiling.completed.md),
  and it is what makes run 3's `-c 98304` at `q8_0` a reasonable thing to try
  rather than a gamble on the cache type.

## The inventory

Bandwidth figures are from specification, not measured here, and they are in the
table because for a memory-bound 7B they decide the speed and the ceiling decides
what fits at all.

| # | Machine | Memory for weights | ~Bandwidth | Ceiling at Q4 | Status as of 2026-09-17 |
| --- | --- | --- | --- | --- | --- |
| 1 | **M1 Pro, 16 GB** — macOS | 16 GB unified | ~200 GB/s | 7B comfortable, 14b tight | Gate probe, container baseline, and four model-in-the-loop runs (coverage, precision, repeat-once, the tool-call probe). The Apple answer for anything that is not confinement |
| 2 | **Mac mini M4, 16 GB** | 16 GB unified | ~120 GB/s | same, slower | **Allocated for the first time**: Metal as the control for RADV's doubt, and the bandwidth floor — [`the-16gb-threshold`](../../RECORD/2026-09-16.the-16gb-threshold.WIP.md) §6–7. Cannot answer confinement at all: `enforcement = "kernel"` denies `run_command` on macOS, so `closes_on` is unreachable there |
| 3 | **MacBook M4 Pro, 48 GB** | 48 GB unified | ~273 GB/s | **32b comfortable** | Size sweep completed — [`the-size-sweep`](../../RECORD/2026-09-03.the-size-sweep.completed.md) |
| 4 | **Ryzen 5 3600 + RTX 5060 Ti** | 16 GB VRAM | ~448 GB/s | 14b comfortable — measured, 11.1/16.3 GB at ctx 8192; **27B IQ3_S is the open question** | Native Linux confinement, the 14B ceiling and KV precision, all closed. **The only box that can answer either of this revision's two crit rows** — a model inside a container (item 3) and 8K against 96K (item 2) — [`landlock-holds-natively`](../../RECORD/2026-09-08.landlock-holds-natively.completed.md), [`the-rtx-holds-14b`](../../RECORD/2026-09-08.the-rtx-holds-14b.completed.md), [`the-14b-context-ceiling`](../../RECORD/2026-09-08.the-14b-context-ceiling.completed.md) |
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

2. **Whether the 27B fits machine 4 at all** — roadmap item 2's precondition.
   [`the-14b-context-ceiling`](../../RECORD/2026-09-08.the-14b-context-ceiling.completed.md)
   closed with *"Not Qwen3.8 anything"*, and that was right about a ~17 GB Q4
   build. A 11.77 GB non-uniform packing is a different file, which is why the
   run is a fit test before it is a comparison. If it does not fit, item 2 moves
   to machine 3 and stops being one flag apart on a 16 GB box.

3. **A judge that is not the model under test** (P1) — roadmap item 13. Not a
   measurement to run: an argument to write. Every number this repository has was
   scored against a key on disk or by a person reading replies, and the tool-call
   probe is already past what that comfortably covers — four verdicts per reply,
   two models, five corpora.

4. **A ceiling** (P2) — the same open-weight family at 70B+, served remotely, to
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
