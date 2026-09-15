#!/usr/bin/env python3
"""Minimal stdlib-only GGUF header/tensor-info reader, for comparing two GGUF
files' metadata and per-tensor quantization without running any inference.
No third-party gguf package is installed on this machine, so this reads the
binary format directly (documented at
https://github.com/ggml-org/ggml/blob/master/docs/gguf.md)."""
import struct
import sys
import json
from collections import Counter

GGUF_TYPE_NAMES = {
    0: "UINT8", 1: "INT8", 2: "UINT16", 3: "INT16", 4: "UINT32", 5: "INT32",
    6: "FLOAT32", 7: "BOOL", 8: "STRING", 9: "ARRAY", 10: "UINT64",
    11: "INT64", 12: "FLOAT64",
}

TENSOR_TYPE_NAMES = {
    0: "F32", 1: "F16", 2: "Q4_0", 3: "Q4_1", 6: "Q5_0", 7: "Q5_1", 8: "Q8_0",
    9: "Q8_1", 10: "Q2_K", 11: "Q3_K", 12: "Q4_K", 13: "Q5_K", 14: "Q6_K",
    15: "Q8_K", 16: "IQ2_XXS", 17: "IQ2_XS", 18: "IQ3_XXS", 19: "IQ1_S",
    20: "IQ4_NL", 21: "IQ3_S", 22: "IQ2_S", 23: "IQ4_XS", 24: "I8", 25: "I16",
    26: "I32", 27: "I64", 28: "F64", 29: "IQ1_M", 30: "BF16",
}


class Reader:
    def __init__(self, f):
        self.f = f

    def read(self, n):
        b = self.f.read(n)
        if len(b) != n:
            raise EOFError("truncated file")
        return b

    def u32(self):
        return struct.unpack("<I", self.read(4))[0]

    def u64(self):
        return struct.unpack("<Q", self.read(8))[0]

    def i32(self):
        return struct.unpack("<i", self.read(4))[0]

    def string(self):
        n = self.u64()
        return self.read(n).decode("utf-8", errors="replace")

    def value(self, vtype):
        if vtype == 8:  # STRING
            return self.string()
        if vtype == 9:  # ARRAY
            item_type = self.u32()
            count = self.u64()
            return [self.value(item_type) for _ in range(count)]
        sizes = {0: 1, 1: 1, 2: 2, 3: 2, 4: 4, 5: 4, 6: 4, 7: 1, 10: 8, 11: 8, 12: 8}
        fmts = {0: "<B", 1: "<b", 2: "<H", 3: "<h", 4: "<I", 5: "<i",
                6: "<f", 7: "<B", 10: "<Q", 11: "<q", 12: "<d"}
        size = sizes[vtype]
        raw = self.read(size)
        val = struct.unpack(fmts[vtype], raw)[0]
        return val


def read_gguf_summary(path):
    with open(path, "rb") as fh:
        r = Reader(fh)
        magic = r.read(4)
        if magic != b"GGUF":
            raise ValueError(f"{path}: not a GGUF file (magic={magic!r})")
        version = r.u32()
        tensor_count = r.u64()
        kv_count = r.u64()
        kv = {}
        for _ in range(kv_count):
            key = r.string()
            vtype = r.u32()
            kv[key] = r.value(vtype)
        tensors = []
        for _ in range(tensor_count):
            name = r.string()
            n_dims = r.u32()
            dims = [r.u64() for _ in range(n_dims)]
            ttype = r.u32()
            offset = r.u64()
            tensors.append((name, dims, ttype, offset))
        return version, kv, tensors


def summarize(path):
    version, kv, tensors = read_gguf_summary(path)
    type_hist = Counter(TENSOR_TYPE_NAMES.get(t, f"?{t}") for _, _, t, _ in tensors)
    # Per-tensor-name type, for a name-by-name diff (not just a histogram).
    per_tensor = {name: TENSOR_TYPE_NAMES.get(t, f"?{t}") for name, _, t, _ in tensors}
    interesting_keys = [k for k in kv if any(
        s in k for s in (
            "quantization", "file_type", "architecture", "rope", "context_length",
            "embedding_length", "block_count", "head_count", "vocab_size",
        )
    )]
    meta = {k: kv[k] for k in sorted(interesting_keys)}
    return {
        "path": path,
        "gguf_version": version,
        "tensor_count": len(tensors),
        "kv_count": len(kv),
        "type_histogram": dict(type_hist),
        "meta": meta,
        "per_tensor": per_tensor,
    }


def main():
    a_path, b_path = sys.argv[1], sys.argv[2]
    a = summarize(a_path)
    b = summarize(b_path)
    print(f"=== {a_path} ===")
    print(json.dumps({k: v for k, v in a.items() if k != "per_tensor"}, indent=2))
    print(f"=== {b_path} ===")
    print(json.dumps({k: v for k, v in b.items() if k != "per_tensor"}, indent=2))

    print("\n=== per-tensor type differences ===")
    names = sorted(set(a["per_tensor"]) | set(b["per_tensor"]))
    diffs = 0
    for name in names:
        ta = a["per_tensor"].get(name, "<missing>")
        tb = b["per_tensor"].get(name, "<missing>")
        if ta != tb:
            diffs += 1
            print(f"  {name}: {ta} -> {tb}")
    print(f"total tensors: {len(names)}, differing: {diffs}")


if __name__ == "__main__":
    main()
