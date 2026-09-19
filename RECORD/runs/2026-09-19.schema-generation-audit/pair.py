#!/usr/bin/env python3
"""The two pre/post pairs the committed evidence supports, prompt by prompt.

`82229cc` (2026-09-15 11:38) changed `run_command`'s schema and nothing else.
Three of the fifteen tool-call-probe prompts call `run_command` — 4, 9 and 14 —
so those three are the whole of what the change could reach. This prints
whether that bound holds in the recordings.

- The **14B pair** is one flag apart: same file, machine, backend, flags and
  day, two binaries. It measures the schema.
- The **7B pair** is not: same weights, engine and flags, but machine 1 → 4, a
  different Ollama version and a different day. It compares, and the 14B pair
  is what says how to read it.

Writes verdicts.json. Run from the repository root.
"""

import json
import pathlib
import re
import sys

ROOT = pathlib.Path(__file__).resolve().parents[3]
HERE = pathlib.Path(__file__).resolve().parent
RUNS = ROOT / "RECORD/runs"
RUN_COMMAND_PROMPTS = {4, 9, 14}

PAIRS = [
    {
        "model": "14B",
        "pre": "2026-09-15.14b-tool-call-probe",
        "post": "2026-09-15.14b-local-postfix",
        "held_constant": ["weights (qwen2.5-coder-14b-instruct-q4_k_m.gguf)",
                          "llama-server", "machine 4", "corpus", "flags",
                          "the day"],
        "also_differs": [],
    },
    {
        "model": "7B",
        "pre": "2026-09-14.tool-call-probe",
        "post": "2026-09-15.7b-ollama-registry",
        "held_constant": ["weights (sha256-60e05f21…)", "Ollama", "corpus",
                          "--context-limit 8192", "--temperature 0", "--seed 7"],
        "also_differs": ["machine 1 → 4", "Ollama version", "the day"],
    },
]


def verdicts(run):
    """A run's per-prompt verdicts, from whichever shape it committed them in."""
    stats = RUNS / run / "stats.json"
    if stats.exists():
        return {row["n"]: row["verdict"] for row in json.loads(stats.read_text())}
    rows = {}
    for line in (RUNS / run / "README.md").read_text().splitlines():
        cells = [c.strip() for c in line.strip().strip("|").split("|")]
        if len(cells) == 2 and re.fullmatch(r"\d+", cells[0]):
            rows[int(cells[0])] = cells[1]
    if not rows:
        sys.exit(f"{run}: no stats.json and no verdict table in README.md")
    return rows


def compare(pair):
    pre, post = verdicts(pair["pre"]), verdicts(pair["post"])
    missing = sorted(set(range(1, 16)) - (pre.keys() & post.keys()))
    if missing:
        sys.exit(f"{pair['pre']} / {pair['post']}: no verdict on both sides for {missing}")

    prompts = [
        {
            "n": n,
            "calls_run_command": n in RUN_COMMAND_PROMPTS,
            "pre_argv": pre[n],
            "post_argv": post[n],
            "changed": pre[n] != post[n],
        }
        for n in range(1, 16)
    ]
    changed = [p["n"] for p in prompts if p["changed"]]
    return {
        **pair,
        "one_flag_apart": not pair["also_differs"],
        "prompts": prompts,
        "changed": changed,
        "changed_that_call_run_command": [n for n in changed if n in RUN_COMMAND_PROMPTS],
        "changed_that_cannot_have_been_the_schema": [
            n for n in changed if n not in RUN_COMMAND_PROMPTS
        ],
    }


def main():
    out = {
        "fix": "82229cc 2026-09-15 11:38:17 +0200",
        "run_command_prompts": sorted(RUN_COMMAND_PROMPTS),
        "pairs": [compare(p) for p in PAIRS],
    }
    (HERE / "verdicts.json").write_text(json.dumps(out, indent=1) + "\n")
    for pair in out["pairs"]:
        flag = "one flag apart" if pair["one_flag_apart"] else "not one flag apart"
        print(f'{pair["model"]}  {pair["pre"]} -> {pair["post"]}  ({flag})')
        for p in pair["prompts"]:
            if p["changed"]:
                tool = "run_command" if p["calls_run_command"] else ""
                print(f'  {p["n"]:>2}  {p["pre_argv"]:<19} -> {p["post_argv"]:<19} {tool}')
        print(f'  changed: {pair["changed"]}, of which run_command:'
              f' {pair["changed_that_call_run_command"]}\n')


if __name__ == "__main__":
    main()
