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
| 4 | **Ryzen 5 3600 + RTX 5060 Ti** | 16 GB VRAM | ~448 GB/s | 14b comfortable — confirmed, 11.1/16.3 GB at ctx 8192 | ~~Native Linux without a VM~~ (`the-landlock-holds-natively`), ~~14B ceiling confirmed~~ (`the-rtx-holds-14b`) |
| 5 | **i5 9400F + GTX 1660 Super** | 6 GB VRAM | ~336 GB/s | 7B at the edge, degrades rather than breaks | ~~Hardware floor measured~~ (`the-floor-on-6gb`) |
| 6 | **BC-250** — Zen 2 + RDNA 2 | 16 GB GDDR6 (~9.4 GiB usable — board carve-out) | ~224 GB/s | 7b comfortable, 14b unproven | ~~Vulkan serving stack measured~~ (`the-bc250-run`) |
| P1 | **BytePlus ModelArk** | — | — | whatever it serves | External judge candidate |
| P2 | **build.nvidia.com**, free tier | — | — | whatever it serves | Remote 32b/70b ceiling |

---

## What each one is for

| Machine | The question only it answers | Status |
| --- | --- | --- |
| ~~**3 · M4 Pro 48 GB**~~ | ~~Does the map's 0/6 → 6/6 survive at 14b and 32b?~~ | **Completed.** Both sizes go 6/6 on map; observed 14b `run_command` confinement denial and 32b unprompted write in `luu chat` — [`the-size-sweep`](../../RECORD/2026-09-03.the-size-sweep.completed.md) |
| ~~**1 · M1 Pro**~~ | ~~`run_command` with a model in the loop under container confinement~~ | **Completed.** Landlock ABI v8 verified; 15-prompt gate probe completed — [`the-container-observed`](../../RECORD/2026-09-03.the-container-observed.completed.md), [`the-gate-probe`](../../RECORD/2026-08-31.the-gate-probe.completed.md) |
| ~~**5 · 1660 Super, 6 GB**~~ | ~~Where the stated target breaks.~~ It doesn't break qualitatively — it degrades to partial CPU offload (18% at 8192) at no measurable decode cost; 100% GPU residency needs under ~400 tokens of configured context, and even that cutover moves with the desktop compositor's own VRAM use | **Completed.** [`the-floor-on-6gb`](../../RECORD/2026-09-08.the-floor-on-6gb.completed.md) |
| ~~**6 · BC-250**~~ | ~~A third serving stack. Neither Metal nor CUDA — llama.cpp over Vulkan on AMD hardware~~ | **Completed.** RADV Vulkan + `llama-server` + `luu`'s OpenAI backend run end to end with no vendor assumptions; board's real usable memory is ~9.4 GiB, not 16 GiB — [`the-bc250-run`](../../RECORD/2026-09-04.the-bc250-run.completed.md) |
| ~~**4 · Ryzen + 5060 Ti**~~ | ~~Native Linux without a VM~~, measuring bare-metal Landlock + seccomp vs VM overhead | **Completed.** `landlock ABI v10 + seccomp` hold the child directly in `pre_exec` — no VM, no container — with a real model (`qwen2.5-coder-7b`) proposing the call — [`the-landlock-holds-natively`](../../RECORD/2026-09-08.landlock-holds-natively.completed.md). The 14B ceiling is confirmed by measurement: 11.1/16.3 GB VRAM at ctx 8192, full offload — [`the-rtx-holds-14b`](../../RECORD/2026-09-08.the-rtx-holds-14b.completed.md) — and stretched further with quantized KV cache (`-ctk`/`-ctv`, mainline `llama-server`, no TurboQuant fork needed): a comfortable ceiling of ctx 49152 at q8_0 or 65536 at q4_0, both verified against a real generated turn — [`the-14b-context-ceiling`](../../RECORD/2026-09-08.the-14b-context-ceiling.completed.md). Speed stays unmeasured by design (this project scores answers, not seconds) |
| **P1 · ModelArk** | **A judge that is not the model under test**, automating probe scoring | Needs design argument |
| **P2 · build.nvidia.com** | **A ceiling.** The same open-weight family at 70B+ served remotely | Unblocked by OpenAI backend |

---

## Next measurements to run

Updated 2026-09-08: machines 4, 5 and 6 have each been reached since this
revision was written — see the struck rows above, each linking the record that
closed it. This is the rot the note above used to warn about; it is now fixed
rather than repeated. What remains of Phase 2:

1. **The BC-250's 14B ceiling** (deferred from `the-bc250-run`): with only ~9.4 GiB usable, confirm by measurement whether a 14B Q4 model loads at all rather than relying on the estimate. Machine 4's equivalent question is now answered — see below — so this is the only unconfirmed ceiling left in the inventory.
