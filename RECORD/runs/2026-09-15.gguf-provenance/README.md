# Two GGUF pairs, read rather than run

No model call in this directory — everything here comes from reading four
GGUF headers on disk, on machine 4. No third-party `gguf` package is
installed here (`python3 -c "import gguf"` fails), so `gguf_diff.py` parses
the format directly against its own documented binary layout
(https://github.com/ggml-org/ggml/blob/master/docs/gguf.md): magic, version,
tensor/KV counts, then the KV pairs, then each tensor's name/dims/type/offset.
No tensor *data* is read, only the header — `llama-gguf r` on this machine
confirms all four files pass its own tensor-data checksum, which this script
never touches.

## The four files

| | local HF cache (what every `llama-server` run in this project uses) | Ollama registry pull |
| --- | --- | --- |
| 7B | `/home/joshua/models/qwen2.5-coder-7b/qwen2.5-coder-7b-instruct-q4_k_m.gguf` | `~/.ollama/models/blobs/sha256-60e05f2100071479f596b964f89f510f057ce397ea22f2833a0cfe029bfc2463` (4 683 074 048 bytes) |
| 14B | `/home/joshua/models/qwen2.5-coder-14b/qwen2.5-coder-14b-instruct-q4_k_m.gguf` | `~/.ollama/models/blobs/sha256-ac9bc7a69dab38da1c790838955f1293420b55ab555ef6b4615efa1c1507b1ed` (8 988 110 784 bytes) |

The registry blobs are named by the layer digest `ollama list` /
`~/.ollama/models/manifests/registry.ollama.ai/library/qwen2.5-coder/{7b,14b}`
resolve to — the same two files
[`the-drift-was-never-the-backend`](../../2026-09-15.the-drift-was-never-the-backend.completed.md)
and
[`a-grammar-for-tool-calls`](../../2026-09-06.a-grammar-for-tool-calls.WIP.md)'s
"the 14B's two-GGUF check" section (2026-09-15) measured against.

## Reproducing

```sh
python3 gguf_diff.py <local.gguf> <registry-blob> | tee pair.diff.txt
```

`7b.diff.txt` and `14b.diff.txt` are that command's output for each pair —
GGUF version, tensor/KV counts, the tensor-type histogram, the filtered
architecture/quantization metadata, and a per-tensor-name type diff.
`7b.metadata.txt` and `14b.metadata.txt` are the *unfiltered* KV dump for each
file in the pair (long array values — `tokenizer.ggml.tokens`,
`tokenizer.ggml.merges`, `tokenizer.ggml.token_type` — elided; everything
scalar kept), which is where the provenance fields
(`general.finetune`, `general.name`, `general.base_model.*`) live that the
filtered diff does not print.

## What they show

**Both pairs: identical nominal quantization, non-identical actual
quantization.** Same `general.file_type` (15, Q4_K_M), same tensor-type
histogram in aggregate (7B: 169 `Q4_K` / 141 `F32` / 29 `Q6_K` of 339 tensors
both files; 14B: 289 / 241 / 49 of 579 both files) — but not the same
*assignment*. 16 of the 7B's 339 tensors and 14 of the 14B's 579 disagree on
which specific `attn_v.weight` / `ffn_down.weight` in which block got `Q4_K`
against `Q6_K`, always as a straight swap (never `Q4_K` or `Q6_K` in both —
one file's `Q4_K` is always the other's `Q6_K` on every disagreeing tensor).
`llama-quantize --help` on this machine documents `--imatrix` as exactly the
kind of input that would move this specific choice: llama.cpp's `*_M` mixture
decides which layers earn the extra bits partly from importance data when an
imatrix is supplied, so two runs quantizing the same fp16 source with
different (or absent) imatrix files produce exactly this shape of
disagreement — same histogram, different map. This is consistent with, not
proof of, an imatrix difference: neither imatrix file (if either quantizer
used one) is available to this session to compare directly.

**The provenance metadata diverges the same way for both pairs, in the
direction that says "two independent conversions," not "one file copied."**
Local 7B: `general.finetune = "Instruct-GGUF"`, `general.name = "Qwen2.5
Coder 7B Instruct GGUF"`, no `general.base_model.*` / `license` / `tags`
block — the shape of a third-party GGUF repack with the source repo's own
metadata stripped. Registry 7B: `general.finetune = "Instruct"`, full
`general.base_model.0.{name,organization,repo_url}` pointing at
`https://huggingface.co/Qwen/Qwen2.5-Coder-7B`, plus `license` and `tags` —
the shape of a conversion that kept the upstream repo's card intact.

**The 14B pair carries a second, much larger difference the 7B pair does
not.** Local 14B: `general.finetune = "Instruct-AWQ"`, `general.name =
"Qwen2.5 Coder 14B Instruct AWQ"`. AWQ is a 4-bit *quantization* method in its
own right — this file's own metadata says its source was already
4-bit-quantized before being converted and requantized to GGUF Q4_K_M, i.e.
every 14B `llama-server` run in this project's records so far ran a
**twice-quantized** model. The registry pull's 14B metadata has no `-AWQ`
anywhere and carries the same clean `base_model.0.repo_url` shape the 7B
registry pull does, pointing at
`https://huggingface.co/Qwen/Qwen2.5-Coder-14B` — a direct, once-quantized
conversion.

`qwen2.context_length`: 131072 (local, both sizes) against 32768 (registry,
both sizes) — a real metadata difference, but not one that reaches behavior
here. Every tool-call-probe run measured against either file used `--ctx-size
8192`, inside both numbers, so no RoPE extension triggers under either
value; this is a provenance detail (which of Qwen's own advertised context
figures each converter chose to write down), not a runtime one.
