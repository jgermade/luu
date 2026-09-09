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

**What changed in this revision:** the previous one carried three unanswered
questions and a note saying geography blocked them. All three have been answered,
and the phase they belonged to is closed except for one ceiling. The file is
shorter than it was on purpose: a machine whose question is answered is a row in
the record, not a row in the plan.

## The inventory

Bandwidth figures are from specification, not measured here, and they are in the
table because for a memory-bound 7B they decide the speed and the ceiling decides
what fits at all.

| # | Machine | Memory for weights | ~Bandwidth | Ceiling at Q4 | Status as of 2026-09-08 |
| --- | --- | --- | --- | --- | --- |
| 1 | **M1 Pro, 16 GB** — macOS | 16 GB unified | ~200 GB/s | 7B comfortable, 14b tight | Gate probe, container baseline, and both model-in-the-loop runs (coverage, precision) |
| 2 | **Mac mini M4, 16 GB** | 16 GB unified | ~120 GB/s | same, slower | Reserve. Never allocated a question of its own |
| 3 | **MacBook M4 Pro, 48 GB** | 48 GB unified | ~273 GB/s | **32b comfortable** | Size sweep completed — [`the-size-sweep`](../../RECORD/2026-09-03.the-size-sweep.completed.md) |
| 4 | **Ryzen 5 3600 + RTX 5060 Ti** | 16 GB VRAM | ~448 GB/s | 14b comfortable — measured, 11.1/16.3 GB at ctx 8192 | Native Linux confinement and the 14B ceiling, both closed — [`landlock-holds-natively`](../../RECORD/2026-09-08.landlock-holds-natively.completed.md), [`the-rtx-holds-14b`](../../RECORD/2026-09-08.the-rtx-holds-14b.completed.md), [`the-14b-context-ceiling`](../../RECORD/2026-09-08.the-14b-context-ceiling.completed.md) |
| 5 | **i5 9400F + GTX 1660 Super** | 6 GB VRAM | ~336 GB/s | 7B at the edge, degrades rather than breaks | Hardware floor measured — [`the-floor-on-6gb`](../../RECORD/2026-09-08.the-floor-on-6gb.completed.md) |
| 6 | **BC-250** — Zen 2 + RDNA 2 | 16 GB GDDR6 (~9.4 GiB usable — board carve-out) | ~224 GB/s | 7b comfortable, **14b unproven** | Vulkan serving stack measured; the ceiling is the one question left open in this file — [`the-bc250-run`](../../RECORD/2026-09-04.the-bc250-run.completed.md) |
| 7 | **A GitHub runner** — `ubuntu-latest`, Linux 6.17 | — | — | no model | **Level 3 on Linux, on every push**: the image builds, the handshake holds, and Landlock ABI v7 + seccomp hold `run_command` inside stock Docker — [`the-container-on-a-runner`](../../RECORD/2026-09-08.the-container-on-a-runner.completed.md) |
| P1 | **BytePlus ModelArk** | — | — | whatever it serves | External judge candidate. Needs a design argument, not an endpoint |
| P2 | **build.nvidia.com**, free tier | — | — | whatever it serves | Remote 32b/70b ceiling. Unblocked by the OpenAI backend and unscheduled |

## What is left

1. **The BC-250's 14B ceiling** — roadmap item 7, deferred from
   [`the-bc250-run`](../../RECORD/2026-09-04.the-bc250-run.completed.md). With
   ~9.4 GiB usable rather than the board's nominal 16, whether a 14B Q4 loads at
   all is an estimate and not a measurement. Machine 4 answered the equivalent
   question on CUDA — including that quantized KV cache moves the ceiling much
   further than expected — so the interesting half here is whether the same
   `-ctk`/`-ctv` trick pays on RADV/Vulkan, where nothing has tested it.

2. **A judge that is not the model under test** (P1) — roadmap item 8. Not a
   measurement to run: an argument to write. Every number this repository has
   was scored against a key on disk or by a person reading replies, and the
   tool-call probe of item 4 is the first whose scoring is four-way and per
   reply.

3. **A ceiling** (P2) — the same open-weight family at 70B+, served remotely, to
   say what the local numbers are a fraction of. Unblocked since the OpenAI
   backend landed and never ordered, because nothing in the tree depends on the
   answer. It stays here so that "nobody scheduled it" does not become "nobody
   remembered it".

**The newest row is not a box anybody owns.** Machine 7 is a CI runner, and it
earns a row under the organising rule like any other: it answers *does level 3
hold on Linux*, on every push, with no hand on anything — a question five local
machines could answer and none had. What it cannot answer is anything with a
model in it, which is what keeps machines 1 and 4 where they are.

**Not in this list, deliberately:** speed. Machine 4's records say it and it is
the standing rule — this project scores answers, not seconds. Tokens per second
would make every box comparable on the axis that says least about whether the
context manager works.
