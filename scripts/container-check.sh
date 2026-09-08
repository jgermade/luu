#!/usr/bin/env bash
# Level 3, run rather than described: the image, the handshake, and the same
# tool calls answered inside the container as outside it.
#
# Every contained run this repository had until now was Docker Desktop on macOS,
# by hand, once — RECORD/2026-09-03.the-container-observed.completed.md. This is
# the same walk, on Linux, in a script, so CI can take it and a person can too:
#
#   scripts/container-check.sh                 # builds the image, uses target/release/luu
#   LUU_BIN=target/debug/luu scripts/container-check.sh
#   SKIP_BUILD=1 scripts/container-check.sh    # against an image already built
#
# It asserts what must hold and *prints* what is measured — which kernel held
# the call is a fact about the machine it ran on, and belongs in a record rather
# than in an assertion.
set -euo pipefail

cd "$(dirname "$0")/.."

luu="${LUU_BIN:-target/release/luu}"
image="${LUU_IMAGE:-luu-worker:dev}"
policy="luu.container.toml"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

[ -x "$luu" ] || { echo "no luu binary at $luu — cargo build --release --bin luu"; exit 1; }

say() { printf '\n\033[1m== %s\033[0m\n' "$1"; }
has() { grep -qF "$2" "$1" || { echo "expected to find: $2"; echo "--- in ---"; cat "$1"; exit 1; }; }
hasnt() { grep -qF "$2" "$1" && { echo "did not expect: $2"; echo "--- in ---"; cat "$1"; exit 1; }; return 0; }

if [ "${SKIP_BUILD:-0}" != "1" ]; then
  say "docker build"
  docker build -t "$image" -f Containerfile .
fi

# 1. The handshake, and the image's manifest against the policy's `commands`.
#    `absent` is the third failure mode — granted by the policy, absent from the
#    image — and an image that has drifted from the file says so here.
say "the resolved sandbox, through the container"
"$luu" tools --sandbox "$policy" >"$work/tools.txt" 2>&1 || { cat "$work/tools.txt"; exit 1; }
cat "$work/tools.txt"
has "$work/tools.txt" "worker     docker ($image)"
has "$work/tools.txt" "enforce    kernel"
hasnt "$work/tools.txt" "absent"

# 2. A read of a file in the tree, executed on the far side of the pipe.
say "a read, inside the container"
"$luu" chat "read it" --sandbox "$policy" --mock-delay-ms 0 \
  --record "$work/contained.jsonl" \
  --mock-reply '```tool
{"name":"read_file","arguments":{"path":"AGENTS.md","max_lines":3}}
```' \
  --mock-reply 'done' >"$work/contained.txt" 2>&1 || { cat "$work/contained.txt"; exit 1; }
cat "$work/contained.txt"
has "$work/contained.txt" "[1] ← ok"

# 3. The same call on the host. A contained call and a host call must answer the
#    same bytes: that is what makes a run under this policy comparable with one
#    without it, and it is the property the `direct` tests assert without a
#    runtime installed.
say "the same read, on the host"
"$luu" chat "read it" --worker host --mock-delay-ms 0 \
  --record "$work/host.jsonl" \
  --mock-reply '```tool
{"name":"read_file","arguments":{"path":"AGENTS.md","max_lines":3}}
```' \
  --mock-reply 'done' >"$work/host.txt" 2>&1 || { cat "$work/host.txt"; exit 1; }

python3 - "$work/contained.jsonl" "$work/host.jsonl" <<'PY'
import json, sys

def result(path):
    """The first `tool_result` of a recording. Lines are `{channel, at_ms,
    message}` around the protocol message, and the header line has neither."""
    for line in open(path):
        message = json.loads(line).get("message") or {}
        if message.get("type") == "tool_result":
            return message
    raise SystemExit(f"no tool_result in {path}")

contained, host = result(sys.argv[1]), result(sys.argv[2])
for field in ("output", "truncated", "error"):
    if contained.get(field) != host.get(field):
        raise SystemExit(
            f"the container and the host disagree on `{field}`:\n"
            f"  contained: {contained.get(field)!r}\n"
            f"  host:      {host.get(field)!r}"
        )
print(f"same bytes both sides: {len(contained.get('output') or '')} of them")
print("contained verdict:", json.dumps(contained.get("verdict")))
print("host verdict:      ", json.dumps(host.get("verdict")))
PY

# 4. A denial is a denial on the far side too — and it is the *sandbox* that
#    refuses, not the container's own filesystem being different.
say "a read outside the tree, inside the container"
"$luu" chat "read it" --sandbox "$policy" --mock-delay-ms 0 \
  --mock-reply '```tool
{"name":"read_file","arguments":{"path":"/etc/passwd"}}
```' \
  --mock-reply 'done' >"$work/denied.txt" 2>&1 || { cat "$work/denied.txt"; exit 1; }
cat "$work/denied.txt"
has "$work/denied.txt" "denied"

# 5. A child process, which is the whole reason level 3 exists: on a Mac there
#    is no Landlock and this is the call that could not be held.
say "run_command, inside the container"
"$luu" chat "list it" --sandbox "$policy" --mock-delay-ms 0 \
  --mock-reply '```tool
{"name":"run_command","arguments":{"command":"ls","args":["-1","AGENTS.md"]}}
```' \
  --mock-reply 'done' >"$work/command.txt" 2>&1 || { cat "$work/command.txt"; exit 1; }
cat "$work/command.txt"
has "$work/command.txt" "[1] ← ok"
has "$work/command.txt" "held by"

say "what held it, on this machine"
grep -o "held by [^·]*" "$work/command.txt" | head -1

say "level 3 holds on $(uname -s) $(uname -r)"
