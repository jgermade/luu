"""An OpenAI-compatible server that answers from a script, for the probe.

    python3 -I fake-model.py <port> <replies.json>

`replies.json` is a list of strings. Each POST /v1/chat/completions takes the
next, and the list starts over after its last (one user's run is the list).
Every request is logged to stderr, one line, so a reader sees which call got
which reply.
"""
import json, sys, threading
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

PORT, REPLIES = int(sys.argv[1]), json.load(open(sys.argv[2]))
lock, state = threading.Lock(), {"n": 0}


class Handler(BaseHTTPRequestHandler):
    def log_message(self, *args):
        pass

    def _json(self, body, status=200):
        raw = json.dumps(body).encode()
        self.send_response(status)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(raw)))
        self.end_headers()
        self.wfile.write(raw)

    def do_GET(self):
        if self.path.endswith("/models"):
            return self._json({"object": "list", "data": [{"id": "probe", "object": "model"}]})
        if self.path == "/reset":
            with lock:
                state["n"] = 0
            return self._json({"ok": True})
        self._json({"error": "no"}, 404)

    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["Content-Length"])))
        with lock:
            reply = REPLIES[state["n"] % len(REPLIES)]
            state["n"] += 1
            n = state["n"]
        last = body["messages"][-1]
        print(f"call {n}: last {last.get('role')} {str(last.get('content'))[:80]!r} -> "
              f"{reply[:40]!r}", file=sys.stderr, flush=True)
        self.send_response(200)
        self.send_header("Content-Type", "text/event-stream")
        self.end_headers()

        def chunk(delta, finish=None, usage=None):
            event = {"id": "x", "object": "chat.completion.chunk", "model": "probe",
                     "choices": [{"index": 0, "delta": delta, "finish_reason": finish}]}
            if usage:
                event["choices"], event["usage"] = [], usage
            self.wfile.write(f"data: {json.dumps(event)}\n\n".encode())

        chunk({"role": "assistant", "content": reply})
        chunk({}, "stop")
        chunk({}, usage={"prompt_tokens": 1, "completion_tokens": 1, "total_tokens": 2})
        self.wfile.write(b"data: [DONE]\n\n")
        self.wfile.flush()


ThreadingHTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
