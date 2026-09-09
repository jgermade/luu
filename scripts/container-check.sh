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

# 6. The whole point of the surface work: a *session* choosing the container,
#    the way a person does it in the browser — a name in config.toml, a POST,
#    and a tool call that runs on the far side of the pipe.
say "a session started on a container posture"
home="$work/home"
mkdir -p "$home"
cat > "$home/config.toml" <<TOML
[provider.here]
backend = "mock"
model = "mock"

[posture.container]
policy = "$(pwd)/$policy"
TOML

port=7893
# Three replies, because a prompt at the gate is three model calls: the plan a
# person answers, the call the approved job makes, and the answer.
LUU_HOME="$home" "$luu" serve --bind "127.0.0.1:$port" --no-store --mock-delay-ms 0 \
  --mock-reply '```plan
{"objective":"list a file","steps":["run ls"],"files":[],"commands":["ls"]}
```' \
  --mock-reply '```tool
{"name":"run_command","arguments":{"command":"ls","args":["-1","AGENTS.md"]}}
```' \
  --mock-reply 'done' >"$work/serve.log" 2>&1 &
serving=$!
trap 'kill $serving 2>/dev/null; rm -rf "$work"' EXIT

for waited in $(seq 1 60); do
  curl -fsS "http://127.0.0.1:$port/api/settings" >/dev/null 2>&1 && break
  sleep 0.5
done

# It starts on the server's own policy file, which has no container in it.
curl -fsS "http://127.0.0.1:$port/api/settings" >"$work/before.json"
python3 -c "
import json, sys
before = json.load(open('$work/before.json'))
assert before['posture'].get('name') is None, before['posture']
assert before['posture']['runtime'] == 'host', before['posture']
print('before:', json.dumps(before['posture']))
"

# The names the page is offered, and then the session.
curl -fsS "http://127.0.0.1:$port/api/postures" >"$work/postures.json"
grep -q '"container"' "$work/postures.json" || { cat "$work/postures.json"; exit 1; }

curl -fsS -X POST "http://127.0.0.1:$port/api/sessions" \
  -H 'content-type: application/json' \
  -d '{"posture":"container"}' >/dev/null

curl -fsS "http://127.0.0.1:$port/api/settings" >"$work/after.json"
python3 -c "
import json
after = json.load(open('$work/after.json'))
posture = after['posture']
assert posture['name'] == 'container', posture
assert 'docker' in posture['runtime'], posture
print('after: ', json.dumps(posture))
"

# And a tool call under it, over the socket the page uses. This step is the
# whole ladder in one line: the same prompt on the *host* posture is refused on
# a machine without Landlock — "the kernel cannot hold this child" — and it runs
# here because the container is what provides one. Node is on every
# GitHub runner and is not a build dependency of this repository, so its absence
# is said out loud rather than failing the check: everything above this line has
# already proved the container.
if ! command -v node >/dev/null 2>&1; then
  echo "no node on PATH: the session's own tool call is not exercised here"
else
node - "$port" <<'NODE' || { cat "$work/serve.log"; exit 1; }
const port = process.argv[2]
const ws = new WebSocket(`ws://127.0.0.1:${port}/ws`)
let job = null
const done = new Promise((resolve, reject) => {
  setTimeout(() => reject(new Error("the turn never finished")), 120000)
  ws.addEventListener("open", () => {
    ws.send(JSON.stringify({ type: "prompt", text: "list it" }))
  })
  ws.addEventListener("message", event => {
    const message = JSON.parse(event.data)
    if (message.type === "job_proposed") {
      job = message.job
      ws.send(JSON.stringify({
        type: "approve_job", job, files: [], writes: [],
        commands: ["ls"], closes_on: null,
      }))
    }
    if (message.type === "tool_result") {
      console.log("  contained call:", JSON.stringify(message.verdict))
      if (message.error) reject(new Error(`the call failed: ${message.error}`))
      resolve()
    }
    if (message.type === "failed") reject(new Error(message.message))
  })
})
await done
ws.close()
NODE
fi

kill $serving 2>/dev/null || true
trap 'rm -rf "$work"' EXIT

say "what held it, on this machine"
grep -o "held by [^·]*" "$work/command.txt" | head -1

say "level 3 holds on $(uname -s) $(uname -r)"
