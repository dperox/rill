"""Verify Japka host and path routing and concurrent least-connections selection."""
import json
import os
import socket
import subprocess
import tempfile
import time
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager, ExitStack
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from threading import Event, Thread

ROOT = Path(__file__).resolve().parents[1]
JAPKA = str(Path(os.getenv("JAPKA", os.getenv("RILL", ROOT / "target/debug/japka"))).resolve())

class Origin(BaseHTTPRequestHandler):
    def do_GET(self):
        if self.path.endswith("/hold"):
            self.server.started.set()
            self.server.release.wait(10)
        body = (self.server.label + " " + self.path).encode()
        self.send_response(200)
        self.send_header("content-type", "text/plain")
        self.send_header("content-length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass

def fetch(port, path, host):
    conn = HTTPConnection("127.0.0.1", port, timeout=12)
    try:
        conn.request("GET", path, headers={"Host": host})
        resp = conn.getresponse()
        body = resp.read().decode()
        assert resp.status == 200, (resp.status, body)
        return body
    finally:
        conn.close()

@contextmanager
def launch(command, env):
    with socket.socket() as sock:
        sock.bind(("127.0.0.1", 0))
        port = sock.getsockname()[1]
    proc = subprocess.Popen(command, env=dict(env, PORT=str(port), LISTEN=f"127.0.0.1:{port}"))
    try:
        for _ in range(200):
            try:
                with socket.create_connection(("127.0.0.1", port), timeout=.1):
                    break
            except OSError:
                if proc.poll() is not None:
                    raise RuntimeError("server exited")
                time.sleep(.02)
        else:
            raise RuntimeError(f"server did not listen on 127.0.0.1:{port}")
        yield port
    finally:
        proc.terminate()
        proc.wait(timeout=5)

def run():
    with tempfile.TemporaryDirectory() as tmp, ExitStack() as stack:
        work = Path(tmp)
        (work / "logs").mkdir()
        backends = []
        for label in ("A", "B"):
            backend = stack.enter_context(ThreadingHTTPServer(("127.0.0.1", 0), Origin))
            backend.label = label
            backend.started, backend.release = Event(), Event()
            Thread(target=backend.serve_forever, daemon=True).start()
            stack.callback(backend.shutdown)
            backends.append(backend)
        addrs = [f"127.0.0.1:{svc.server_port}" for svc in backends]
        rules = [dict(prefix="/api/", upstreams=addrs),
                 dict(host="docs.example", prefix="/api/docs/", upstreams=[addrs[1]])]
        env = dict(os.environ, DOCUMENT_ROOT=tmp, JAPKA_ROUTES=json.dumps(rules), JAPKA_HTTP_BALANCE="least_connections")
        requests = [("app.example", "/api/users?q=1", "A"), ("docs.example", "/api/docs/item", "B"),
                    ("app.example", "/api/docs/item", "B"), ("DOCS.EXAMPLE", "/api/docs/page", "B")]
        with launch([JAPKA], env) as port:
            for host, path, backend in requests:
                assert fetch(port, path, host) == f"{backend} {path}"
        for backend in backends:
            backend.started.clear()
            backend.release.clear()
        with launch([JAPKA], env) as port, ThreadPoolExecutor(1) as executor:
            held = executor.submit(fetch, port, "/api/hold", "app.example")
            try:
                assert backends[0].started.wait(3)
                for _ in range(3):
                    assert fetch(port, "/api/short", "app.example") == "B /api/short"
            finally:
                backends[0].release.set()
            assert held.result() == "A /api/hold"
        print("Concurrent least-connections routing passed.")

if __name__ == "__main__":
    run()
