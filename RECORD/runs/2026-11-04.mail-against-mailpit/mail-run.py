"""Row 71 against Mailpit: `luu config owner` under each kind of URL, then a
user's login code by email from a running server. Usage:

    python3 -I mail-run.py <luu binary> <out dir>

Mailpit APIs: plain on 8025, STARTTLS on 8026, implicit TLS on 8027.
"""
import json, os, re, select, shutil, subprocess, sys, time, urllib.request

LUU, OUT = sys.argv[1], sys.argv[2]
PW = os.path.join(OUT, "pw")
os.makedirs(OUT, exist_ok=True)
with open(PW, "w") as f:
    f.write("s3cret-pw\n")
os.chmod(PW, 0o600)
WRONG = os.path.join(OUT, "pw-wrong")
with open(WRONG, "w") as f:
    f.write("not-it\n")
os.chmod(WRONG, 0o600)


def api(port, path, method="GET", body=None, headers=None):
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", method=method,
                                 data=body, headers=headers or {})
    with urllib.request.urlopen(req, timeout=10) as r:
        raw = r.read()
        return r.status, dict(r.headers), raw


def inbox(port):
    return json.loads(api(port, "/api/v1/messages")[2])["messages"]


def code_for(port, to, seen):
    for _ in range(100):
        for m in inbox(port):
            if m["ID"] in seen:
                continue
            if any(a["Address"] == to for a in m["To"]):
                full = json.loads(api(port, f"/api/v1/message/{m['ID']}")[2])
                code = re.search(r"\b(\d{6})\b", full["Text"]).group(1)
                return m, full, code
        time.sleep(0.1)
    return None, None, None


def home(name):
    h = os.path.join(OUT, name)
    shutil.rmtree(h, ignore_errors=True)
    os.makedirs(h)
    env = dict(os.environ, LUU_HOME=h)
    r = subprocess.run([LUU, "config", "server", "--origin", "https://luu.example.test",
                        "--bind", "127.0.0.1:7990", "--ssh", "none"], env=env, capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    return h, env


def owner(name, url, port, pwfile, user="luu-mail"):
    h, env = home(name)
    to = f"owner-{name}@example.test"
    seen = {m["ID"] for m in inbox(port)} if port else set()
    p = subprocess.Popen([LUU, "config", "owner", "--email", to, "--smtp-url", url,
                          "--smtp-from", "luu <luu@example.test>", "--smtp-user", user,
                          "--smtp-password-file", pwfile],
                         env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, text=True)
    err = ""
    deadline = time.time() + 40
    while time.time() < deadline and p.poll() is None and "The code:" not in err:
        r, _, _ = select.select([p.stderr], [], [], 0.2)
        if r:
            err += os.read(p.stderr.fileno(), 4096).decode()
    msg = None
    if "The code:" in err:
        msg, full, code = code_for(port, to, seen)
        p.stdin.write((code or "") + "\n")
        p.stdin.flush()
    out, rest = p.communicate(timeout=30)
    err += rest
    files = sorted(os.listdir(h))
    config = open(os.path.join(h, "config.toml")).read()
    result = {
        "case": name, "url": url, "exit": p.returncode,
        "stdout": out.strip(), "stderr": err.strip(),
        "files": files, "smtp saved": "[auth.smtp]" in config,
        "password in config.toml": "s3cret-pw" in config,
    }
    if msg:
        result["mail"] = {"to": [a["Address"] for a in msg["To"]], "subject": msg["Subject"],
                          "text": full["Text"].strip()}
    if "smtp.password" in files:
        result["smtp.password mode"] = oct(os.stat(os.path.join(h, "smtp.password")).st_mode & 0o777)
    return result, h, env


results = []
cases = [
    ("plain-wrong-password", "smtp://127.0.0.1:1025", 8025, WRONG),
    ("plain", "smtp://127.0.0.1:1025", 8025, PW),
    ("starttls", "smtp://localhost:1587?tls=required", 8026, PW),
    ("smtps", "smtps://localhost:1465", 8027, PW),
    ("starttls-server-without-tls", "smtp://localhost:1587", 8026, PW),
]
if len(sys.argv) > 3:  # an address that is not loopback, for tls=opportunistic
    cases.append(("opportunistic-not-loopback", f"smtp://{sys.argv[3]}:1025?tls=opportunistic",
                  8025, PW))
for name, url, port, pw in cases:
    r, h, env = owner(name, url, port, pw)
    results.append(r)
    if name == "plain" and r["exit"] == 0:
        plain_home, plain_env = h, env

# After setup: a user's code by email, from a running server, and a login.
if "plain_home" in globals():
    env = plain_env
    subprocess.run([LUU, "user", "add", "bo@example.test"], env=env, check=True,
                   capture_output=True)
    server = subprocess.Popen([LUU, "serve", "--no-store"], env=env,
                              stdout=open(os.path.join(OUT, "serve.out"), "w"),
                              stderr=subprocess.STDOUT)
    try:
        for _ in range(100):
            try:
                api(7990, "/api/auth/me.json")
                break
            except Exception:
                time.sleep(0.2)
        hdr = {"Content-Type": "application/json", "Origin": "https://luu.example.test"}
        seen = {m["ID"] for m in inbox(8025)}
        st, _, body = api(7990, "/api/auth/code", "POST",
                          json.dumps({"email": "bo@example.test"}).encode(), hdr)
        m, full, code = code_for(8025, "bo@example.test", seen)
        try:
            st2, h2, _ = api(7990, "/api/auth/login", "POST",
                             json.dumps({"email": "bo@example.test", "code": code}).encode(), hdr)
        except urllib.error.HTTPError as e:
            st2, h2 = e.code, dict(e.headers)
        results.append({"case": "login code from serve", "code request": st,
                        "mail": {"to": [a["Address"] for a in m["To"]], "subject": m["Subject"]}
                        if m else None,
                        "login": st2, "cookie set": "Set-Cookie" in h2 or "set-cookie" in h2})
    finally:
        server.terminate()
        server.wait()

json.dump(results, open(os.path.join(OUT, "results.json"), "w"), indent=2)
print(json.dumps(results, indent=2))
