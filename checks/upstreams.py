"""Check health routing, TLS client/server identity and coalesced cache misses."""
from collections import Counter
from concurrent.futures import ThreadPoolExecutor
from contextlib import contextmanager
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import socket
import ssl
import subprocess
import tempfile
from threading import Barrier, Event, Lock, Thread
import time

from policy import request
from routing import launch, RILL


class Origin(BaseHTTPRequestHandler):
    def do_GET(self):
        srv = self.server
        with srv.lock:
            srv.calls[self.path] += 1
            n = srv.calls[self.path]
        if self.path.startswith("/api/burst"):
            time.sleep(.15)
        status = 503 if self.path == "/ready" and not srv.healthy else 200
        body = (f"{srv.label}:{n}" if self.path.startswith("/api/burst") else srv.label).encode()
        if self.path == "/api/version":
            body = self.server.version
        self.send_response(status)
        self.send_header("Content-Length", str(len(body)))
        self.send_header("Cache-Control", "private" if "private" in self.path else "public, max-age=30")
        self.end_headers()
        if self.path == "/api/version" and body == b"old":
            self.server.started.set()
            assert self.server.release.wait(5)
        self.wfile.write(body)

    def do_POST(self):
        self.server.version = b"new"
        self.send_response(204)
        self.end_headers()

    def log_message(self, *args):
        pass


class OriginServer(ThreadingHTTPServer):
    # Admit the full eight-client burst even before the accept loop runs.
    request_queue_size = 16


@contextmanager
def origin(label, port=0, tls=None):
    with OriginServer(("127.0.0.1", port), Origin) as srv:
        srv.label, srv.calls, srv.lock, srv.healthy = label, Counter(), Lock(), True
        srv.started, srv.release, srv.version = Event(), Event(), b"old"
        if tls:
            srv.socket = tls.wrap_socket(srv.socket, server_side=True)
        thread = Thread(target=srv.serve_forever, daemon=True)
        thread.start()
        try:
            yield srv
        finally:
            srv.shutdown()
            thread.join()


def eventually(check):
    until = time.monotonic() + 6
    while not check():
        if time.monotonic() > until:
            raise AssertionError("expected health transition did not occur")
        time.sleep(.05)


def start(root, peers, **settings):
    env = {k: v for k, v in os.environ.items() if k not in {"CACHE_TTL", "CACHE_COALESCE", "UPSTREAMS", "RILL_ROUTES"} and not k.startswith(("RILL_UPSTREAM_", "RILL_HEALTH_"))}
    return launch([RILL], dict(env, DOCUMENT_ROOT=str(root), UPSTREAMS=peers, **settings))


class KeepAliveOrigin(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def do_GET(self):
        if self.path == "/api/disconnect":
            self.server.disconnects += 1
            self.close_connection = True
            return
        body = f"{self.client_address[1]} {self.headers['Host']} {self.path}".encode()
        self.send_response(200)
        self.send_header("Content-Length", str(len(body)))
        if self.path == "/api/retire":
            self.send_header("Connection", "close")
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *args):
        pass


def fixed_peer(root):
    with OriginServer(("127.0.0.1", 0), KeepAliveOrigin) as srv:
        srv.disconnects = 0
        thread = Thread(target=srv.serve_forever, daemon=True)
        thread.start()
        try:
            with start(root, f"127.0.0.1:{srv.server_port}") as port:
                # Each request opens a fresh downstream connection. The upstream stays reusable.
                def get(path):
                    status, _, body = request(port, path)
                    assert status == 200, (status, body)
                    source, host, target = body.decode().split()
                    assert (host, target) == ("backend", path), (host, target)
                    return source
                first = get("/api/item?x=1&x=2")
                assert get("/api/retire") == first
                second = get("/api/item?x=3")
                assert second != first
                assert request(port, "/api/disconnect")[0] == 502
                assert srv.disconnects == 1, "an accepted GET was replayed"
                get("/api/after-error")
                assert request(port, "/outside-route")[0] == 404
        finally:
            srv.shutdown()
            thread.join()
    print("Fixed-peer keep-alive, origin-form query, connection retirement and no replay after delivery passed")


def health(root):
    for mode in ("round_robin", "least_connections"):
        with origin("a") as a, origin("b") as b:
            peers = f"127.0.0.1:{a.server_port},127.0.0.1:{b.server_port}"
            a.healthy = False
            with start(root, peers, RILL_HTTP_BALANCE=mode, RILL_HEALTH_PATH="/ready",
                       RILL_HEALTH_INTERVAL_SECS="1") as port:
                def bodies():
                    return [request(port, "/api/test")[2] for _ in range(4)]
                eventually(lambda: bodies() == [b"b"] * 4)
                b.healthy = False
                eventually(lambda: request(port, "/api/test")[0] == 503)
                a.healthy = True
                eventually(lambda: bodies() == [b"a"] * 4)
    with socket.socket() as reserved:
        reserved.bind(("127.0.0.1", 0))
        backend = reserved.getsockname()[1]
    with start(root, f"127.0.0.1:{backend}", RILL_HEALTH_INTERVAL_SECS="1") as port:
        eventually(lambda: request(port, "/api/test")[0] == 503)
        with origin("revived", backend):
            eventually(lambda: request(port, "/api/test")[2] == b"revived")
    print("TCP and HTTP health exclusion, all-down 503, and recovery passed", flush=True)


def certificates(root):
    def sslcmd(*args):
        subprocess.run(["openssl", *args], cwd=root, capture_output=True, check=True)
    sslcmd("req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1", "-subj",
           "/CN=Test CA", "-keyout", "ca.key", "-out", "ca.pem", "-addext", "basicConstraints=critical,CA:TRUE")
    for name, usage in [("server", "serverAuth"), ("client", "clientAuth")]:
        sslcmd("req", "-newkey", "rsa:2048", "-nodes", "-subj", "/CN=" + name, "-keyout",
               name + ".key", "-out", name + ".csr")
        ext = "extendedKeyUsage=" + usage + "\n"
        if name == "server":
            ext += "subjectAltName=DNS:localhost\n"
        (root / "ext").write_text(ext)
        sslcmd("x509", "-req", "-in", name + ".csr", "-CA", "ca.pem", "-CAkey", "ca.key", "-CAcreateserial",
               "-days", "1", "-extfile", "ext", "-out", name + ".pem")


def tls(root):
    certificates(root)
    ctx = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
    ctx.load_cert_chain(root / "server.pem", root / "server.key")
    ctx.load_verify_locations(root / "ca.pem")
    ca = dict(RILL_UPSTREAM_CA=str(root / "ca.pem"))
    identity = dict(RILL_UPSTREAM_CERT=str(root / "client.pem"), RILL_UPSTREAM_KEY=str(root / "client.key"))
    with origin("encrypted", tls=ctx) as o:
        peer = f"https://localhost:{o.server_port}"
        with start(root, peer, **ca) as port:
            assert request(port, "/api/test")[2] == b"encrypted"
        with start(root, peer.replace("localhost", "127.0.0.1"), **ca) as port:
            assert request(port, "/api/test")[0] == 502
        with start(root, peer) as port:
            assert request(port, "/api/test")[0] == 502
        ctx.verify_mode = ssl.CERT_REQUIRED
        with start(root, peer, **ca) as port:
            assert request(port, "/api/test")[0] == 502
        with start(root, peer, **ca, **identity, RILL_HEALTH_PATH="/ready", RILL_HEALTH_INTERVAL_SECS="1") as port:
            eventually(lambda: o.calls["/ready"] > 0)
            assert request(port, "/api/test")[2] == b"encrypted"
    print("HTTPS, CA and hostname validation, and required mTLS identity passed", flush=True)


def cache(root):
    with origin("cache") as o, start(root, f"127.0.0.1:{o.server_port}", CACHE_COALESCE="1", CACHE_TTL="30") as port:
        def burst(path):
            gate = Barrier(8)
            def one(i):
                gate.wait(timeout=4)
                status, _, body = request(port, path)
                assert status == 200
                return body
            with ThreadPoolExecutor(8) as clients:
                return set(clients.map(one, range(8)))
        assert burst("/api/burst") == {b"cache:1"}
        assert o.calls["/api/burst"] == 1
        assert len(burst("/api/burst-private")) == 8
        assert o.calls["/api/burst-private"] == 8
        # A completed write invalidates any older in-flight cache fill.
        with ThreadPoolExecutor(1) as client:
            pending = client.submit(request, port, "/api/version")
            try:
                assert o.started.wait(3)
                assert request(port, "/api/version", method="POST")[0] == 204
            finally:
                o.release.set()
            assert pending.result()[2] == b"old"
        assert request(port, "/api/version")[2] == b"new"
        assert request(port, "/api/version")[2] == b"new"
        assert o.calls["/api/version"] == 2
    print("Eight simultaneous cacheable misses used one upstream GET; eight private responses stayed independent", flush=True)


if __name__ == "__main__":
    with tempfile.TemporaryDirectory() as tmp:
        root = Path(tmp)
        fixed_peer(root)
        cache(root)
        health(root)
        keys = root / "certs"
        keys.mkdir()
        tls(keys)
