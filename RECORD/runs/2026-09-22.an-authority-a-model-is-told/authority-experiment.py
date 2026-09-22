#!/usr/bin/env python3
import json, subprocess, sys, os, time, shutil, tempfile

BIN = "/Users/JG31772/dev/joshua/luu/target/debug/luu"
TEMPLATE = "/Users/JG31772/dev/joshua/luu/.tmp/authority-task"
TOKENIZER = os.path.expanduser("~/models/qwen3.8-27b/tokenizer.json")
OUT_DIR = "/Users/JG31772/dev/joshua/luu/.tmp/authority-runs"
os.makedirs(OUT_DIR, exist_ok=True)

PROMPT = ("There's a bug in src/greeting.rs: the doc comment says it should "
          "say \"Hi\" and it says \"Hello\". Fix it.")

def run_arm(name, luu_home):
    scratch = tempfile.mkdtemp(prefix="authority-task-")
    shutil.copytree(os.path.join(TEMPLATE, "src"), os.path.join(scratch, "src"))
    env = dict(os.environ)
    env["LUU_HOME"] = luu_home
    record_path = os.path.join(OUT_DIR, f"{name}.jsonl")
    proc = subprocess.Popen(
        [BIN, "stdio", "--allow-write", ".",
         "--backend", "openai", "--openai-url", "http://127.0.0.1:8100/v1",
         "--model", "qwen3.8-27b",
         "--context-limit", "8192", "--reserve", "512",
         "--temperature", "0", "--seed", "1",
         "--tokenizer", TOKENIZER,
         "--record", record_path,
         "--max-tool-steps", "8"],
        cwd=scratch, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT, text=True, bufsize=1,
    )
    transcript = []
    def send(obj):
        line = json.dumps(obj)
        transcript.append(f">>> {line}")
        proc.stdin.write(line + "\n")
        proc.stdin.flush()

    def read_until(predicate, timeout=180):
        deadline = time.time() + timeout
        while time.time() < deadline:
            line = proc.stdout.readline()
            if not line:
                break
            line = line.rstrip("\n")
            transcript.append(f"<<< {line}")
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if predicate(msg):
                return msg
        return None

    send({"type": "hello", "protocol": 8, "format": 18})
    hello = read_until(lambda m: m.get("type") == "hello", timeout=20)
    send({"type": "prompt", "text": PROMPT})
    ended = read_until(lambda m: m.get("type") in ("ended", "failed"), timeout=900)

    # Drain a little longer in case trailing lines follow.
    time.sleep(0.5)
    proc.stdin.close()
    try:
        proc.wait(timeout=5)
    except subprocess.TimeoutExpired:
        proc.terminate()

    with open(os.path.join(OUT_DIR, f"{name}.txt"), "w") as f:
        f.write("\n".join(transcript))
    print(f"=== {name}: done, ended={ended is not None} ===", file=sys.stderr)
    shutil.rmtree(scratch, ignore_errors=True)

if __name__ == "__main__":
    which = sys.argv[1]
    homes = {
        "with-note": "/Users/JG31772/dev/joshua/luu/.tmp/authority-home-with-note",
        "bare": "/Users/JG31772/dev/joshua/luu/.tmp/authority-home-bare",
    }
    run_arm(which, homes[which])
