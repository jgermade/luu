#!/usr/bin/env python3
"""What a recording cost, so two runs can be compared rather than recalled.

    ./scripts/record-summary.py off.jsonl age.jsonl watermark.jsonl

One row per file: what was sent, how much of it a prefix cache could reuse, and
what the two mechanisms that give way — eviction and pruning — actually did.

Reads only what the format guarantees: `budget` and `prefix_reuse` describe the
call that starts a turn, `step_call` describes the ones a tool round trip adds,
and `evicted` / `pruned` are the tombstones. Every count is the recording's own,
by the counter the header names — this script adds, it does not measure.
"""

import json
import sys


def summarize(path):
    turns = calls = 0
    prompt_tokens = 0          # every call, the tool round trips included
    first_call_tokens = 0      # the turn's own prompt only
    shared = total = 0         # for the mean reuse, over every measured call
    evicted_turns = evicted_tokens = 0
    pruned_turns = pruned_tokens = 0
    evictions = prunes = 0
    header = {}

    for line in open(path):
        line = line.strip()
        if not line:
            continue
        row = json.loads(line)
        if row["channel"] == "header":
            header = row
            continue
        message = row["message"]
        kind = message.get("type")
        if kind == "turn_started":
            turns += 1
        elif kind == "budget":
            calls += 1
            spent = sum(b["tokens"] for b in message["buckets"] if b["name"] != "reserve")
            prompt_tokens += spent
            first_call_tokens += spent
        elif kind == "prefix_reuse":
            shared += message["shared_tokens"]
            total += message["prompt_tokens"]
        elif kind == "step_call":
            calls += 1
            prompt_tokens += message["prompt_tokens"]
            shared += message["shared_tokens"]
            total += message["prompt_tokens"]
        elif kind == "evicted":
            evictions += 1
            evicted_turns += len(message["turns"])
            evicted_tokens += message["tokens"]
        elif kind == "pruned":
            prunes += 1
            pruned_turns += len(message["turns"])
            pruned_tokens += message["tokens"]

    return {
        "file": path.rsplit("/", 1)[-1],
        "eviction": (header.get("eviction") or {}).get("policy", "?"),
        "pruning": (header.get("pruning") or {}).get("policy", "none"),
        "turns": turns,
        "calls": calls,
        "prompt tokens": prompt_tokens,
        "first calls": first_call_tokens,
        "reuse %": round(100 * shared / total, 1) if total else 0.0,
        "evictions": evictions,
        "turns evicted": evicted_turns,
        "tokens evicted": evicted_tokens,
        "prunes": prunes,
        "turns pruned": pruned_turns,
        "tokens pruned": pruned_tokens,
    }


def main(paths):
    rows = [summarize(path) for path in paths]
    columns = list(rows[0])
    widths = [max(len(str(row[c])) for row in rows + [dict(zip(columns, columns))]) for c in columns]
    print(" | ".join(c.ljust(w) for c, w in zip(columns, widths)))
    print("-+-".join("-" * w for w in widths))
    for row in rows:
        print(" | ".join(str(row[c]).ljust(w) for c, w in zip(columns, widths)))


if __name__ == "__main__":
    if len(sys.argv) < 2:
        sys.exit(__doc__)
    main(sys.argv[1:])
