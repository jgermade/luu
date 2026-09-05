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

## The inventory

Bandwidth figures are from specification, not measured here, and they are in the
table because for a memory-bound 7B they decide the speed and the ceiling decides
what fits at all.

| # | Machine | Memory for weights | ~Bandwidth | Ceiling at Q4 | Status as of 2026-09-05 |
| --- | --- | --- | --- | --- | --- |
| 1 | **M1 Pro, 16 GB** — macOS | 16 GB unified | ~200 GB/s | 7B comfortable, 14b tight | Gate probe and container baseline measured |
| 2 | **Mac mini M4, 16 GB** | 16 GB unified | ~120 GB/s | same, slower | Reserve / federation peer |
| 3 | **MacBook M4 Pro, 48 GB** | 48 GB unified | ~273 GB/s | **32b comfortable** | ~~Size sweep completed~~ (`the-size-sweep`) |
| 4 | **Ryzen 5 3600 + RTX 5060 Ti** | 16 GB VRAM | ~448 GB/s | 14b comfortable | Native Linux without a VM |
| 5 | **i5 9400F + GTX 1660 Super** | 6 GB VRAM | ~336 GB/s | 7B at the edge | Testing the hardware floor |
| 6 | **BC-250** — Zen 2 + RDNA 2 | 16 GB GDDR6 (~9.4 GiB usable — board carve-out) | ~224 GB/s | 7b comfortable, 14b unproven | ~~Vulkan serving stack measured~~ (`the-bc250-run`) |
| P1 | **BytePlus ModelArk** | — | — | whatever it serves | External judge candidate |
| P2 | **build.nvidia.com**, free tier | — | — | whatever it serves | Remote 32b/70b ceiling |

---

## What each one is for

| Machine | The question only it answers | Status |
| --- | --- | --- |
| ~~**3 · M4 Pro 48 GB**~~ | ~~Does the map's 0/6 → 6/6 survive at 14b and 32b?~~ | **Completed.** Both sizes go 6/6 on map; observed 14b `run_command` confinement denial and 32b unprompted write in `luu chat` — [`the-size-sweep`](../../RECORD/2026-09-03.the-size-sweep.completed.md) |
| ~~**1 · M1 Pro**~~ | ~~`run_command` with a model in the loop under container confinement~~ | **Completed.** Landlock ABI v8 verified; 15-prompt gate probe completed — [`the-container-observed`](../../RECORD/2026-09-03.the-container-observed.completed.md), [`the-gate-probe`](../../RECORD/2026-08-31.the-gate-probe.completed.md) |
| **5 · 1660 Super, 6 GB** | **Where the stated target breaks.** How much context a 6 GB card actually gives a 7B before out-of-memory | Unblocked by OpenAI backend |
| ~~**6 · BC-250**~~ | ~~A third serving stack. Neither Metal nor CUDA — llama.cpp over Vulkan on AMD hardware~~ | **Completed.** RADV Vulkan + `llama-server` + `luu`'s OpenAI backend run end to end with no vendor assumptions; board's real usable memory is ~9.4 GiB, not 16 GiB — [`the-bc250-run`](../../RECORD/2026-09-04.the-bc250-run.completed.md) |
| **4 · Ryzen + 5060 Ti** | **Native Linux without a VM**, measuring bare-metal Landlock + seccomp vs VM overhead | Unblocked |
| **P1 · ModelArk** | **A judge that is not the model under test**, automating probe scoring | Needs design argument |
| **P2 · build.nvidia.com** | **A ceiling.** The same open-weight family at 70B+ served remotely | Unblocked by OpenAI backend |

---

## Next measurements to run

Unchanged from [the 2026-09-04 revision](../2026-09-04/machines.md): no machine
was reached since, and a status line that moved without a run behind it is the
exact rot this file's revisions exist to prevent. What remains of Phase 2:

1. **The hardware floor (Machine 5 - 1660 Super 6 GB)**: Run `luu chat` with `--backend openai` against `llama-server` on the 6 GB card. Test 7B with 8K context and find where VRAM limits force offloading or truncation.
2. **Native Linux confinement (Machine 4 - Linux RTX)**: Test native Landlock enforcement without a virtualization boundary.
3. **The BC-250's 14B ceiling** (deferred from `the-bc250-run`): with only ~9.4 GiB usable, confirm by measurement whether a 14B Q4 model loads at all rather than relying on the estimate.
