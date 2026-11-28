"""Exercise HTTP policy behavior over real sockets. Run after cargo build --locked."""

from collections import Counter
from contextlib import contextmanager
import gzip
from http.client import HTTPConnection
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import os
from pathlib import Path
import socket
import subprocess
import tempfile
from threading import Thread
import time

REPO = Path(__file__).resolve().parents[1]
BINARY = Path(os.environ.get("JAPKA", os.environ.get("RILL", REPO / "target/debug/japka"))).resolve()
POLICY = {"ALLOW_IPS", "DENY_IPS", "RATE_LIMIT", "COMPRESS", "CACHE_TTL"}
PLAIN = b"compressible body\n" * 256


class Backend(BaseHTTPRequestHandler):
    calls = Counter()

    def do_GET(self):
        self.calls[self.path] += 1
        body = f"{self.calls[self.path]}:".encode() + PLAIN
        if self.path == "/api/oversized":
            body *= 30
        self.send_response(200)
        self.send_header("Content-Type", "text/plain")
        self.send_header("Content-Length", str(len(body)))
        self.send_header("ETag", '"stable"')
        self.send_header("Cache-Control", {
            "/api/private": "private, max-age=60",
            "/api/no-transform": "public, max-age=60, no-transform",
            "/api/expires": "public, max-age=1",
            "/api/no-cache": "public, max-age=60, no-cache",
            "/api/shared": "public, max-age=0, s-maxage=60",
        }.get(self.path, "public, max-age=60"))
        if self.path == "/api/cookie":
            self.send_header("Set-Cookie", "session=upstream")
        if self.path == "/api/vary":
            self.send_header("Vary", "Authorization")
        if self.path == "/api/stale":
            self.send_header("Age", "61")
        self.end_headers()
        if self.command != "HEAD":
            self.wfile.write(body)

    do_POST = do_HEAD = do_GET

    def log_message(self, *args):
        pass


def request(port, path="/", method="GET", headers=None, host="127.0.0.1"):
    connection = HTTPConnection(host, port, timeout=5)
    connection.request(method, path, headers=headers or {})
    response = connection.getresponse()
    result = response.status, dict((k.lower(), v) for k, v in response.getheaders()), response.read()
    connection.close()
    return result


@contextmanager
def running(root, backend, **settings):
    with socket.socket() as listener:
        listener.bind(("127.0.0.1", 0))
        port = listener.getsockname()[1]
    env = {key: value for key, value in os.environ.items() if key not in POLICY}
    env.update(LISTEN=f"127.0.0.1:{port}", DOCUMENT_ROOT=str(root),
               UPSTREAMS=f"127.0.0.1:{backend.server_port}", **settings)
    log_path = Path(root) / ".japka-policy-stderr.log"
    with log_path.open("wb") as err_file, subprocess.Popen([BINARY], cwd=REPO, env=env, stderr=err_file) as process:
        try:
            deadline = time.monotonic() + 5
            while True:
                if process.poll() is not None:
                    raise RuntimeError(log_path.read_text(errors="replace"))
                try:
                    with socket.create_connection(("127.0.0.1", port), timeout=0.1):
                        break
                except ConnectionRefusedError:
                    if time.monotonic() >= deadline:
                        raise
                    time.sleep(0.01)
            yield port
        finally:
            if process.poll() is None:
                process.terminate()
                process.wait(timeout=5)


def cache_checks(root, backend):
    with running(root, backend, CACHE_TTL="10", COMPRESS="1") as port:
        path = "/api/public?first"
        status, headers, first = request(port, path)
        assert status == 200 and "age" not in headers
        status, headers, hit = request(port, path)
        assert status == 200 and headers["age"] == "0" and first == hit
        assert backend.RequestHandlerClass.calls[path] == 1
        assert request(port, "/api/public?second")[2] == first
        assert request(port, path, headers={"Host": "distinct.test"})[2] != first

        for header in ("Authorization", "Cookie", "If-Match", "Range", "Cache-Control",
                       "X-Forwarded-For", "X-Session"):
            assert request(port, path, headers={header: "test"})[2] != first
        assert request(port, path)[2] == first
        assert request(port, path, method="HEAD")[0] == 200
        assert request(port, path, method="POST")[0] == 200
        assert request(port, path)[2] != first

        for name in ("private", "no-cache", "cookie", "vary", "stale", "oversized"):
            path = f"/api/{name}"
            assert request(port, path)[2] != request(port, path)[2], name

        shared = request(port, "/api/shared")[2]
        assert request(port, "/api/shared")[2] == shared
        expiring = request(port, "/api/expires")[2]
        time.sleep(1.05)
        assert request(port, "/api/expires")[2] != expiring

        gzip_headers = {"Accept-Encoding": "gzip"}
        status, headers, compressed = request(port, "/", headers=gzip_headers)
        assert status == 200 and headers["content-encoding"] == "gzip"
        assert "accept-encoding" in headers["vary"].lower()
        assert gzip.decompress(compressed) == PLAIN
        assert headers["etag"].startswith("W/")
        assert request(port, "/", headers={"Accept-Encoding": "gzip;q=0"})[2] == PLAIN
        assert request(port, "/", headers={**gzip_headers, "Range": "bytes=0-7"})[2] == PLAIN[:8]
        assert "content-encoding" not in request(port, "/", method="HEAD", headers=gzip_headers)[1]
        assert "content-encoding" not in request(port, "/api/no-transform", headers=gzip_headers)[1]
        path = "/api/compressed"
        status, headers, compressed = request(port, path, headers=gzip_headers)
        assert status == 200 and gzip.decompress(compressed).endswith(PLAIN)
        status, headers, hit = request(port, path, headers=gzip_headers)
        assert status == 200 and headers["age"] == "0" and compressed == hit


def access_checks(root, backend):
    # Exercise two peer identities using only the standard loopback addresses.
    with socket.socket(socket.AF_INET6) as listener:
        listener.bind(("::1", 0))
        v6_port = listener.getsockname()[1]
    second_listener = f"[::1]:{v6_port}"
    with running(root, backend, JAPKA_STATIC_LISTEN=second_listener,
                 ALLOW_IPS="127.0.0.0/8,::1", DENY_IPS="::1") as port:
        assert request(port)[0] == 200
        assert request(v6_port, host="::1", headers={"X-Forwarded-For": "127.0.0.1"})[0] == 403
        assert request(port, headers={"X-Forwarded-For": "::1"})[0] == 200
    with running(root, backend, ALLOW_IPS="::1/128,192.0.2.1") as port:
        assert request(port)[0] == 403
    with running(root, backend, JAPKA_STATIC_LISTEN=second_listener, RATE_LIMIT="2", CACHE_TTL="10") as port:
        assert request(port, "/api/limited")[0] == 200
        assert request(port, "/api/limited")[1]["age"] == "0"
        status, headers, _ = request(port)
        assert status == 429 and headers["retry-after"] == "1"
        assert request(v6_port, host="::1")[0] == 200
        time.sleep(1.05)
        assert request(port)[0] == 200


def main():
    backend = ThreadingHTTPServer(("127.0.0.1", 0), Backend)
    thread = Thread(target=backend.serve_forever, daemon=True)
    thread.start()
    try:
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            (root / "index.html").write_bytes(PLAIN)
            cache_checks(root, backend)
            access_checks(root, backend)
    finally:
        backend.shutdown()
        backend.server_close()
        thread.join()
    print("HTTP policy checks passed")


if __name__ == "__main__":
    main()
