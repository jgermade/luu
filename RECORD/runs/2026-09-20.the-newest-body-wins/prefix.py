#!/usr/bin/env python3
"""The cost of the repair, out of the recording's own `prefix_reuse` lines.

`luu count` has no opinion about prefixes and should not grow one: a prefix is
a fact about two consecutive renders, and every number that surface carries is
a fact about one session. So this reads the lines directly, which is the one
place in this run where a `grep` is the right instrument rather than the
shortcut `one-counting-surface` was written against.

Takes the run directory and writes the table to stdout.
"""
import json
import pathlib
import sys

out = pathlib.Path(sys.argv[1])
rows = {}
for arm in ["before", "after", "before-sel", "after-sel"]:
    reuse, history = [], 0
    for line in (out / f"{arm}.jsonl").read_text().splitlines():
        message = json.loads(line).get("message") or {}
        if message.get("type") == "prefix_reuse":
            reuse.append((message["turn"], message["shared_tokens"], message["prompt_tokens"]))
        if message.get("type") == "budget":
            history += next(
                (b["tokens"] for b in message["buckets"] if b["name"] == "history"), 0
            )
    rows[arm] = (reuse, history)

print("# Prefix reuse, the fix against its parent commit\n")
print("Turn 1 has no previous render to share a prefix with and is not in the")
print("table. `before` is `2e87d2c`, `after` is the commit this run belongs to.\n")
for before, after, label in [
    ("before", "after", "fragments, the corpus as written"),
    ("before-sel", "after-sel", "`--select-tokens 1024`"),
]:
    a = {t: (s, p) for t, s, p in rows[before][0]}
    b = {t: (s, p) for t, s, p in rows[after][0]}
    sa, pa = sum(s for s, _ in a.values()), sum(p for _, p in a.values())
    sb, pb = sum(s for s, _ in b.values()), sum(p for _, p in b.values())
    print(f"## {label}\n")
    print(f"- session prefix reuse **{100 * sa / pa:.2f}% -> {100 * sb / pb:.2f}%**, "
          f"{100 * sb / pb - 100 * sa / pa:+.2f} points")
    print(f"- history bucket, summed over renders: **{rows[before][1]} -> {rows[after][1]} tokens**\n")
    print("| turn | reuse before | reuse after | prompt before | prompt after |")
    print("| ---: | ---: | ---: | ---: | ---: |")
    for turn in sorted(a):
        (s, p), (t, q) = a[turn], b[turn]
        print(f"| {turn} | {100 * s / p:.1f}% | {100 * t / q:.1f}% | {p} | {q} |")
    print()
