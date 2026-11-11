"""Row 71 with a CA file: `luu config owner` against a Mailpit whose
certificate chains to a private CA, then a user's login code from a running
server. It makes the CA and the certificates, starts Mailpit twice (implicit
TLS, STARTTLS required) and stops them. Usage:

    python3 -I mail-run.py <luu binary> <mailpit binary> <out dir>

SMTP on 1465 (TLS) and 1587 (STARTTLS); their APIs on 8027 and 8026.
"""
import json, os, re, select, shutil, subprocess, sys, time, urllib.request, urllib.error

LUU, MAILPIT, OUT = sys.argv[1], sys.argv[2], os.path.abspath(sys.argv[3])
shutil.rmtree(OUT, ignore_errors=True)
os.makedirs(OUT)


def sh(*argv):
    subprocess.run(argv, check=True, capture_output=True, cwd=OUT)


def ca(name):
    sh("openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "2",
       "-keyout", f"{name}.key", "-out", f"{name}.pem", "-subj", f"/CN={name}",
       "-addext", "basicConstraints=critical,CA:TRUE", "-addext", "keyUsage=critical,keyCertSign")
    return os.path.join(OUT, f"{name}.pem")


# The CA Mailpit's certificate chains to, and one it does not.
OURS, OTHER = ca("private-ca"), ca("another-ca")
# The server's certificate: for `localhost` and nothing else, so 127.0.0.1 is
# a name it does not carry.
sh("openssl", "req", "-newkey", "rsa:2048", "-nodes", "-keyout", "server.key",
   "-out", "server.csr", "-subj", "/CN=localhost")
with open(os.path.join(OUT, "san.cnf"), "w") as f:
    f.write("subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n")
sh("openssl", "x509", "-req", "-in", "server.csr", "-CA", "private-ca.pem", "-CAkey",
   "private-ca.key", "-CAcreateserial", "-days", "2", "-out", "server.pem", "-extfile", "san.cnf")

PW = os.path.join(OUT, "pw")
with open(PW, "w") as f:
    f.write("s3cret-pw\n")
os.chmod(PW, 0o600)
with open(os.path.join(OUT, "auth"), "w") as f:
    f.write("luu-mail:s3cret-pw\n")


def mailpit(smtp, http, how):
    return subprocess.Popen(
        [MAILPIT, "--smtp", f"127.0.0.1:{smtp}", "--listen", f"127.0.0.1:{http}",
         "--smtp-auth-file", "auth", "--smtp-tls-cert", "server.pem", "--smtp-tls-key",
         "server.key", how, "--disable-version-check", "--database", f"mp-{smtp}.db"],
        cwd=OUT, stdout=open(os.path.join(OUT, f"mailpit-{smtp}.log"), "w"),
        stderr=subprocess.STDOUT)


def api(port, path, method="GET", body=None, headers=None):
    req = urllib.request.Request(f"http://127.0.0.1:{port}{path}", method=method,
                                 data=body, headers=headers or {})
    opener = urllib.request.build_opener(urllib.request.ProxyHandler({}))
    with opener.open(req, timeout=10) as r:
        return r.status, dict(r.headers), r.read()


def inbox(port):
    return json.loads(api(port, "/api/v1/messages")[2])["messages"]


def code_for(port, to, seen):
    for _ in range(100):
        for m in inbox(port):
            if m["ID"] not in seen and any(a["Address"] == to for a in m["To"]):
                full = json.loads(api(port, f"/api/v1/message/{m['ID']}")[2])
                return m, full, re.search(r"\b(\d{6})\b", full["Text"]).group(1)
        time.sleep(0.1)
    return None, None, None


def home(name):
    h = os.path.join(OUT, "home-" + name)
    os.makedirs(h)
    env = dict(os.environ, LUU_HOME=h, RUST_BACKTRACE="0")
    r = subprocess.run([LUU, "config", "server", "--origin", "https://luu.example.test",
                        "--bind", "127.0.0.1:7990", "--ssh", "none"],
                       env=env, capture_output=True, text=True)
    assert r.returncode == 0, r.stderr
    return h, env


def owner(name, url, port, ca_file):
    h, env = home(name)
    to = f"owner-{name}@example.test"
    seen = {m["ID"] for m in inbox(port)}
    argv = [LUU, "config", "owner", "--email", to, "--smtp-url", url, "--smtp-from",
            "luu <luu@example.test>", "--smtp-user", "luu-mail", "--smtp-password-file", PW]
    if ca_file:
        argv += ["--smtp-ca-file", ca_file]
    p = subprocess.Popen(argv, env=env, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                         stderr=subprocess.PIPE, text=True)
    err, deadline = "", time.time() + 40
    while time.time() < deadline and p.poll() is None and "The code:" not in err:
        if select.select([p.stderr], [], [], 0.2)[0]:
            err += os.read(p.stderr.fileno(), 4096).decode()
    msg = None
    if "The code:" in err:
        msg, full, code = code_for(port, to, seen)
        p.stdin.write((code or "") + "\n")
        p.stdin.flush()
    out, rest = p.communicate(timeout=30)
    config = open(os.path.join(h, "config.toml")).read()
    said = (err + rest).strip().replace(OUT, "<out>")
    result = {"case": name, "url": url, "ca-file": ca_file and os.path.basename(ca_file),
              "exit": p.returncode, "stderr": said,
              "smtp saved": "[auth.smtp]" in config, "ca-file saved": "ca-file" in config}
    if msg:
        result["mail"] = {"to": [a["Address"] for a in msg["To"]], "subject":
                          re.sub(r"\d{6}", "<code>", msg["Subject"])}
    return result, h, env


servers = [mailpit(1465, 8027, "--smtp-require-tls"),
           mailpit(1587, 8026, "--smtp-require-starttls")]
results = []
try:
    for port in (8027, 8026):
        for _ in range(100):
            try:
                inbox(port)
                break
            except Exception:
                time.sleep(0.1)
    cases = [
        ("smtps-no-ca", "smtps://localhost:1465", 8027, None),
        ("smtps-with-ca", "smtps://localhost:1465", 8027, OURS),
        ("starttls-with-ca", "smtp://localhost:1587?tls=required", 8026, OURS),
        ("smtps-another-ca", "smtps://localhost:1465", 8027, OTHER),
        ("smtps-ca-wrong-name", "smtps://127.0.0.1:1465", 8027, OURS),
        ("smtps-ca-missing", "smtps://localhost:1465", 8027, os.path.join(OUT, "nope.pem")),
    ]
    for name, url, port, ca_file in cases:
        r, h, env = owner(name, url, port, ca_file)
        results.append(r)
        if name == "smtps-with-ca" and r["exit"] == 0:
            kept = env

    # After setup: a user's code by email from a running server, under the
    # `ca-file` the owner step saved, and a login with it.
    if "kept" in globals():
        subprocess.run([LUU, "user", "add", "bo@example.test"], env=kept, check=True,
                       capture_output=True)
        server = subprocess.Popen([LUU, "serve", "--no-store"], env=kept,
                                  stdout=open(os.path.join(OUT, "serve.out"), "w"),
                                  stderr=subprocess.STDOUT)
        try:
            for _ in range(100):
                try:
                    api(7990, "/api/auth/me.json")
                    break
                except urllib.error.HTTPError:
                    break
                except Exception:
                    time.sleep(0.2)
            hdr = {"Content-Type": "application/json", "Origin": "https://luu.example.test"}
            seen = {m["ID"] for m in inbox(8027)}
            st, _, _ = api(7990, "/api/auth/code", "POST",
                           json.dumps({"email": "bo@example.test"}).encode(), hdr)
            m, full, code = code_for(8027, "bo@example.test", seen)
            try:
                st2, h2, _ = api(7990, "/api/auth/login", "POST", json.dumps(
                    {"email": "bo@example.test", "code": code}).encode(), hdr)
            except urllib.error.HTTPError as e:
                st2, h2 = e.code, dict(e.headers)
            results.append({"case": "login code from serve, under the saved ca-file",
                            "code request": st, "mail arrived": bool(m), "login": st2,
                            "cookie set": any(k.lower() == "set-cookie" for k in h2)})
        finally:
            server.terminate()
            server.wait()
finally:
    for s in servers:
        s.terminate()
        s.wait()

json.dump(results, open(os.path.join(OUT, "results.json"), "w"), indent=2)
print(json.dumps(results, indent=2))
